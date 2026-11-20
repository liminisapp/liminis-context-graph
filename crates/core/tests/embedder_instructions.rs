//! Issue #790: embedder roles and instruction profiles.
//!
//! Covers the behavioural acceptance criteria end to end with a recording embedder (the exact
//! strings and roles the embedder receives from every call site), and the wire-level behaviour of
//! the role-advertising sidecar protocol with a stub HTTP server:
//!
//! - SC-001: profile `none` sends the raw text, byte for byte, from every call site.
//! - SC-002/SC-009: per-profile exact strings; exactly one instruction per text, never two.
//! - SC-003/SC-004: query-prefix-only switch is a `match`; a document-prefix switch is a
//!   `mismatch` that a live write cannot silently clear, and that a rebuild does.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::build_embedder_stack,
    extractor::ConfigurableExtractor,
    handlers,
    ipc::IpcRequest,
    telemetry::NoopSink,
    types::{ExtractedEdge, ExtractedEntity, ExtractionResult},
    EmbedRole, Embedder, EmbedderAdvert, EmbeddingIdentity, InstructedEmbedder, InstructionConfig,
    InstructionProfile, InstructionSource, OaiEmbedder,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const GRP: &str = "test-group";
const BGE: &str = "Represent this sentence for searching relevant passages: ";

// ── Recording embedder ───────────────────────────────────────────────────────

#[derive(Default)]
struct Recording {
    seen: Mutex<Vec<(String, EmbedRole)>>,
}

impl Recording {
    fn take(&self) -> Vec<(String, EmbedRole)> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}

impl Embedder for Recording {
    fn embed<'a>(
        &'a self,
        text: &'a str,
        role: EmbedRole,
    ) -> BoxFuture<'a, Result<Vec<f32>, lcg_core::error::Error>> {
        self.seen.lock().unwrap().push((text.to_string(), role));
        // Deterministic, non-zero vector derived from the text so search has something to rank.
        let n = text.bytes().map(|b| b as u32).sum::<u32>() as f32;
        Box::pin(async move { Ok(vec![1.0, (n % 7.0) + 1.0, (n % 5.0) + 1.0, 0.5]) })
    }

    fn dim(&self) -> usize {
        DIM
    }
}

fn instructed(profile: InstructionProfile) -> (Arc<dyn Embedder>, Arc<Recording>) {
    let rec = Arc::new(Recording::default());
    let eff = InstructionConfig {
        profile,
        ..Default::default()
    }
    .resolve(None)
    .unwrap();
    let stack: Arc<dyn Embedder> = if eff.source == InstructionSource::Lcg {
        Arc::new(InstructedEmbedder::new(rec.clone(), eff))
    } else {
        rec.clone()
    };
    (stack, rec)
}

// ── State plumbing ───────────────────────────────────────────────────────────

fn open_db(dir: &TempDir) -> Arc<Db> {
    let db = Arc::new(Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.build_indices_and_constraints().unwrap();
    }
    db
}

