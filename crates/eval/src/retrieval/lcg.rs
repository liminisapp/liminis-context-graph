//! LCG driver (FR-004, FR-005): spawns the shipped `liminis-context-graph` service in a temp
//! workspace, speaks newline-delimited JSON-RPC 2.0 over its Unix socket, ingests the fixture and
//! exposes the three LCG configurations as [`Retriever`]s.
//!
//! The service route (not an in-process `AppState`) is deliberate — see ADR-0754: it measures the
//! shipped IPC surface and does not duplicate the service's startup wiring.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

use super::fixture::Fixture;
use super::mapping::{
    chunk_markdown, edge_groups, entity_groups, map_edges, map_entities, map_passages,
};
use super::{RankedResult, Retriever};

/// Group every eval episode is written to and read from.
pub const EVAL_GROUP: &str = "retrieval-eval";

/// One JSON-RPC call. Trait-ified so tests use a canned responder.
pub trait RpcTransport: Send + Sync {
    fn call<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, String>>;
}

/// Newline-delimited JSON-RPC over a Unix socket. Calls are serialized on one connection.
pub struct UdsClient {
    conn: Mutex<BufReader<UnixStream>>,
    next_id: AtomicU64,
    timeout: Duration,
}

/// Per-request ceiling. Generous because `knowledge_process_chunk` makes extraction-LLM calls and
/// `knowledge_build_indices` can be slow, but finite so a hung service fails the run instead of
/// stalling it forever.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(900);

impl UdsClient {
    pub async fn connect(path: &Path) -> Result<Self, String> {
        let s = UnixStream::connect(path)
            .await
            .map_err(|e| format!("cannot connect to {}: {e}", path.display()))?;
        Ok(Self {
            conn: Mutex::new(BufReader::new(s)),
            next_id: AtomicU64::new(1),
            timeout: RPC_TIMEOUT,
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn exchange(&self, method: &str, params: Value) -> Result<Value, String> {
        {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let mut line = serde_json::to_string(
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .map_err(|e| e.to_string())?;
            line.push('\n');
            let mut conn = self.conn.lock().await;
            conn.get_mut()
                .write_all(line.as_bytes())
                .await
                .map_err(|e| format!("{method}: write failed: {e}"))?;
            loop {
                let mut resp = String::new();
                let n = conn
                    .read_line(&mut resp)
                    .await
                    .map_err(|e| format!("{method}: read failed: {e}"))?;
                if n == 0 {
                    return Err(format!("{method}: service closed the connection"));
                }
                let v: Value = serde_json::from_str(resp.trim())
                    .map_err(|e| format!("{method}: response not JSON: {e}"))?;
                // Skip streamed progress lines; the terminal line carries our id.
                if v["type"] == "progress" || v["id"] != json!(id) {
                    continue;
                }
                if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                    return Err(format!(
                        "{method}: {}",
                        err["message"].as_str().unwrap_or("service error")
                    ));
                }
                return Ok(v["result"].clone());
            }
        }
    }
}

impl RpcTransport for UdsClient {
    fn call<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            tokio::time::timeout(self.timeout, self.exchange(method, params))
                .await
                .unwrap_or_else(|_| {
                    Err(format!(
                        "{method}: no response from the service within {}s",
                        self.timeout.as_secs_f64()
                    ))
                })
        })
    }
}

/// How the service is told which embedder/extractor to use.
#[derive(Debug, Clone, Default)]
pub struct ServiceOpts {
    pub bin: PathBuf,
    pub embedder_http: Option<String>,
    pub embedder_uds: Option<String>,
    pub extractor_http: Option<String>,
    pub extractor_uds: Option<String>,
    /// `LCG_REPLAY_LLM` cassette: free, reproducible extraction (ADR-0044).
    pub replay_cassette: Option<String>,
    /// `LCG_RECORD_LLM` cassette to write during this run's ingest.
    pub record_cassette: Option<String>,
}