fn make_state(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    extractor: ConfigurableExtractor,
    wal_root: Option<std::path::PathBuf>,
) -> Arc<AppState> {
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder,
        extractor: Arc::new(extractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink: Arc::new(NoopSink),
        db_path: "test.db".to_string(),
        wal_root,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built: Arc::new(AtomicBool::new(true)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: None,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn extraction() -> ConfigurableExtractor {
    ConfigurableExtractor::new(vec![ExtractionResult {
        entities: vec![
            ExtractedEntity {
                name: "Alice".to_string(),
                entity_type: "Entity".to_string(),
                summary: "runs a bakery".to_string(),
                original_entity_type: None,
            },
            ExtractedEntity {
                name: "Bob".to_string(),
                entity_type: "Entity".to_string(),
                summary: "".to_string(),
                original_entity_type: None,
            },
        ],
        edges: vec![
            ExtractedEdge {
                source_name: "Alice".to_string(),
                target_name: "Bob".to_string(),
                fact: "Alice knows Bob".to_string(),
                ..Default::default()
            },
            // Off-list endpoint "Dave" is looked up (salvage) as a Document.
            ExtractedEdge {
                source_name: "Dave".to_string(),
                target_name: "Bob".to_string(),
                fact: "Dave mentors Bob".to_string(),
                ..Default::default()
            },
        ],
    }])
}

async fn dispatch(method: &str, params: Value, state: Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    let v = serde_json::to_value(handlers::dispatch(req, state, None).await).unwrap();
    assert!(v.get("error").is_none(), "{method} errored: {}", v["error"]);
    v["result"].clone()
}

async fn ingest(state: &Arc<AppState>) {
    dispatch(
        "knowledge_add_episode",
        json!({
            "name": "episode-1",
            "episode_body": "Alice runs a bakery and knows Bob.",
            "source": "text",
            "reference_time": "2026-01-01T00:00:00Z",
            "group_id": GRP,
        }),
        Arc::clone(state),
    )
    .await;
}

const QUERIES: [&str; 3] = ["bread shop", "who runs it", "acquaintance"];

async fn run_all_searches(state: &Arc<AppState>) {
    dispatch(
        "knowledge_search_passages",
        json!({ "query": QUERIES[0], "group_ids": [GRP], "num_results": 3 }),
        Arc::clone(state),
    )
    .await;
    dispatch(
        "knowledge_find_entities",
        json!({ "query": QUERIES[1], "group_ids": [GRP], "num_results": 3 }),
        Arc::clone(state),
    )
    .await;
    dispatch(
        "knowledge_find_relationships",
        json!({ "query": QUERIES[2], "group_ids": [GRP], "num_results": 3 }),
        Arc::clone(state),
    )
    .await;
}

async fn workload(profile: InstructionProfile) -> Vec<(String, EmbedRole)> {
    let dir = TempDir::new().unwrap();
    let (stack, rec) = instructed(profile);
    let state = make_state(open_db(&dir), stack, extraction(), None);
    ingest(&state).await;
    run_all_searches(&state).await;
    rec.take()
}

fn sorted(mut v: Vec<(String, EmbedRole)>) -> Vec<(String, u8)> {
    v.sort_by(|a, b| a.0.cmp(&b.0).then((a.1 as u8).cmp(&(b.1 as u8))));
    v.into_iter().map(|(t, r)| (t, r as u8)).collect()
}

// ── SC-001 / call-site roles ─────────────────────────────────────────────────

/// Under profile `none` the embedder receives the raw text, and every call site states the role
/// the catalogue assigns it: the three search strings are `Query`; everything stored is
/// `Document`.
#[tokio::test]
async fn none_profile_sends_raw_text_with_catalogue_roles() {
    let seen = workload(InstructionProfile::None).await;
    let queries: Vec<&str> = seen
        .iter()
        .filter(|(_, r)| *r == EmbedRole::Query)
        .map(|(t, _)| t.as_str())
        .collect();
    let mut q_sorted = queries.clone();
    q_sorted.sort();
    let mut expected = QUERIES.to_vec();
    expected.sort();
    assert_eq!(
        q_sorted, expected,
        "exactly the three search strings are Query"
    );

    let docs: Vec<&str> = seen
        .iter()
        .filter(|(_, r)| *r == EmbedRole::Document)
        .map(|(t, _)| t.as_str())
        .collect();
    for raw in [
        "Alice runs a bakery and knows Bob.", // episode content
        "Alice",                              // entity name
        "runs a bakery",                      // entity summary
        "Alice knows Bob",                    // edge fact
        "Dave",                               // off-list endpoint materialised/looked up
    ] {
        assert!(
            docs.contains(&raw),
            "missing Document embed of {raw:?}: {docs:?}"
        );
    }
    // Nothing was prefixed: no stored text contains a profile prefix.
    assert!(seen.iter().all(|(t, _)| !t.starts_with("search_")
        && !t.starts_with(BGE)
        && !t.starts_with("query: ")));
}

/// SC-002: for each profile the strings the embedder sees are exactly the `none` run's strings
/// with the profile's prefix for that string's role prepended — same call sites, same roles, no
/// extra or missing embeds.
#[tokio::test]
async fn profiles_prefix_exactly_per_role_across_every_call_site() {
    let baseline = workload(InstructionProfile::None).await;
    for (profile, q_prefix, d_prefix) in [
        (InstructionProfile::BgeV15, BGE, ""),
        (
            InstructionProfile::NomicV15,
            "search_query: ",
            "search_document: ",
        ),
        (InstructionProfile::E5, "query: ", "passage: "),
    ] {
        let seen = workload(profile).await;
        let expected: Vec<(String, EmbedRole)> = baseline
            .iter()
            .map(|(t, r)| {
                let p = if *r == EmbedRole::Query {
                    q_prefix
                } else {
                    d_prefix
                };
                // Blank text is never prefixed (FR-014).
                if t.trim().is_empty() {
                    (t.clone(), *r)
                } else {
                    (format!("{p}{t}"), *r)
                }
            })
            .collect();
        assert_eq!(
            sorted(seen),
            sorted(expected),
            "profile {} must prefix per role and change nothing else",
            profile.as_str()
        );
    }
}

// ── Status / identity (SC-003, SC-004) ───────────────────────────────────────

async fn group_status(state: &Arc<AppState>) -> Value {
    let status = dispatch("knowledge_status", json!({}), Arc::clone(state)).await;
    status["wal_groups"][GRP].clone()
}

fn identity(prefix: &str) -> EmbeddingIdentity {
    EmbeddingIdentity {
        model: "bge-base-en-v1.5".to_string(),
        dim: DIM as i64,
        doc_prefix: prefix.to_string(),
    }
}

/// SC-003: a workspace recorded under `none` and started under `bge-v1.5` (query instruction
/// only, empty document prefix) is a `match`; the next search carries the instruction.
#[tokio::test]
async fn bge_profile_on_existing_workspace_is_match_and_instructs_queries() {
    let dir = TempDir::new().unwrap();
    let wal = TempDir::new().unwrap();
    let db = open_db(&dir);
    // Recorded under profile `none` (and, like every pre-#790 row, with no prefix at all).
    db.connect()
        .unwrap()
        .set_wal_position(GRP, 1, None, Some(&identity("")))
        .unwrap();
    std::fs::create_dir_all(wal.path().join(GRP)).ok();
    let (stack, rec) = instructed(InstructionProfile::BgeV15);
    let state = make_state(db, stack, extraction(), Some(wal.path().to_path_buf()));
    ingest(&state).await;
    rec.take();

    let status = group_status(&state).await;
    assert_eq!(status["embedding_model_status"], "match", "{status}");

    dispatch(
        "knowledge_search_passages",
        json!({ "query": "q", "group_ids": [GRP], "num_results": 1 }),
        Arc::clone(&state),
    )
    .await;
    assert_eq!(
        rec.take(),
        vec![(format!("{BGE}q"), EmbedRole::Query)],
        "the search string is embedded with the BGE instruction"
    );
}

/// The effective instructions are visible in `knowledge_status` (FR-017).
#[tokio::test]
async fn status_reports_effective_instructions() {
    let dir = TempDir::new().unwrap();
    let (stack, _rec) = instructed(InstructionProfile::NomicV15);
    let state = make_state(open_db(&dir), stack, extraction(), None);
    let status = dispatch("knowledge_status", json!({}), state).await;
    assert_eq!(
        status["embedding_instructions"],
        json!({
            "profile": "nomic-v1.5",
            "source": "lcg",
            "query_prefix": "search_query: ",
            "document_prefix": "search_document: ",
        })
    );
}

/// SC-004: `none` → `nomic-v1.5` on an existing workspace is a `mismatch`; an ordinary live
/// write must not silently clear it; the documented rebuild path does, and re-embeds every
/// stored vector under the new document prefix.
#[tokio::test]
async fn document_prefix_switch_is_mismatch_until_rebuild() {
    let db_dir = TempDir::new().unwrap();
    let wal = TempDir::new().unwrap();
    let db = open_db(&db_dir);

    // Phase 1: ingest under profile `none` — the workspace records an empty document prefix.
    let (none_stack, _r) = instructed(InstructionProfile::None);
    let state_none = make_state(
        Arc::clone(&db),
        none_stack,
        extraction(),
        Some(wal.path().to_path_buf()),
    );
    ingest(&state_none).await;
    let status = group_status(&state_none).await;
    assert_eq!(status["embedding_model_status"], "match", "{status}");

    // Phase 2: same workspace, now nomic-v1.5 (document prefix `search_document: `).
    let (nomic_stack, rec) = instructed(InstructionProfile::NomicV15);
    let state_nomic = make_state(
        Arc::clone(&db),
        nomic_stack,
        extraction(),
        Some(wal.path().to_path_buf()),
    );
    let status = group_status(&state_nomic).await;
    assert_eq!(status["embedding_model_status"], "mismatch", "{status}");

    // A live write under the new profile advances the position but must not flip to `match`.
    dispatch(
        "knowledge_add_episode",
        json!({
            "name": "episode-2",
            "episode_body": "A second episode.",
            "source": "text",
            "reference_time": "2026-01-02T00:00:00Z",
            "group_id": GRP,
        }),
        Arc::clone(&state_nomic),
    )
    .await;
    let status = group_status(&state_nomic).await;
    assert_eq!(
        status["embedding_model_status"], "mismatch",
        "a live write must not silently mix vectors by re-stamping the prefix: {status}"
    );

    // Phase 3: the documented rebuild path recomputes every stored vector (as Document, with the
    // new prefix) and records the new identity.
    rec.take();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(7),
        method: "knowledge_rebuild_from_wal".to_string(),
        params: json!({"group_id": GRP, "from_seq": 0, "force_clear": true}),
    };
    let resp =
        serde_json::to_value(handlers::dispatch(req, Arc::clone(&state_nomic), Some(tx)).await)
            .unwrap();
    assert!(resp.get("error").is_none(), "rebuild errored: {resp}");
    let recomputed = rec.take();
    assert!(
        !recomputed.is_empty()
            && recomputed
                .iter()
                .all(|(t, r)| *r == EmbedRole::Document && t.starts_with("search_document: ")),
        "every recomputed vector is a Document embed carrying the new prefix: {recomputed:?}"
    );
    let status = group_status(&state_nomic).await;
    assert_eq!(status["embedding_model_status"], "match", "{status}");
    assert_eq!(status["embedding_doc_prefix"], "search_document: ");
}

// ── Wire protocol and single-applier (SC-009) ────────────────────────────────

/// A stub `/v1/embeddings` server that records every request body and optionally advertises
/// role support, as the bundled sidecar does.
async fn spawn_stub(advertise: bool) -> (String, Arc<Mutex<Vec<Value>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let bodies_c = Arc::clone(&bodies);
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let bodies = Arc::clone(&bodies_c);
            tokio::spawn(async move {
                let (r, mut w) = stream.into_split();
                let mut r = BufReader::new(r);
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).await.unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; len];
                r.read_exact(&mut body).await.ok();
                let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let n = req["input"].as_array().map(|a| a.len()).unwrap_or(1);
                bodies.lock().unwrap().push(req);
                let data: Vec<Value> = (0..n)
                    .map(|i| json!({"object":"embedding","embedding":[0.1,0.2,0.3,0.4],"index":i}))
                    .collect();
                let mut resp = json!({"object":"list","data":data,"model":"stub-model"});
                if advertise {
                    resp["instruction_roles"] = json!({"query": BGE, "document": ""});
                }
                let body = resp.to_string();
                let http = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                w.write_all(http.as_bytes()).await.ok();
            });
        }
    });
    (format!("http://{addr}/v1/embeddings"), bodies)
}