/// Mock and hash embedders produce meaningless vectors, so a real embedder must be named
/// explicitly. (`LCG_EMBEDDING_URL` in the environment counts as explicit.)
pub fn validate_embedder(o: &ServiceOpts, env_url: Option<&str>) -> Result<(), String> {
    if o.embedder_http.is_some() && o.embedder_uds.is_some() {
        return Err("--embedder-http and --embedder-uds are mutually exclusive".into());
    }
    if o.embedder_http.is_none() && o.embedder_uds.is_none() && env_url.is_none_or(str::is_empty) {
        return Err(
            "no real embedder configured: pass --embedder-http/--embedder-uds (or set \
             LCG_EMBEDDING_URL). Mock and hash embedders give meaningless retrieval numbers, so \
             the retrieval eval refuses to run without one."
                .into(),
        );
    }
    Ok(())
}

pub fn service_args(o: &ServiceOpts) -> Vec<String> {
    let mut a = Vec::new();
    for (flag, v) in [
        ("--embedder-http", &o.embedder_http),
        ("--embedder-uds", &o.embedder_uds),
        ("--extractor-http", &o.extractor_http),
        ("--extractor-uds", &o.extractor_uds),
    ] {
        if let Some(v) = v {
            a.push(flag.to_string());
            a.push(v.clone());
        }
    }
    a
}

/// A running service. Dropping it kills the child (`kill_on_drop`) and removes the workspace.
pub struct ServiceGuard {
    pub client: Arc<UdsClient>,
    _child: tokio::process::Child,
    _workspace: tempfile::TempDir,
    pub version: String,
}

/// Version string of the service binary (`--version`), for the reproduction block.
pub async fn service_version(bin: &Path) -> String {
    match tokio::process::Command::new(bin)
        .arg("--version")
        .output()
        .await
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}

/// Last few lines of the service's captured stderr, for startup-failure messages.
fn stderr_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return "(no stderr output)".into();
    }
    lines[lines.len().saturating_sub(5)..].join(" | ")
}