fn advert() -> EmbedderAdvert {
    EmbedderAdvert {
        query: BGE.to_string(),
        document: String::new(),
    }
}

#[tokio::test]
async fn probe_reports_advertisement_only_when_present() {
    let (url, _) = spawn_stub(true).await;
    let info = OaiEmbedder::new_http(url, "m", 1)
        .unwrap()
        .probe()
        .await
        .unwrap();
    assert_eq!(info.model, "stub-model");
    assert_eq!(info.advert, Some(advert()));

    let (url, _) = spawn_stub(false).await;
    let info = OaiEmbedder::new_http(url, "m", 1)
        .unwrap()
        .probe()
        .await
        .unwrap();
    assert_eq!(info.advert, None);
}

/// FR-023 / SC-010: with nothing opted into, the request is exactly `{input, model}` — no
/// `input_type` — even against a server that advertised role support (an advertisement alone
/// never changes behaviour).
#[tokio::test]
async fn advertised_support_alone_sends_plain_input_model_requests() {
    let (url, bodies) = spawn_stub(true).await;
    let oai = OaiEmbedder::new_http(url, "m", 4).unwrap();
    let info = oai.probe().await.unwrap();
    let (stack, eff) =
        build_embedder_stack(oai, &InstructionConfig::default(), info.advert.as_ref()).unwrap();
    assert_eq!(eff.source, InstructionSource::None);
    stack.embed("hello", EmbedRole::Query).await.unwrap();
    stack
        .embed_batch(&["a", "b"], EmbedRole::Document)
        .await
        .unwrap();
    for b in bodies.lock().unwrap().iter().skip(1) {
        let keys: Vec<&String> = b.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 2, "only input+model: {b}");
        assert!(b.get("input_type").is_none());
    }
}