pub async fn spawn_service(o: &ServiceOpts) -> Result<ServiceGuard, String> {
    validate_embedder(o, std::env::var("LCG_EMBEDDING_URL").ok().as_deref())?;
    let ws = tempfile::tempdir().map_err(|e| format!("cannot create workspace: {e}"))?;
    // Keep the service's stderr so a startup failure can be reported with its real cause.
    let stderr_path = ws.path().join("service.stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path)
        .map_err(|e| format!("cannot create stderr log: {e}"))?;
    let mut cmd = tokio::process::Command::new(&o.bin);
    cmd.args(service_args(o))
        .current_dir(ws.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr_file)
        .kill_on_drop(true);
    if let Some(p) = &o.replay_cassette {
        cmd.env("LCG_REPLAY_LLM", p);
    }
    if let Some(p) = &o.record_cassette {
        cmd.env("LCG_RECORD_LLM", p);
    }
    let mut child = cmd.spawn().map_err(|e| {
        format!(
            "cannot start service '{}': {e} (pass --service-bin, or build with `cargo build --release -p lcg-service`)",
            o.bin.display()
        )
    })?;
    let sock = ws.path().join(".lcg").join("service.sock");
    let deadline = Instant::now() + Duration::from_secs(120);
    let client = loop {
        // Fail fast if the service died at startup (bad embedder URL, conflicting record/replay
        // env, workspace error) instead of polling for the whole deadline.
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "service exited during startup ({status}): {}",
                stderr_tail(&stderr_path)
            ));
        }
        if sock.exists() {
            if let Ok(c) = UdsClient::connect(&sock).await {
                // The socket can be bound before the service answers; bound the probe by what is
                // left of the startup deadline so a silent service cannot outlive it.
                let remaining = deadline.saturating_duration_since(Instant::now());
                if tokio::time::timeout(remaining, c.call("knowledge_status", json!({})))
                    .await
                    .is_ok_and(|r| r.is_ok())
                {
                    break c;
                }
            }
        }
        if Instant::now() > deadline {
            return Err(format!(
                "service did not become ready within 120s; stderr: {}",
                stderr_tail(&stderr_path)
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    Ok(ServiceGuard {
        client: Arc::new(client),
        _child: child,
        _workspace: ws,
        version: service_version(&o.bin).await,
    })
}

/// Ingest-time lookup tables plus the cost of building them.
#[derive(Debug, Default)]
pub struct IngestMap {
    pub episode_note: HashMap<String, String>,
    pub entity_notes: HashMap<String, BTreeSet<String>>,
    pub chunks: usize,
    pub supersessions: usize,
    /// `knowledge_process_chunk` calls — each makes ≥1 extraction-LLM call (not individually counted).
    pub extraction_requests: usize,
    pub rpc_calls: usize,
    /// Entity→note links accepted although the service reported no `episode_uuids` to confirm
    /// them (the `source` filter is a CONTAINS match). Non-zero means `lcg-entities-facts`
    /// note credit may be over-attributed; surfaced in the report.
    pub unconfirmed_entity_links: usize,
    pub seconds: f64,
}

pub async fn ingest(t: &dyn RpcTransport, fx: &Fixture) -> Result<IngestMap, String> {
    let started = Instant::now();
    let mut map = IngestMap::default();
    let mut note_episodes: HashMap<String, BTreeSet<String>> = HashMap::new();

    for n in &fx.manifest.notes {
        let body = &fx.notes[&n.id];
        for (i, chunk) in chunk_markdown(body, fx.manifest.max_chunk_tokens)
            .iter()
            .enumerate()
        {
            let mut params = json!({
                "chunk_text": chunk,
                "chunk_id": format!("{}#{i}", n.id),
                "source_file": n.id,
                "group_id": EVAL_GROUP,
                "doc_id": n.id,
            });
            if let Some(rt) = &n.reference_time {
                params["reference_time"] = json!(rt);
            }
            let r = t
                .call("knowledge_process_chunk", params)
                .await
                .map_err(|e| format!("ingest of note '{}' chunk {i}: {e}", n.id))?;
            map.rpc_calls += 1;
            map.extraction_requests += 1;
            map.chunks += 1;
            let ep = r["episode_uuid"]
                .as_str()
                .ok_or_else(|| format!("ingest of '{}': response has no episode_uuid", n.id))?;
            map.episode_note.insert(ep.to_string(), n.id.clone());
            note_episodes
                .entry(n.id.clone())
                .or_default()
                .insert(ep.to_string());
        }
    }

    for s in &fx.manifest.supersedes {
        t.call(
            "knowledge_supersede_document",
            json!({
                "group_id": EVAL_GROUP,
                "superseding_doc_id": s.new_note,
                "superseded_doc_id": s.old_note,
                "effective_at": s.effective_at,
            }),
        )
        .await
        .map_err(|e| format!("supersede {}→{}: {e}", s.old_note, s.new_note))?;
        map.rpc_calls += 1;
        map.supersessions += 1;
    }

    // Explicit index build so the lazy first-search build never lands in a timed query.
    t.call("knowledge_build_indices", json!({})).await?;
    map.rpc_calls += 1;

    for n in &fx.manifest.notes {
        let r = t
            .call(
                "knowledge_get_entities_by_source",
                json!({"source": format!("{}:", n.id), "num_results": 1000, "group_ids": [EVAL_GROUP]}),
            )
            .await?;
        map.rpc_calls += 1;
        let mine = note_episodes.get(&n.id);
        for node in r["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let Some(uuid) = node["uuid"].as_str() else {
                continue;
            };
            // `source` is a CONTAINS match, so confirm via episode uuids when they are reported.
            let eps: Vec<&str> = node["episode_uuids"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let belongs =
                eps.is_empty() || mine.is_some_and(|m| eps.iter().any(|e| m.contains(*e)));
            if belongs {
                if eps.is_empty() {
                    map.unconfirmed_entity_links += 1;
                }
                map.entity_notes
                    .entry(uuid.to_string())
                    .or_default()
                    .insert(n.id.clone());
            }
        }
    }
    map.seconds = started.elapsed().as_secs_f64();
    Ok(map)
}

fn items(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}

/// `lcg-passages`: `knowledge_search_passages`. **Vector-only** — `search_passages` does no FTS
/// and no RRF — and says so in its description.
pub struct LcgPassages {
    pub t: Arc<dyn RpcTransport>,
}

impl Retriever for LcgPassages {
    fn id(&self) -> &str {
        "lcg-passages"
    }
    fn description(&self) -> String {
        "knowledge_search_passages (vector-only: no FTS, no RRF), min_score=0".into()
    }
    fn retrieve<'a>(&'a self, q: &'a str, k: usize) -> BoxFuture<'a, Result<RankedResult, String>> {
        Box::pin(async move {
            let t0 = Instant::now();
            let r = self
                .t
                .call(
                    "knowledge_search_passages",
                    json!({"query": q, "num_results": k, "min_score": 0.0, "group_ids": [EVAL_GROUP]}),
                )
                .await?;
            let latency = t0.elapsed();
            let (notes, evidence) = map_passages(&items(&r, "passages"));
            Ok(RankedResult {
                notes,
                evidence,
                model_calls: Some(1),
                lookup_calls: 0,
                latency,
            })
        })
    }
}

/// `lcg-entities-facts` (default reads) and `lcg-entities-facts-belief` (`believed_only=true` on
/// the fact read — only edge reads can express belief, ADR-0705).
pub struct LcgEntitiesFacts {
    pub t: Arc<dyn RpcTransport>,
    pub map: Arc<IngestMap>,
    pub belief: bool,
}

/// Per-read `num_results` for the entities+facts configurations: k is split across the entity and
/// fact reads (rounding up, so an odd k asks for one extra) so the retrieval budget is about k, the
/// same as the single-read configurations. An entity can still expand to several notes.
pub fn per_read_budget(k: usize) -> usize {
    k.div_ceil(2).max(1)
}

/// Interleaves two ranked lists (a0, b0, a1, b1, …) so neither dominates the top ranks.
pub fn interleave<T: Clone>(a: &[T], b: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    for i in 0..a.len().max(b.len()) {
        if let Some(x) = a.get(i) {
            out.push(x.clone());
        }
        if let Some(x) = b.get(i) {
            out.push(x.clone());
        }
    }
    out
}

impl Retriever for LcgEntitiesFacts {
    fn id(&self) -> &str {
        if self.belief {
            "lcg-entities-facts-belief"
        } else {
            "lcg-entities-facts"
        }
    }
    fn description(&self) -> String {
        format!(
            "knowledge_find_entities + knowledge_find_relationships (hybrid, RRF), min_similarity=0, {}; k split across the two reads (ceil(k/2) each), ranks interleaved",
            if self.belief { "facts read with believed_only=true" } else { "default reads" }
        )
    }
    fn retrieve<'a>(&'a self, q: &'a str, k: usize) -> BoxFuture<'a, Result<RankedResult, String>> {
        Box::pin(async move {
            let t0 = Instant::now();
            // Split k across the two reads so this configuration spends the same retrieval budget
            // (k results) as `lcg-passages` and qmd, instead of 2k entity + fact results.
            let per_read = per_read_budget(k);
            let ents = self
                .t
                .call(
                    "knowledge_find_entities",
                    json!({"query": q, "num_results": per_read, "min_similarity": 0.0, "group_ids": [EVAL_GROUP]}),
                )
                .await?;
            let mut fact_params = json!({"query": q, "num_results": per_read, "min_similarity": 0.0, "group_ids": [EVAL_GROUP]});
            if self.belief {
                fact_params["believed_only"] = json!(true);
            }
            let facts = self
                .t
                .call("knowledge_find_relationships", fact_params)
                .await?;
            let mut latency = t0.elapsed();

            let ent_nodes = items(&ents, "nodes");
            let (_, ent_evidence) = map_entities(&ent_nodes, &self.map.entity_notes);
            let ent_groups = entity_groups(&ent_nodes, &self.map.entity_notes);

            let mut edges = items(&facts, "edges");
            let unmapped = map_edges(&edges, &self.map.episode_note).unmapped;
            let mut lookup_calls = 0;
            // Fallback (counted): the read path did not carry `episodes`, so fetch them.
            if !unmapped.is_empty() {
                let t1 = Instant::now();
                let r = self
                    .t
                    .call("knowledge_get_edges_by_uuids", json!({"uuids": unmapped}))
                    .await?;
                latency += t1.elapsed();
                lookup_calls = 1;
                let fetched: HashMap<String, Value> = items(&r, "edges")
                    .into_iter()
                    .filter_map(|e| Some((e["uuid"].as_str()?.to_string(), e)))
                    .collect();
                for e in edges.iter_mut() {
                    if e["episodes"].as_array().is_none_or(Vec::is_empty) {
                        if let Some(f) = e["uuid"].as_str().and_then(|u| fetched.get(u)) {
                            e["episodes"] = f["episodes"].clone();
                        }
                    }
                }
            }
            let fact_evidence = map_edges(&edges, &self.map.episode_note).evidence;
            let fact_groups = edge_groups(&edges, &self.map.episode_note);
            let notes: Vec<String> = interleave(&ent_groups, &fact_groups)
                .into_iter()
                .flatten()
                .collect();
            let evidence = interleave(&ent_evidence, &fact_evidence);
            Ok(RankedResult {
                notes,
                evidence,
                model_calls: Some(2),
                lookup_calls,
                latency,
            })
        })
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Canned transport: `handler(method, params)`; every call is logged.
    type Handler = Box<dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync>;

    pub struct FakeTransport {
        pub handler: Handler,
        pub log: StdMutex<Vec<(String, Value)>>,
    }
    impl FakeTransport {
        pub fn new(
            f: impl Fn(&str, &Value) -> Result<Value, String> + Send + Sync + 'static,
        ) -> Self {
            Self {
                handler: Box::new(f),
                log: StdMutex::new(Vec::new()),
            }
        }
    }
    impl RpcTransport for FakeTransport {
        fn call<'a>(&'a self, m: &'a str, p: Value) -> BoxFuture<'a, Result<Value, String>> {
            self.log.lock().unwrap().push((m.to_string(), p.clone()));
            let r = (self.handler)(m, &p);
            Box::pin(async move { r })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeTransport;
    use super::*;

    #[test]
    fn embedder_is_required() {
        let o = ServiceOpts::default();
        assert!(validate_embedder(&o, None)
            .unwrap_err()
            .contains("no real embedder"));
        assert!(validate_embedder(&o, Some("")).is_err());
        assert!(validate_embedder(&o, Some("http://x")).is_ok());
        let o = ServiceOpts {
            embedder_http: Some("http://e".into()),
            ..Default::default()
        };
        assert!(validate_embedder(&o, None).is_ok());
        let both = ServiceOpts {
            embedder_http: Some("a".into()),
            embedder_uds: Some("b".into()),
            ..Default::default()
        };
        assert!(validate_embedder(&both, None)
            .unwrap_err()
            .contains("exclusive"));
    }

    #[test]
    fn service_args_pass_flags() {
        let o = ServiceOpts {
            embedder_http: Some("http://e".into()),
            extractor_uds: Some("/s".into()),
            ..Default::default()
        };
        assert_eq!(
            service_args(&o),
            vec!["--embedder-http", "http://e", "--extractor-uds", "/s"]
        );
    }

    #[test]
    fn interleave_alternates() {
        assert_eq!(interleave(&[1, 2, 3], &[10, 20]), vec![1, 10, 2, 20, 3]);
        assert_eq!(interleave::<i32>(&[], &[]), Vec::<i32>::new());
    }

    #[tokio::test]
    async fn passages_retriever_sets_k_and_zero_floor() {
        let t = Arc::new(FakeTransport::new(|m, _| {
            assert_eq!(m, "knowledge_search_passages");
            Ok(json!({"passages":[{"content":"c","source_description":"n1:n1#0"}],"count":1}))
        }));
        let r = LcgPassages { t: t.clone() }.retrieve("q", 7).await.unwrap();
        assert_eq!(r.notes, vec!["n1"]);
        assert_eq!(r.model_calls, Some(1));
        let log = t.log.lock().unwrap();
        assert_eq!(log[0].1["num_results"], 7);
        assert_eq!(log[0].1["min_score"], 0.0);
        assert_eq!(log[0].1["group_ids"], json!([EVAL_GROUP]));
    }

    fn map() -> Arc<IngestMap> {
        let mut m = IngestMap::default();
        m.episode_note.insert("ep1".into(), "note-a".into());
        m.episode_note.insert("ep2".into(), "note-b".into());
        m.entity_notes
            .insert("e1".into(), ["note-b".to_string()].into_iter().collect());
        Arc::new(m)
    }

    #[tokio::test]
    async fn entities_facts_interleave_and_belief_flag() {
        let t = Arc::new(FakeTransport::new(|m, _| match m {
            "knowledge_find_entities" => {
                Ok(json!({"nodes":[{"uuid":"e1","name":"E","summary":"s"}]}))
            }
            "knowledge_find_relationships" => {
                Ok(json!({"edges":[{"uuid":"f1","fact":"F","episodes":["ep1"]}]}))
            }
            other => Err(format!("unexpected {other}")),
        }));
        let rt = LcgEntitiesFacts {
            t: t.clone(),
            map: map(),
            belief: true,
        };
        assert_eq!(rt.id(), "lcg-entities-facts-belief");
        let r = rt.retrieve("q", 5).await.unwrap();
        assert_eq!(r.notes, vec!["note-b", "note-a"]);
        assert_eq!(r.evidence, vec!["E: s", "F"]);
        assert_eq!(r.lookup_calls, 0);
        let log = t.log.lock().unwrap();
        let fact_call = log
            .iter()
            .find(|(m, _)| m == "knowledge_find_relationships")
            .unwrap();
        assert_eq!(fact_call.1["believed_only"], true);
        let ent_call = log
            .iter()
            .find(|(m, _)| m == "knowledge_find_entities")
            .unwrap();
        assert!(ent_call.1.get("believed_only").is_none());
        assert_eq!(
            ent_call.1["num_results"], 3,
            "k=5 is split across the entity and fact reads (ceil(5/2))"
        );
        assert_eq!(fact_call.1["num_results"], 3);
        assert_eq!(per_read_budget(10), 5);
        assert_eq!(per_read_budget(1), 1);
    }

    #[tokio::test]
    async fn missing_episodes_triggers_counted_fallback_lookup() {
        let t = Arc::new(FakeTransport::new(|m, _| match m {
            "knowledge_find_entities" => Ok(json!({"nodes":[]})),
            "knowledge_find_relationships" => Ok(json!({"edges":[{"uuid":"f1","fact":"F"}]})),
            "knowledge_get_edges_by_uuids" => {
                Ok(json!({"edges":[{"uuid":"f1","episodes":["ep2"]}]}))
            }
            other => Err(format!("unexpected {other}")),
        }));
        let rt = LcgEntitiesFacts {
            t,
            map: map(),
            belief: false,
        };
        let r = rt.retrieve("q", 5).await.unwrap();
        assert_eq!(r.notes, vec!["note-b"]);
        assert_eq!(r.lookup_calls, 1);
    }

    fn fixture() -> Fixture {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("notes")).unwrap();
        std::fs::write(
            d.path().join("manifest.json"),
            r#"{"corpus_version":"v1","notes":[{"id":"old","reference_time":"2026-01-01T00:00:00Z"},{"id":"new"}],
               "supersedes":[{"old_note":"old","new_note":"new","effective_at":"2026-02-01T00:00:00Z"}]}"#,
        )
        .unwrap();
        std::fs::write(d.path().join("notes/old.md"), "# Old\nuse vendor X").unwrap();
        std::fs::write(d.path().join("notes/new.md"), "# New\nuse vendor Y").unwrap();
        std::fs::write(
            d.path().join("questions.jsonl"),
            r#"{"id":"q1","category":"direct","text":"hi","relevant_notes":[{"note":"new"}]}"#,
        )
        .unwrap();
        let f = super::super::fixture::load(d.path()).unwrap();
        std::mem::forget(d);
        f
    }

    #[tokio::test]
    async fn a_silent_service_times_out_instead_of_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        // Accepts the connection, reads nothing back, never answers.
        let _server = tokio::spawn(async move {
            let (_s, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let c = UdsClient::connect(&sock)
            .await
            .unwrap()
            .with_timeout(Duration::from_millis(100));
        let started = Instant::now();
        let e = c.call("knowledge_status", json!({})).await.unwrap_err();
        assert!(e.contains("no response"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn ingest_builds_maps_supersedes_and_builds_indices() {
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c2 = counter.clone();
        let t = FakeTransport::new(move |m, p| match m {
            "knowledge_process_chunk" => {
                let n = c2.fetch_add(1, Ordering::Relaxed);
                assert_eq!(p["group_id"], EVAL_GROUP);
                assert_eq!(p["doc_id"], p["source_file"]);
                Ok(json!({"episode_uuid": format!("ep{n}")}))
            }
            "knowledge_supersede_document" => Ok(json!({})),
            "knowledge_build_indices" => Ok(json!({})),
            "knowledge_get_entities_by_source" => {
                if p["source"] == "old:" {
                    Ok(
                        json!({"nodes":[{"uuid":"e1","episode_uuids":["ep0"]},{"uuid":"e9","episode_uuids":["ep1"]}]}),
                    )
                } else {
                    Ok(json!({"nodes":[{"uuid":"e2"}]}))
                }
            }
            other => Err(format!("unexpected {other}")),
        });
        let m = ingest(&t, &fixture()).await.unwrap();
        assert_eq!(m.chunks, 2);
        assert_eq!(m.supersessions, 1);
        assert_eq!(m.episode_note["ep0"], "old");
        // e9's episodes belong to note "new", so CONTAINS over-matching must not credit "old".
        assert!(m.entity_notes["e1"].contains("old"));
        assert!(!m.entity_notes.contains_key("e9"));
        assert!(
            m.entity_notes["e2"].contains("new"),
            "no episode_uuids reported → accepted"
        );
        assert_eq!(
            m.unconfirmed_entity_links, 1,
            "the link accepted without episode_uuids is counted, not hidden"
        );
        let log = t.log.lock().unwrap();
        let order: Vec<&str> = log.iter().map(|(m, _)| m.as_str()).collect();
        let sup = order
            .iter()
            .position(|m| *m == "knowledge_supersede_document")
            .unwrap();
        let idx = order
            .iter()
            .position(|m| *m == "knowledge_build_indices")
            .unwrap();
        assert!(sup < idx);
        assert_eq!(log[0].1["reference_time"], "2026-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn ingest_failure_names_the_note() {
        let t = FakeTransport::new(|_, _| Err("boom".into()));
        let e = ingest(&t, &fixture()).await.unwrap_err();
        assert!(e.contains("'old'") && e.contains("boom"), "{e}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn service_that_dies_at_startup_fails_fast_with_its_stderr() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("fake-service");
        std::fs::write(
            &bin,
            "#!/bin/sh\necho 'embedder unreachable: boom' >&2\nexit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let o = ServiceOpts {
            bin,
            embedder_http: Some("http://127.0.0.1:1".into()),
            ..Default::default()
        };
        let t0 = Instant::now();
        let e = match spawn_service(&o).await {
            Err(e) => e,
            Ok(_) => panic!("expected startup failure"),
        };
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "must not wait out the deadline"
        );
        assert!(
            e.contains("exited during startup") && e.contains("embedder unreachable: boom"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn uds_client_round_trips_and_skips_progress_lines() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            let mut r = BufReader::new(s);
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let req: Value = serde_json::from_str(&line).unwrap();
                let id = req["id"].clone();
                let out = if req["method"] == "fail" {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-1,"message":"nope"}})
                } else {
                    json!({"jsonrpc":"2.0","id":id,"result":{"echo":req["method"]}})
                };
                let w = r.get_mut();
                w.write_all(b"{\"type\":\"progress\",\"n\":1}\n")
                    .await
                    .unwrap();
                w.write_all(format!("{out}\n").as_bytes()).await.unwrap();
            }
        });
        let c = UdsClient::connect(&path).await.unwrap();
        assert_eq!(c.call("hello", json!({})).await.unwrap()["echo"], "hello");
        assert!(c
            .call("fail", json!({}))
            .await
            .unwrap_err()
            .contains("nope"));
    }
}