/// Profile `embedder` against an advertising server: plain text plus `input_type`, no LCG prefix.
#[tokio::test]
async fn embedder_profile_sends_plain_text_with_input_type() {
    let (url, bodies) = spawn_stub(true).await;
    let oai = OaiEmbedder::new_http(url, "m", 4).unwrap();
    let info = oai.probe().await.unwrap();
    let cfg = InstructionConfig {
        profile: InstructionProfile::Embedder,
        ..Default::default()
    };
    let (stack, eff) = build_embedder_stack(oai, &cfg, info.advert.as_ref()).unwrap();
    assert_eq!(eff.source, InstructionSource::Embedder);
    assert_eq!(
        stack.instructions().document_prefix,
        "",
        "identity reflects the advert"
    );
    bodies.lock().unwrap().clear();

    stack.embed("find bread", EmbedRole::Query).await.unwrap();
    stack.embed("a loaf", EmbedRole::Document).await.unwrap();
    stack
        .embed_batch(&["x", "y"], EmbedRole::Query)
        .await
        .unwrap();
    let b = bodies.lock().unwrap().clone();
    assert_eq!(
        b[0]["input"], "find bread",
        "plain text, no LCG-side prefix"
    );
    assert_eq!(b[0]["input_type"], "query");
    assert_eq!(b[1]["input"], "a loaf");
    assert_eq!(b[1]["input_type"], "document");
    assert_eq!(b[2]["input"], json!(["x", "y"]));
    assert_eq!(b[2]["input_type"], "query");
}

/// SC-009 / FR-022: a role-advertising server plus an LCG profile — LCG is the single applier:
/// the prefix is in the text, `input_type` is never sent, so the sidecar adds nothing. Exactly
/// one instruction per text.
#[tokio::test]
async fn lcg_profile_against_advertising_server_applies_exactly_one_instruction() {
    let (url, bodies) = spawn_stub(true).await;
    let oai = OaiEmbedder::new_http(url, "m", 4).unwrap();
    let info = oai.probe().await.unwrap();
    let cfg = InstructionConfig {
        profile: InstructionProfile::BgeV15,
        ..Default::default()
    };
    let (stack, eff) = build_embedder_stack(oai, &cfg, info.advert.as_ref()).unwrap();
    assert_eq!(eff.source, InstructionSource::Lcg);
    bodies.lock().unwrap().clear();

    stack.embed("find bread", EmbedRole::Query).await.unwrap();
    stack.embed("a loaf", EmbedRole::Document).await.unwrap();
    let b = bodies.lock().unwrap().clone();
    assert_eq!(b[0]["input"], format!("{BGE}find bread"));
    assert_eq!(b[0]["input"].as_str().unwrap().matches(BGE).count(), 1);
    assert!(
        b[0].get("input_type").is_none(),
        "server must not also apply its own"
    );
    assert_eq!(b[1]["input"], "a loaf");
    assert!(b[1].get("input_type").is_none());
}

/// `embedder` profile against a server that advertised nothing falls back to `none`: plain
/// `{input, model}`, no `input_type`, no prefix (never send `input_type` unprompted).
#[tokio::test]
async fn embedder_profile_without_advertisement_behaves_as_none() {
    let (url, bodies) = spawn_stub(false).await;
    let oai = OaiEmbedder::new_http(url, "m", 4).unwrap();
    let info = oai.probe().await.unwrap();
    let cfg = InstructionConfig {
        profile: InstructionProfile::Embedder,
        ..Default::default()
    };
    let (stack, eff) = build_embedder_stack(oai, &cfg, info.advert.as_ref()).unwrap();
    assert_eq!(eff.source, InstructionSource::None);
    bodies.lock().unwrap().clear();
    stack.embed("find bread", EmbedRole::Query).await.unwrap();
    let b = bodies.lock().unwrap().clone();
    assert_eq!(b[0]["input"], "find bread");
    assert!(b[0].get("input_type").is_none());
}

#[tokio::test]
async fn embedder_profile_with_prefix_override_is_rejected_at_startup() {
    let (url, _) = spawn_stub(true).await;
    let oai = OaiEmbedder::new_http(url, "m", 4).unwrap();
    let cfg = InstructionConfig {
        profile: InstructionProfile::Embedder,
        query_override: Some("x: ".to_string()),
        document_override: None,
    };
    assert!(build_embedder_stack(oai, &cfg, Some(&advert())).is_err());
}
