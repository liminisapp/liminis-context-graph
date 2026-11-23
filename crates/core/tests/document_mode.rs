//! Issue #785 / ADR-0785: document mode. `knowledge_process_document` ingests one whole
//! document as a completeness claim; the server chunks, diffs against the previous committed
//! revision, extracts only what changed (with document context), and commits the revision in one
//! step so a failed ingest retires nothing. `knowledge_preview_chunks` is the read-only chunker.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::ExtractOptions,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    Db, Error, ExtractedEdge, ExtractedEntity, ExtractionOutcome, ExtractionResult, Extractor,
    WalReplayer, WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "liminis";

// ── Fixture ─────────────────────────────────────────────────────────────────

/// One recorded extraction call.
#[derive(Debug, Clone)]
struct Call {
    body: String,
    context: Option<String>,
}

/// Extracts exactly one fact per call — the body's last non-empty line — between two entities
/// named after it, so every chunk's facts are identifiable. Records each call and can be told to
/// fail on its n-th call (1-based).
#[derive(Default)]
struct ScriptExtractor {
    calls: Mutex<Vec<Call>>,
    fail_on: AtomicUsize,
    count: AtomicUsize,
}

fn fact_of(body: &str) -> String {
    body.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

impl ScriptExtractor {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl Extractor for ScriptExtractor {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        let fail = self.fail_on.load(Ordering::SeqCst) == n;
        self.calls.lock().unwrap().push(Call {
            body: opts.episode_body.to_string(),
            context: opts.document_context.map(str::to_string),
        });
        let fact = fact_of(opts.episode_body);
        Box::pin(async move {
            if fail {
                return Err(Error::Ipc("injected extractor failure".to_string()));
            }
            let (s, t) = (format!("{fact} src"), format!("{fact} dst"));
            Ok(ExtractionResult {
                entities: [&s, &t]
                    .iter()
                    .map(|n| ExtractedEntity {
                        name: n.to_string(),
                        entity_type: "Thing".to_string(),
                        summary: String::new(),
                        original_entity_type: None,
                    })
                    .collect(),
                edges: vec![ExtractedEdge {
                    source_name: s,
                    target_name: t,
                    fact,
                    relation_type: Some("RELATED_TO".to_string()),
                    valid_at: None,
                    invalid_at: None,
                    original_relation_type: None,
                }],
            }
            .into())
        })
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        _allowed: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let n = entities.len();
        Box::pin(async move { Ok(vec![String::new(); n]) })
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        _allowed: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let n = edges.len();
        Box::pin(async move { Ok(vec![String::new(); n]) })
    }
}

struct Fx {
    state: Arc<AppState>,
    db: Arc<Db>,
    ex: Arc<ScriptExtractor>,
    wal: TempDir,
    _dir: TempDir,
}

fn make_state(
    db: Arc<Db>,
    wal_dir: Option<PathBuf>,
    extractor: Arc<dyn Extractor>,
) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let writers: HashMap<String, WalWriter> = wal_dir
        .as_ref()
        .and_then(|d| WalWriter::new(d, 10_000, 5 * 1024 * 1024).ok())
        .map(|w| (G.to_string(), w))
        .into_iter()
        .collect();
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor,
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(tokio::sync::RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: wal_dir,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(writers)),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: None,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        chunk_inflight: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn fresh_db(dir: &TempDir) -> Arc<Db> {
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    let conn = db.connect().unwrap();
    conn.init_schema(DIM).unwrap();
    conn.create_vector_indexes().unwrap();
    drop(conn);
    db
}

fn fx() -> Fx {
    let dir = TempDir::new().unwrap();
    let wal = TempDir::new().unwrap();
    let db = fresh_db(&dir);
    let ex = Arc::new(ScriptExtractor::default());
    let state = make_state(
        Arc::clone(&db),
        Some(wal.path().to_path_buf()),
        Arc::clone(&ex) as Arc<dyn Extractor>,
    );
    Fx {
        state,
        db,
        ex,
        wal,
        _dir: dir,
    }
}

async fn try_call(method: &str, params: Value, state: &Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, Arc::clone(state), None).await).unwrap()
}

async fn call(method: &str, params: Value, state: &Arc<AppState>) -> Value {
    let v = try_call(method, params, state).await;
    assert!(v.get("error").is_none(), "{method} errored: {v}");
    v["result"].clone()
}

fn doc_params(doc_id: &str, content: &str) -> Value {
    json!({
        "doc_id": doc_id,
        "group_id": G,
        "content": content,
        "title": "Design note",
        "source_path": "notes/design.md",
        "reference_time": "2026-11-01T00:00:00Z",
    })
}

async fn ingest(state: &Arc<AppState>, doc_id: &str, content: &str) -> Value {
    call(
        "knowledge_process_document",
        doc_params(doc_id, content),
        state,
    )
    .await
}

fn classes(r: &Value) -> Vec<String> {
    r["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["classification"].as_str().unwrap().to_string())
        .collect()
}

fn uuids(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|u| u.as_str().unwrap().to_string())
        .collect()
}

/// Facts the graph believes now (ADR-0705 belief, ADR-0785 gating).
async fn believed(state: &Arc<AppState>) -> BTreeSet<String> {
    let r = call(
        "knowledge_get_edges_by_group",
        json!({"group_ids": [G], "believed_only": true}),
        state,
    )
    .await;
    r["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["fact"].as_str().unwrap().to_string())
        .collect()
}

/// Every episode as returned by `knowledge_get_episodes`, by uuid.
async fn episodes(state: &Arc<AppState>) -> BTreeMap<String, Value> {
    let r = call(
        "knowledge_get_episodes",
        json!({"group_ids": [G], "last_n": 1000}),
        state,
    )
    .await;
    r["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["uuid"].as_str().unwrap().to_string(), e.clone()))
        .collect()
}

/// `(uuid, revision_id, committed?, span, heading_path, superseded?, reason)` per episode, sorted.
fn snapshot(db: &Db) -> Vec<String> {
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 1000, None, None).unwrap();
    let ids: Vec<String> = eps.iter().map(|(e, _)| e.uuid.clone()).collect();
    let sup = conn.get_episode_supersession(&ids).unwrap();
    let mut rows: Vec<String> = eps
        .iter()
        .map(|(e, _)| {
            let s = &sup[&e.uuid];
            format!(
                "{} {} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                e.uuid,
                e.name,
                e.revision_id,
                s.revision_committed_at_us,
                e.byte_start,
                e.byte_end,
                e.heading_path,
                e.chunk_hash,
                s.superseded_at_us,
                e.superseded_by,
                e.supersession_reason
            )
        })
        .collect();
    rows.sort();
    rows
}

const NOTE: &str = "# Design note\n\nWe need a cache.\n\n## Goals\n\nShip the cache.\n\n## Options\n\nRedis or memcached.\n\n## Decision\n\nWe chose Redis.\n\n## Risks\n\nOperational load.\n";

// ── US1: ingest and free re-ingest ──────────────────────────────────────────

/// US1 AC1/AC2, SC-001, SC-006: five chunks are extracted once, each episode carries the doc,
/// origin, a server-minted revision and an exact span; re-submitting the same bytes makes zero
/// extraction calls and changes nothing.
#[tokio::test]
async fn ingest_then_identical_reingest_makes_zero_extraction_calls() {
    let f = fx();
    let r = ingest(&f.state, "note-1", NOTE).await;
    assert_eq!(r["status"], "committed", "{r}");
    assert_eq!(r["extraction_calls"], 5);
    assert_eq!(classes(&r), vec!["added"; 5]);
    let rev = r["revision_id"].as_str().unwrap().to_string();
    assert!(r["previous_revision_id"].is_null());
    assert_eq!(f.ex.call_count(), 5);

    let eps = episodes(&f.state).await;
    assert_eq!(eps.len(), 5);
    for e in eps.values() {
        assert_eq!(e["doc_id"], "note-1");
        assert_eq!(e["origin"], "primary");
        assert_eq!(e["revision_id"], rev.as_str());
        assert!(e["revision_committed_at"].is_string(), "{e}");
        let (s, t) = (
            e["byte_start"].as_u64().unwrap() as usize,
            e["byte_end"].as_u64().unwrap() as usize,
        );
        assert_eq!(&NOTE[s..t], e["content"].as_str().unwrap(), "SC-006");
    }
    let before_snapshot = snapshot(&f.db);
    let before_facts = believed(&f.state).await;
    assert_eq!(before_facts.len(), 5);

    let r2 = ingest(&f.state, "note-1", NOTE).await;
    assert_eq!(r2["status"], "unchanged", "{r2}");
    assert_eq!(r2["extraction_calls"], 0);
    assert_eq!(classes(&r2), vec!["unchanged"; 5]);
    assert_eq!(r2["revision_id"], rev.as_str());
    assert!(uuids(&r2["superseded_episode_uuids"]).is_empty());
    assert_eq!(f.ex.call_count(), 5, "SC-001: no extraction on re-ingest");
    assert_eq!(snapshot(&f.db), before_snapshot, "FR-021: no belief change");
    assert_eq!(believed(&f.state).await, before_facts);
}

/// FR-021 with metadata: identical content under new `attributes` / `effective_at` / `title` is a
/// new completeness claim. It commits a revision with zero extraction that records the new
/// metadata, and changes no belief. The same metadata again, even with `effective_at` written in
/// another notation for the same instant, is still a no-op.
#[tokio::test]
async fn metadata_only_change_commits_a_revision_without_extraction() {
    let f = fx();
    let with = |extra: Value| {
        let mut p = doc_params("note-1", NOTE);
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        p
    };
    let meta = json!({"attributes": {"status": "draft"}, "effective_at": "2026-10-01T12:00:00Z"});
    let r1 = call("knowledge_process_document", with(meta.clone()), &f.state).await;
    assert_eq!(r1["status"], "committed", "{r1}");
    let facts = believed(&f.state).await;
    let snap = snapshot(&f.db);

    // Same instant, other notation; attributes with the same value: unchanged.
    let same =
        json!({"attributes": {"status": "draft"}, "effective_at": "2026-10-01T14:00:00+02:00"});
    let r = call("knowledge_process_document", with(same), &f.state).await;
    assert_eq!(r["status"], "unchanged", "{r}");
    assert_eq!(snapshot(&f.db), snap, "nothing written");

    for (field, changed) in [
        (
            "attributes",
            json!({"attributes": {"status": "reviewed"}, "effective_at": "2026-10-01T12:00:00Z"}),
        ),
        (
            "effective_at",
            json!({"attributes": {"status": "reviewed"}, "effective_at": "2026-10-02T12:00:00Z"}),
        ),
        (
            "title",
            json!({"attributes": {"status": "reviewed"}, "effective_at": "2026-10-02T12:00:00Z", "title": "Cache design"}),
        ),
    ] {
        let prev =
            f.db.connect()
                .unwrap()
                .latest_doc_revision(G, "note-1")
                .unwrap()
                .unwrap();
        let r = call(
            "knowledge_process_document",
            with(changed.clone()),
            &f.state,
        )
        .await;
        assert_eq!(r["status"], "committed", "{field}: {r}");
        assert_eq!(r["extraction_calls"], 0, "{field}");
        assert_eq!(classes(&r), vec!["unchanged"; 5], "{field}");
        assert_eq!(
            r["previous_revision_id"],
            prev.revision_id.as_str(),
            "{field}"
        );
        let rev =
            f.db.connect()
                .unwrap()
                .latest_doc_revision(G, "note-1")
                .unwrap()
                .unwrap();
        assert_ne!(rev.revision_id, prev.revision_id, "{field}: a new revision");
        assert_eq!(
            serde_json::from_str::<Value>(&rev.attributes).unwrap(),
            changed["attributes"],
            "{field}"
        );
        assert!(uuids(&r["superseded_episode_uuids"]).is_empty(), "{field}");
        assert_eq!(believed(&f.state).await, facts, "{field}: no belief change");
        // Resending the same metadata is a no-op again.
        let again = call("knowledge_process_document", with(changed), &f.state).await;
        assert_eq!(again["status"], "unchanged", "{field}: {again}");
    }
    assert_eq!(f.ex.call_count(), 5, "only the first ingest extracted");
    let rev =
        f.db.connect()
            .unwrap()
            .latest_doc_revision(G, "note-1")
            .unwrap()
            .unwrap();
    assert_eq!(rev.title.as_deref(), Some("Cache design"));
    assert_live_episodes_match(&f.state, NOTE, &rev.revision_id).await;
}

/// US1 AC3 and the input edge cases: no doc_id, empty and headings-only documents are refused
/// with nothing written.
#[tokio::test]
async fn invalid_documents_are_refused_with_nothing_written() {
    let f = fx();
    let no_doc = try_call(
        "knowledge_process_document",
        json!({"group_id": G, "content": NOTE}),
        &f.state,
    )
    .await;
    assert!(no_doc["error"]["message"]
        .as_str()
        .unwrap()
        .contains("doc_id is required"));
    for content in ["", "   \n\n", "# Only\n## Headings\n"] {
        let v = try_call(
            "knowledge_process_document",
            doc_params("d", content),
            &f.state,
        )
        .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("empty_document"),
            "{v}"
        );
    }
    assert_eq!(f.ex.call_count(), 0);
    assert!(episodes(&f.state).await.is_empty());
}

/// A realistic markdown document of at least `bytes` bytes: numbered sections of prose
/// paragraphs, each paragraph unique so every chunk is distinct.
fn big_doc(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes + 1024);
    let mut i = 0usize;
    while out.len() < bytes {
        if i.is_multiple_of(8) {
            out.push_str(&format!("## Section {}\n\n", i / 8));
        }
        out.push_str(&format!(
            "Paragraph {i} describes the subsystem and its behaviour at length. {}\n\n",
            "The quick brown fox jumps over the lazy dog. ".repeat(24)
        ));
        i += 1;
    }
    out
}

/// SC-011, FR-016, #862 FR-015: a document over the cap is refused before any extraction or
/// write, and the error names the effective cap (2 MiB by default) and `LCG_DOCUMENT_MAX_BYTES`.
#[tokio::test]
async fn oversized_document_is_rejected_before_any_work() {
    let f = fx();
    let big = big_doc(3 * 1000 * 1000);
    assert!(big.len() > 2 * 1024 * 1024);
    let v = try_call(
        "knowledge_process_document",
        doc_params("big", &big),
        &f.state,
    )
    .await;
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("document_too_large")
            && msg.contains("2097152")
            && msg.contains("LCG_DOCUMENT_MAX_BYTES"),
        "{msg}"
    );
    assert!(!msg.contains("524288"), "{msg}");
    assert_eq!(f.ex.call_count(), 0);
    assert!(episodes(&f.state).await.is_empty());
    let preview = try_call(
        "knowledge_preview_chunks",
        json!({"content": big}),
        &f.state,
    )
    .await;
    assert!(preview.get("error").is_some());
}

/// #862: a 1.5 MB document (over the old 512 KiB cap, under the new default) is accepted and
/// ingested in full — nothing truncated.
#[tokio::test]
async fn one_and_a_half_mb_document_is_accepted_by_default() {
    let f = fx();
    let doc = big_doc(1_500_000);
    let r = ingest(&f.state, "web-page", &doc).await;
    assert!(r.get("error").is_none(), "{r}");
    assert!(f.ex.call_count() > 100, "calls: {}", f.ex.call_count());
    // The last paragraph (the document's tail) was extracted: nothing was truncated.
    let last = f.ex.calls().last().unwrap().body.clone();
    assert!(last.contains("Paragraph"), "{last}");
}

/// #862: `LCG_DOCUMENT_MAX_BYTES=4MiB` accepts the 3 MB document the default refuses. The
/// installed cap is process-wide and install-once, so this goes through the explicit-cap entry
/// point (the same function the handler calls).
#[test]
fn three_mb_document_is_accepted_under_a_4mib_cap_and_refused_under_the_default() {
    use lcg_core::document::ingest::chunk_for_ingest_with_cap;
    use lcg_core::document_config::{parse_max_bytes, DEFAULT_MAX_BYTES};
    let doc = big_doc(3 * 1000 * 1000);
    let four_mib = parse_max_bytes("4MiB").unwrap();
    assert!(chunk_for_ingest_with_cap(&doc, four_mib).is_ok());
    let e = chunk_for_ingest_with_cap(&doc, DEFAULT_MAX_BYTES)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("document_too_large") && e.contains("LCG_DOCUMENT_MAX_BYTES"),
        "{e}"
    );
}

/// #862 edge cases: exactly at the cap is accepted, one byte over is refused, and the cap is in
/// bytes, not characters.
#[test]
fn cap_boundary_is_inclusive_and_measured_in_bytes() {
    use lcg_core::document::ingest::chunk_for_ingest_with_cap;
    let at = "x".repeat(1000);
    assert!(chunk_for_ingest_with_cap(&at, 1000).is_ok());
    let over = "x".repeat(1001);
    let e = chunk_for_ingest_with_cap(&over, 1000)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("document_too_large") && e.contains("1000 bytes"),
        "{e}"
    );
    // 600 characters of 2-byte text is 1200 bytes: over a 1000-byte cap despite 600 < 1000 chars.
    let multibyte = "é".repeat(600);
    assert_eq!(multibyte.chars().count(), 600);
    assert!(chunk_for_ingest_with_cap(&multibyte, 1000).is_err());
}

// ── #862 measurement: cost of a document at the default 2 MiB cap ───────────
//
// `#[ignore]`d: run on demand to record numbers for ADR-0862, e.g.
//   cargo test -p lcg-core --test document_mode --no-run
//   /usr/bin/time -l <test-binary> --ignored --nocapture --test-threads=1 measure_
// Mock providers only (`ScriptExtractor`, `MockEmbedder`), so the cost shown is the server's own:
// chunker, diff, per-episode writes and the WAL.

const TWO_MIB: usize = 2 * 1024 * 1024;

fn truncate_to(mut s: String, bytes: usize) -> String {
    while s.len() > bytes {
        s.pop();
    }
    s
}

async fn measure(label: &str, doc: String) {
    assert!(doc.len() <= TWO_MIB, "{label}: {} bytes", doc.len());
    let f = fx();
    let t = std::time::Instant::now();
    let chunk_t = std::time::Instant::now();
    let chunked = lcg_core::document::ingest::chunk_for_ingest_with_cap(&doc, TWO_MIB);
    let chunk_ms = chunk_t.elapsed().as_millis();
    let units = chunked.as_ref().map(|c| c.chunks.len()).unwrap_or(0);
    let r = try_call(
        "knowledge_process_document",
        doc_params("measure", &doc),
        &f.state,
    )
    .await;
    let total_ms = t.elapsed().as_millis();
    println!(
        "MEASURE {label}: bytes={} units={units} chunk_only_ms={chunk_ms} \
         end_to_end_ms={total_ms} extractor_calls={} ok={}",
        doc.len(),
        f.ex.call_count(),
        r.get("error").is_none()
    );
}

#[tokio::test]
#[ignore = "measurement: run on demand (ADR-0862)"]
async fn measure_realistic_2mib_document() {
    measure("realistic", truncate_to(big_doc(TWO_MIB), TWO_MIB)).await;
}

#[tokio::test]
#[ignore = "measurement: run on demand (ADR-0862)"]
async fn measure_unclosed_fence_2mib_document() {
    let mut d = String::from("# Fence\n\n```\n");
    while d.len() < TWO_MIB {
        d.push_str("let x = 1; // line inside a fence that never closes\n");
    }
    measure("unclosed_fence", truncate_to(d, TWO_MIB)).await;
}

#[tokio::test]
#[ignore = "measurement: run on demand (ADR-0862)"]
async fn measure_many_tables_2mib_document() {
    let mut d = String::from("# Tables\n\n");
    let mut i = 0;
    while d.len() < TWO_MIB {
        d.push_str(&format!(
            "| key | value |\n|-----|-------|\n| a{i} | b{i} |\n| c{i} | d{i} |\n\n"
        ));
        i += 1;
    }
    measure("many_tables", truncate_to(d, TWO_MIB)).await;
}

#[tokio::test]
#[ignore = "measurement: run on demand (ADR-0862)"]
async fn measure_single_long_line_2mib_document() {
    let d = format!("# Line\n\n{}", "word ".repeat(TWO_MIB / 5));
    measure("single_long_line", truncate_to(d, TWO_MIB)).await;
}

/// Chunker-only timing for every pathological shape, including many tiny sections (about
/// 80 000 units, each of which would be one extractor call — a cost of the document, not a
/// superlinear chunker, so it is timed here without ingesting).
#[test]
#[ignore = "measurement: run on demand (ADR-0862)"]
fn measure_chunker_only_on_pathological_2mib_documents() {
    let mut headings = String::new();
    let mut i = 0;
    while headings.len() < TWO_MIB {
        headings.push_str(&format!("## H{i}\n\nShort body {i}.\n\n"));
        i += 1;
    }
    let mut fence = String::from("# Fence\n\n```\n");
    while fence.len() < TWO_MIB {
        fence.push_str("let x = 1; // line inside a fence that never closes\n");
    }
    let mut tables = String::from("# Tables\n\n");
    let mut j = 0;
    while tables.len() < TWO_MIB {
        tables.push_str(&format!(
            "| key | value |\n|-----|-------|\n| a{j} | b{j} |\n\n"
        ));
        j += 1;
    }
    let mut front = String::from("---\ntitle: x\n");
    while front.len() < TWO_MIB {
        front.push_str("key: value line in a front-matter block that never closes\n");
    }
    let one_line = format!("# Line\n\n{}", "word ".repeat(TWO_MIB / 5));
    for (label, d) in [
        ("many_headings", headings),
        ("unclosed_fence", fence),
        ("many_tables", tables),
        ("unclosed_front_matter", front),
        ("single_long_line", one_line),
        ("realistic", big_doc(TWO_MIB)),
    ] {
        let d = truncate_to(d, TWO_MIB);
        let t = std::time::Instant::now();
        let r = lcg_core::document::ingest::chunk_for_ingest_with_cap(&d, TWO_MIB);
        println!(
            "MEASURE chunker_only {label}: bytes={} units={} ms={}",
            d.len(),
            r.map(|c| c.chunks.len()).unwrap_or(0),
            t.elapsed().as_millis()
        );
    }
}

// ── US2: the server applies the diff ────────────────────────────────────────

/// US2 AC1, SC-002: one edited paragraph re-extracts exactly one chunk; its old episode is
/// superseded with reason `revision` by the new one, and nothing else moves.
#[tokio::test]
async fn one_paragraph_edit_reextracts_one_chunk() {
    let f = fx();
    let r1 = ingest(&f.state, "note-1", NOTE).await;
    let old_options = uuids(&r1["chunks"][2]["episode_uuids"]);
    let edited = NOTE.replace("Redis or memcached.", "Redis, memcached or Valkey.");
    let r2 = ingest(&f.state, "note-1", &edited).await;
    assert_eq!(r2["status"], "committed");
    assert_eq!(r2["extraction_calls"], 1);
    assert_eq!(
        classes(&r2),
        vec![
            "unchanged",
            "unchanged",
            "changed",
            "unchanged",
            "unchanged"
        ]
    );
    assert_eq!(
        r2["chunks"][2]["name"], r1["chunks"][2]["name"],
        "stable name"
    );
    assert_eq!(uuids(&r2["superseded_episode_uuids"]), old_options);
    assert_eq!(r2["previous_revision_id"], r1["revision_id"]);
    let new_options = uuids(&r2["chunks"][2]["episode_uuids"]);

    let eps = episodes(&f.state).await;
    let old = &eps[&old_options[0]];
    assert_eq!(old["supersession_reason"], "revision");
    assert_eq!(old["superseded_by"], new_options[0].as_str());
    let superseded: Vec<_> = eps
        .values()
        .filter(|e| e["superseded_at"].is_string())
        .collect();
    assert_eq!(superseded.len(), 1, "other episodes untouched");
    let facts = believed(&f.state).await;
    assert!(facts.contains("Redis, memcached or Valkey."));
    assert!(!facts.contains("Redis or memcached."));
    assert_eq!(facts.len(), 5);
}

/// US2 AC2, SC-003: a section dropped from the full document is retired with `chunk_removed`
/// without the client sending any removal, and its fact loses support.
#[tokio::test]
async fn deleted_section_is_retired_by_absence() {
    let f = fx();
    let r1 = ingest(&f.state, "note-1", NOTE).await;
    let risks = uuids(&r1["chunks"][4]["episode_uuids"]);
    let r2 = ingest(
        &f.state,
        "note-1",
        &NOTE.replace("## Risks\n\nOperational load.\n", ""),
    )
    .await;
    assert_eq!(r2["extraction_calls"], 0);
    assert_eq!(classes(&r2), vec!["unchanged"; 4]);
    assert_eq!(uuids(&r2["retired_episode_uuids"]), risks);
    assert_eq!(r2["removed"][0]["classification"], "removed");
    let eps = episodes(&f.state).await;
    assert_eq!(eps[&risks[0]]["supersession_reason"], "chunk_removed");
    assert!(eps[&risks[0]].get("superseded_by").is_none());
    let facts = believed(&f.state).await;
    assert!(!facts.contains("Operational load."));
    assert_eq!(facts.len(), 4);
}

/// US2 AC3, SC-009: a heading rename re-extracts only the renamed section (its own text changed),
/// never its subsections; a move re-extracts nothing.
#[tokio::test]
async fn heading_rename_and_move_reextract_only_what_changed() {
    let f = fx();
    let nested =
        "# Plan\n\nContext.\n\n## Accepted\n\nOption A.\n\n### Detail\n\nA in depth.\n\n## Later\n\nFollow up.\n";
    ingest(&f.state, "d", nested).await;
    let r = ingest(&f.state, "d", &nested.replace("## Accepted", "## Rejected")).await;
    assert_eq!(r["extraction_calls"], 1, "{r}");
    assert_eq!(
        classes(&r),
        vec!["unchanged", "changed", "unchanged", "unchanged"]
    );
    // The changed chunk was re-extracted under its new heading path.
    let last = f.ex.calls().last().unwrap().clone();
    assert!(last.context.unwrap().contains("Section: Plan > Rejected"));

    let moved = "# Plan\n\nContext.\n\n## Later\n\nFollow up.\n\n## Rejected\n\nOption A.\n\n### Detail\n\nA in depth.\n";
    let r = ingest(&f.state, "d", moved).await;
    assert_eq!(r["extraction_calls"], 0);
    assert_eq!(classes(&r), vec!["unchanged"; 4]);
    assert_eq!(r["status"], "committed", "a new order is a new revision");
}

/// Asserts that every live (unsuperseded) episode of the group is stamped with `rev`, slices
/// `content` exactly at its span, and carries the heading path the chunker gives that span now.
async fn assert_live_episodes_match(state: &Arc<AppState>, content: &str, rev: &str) {
    let preview = call(
        "knowledge_preview_chunks",
        json!({"content": content}),
        state,
    )
    .await;
    let path_at = |s: u64| -> Value {
        preview["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["byte_start"].as_u64().unwrap() <= s && s < c["byte_end"].as_u64().unwrap())
            .map(|c| c["heading_path"].clone())
            .unwrap_or(Value::Null)
    };
    let eps = episodes(state).await;
    let live: Vec<_> = eps
        .values()
        .filter(|e| e["superseded_at"].is_null())
        .collect();
    assert!(!live.is_empty());
    for e in live {
        let (s, t) = (
            e["byte_start"].as_u64().unwrap(),
            e["byte_end"].as_u64().unwrap(),
        );
        assert_eq!(
            &content[s as usize..t as usize],
            e["content"].as_str().unwrap(),
            "SC-006: a live episode's span slices the latest revision: {e}"
        );
        assert_eq!(
            e["revision_id"], rev,
            "carried-forward episode re-stamped: {e}"
        );
        assert_eq!(e["heading_path"], path_at(s), "current heading path: {e}");
    }
}

/// FR-013, SC-006: an unchanged chunk's episodes are carried forward, not re-extracted, but are
/// re-stamped at commit with the new revision id, their span in the new content and their
/// current heading path — so an edit that shifts later offsets, or a parent heading rename, never
/// leaves a live episode pointing into a revision the client no longer holds. Split units of an
/// unchanged section shift together.
#[tokio::test]
async fn unchanged_chunks_are_restamped_with_the_new_revisions_spans() {
    let f = fx();
    let para = "A sentence about the cache layer and how it behaves under load. ".repeat(30);
    let doc = format!(
        "# Plan\n\nIntro.\n\n## Accepted\n\nOption A.\n\n### Detail\n\n{p}\n\n{p}\n\n{p}\n\n{p}\n\n## Later\n\nFollow up.\n",
        p = para.trim()
    );
    let r1 = ingest(&f.state, "d", &doc).await;
    assert!(
        r1["chunks"][2]["units"].as_u64().unwrap() > 1,
        "the Detail section is split: {r1}"
    );
    let rev1 = r1["revision_id"].as_str().unwrap().to_string();
    assert_live_episodes_match(&f.state, &doc, &rev1).await;

    // Lengthening the intro shifts every later byte; only the intro is re-extracted.
    let longer = doc.replace("Intro.", "A much longer introduction than before.");
    let r2 = ingest(&f.state, "d", &longer).await;
    assert_eq!(r2["extraction_calls"], 1, "{r2}");
    assert_eq!(
        classes(&r2),
        vec!["changed", "unchanged", "unchanged", "unchanged"]
    );
    let rev2 = r2["revision_id"].as_str().unwrap().to_string();
    assert_live_episodes_match(&f.state, &longer, &rev2).await;
    // The response and the stored episodes agree on every chunk's span.
    let eps = episodes(&f.state).await;
    for c in r2["chunks"].as_array().unwrap() {
        let first = &eps[&uuids(&c["episode_uuids"])[0]];
        assert_eq!(first["byte_start"], c["byte_start"], "{c}");
    }

    // Renaming the parent re-extracts the parent only; its unchanged subsection's episodes take
    // the new path (and the next diff pairs against it, not the stale one).
    let renamed = longer.replace("## Accepted", "## Rejected");
    let r3 = ingest(&f.state, "d", &renamed).await;
    assert_eq!(r3["extraction_calls"], 1, "{r3}");
    let rev3 = r3["revision_id"].as_str().unwrap().to_string();
    assert_live_episodes_match(&f.state, &renamed, &rev3).await;
    assert_eq!(
        r3["chunks"][2]["heading_path"],
        json!(["Plan", "Rejected", "Detail"])
    );

    // Carrying forward never moves belief: nothing unchanged was superseded along the way.
    let superseded = episodes(&f.state)
        .await
        .values()
        .filter(|e| e["superseded_at"].is_string())
        .count();
    assert_eq!(
        superseded, 2,
        "only the intro and the renamed parent were replaced"
    );
    // And a byte-identical resubmission is still a no-op.
    let r4 = ingest(&f.state, "d", &renamed).await;
    assert_eq!(r4["status"], "unchanged");
    assert_eq!(r4["extraction_calls"], 0);
}

// ── US3: atomicity ──────────────────────────────────────────────────────────

/// US3 AC1/AC3, SC-004: an extractor failure part-way through a revision leaves the previous
/// revision fully believed, supersedes nothing, and the retry applies the full diff against the
/// last committed revision.
#[tokio::test]
async fn failed_ingest_retires_nothing_and_retry_commits() {
    let f = fx();
    ingest(&f.state, "note-1", NOTE).await;
    let v1_facts = believed(&f.state).await;
    let v1_snapshot = snapshot(&f.db);

    // Rewrite every section, then make the third extraction of the new revision fail.
    let v2 = NOTE.replace('.', "!");
    f.ex.fail_on.store(f.ex.call_count() + 3, Ordering::SeqCst);
    let v = try_call(
        "knowledge_process_document",
        doc_params("note-1", &v2),
        &f.state,
    )
    .await;
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("was not committed"), "{msg}");

    assert_eq!(
        believed(&f.state).await,
        v1_facts,
        "previous revision still believed"
    );
    let eps = episodes(&f.state).await;
    assert!(
        eps.values().all(|e| e.get("superseded_at").is_none()),
        "nothing superseded"
    );
    let pending: Vec<_> = eps
        .values()
        .filter(|e| e.get("revision_committed_at").is_none())
        .collect();
    assert_eq!(
        pending.len(),
        2,
        "two units written pending before the failure"
    );
    assert!(pending.iter().all(|e| e["revision_id"].is_string()));
    // The pending rows are new; every v1 row is exactly as it was.
    let now = snapshot(&f.db);
    assert!(v1_snapshot.iter().all(|row| now.contains(row)));

    // Retry: diffs against the last *committed* revision, so all five chunks are re-extracted.
    f.ex.fail_on.store(0, Ordering::SeqCst);
    let r = ingest(&f.state, "note-1", &v2).await;
    assert_eq!(r["extraction_calls"], 5);
    assert_eq!(classes(&r), vec!["changed"; 5]);
    let facts = believed(&f.state).await;
    assert_eq!(facts.len(), 5);
    assert!(facts.iter().all(|f| f.ends_with('!')), "{facts:?}");
}

/// FR-022: two concurrent ingests of one document serialize; afterwards exactly one revision's
/// content is live.
#[tokio::test]
async fn concurrent_ingests_of_one_document_serialize() {
    let f = fx();
    let a = NOTE.to_string();
    let b = NOTE.replace("We chose Redis.", "We chose memcached.");
    let (s1, s2) = (Arc::clone(&f.state), Arc::clone(&f.state));
    let (r1, r2) = tokio::join!(
        async move { call("knowledge_process_document", doc_params("c", &a), &s1).await },
        async move { call("knowledge_process_document", doc_params("c", &b), &s2).await },
    );
    assert_eq!(r1["status"], "committed");
    assert_eq!(r2["status"], "committed");
    // The later of the two diffed against the earlier: exactly one of them extracted 5 chunks.
    let calls = [
        r1["extraction_calls"].as_u64(),
        r2["extraction_calls"].as_u64(),
    ];
    assert!(
        calls.contains(&Some(5)) && calls.contains(&Some(1)),
        "{calls:?}"
    );
    let facts = believed(&f.state).await;
    assert_eq!(facts.len(), 5, "{facts:?}");
    assert!(facts.contains("We chose Redis.") ^ facts.contains("We chose memcached."));
}

// ── US4: document context ───────────────────────────────────────────────────

/// US4 AC1/AC2, SC-010, FR-012: every extraction carries the title, heading path, origin and
/// document map in a delimited context block, never in the source text; each fact cites only its
/// own chunk's episode.
#[tokio::test]
async fn extractor_receives_document_context_and_facts_cite_only_their_chunk() {
    let f = fx();
    let doc = "# Decision\n\n## Options\n\nX, Y or Z.\n\n## Outcome\n\nWe chose Z.\n";
    let mut p = doc_params("adr", doc);
    p["origin"] = json!("derived");
    let r = call("knowledge_process_document", p, &f.state).await;
    let calls = f.ex.calls();
    assert_eq!(calls.len(), 2);
    let ctx = calls[0].context.clone().expect("SC-010: context present");
    assert!(ctx.starts_with("<DOCUMENT_CONTEXT>") && ctx.ends_with("</DOCUMENT_CONTEXT>"));
    for needle in [
        "Title: Design note",
        "Source: notes/design.md",
        "Origin: derived",
        "Section: Decision > Options",
        "Document map:",
        "- Options  <- this section",
        "- Outcome",
    ] {
        assert!(ctx.contains(needle), "missing {needle:?} in {ctx}");
    }
    assert!(!calls[0].body.contains("DOCUMENT_CONTEXT"));
    assert_eq!(calls[0].body, "## Options\n\nX, Y or Z.");

    let conn = f.db.connect().unwrap();
    let edges = conn.get_edges_by_group_ids(None).unwrap();
    for (i, chunk) in r["chunks"].as_array().unwrap().iter().enumerate() {
        let ep = uuids(&chunk["episode_uuids"]);
        let e = edges
            .iter()
            .find(|e| e.fact == fact_of(&calls[i].body))
            .unwrap();
        let rows = conn
            .get_edge_belief_rows(std::slice::from_ref(&e.uuid))
            .unwrap();
        assert_eq!(rows[&e.uuid].episodes, ep, "FR-012");
    }
    // FR-018: the document's origin is every episode's origin.
    assert!(episodes(&f.state)
        .await
        .values()
        .all(|e| e["origin"] == "derived"));
}

// ── US5: preview and provenance ─────────────────────────────────────────────

fn wal_bytes(dir: &Path) -> Vec<u8> {
    let mut files: Vec<PathBuf> = Vec::new();
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        if p.is_dir() {
            for e in std::fs::read_dir(p).unwrap() {
                walk(&e.unwrap().path(), out);
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    walk(dir, &mut files);
    files.sort();
    files
        .iter()
        .flat_map(|f| std::fs::read(f).unwrap())
        .collect()
}

/// US5 AC1/AC2, SC-007, SC-006, FR-017: the preview is exactly the ingest's chunking, writes
/// nothing, and an oversized section's split units tile it — all of them sharing the chunk's name
/// and superseded together by the next revision's final unit.
#[tokio::test]
async fn preview_matches_ingest_and_split_units_tile_their_section() {
    let f = fx();
    let para = "A sentence about the cache layer and how it behaves under load. ".repeat(30);
    let doc = format!(
        "# Big\n\nIntro.\n\n## Long\n\n{p}\n\n{p}\n\n{p}\n\n{p}\n\n## Short\n\nDone.\n",
        p = para.trim()
    );
    let wal_before = wal_bytes(f.wal.path());
    let snap_before = snapshot(&f.db);
    let preview = call(
        "knowledge_preview_chunks",
        json!({"content": doc}),
        &f.state,
    )
    .await;
    assert_eq!(wal_bytes(f.wal.path()), wal_before, "SC-007: no WAL write");
    assert_eq!(snapshot(&f.db), snap_before, "SC-007: no graph write");
    assert_eq!(f.ex.call_count(), 0);

    let r = ingest(&f.state, "big", &doc).await;
    let (pc, rc) = (
        preview["chunks"].as_array().unwrap(),
        r["chunks"].as_array().unwrap(),
    );
    assert_eq!(pc.len(), rc.len());
    for (p, c) in pc.iter().zip(rc) {
        for k in ["heading_path", "byte_start", "byte_end", "content_hash"] {
            assert_eq!(p[k], c[k], "{k}");
        }
        assert_eq!(p["units"].as_array().unwrap().len() as u64, c["units"]);
    }
    let long = &pc[1];
    let units = long["units"].as_array().unwrap();
    assert!(units.len() >= 2, "the long section is split: {long}");
    assert_eq!(
        r["extraction_calls"].as_u64().unwrap() as usize,
        2 + units.len()
    );

    let eps = episodes(&f.state).await;
    let long_eps = uuids(&rc[1]["episode_uuids"]);
    assert_eq!(long_eps.len(), units.len());
    let mut prev_end = long["byte_start"].as_u64().unwrap();
    for (u, ep) in units.iter().zip(&long_eps) {
        let e = &eps[ep];
        assert_eq!(e["byte_start"], u["byte_start"]);
        assert_eq!(e["byte_start"].as_u64().unwrap(), prev_end, "units tile");
        prev_end = e["byte_end"].as_u64().unwrap();
        let (s, t) = (
            e["byte_start"].as_u64().unwrap() as usize,
            e["byte_end"].as_u64().unwrap() as usize,
        );
        assert_eq!(&doc[s..t], e["content"].as_str().unwrap(), "SC-006");
        assert_eq!(e["name"], rc[1]["name"], "units share the chunk's name");
    }
    assert_eq!(prev_end, long["byte_end"].as_u64().unwrap());

    // Editing the long section supersedes all of its old units by the new final unit.
    let edited = doc.replace("## Long\n\n", "## Long\n\nNew lead. ");
    let r2 = ingest(&f.state, "big", &edited).await;
    let new_long = uuids(&r2["chunks"][1]["episode_uuids"]);
    let final_unit = new_long.last().unwrap();
    let eps = episodes(&f.state).await;
    for old in &long_eps {
        assert_eq!(eps[old]["superseded_by"], final_unit.as_str());
        assert_eq!(eps[old]["supersession_reason"], "revision");
    }
    for unit in &new_long {
        assert!(
            eps[unit].get("superseded_at").is_none(),
            "siblings never supersede each other"
        );
    }
}

// ── US6: origin, mode transition, chunk-mode safety ─────────────────────────

/// US6 AC2, FR-018: a later ingest of the same doc_id with another origin is refused under the
/// existing `origin_conflict` rule, writing nothing.
#[tokio::test]
async fn conflicting_origin_for_a_document_is_refused() {
    let f = fx();
    let mut p = doc_params("o", NOTE);
    p["origin"] = json!("derived");
    call("knowledge_process_document", p, &f.state).await;
    let before = snapshot(&f.db);
    let calls = f.ex.call_count();
    let v = try_call(
        "knowledge_process_document",
        doc_params("o", &NOTE.replace("Redis.", "Valkey.")),
        &f.state,
    )
    .await;
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("IPC error: origin_conflict:")
            || v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("origin_conflict:")
    );
    assert_eq!(f.ex.call_count(), calls);
    assert_eq!(snapshot(&f.db), before);
}

/// Review finding on PR 791: a failed first ingest leaves only pending episodes. Those must not
/// fix the document's origin, so a retry with a different origin is accepted and commits.
#[tokio::test]
async fn failed_first_ingest_does_not_pin_the_documents_origin() {
    let f = fx();
    let mut p = doc_params("o", NOTE);
    p["origin"] = json!("derived");
    f.ex.fail_on.store(f.ex.call_count() + 2, Ordering::SeqCst);
    let v = try_call("knowledge_process_document", p, &f.state).await;
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("was not committed"),
        "{v}"
    );
    let pending: Vec<_> = episodes(&f.state)
        .await
        .into_values()
        .filter(|e| e["origin"] == "derived" && e.get("revision_committed_at").is_none())
        .collect();
    assert_eq!(pending.len(), 1, "one derived unit written pending");

    f.ex.fail_on.store(0, Ordering::SeqCst);
    let r = ingest(&f.state, "o", NOTE).await;
    assert_eq!(r["status"], "committed", "{r}");
    assert_eq!(r["extraction_calls"], 5);
    // A committed revision does fix the origin: a different one is refused again.
    let mut p = doc_params("o", &NOTE.replace("Redis.", "Valkey."));
    p["origin"] = json!("derived");
    let v = try_call("knowledge_process_document", p, &f.state).await;
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("origin_conflict:"),
        "{v}"
    );
}

/// FR-023: the first document-mode ingest of a doc_id replaces that document's live chunk-mode
/// episodes (reason `revision`) and reports them.
#[tokio::test]
async fn first_document_ingest_supersedes_chunk_mode_episodes_of_the_doc() {
    let f = fx();
    let c = call(
        "knowledge_process_chunk",
        json!({
            "chunk_text": "Legacy paragraph.", "chunk_id": "note-1::Old::0",
            "source_file": "notes/design.md", "group_id": G, "doc_id": "note-1",
        }),
        &f.state,
    )
    .await;
    let legacy = c["episode_uuid"].as_str().unwrap().to_string();
    let r = ingest(&f.state, "note-1", NOTE).await;
    assert_eq!(
        uuids(&r["mode_transition_superseded_episode_uuids"]),
        vec![legacy.clone()]
    );
    let eps = episodes(&f.state).await;
    assert_eq!(eps[&legacy]["supersession_reason"], "revision");
    assert!(!believed(&f.state).await.contains("Legacy paragraph."));
}

/// US6 AC3, SC-008: chunk mode is unchanged — no revision fields on the read or the row, and a
/// chunk-mode revise by name never touches a document-mode episode that happens to share it.
#[tokio::test]
async fn chunk_mode_episodes_carry_no_revision_and_never_touch_document_rows() {
    let f = fx();
    let r = ingest(&f.state, "note-1", NOTE).await;
    let doc_name = r["chunks"][1]["name"].as_str().unwrap().to_string();
    let doc_ep = uuids(&r["chunks"][1]["episode_uuids"])[0].clone();
    let c = call(
        "knowledge_process_chunk",
        json!({
            "chunk_text": "Chunk-mode text.", "chunk_id": doc_name,
            "source_file": "other.md", "group_id": G, "doc_id": "other",
        }),
        &f.state,
    )
    .await;
    assert!(uuids(&c["superseded_episode_uuids"]).is_empty());
    assert_eq!(uuids(&c["left_alone_episode_uuids"]), vec![doc_ep.clone()]);
    let eps = episodes(&f.state).await;
    let chunk_ep = &eps[c["episode_uuid"].as_str().unwrap()];
    for k in [
        "revision_id",
        "revision_committed_at",
        "byte_start",
        "byte_end",
        "heading_path",
        "chunk_hash",
    ] {
        assert!(
            chunk_ep.get(k).is_none(),
            "chunk-mode read gained {k}: {chunk_ep}"
        );
    }
    assert!(eps[&doc_ep].get("superseded_at").is_none());
    let conn = f.db.connect().unwrap();
    let wp = conn
        .get_episode_write_paths(&[doc_ep.clone(), c["episode_uuid"].as_str().unwrap().into()])
        .unwrap();
    assert_eq!(wp[&doc_ep].as_deref(), Some("document"));
}

/// A purged group takes its revision records with it, so a re-ingest starts from scratch.
#[tokio::test]
async fn group_purge_removes_revisions() {
    let f = fx();
    ingest(&f.state, "note-1", NOTE).await;
    call(
        "knowledge_delete_by_group",
        json!({"group_ids": [G], "confirm": true}),
        &f.state,
    )
    .await;
    let conn = f.db.connect().unwrap();
    assert!(conn.latest_doc_revision(G, "note-1").unwrap().is_none());
    drop(conn);
    let r = ingest(&f.state, "note-1", NOTE).await;
    assert_eq!(classes(&r), vec!["added"; 5]);
}

// ── Replay, rebuild and crash (Task 10) ─────────────────────────────────────

async fn replayed(source: &Path) -> (Arc<AppState>, Arc<Db>, TempDir, Arc<ScriptExtractor>) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("r.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        WalReplayer::new(source)
            .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
            .unwrap();
    }
    let ex = Arc::new(ScriptExtractor::default());
    let state = make_state(Arc::clone(&db), None, Arc::clone(&ex) as Arc<dyn Extractor>);
    (state, db, dir, ex)
}

async fn history(f: &Fx) {
    ingest(&f.state, "note-1", NOTE).await;
    ingest(
        &f.state,
        "note-1",
        &NOTE.replace("Redis or memcached.", "Redis or Valkey."),
    )
    .await;
    ingest(
        &f.state,
        "note-1",
        &NOTE
            .replace("Redis or memcached.", "Redis or Valkey.")
            .replace("## Risks\n\nOperational load.\n", ""),
    )
    .await;
}

/// US6 AC1, SC-005: rebuilding from the WAL — raw or compacted — reproduces every revision id,
/// span, commit stamp and supersession, and the same believed facts.
#[tokio::test]
async fn wal_replay_and_dump_reproduce_revisions_spans_and_belief() {
    let f = fx();
    history(&f).await;
    let snap = snapshot(&f.db);
    let facts = believed(&f.state).await;
    assert_eq!(facts.len(), 4);

    let (s2, db2, _d2, _) = replayed(f.wal.path()).await;
    assert_eq!(snapshot(&db2), snap);
    assert_eq!(believed(&s2).await, facts);
    // The rebuilt graph keeps diffing: the same document is still a no-op.
    let conn = db2.connect().unwrap();
    let rev = conn.latest_doc_revision(G, "note-1").unwrap().unwrap();
    let orig =
        f.db.connect()
            .unwrap()
            .latest_doc_revision(G, "note-1")
            .unwrap()
            .unwrap();
    assert_eq!(rev, orig);

    let out = TempDir::new().unwrap();
    let target = out.path().join("dump");
    let r = call(
        "knowledge_dump_wal",
        json!({"target_dir": target.to_str().unwrap()}),
        &f.state,
    )
    .await;
    assert_eq!(r["success"], true, "{r}");
    let (s3, db3, _d3, _) = replayed(&target).await;
    assert_eq!(snapshot(&db3), snap);
    assert_eq!(believed(&s3).await, facts);
    assert_eq!(
        db3.connect()
            .unwrap()
            .latest_doc_revision(G, "note-1")
            .unwrap()
            .unwrap(),
        orig
    );
}

/// Copies `src` to a fresh directory, truncating the WAL right before the first line for which
/// `cut` returns true (and dropping every later file).
fn truncated_wal(src: &Path, cut: impl Fn(&Value) -> bool) -> TempDir {
    let out = TempDir::new().unwrap();
    let mut files: Vec<PathBuf> = std::fs::read_dir(src)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut cutting = false;
    for f in files {
        let name = f.file_name().unwrap().to_owned();
        if f.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            if f.is_file() {
                std::fs::copy(&f, out.path().join(&name)).unwrap();
            }
            continue;
        }
        if cutting {
            continue;
        }
        let mut kept = String::new();
        for line in std::fs::read_to_string(&f).unwrap().lines() {
            let v: Value = serde_json::from_str(line).unwrap();
            if cut(&v) {
                cutting = true;
                break;
            }
            kept.push_str(line);
            kept.push('\n');
        }
        std::fs::write(out.path().join(&name), kept).unwrap();
    }
    out
}

fn cypher(v: &Value) -> &str {
    v["cypher"].as_str().unwrap_or("")
}

/// SC-006 after a torn re-stamp: a WAL tail cut between the re-stamp statements of a split
/// chunk leaves its units carrying spans from two different revisions. The next commit must
/// still place every unit exactly in the new content — it re-locates episodes by their text,
/// not by shifting a stored offset — and nothing is re-extracted.
#[tokio::test]
async fn torn_restamp_of_a_split_chunk_is_relocated_exactly_on_next_ingest() {
    let f = fx();
    let para = "A sentence about the cache layer and how it behaves under load. ".repeat(30);
    let doc = |intro: &str| {
        format!(
            "# Plan\n\n{intro}\n\n## Detail\n\n{p}\n\n{p}\n\n{p}\n\n{p}\n\n## Later\n\nFollow up.\n",
            p = para.trim()
        )
    };
    let r1 = ingest(&f.state, "d", &doc("Intro.")).await;
    let units = r1["chunks"][1]["units"].as_u64().unwrap();
    assert!(units > 1, "the Detail section is split: {r1}");
    let r2 = ingest(&f.state, "d", &doc("A longer introduction.")).await;
    assert_eq!(classes(&r2), vec!["changed", "unchanged", "unchanged"]);

    // Cut the WAL at the second re-stamp statement: the Detail chunk's first unit carries the
    // second revision's span, the rest still carry the first revision's.
    let seen = AtomicUsize::new(0);
    let wal = truncated_wal(f.wal.path(), |v| {
        cypher(v).contains("SET ep.revision_id = $revision_id")
            && seen.fetch_add(1, Ordering::SeqCst) == 1
    });
    let (s2, _db2, _d, ex) = replayed(wal.path()).await;
    let live: Vec<Value> = episodes(&s2)
        .await
        .into_values()
        .filter(|e| e["superseded_at"].is_null() && e["heading_path"] == json!(["Plan", "Detail"]))
        .collect();
    let revs: BTreeSet<String> = live
        .iter()
        .map(|e| e["revision_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(revs.len(), 2, "the torn tail left mixed stamps: {live:?}");

    let v3 = doc("An even longer introduction than both earlier ones.");
    let r3 = call("knowledge_process_document", doc_params("d", &v3), &s2).await;
    assert_eq!(r3["status"], "committed", "{r3}");
    assert_eq!(r3["extraction_calls"], 1, "only the intro");
    assert_eq!(ex.call_count(), 1);
    assert!(
        uuids(&r3["stale_superseded_episode_uuids"]).is_empty(),
        "{r3}"
    );
    let rev3 = r3["revision_id"].as_str().unwrap().to_string();
    assert_live_episodes_match(&s2, &v3, &rev3).await;
}

/// SC-006, FR-021: a torn tail that kept a revision's `DocRevision` row and commit stamp but cut
/// all of its re-stamps leaves carried-forward episodes on the older revision's spans and id,
/// while the latest revision's content hash already equals the document. Resending the same
/// bytes must not answer `unchanged` over those stale spans: it commits a zero-extraction
/// revision that re-stamps them, after which a resend is a true no-op again.
#[tokio::test]
async fn resend_after_torn_restamps_repairs_spans_instead_of_returning_unchanged() {
    let f = fx();
    let para = "A sentence about the cache layer and how it behaves under load. ".repeat(30);
    let doc = |intro: &str| {
        format!(
            "# Plan\n\n{intro}\n\n## Detail\n\n{p}\n\n{p}\n\n{p}\n\n{p}\n\n## Later\n\nFollow up.\n",
            p = para.trim()
        )
    };
    ingest(&f.state, "d", &doc("Intro.")).await;
    let v2 = doc("A longer introduction.");
    let r2 = ingest(&f.state, "d", &v2).await;
    assert_eq!(classes(&r2), vec!["changed", "unchanged", "unchanged"]);
    let rev2 = r2["revision_id"].as_str().unwrap().to_string();

    // Cut at the first re-stamp: the second revision's row, commit stamp and supersessions are
    // kept, every re-stamp of its carried-forward episodes is lost.
    let wal = truncated_wal(f.wal.path(), |v| {
        cypher(v).contains("SET ep.revision_id = $revision_id")
    });
    let (s2, db2, _d, ex) = replayed(wal.path()).await;
    let latest = db2
        .connect()
        .unwrap()
        .latest_doc_revision(G, "d")
        .unwrap()
        .unwrap();
    assert_eq!(latest.revision_id, rev2, "the second revision committed");
    let stale: Vec<Value> = episodes(&s2)
        .await
        .into_values()
        .filter(|e| e["superseded_at"].is_null() && e["revision_id"] != rev2.as_str())
        .collect();
    assert!(
        !stale.is_empty(),
        "carried-forward episodes kept the older stamp"
    );
    assert!(
        stale.iter().any(|e| {
            let (s, t) = (
                e["byte_start"].as_u64().unwrap() as usize,
                e["byte_end"].as_u64().unwrap() as usize,
            );
            v2.get(s..t) != e["content"].as_str()
        }),
        "and their spans no longer slice the document"
    );

    // Resending the identical document repairs them without extraction.
    let r = call("knowledge_process_document", doc_params("d", &v2), &s2).await;
    assert_eq!(
        r["status"], "committed",
        "must not be a no-op over stale spans: {r}"
    );
    assert_eq!(r["extraction_calls"], 0);
    assert_eq!(ex.call_count(), 0);
    assert_eq!(classes(&r), vec!["unchanged"; 3]);
    assert!(uuids(&r["superseded_episode_uuids"]).is_empty());
    let rev3 = r["revision_id"].as_str().unwrap().to_string();
    assert_live_episodes_match(&s2, &v2, &rev3).await;

    // Now consistent: the same bytes again are a true no-op.
    let r = call("knowledge_process_document", doc_params("d", &v2), &s2).await;
    assert_eq!(r["status"], "unchanged", "{r}");
}

/// US3 AC2, SC-004: a crash before the second revision's commit chunk rebuilds to the fully
/// "before" state — its pending episodes exist but nothing of them is believed.
#[tokio::test]
async fn crash_before_commit_rebuilds_to_the_previous_revision() {
    let f = fx();
    ingest(&f.state, "note-1", NOTE).await;
    let v1 = believed(&f.state).await;
    ingest(&f.state, "note-1", &NOTE.replace('.', "!")).await;
    // Cut at the *second* DocRevision write (the second revision's commit step).
    let seen = AtomicUsize::new(0);
    let wal = truncated_wal(f.wal.path(), |v| {
        cypher(v).contains("DocRevision") && seen.fetch_add(1, Ordering::SeqCst) == 1
    });
    let (s2, db2, _d, _) = replayed(wal.path()).await;
    assert_eq!(believed(&s2).await, v1, "fully before");
    let pending = snapshot(&db2)
        .iter()
        .filter(|r| r.contains("Some(\"") && r.contains(" None "))
        .count();
    assert!(
        pending >= 5,
        "the second revision's episodes are present but pending"
    );
}

/// A torn tail inside the commit chunk — the commit stamp landed, the supersessions did not —
/// leaves an overlap, which the next ingest of the document self-heals (stale episodes
/// superseded) without any extraction.
#[tokio::test]
async fn torn_commit_tail_overlap_self_heals_on_next_ingest() {
    let f = fx();
    ingest(&f.state, "note-1", NOTE).await;
    let v2 = NOTE
        .replace("We chose Redis.", "We chose Valkey.")
        .replace("## Risks\n\nOperational load.\n", "");
    let r2 = ingest(&f.state, "note-1", &v2).await;
    assert_eq!(r2["extraction_calls"], 1);
    let v2_facts = believed(&f.state).await;
    let seen = AtomicUsize::new(0);
    let wal = truncated_wal(f.wal.path(), |v| {
        // The second revision's first supersession statement (after its commit stamp).
        cypher(v).contains("SET ep.superseded_at")
            && cypher(v).contains("superseded_at IS NULL")
            && seen.fetch_add(1, Ordering::SeqCst) == 0
    });
    let (s2, _db2, _d, ex) = replayed(wal.path()).await;
    let overlap = believed(&s2).await;
    assert!(overlap.contains("We chose Redis.") && overlap.contains("We chose Valkey."));
    assert!(overlap.contains("Operational load."));

    let r = call("knowledge_process_document", doc_params("note-1", &v2), &s2).await;
    assert_eq!(r["status"], "committed");
    assert_eq!(r["extraction_calls"], 0);
    assert_eq!(ex.call_count(), 0);
    assert_eq!(uuids(&r["stale_superseded_episode_uuids"]).len(), 1);
    assert_eq!(uuids(&r["retired_episode_uuids"]).len(), 1);
    assert_eq!(believed(&s2).await, v2_facts, "self-healed");
}

// ── Issue #883: entity provenance uses the belief liveness predicate ────────────────────────

async fn find_entity(state: &Arc<AppState>, name: &str) -> Value {
    let r = call(
        "knowledge_find_entities",
        json!({"query": name, "group_ids": [G], "num_results": 50, "include_episodes": true}),
        state,
    )
    .await;
    r["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == name)
        .unwrap_or_else(|| panic!("no entity {name} in {r}"))
        .clone()
}

/// Ingests `NOTE`, then makes a rewrite (`.` -> `!`) fail on its third extraction so two pending
/// episodes remain. Returns the fixture, the v2 content, and the first pending unit's source
/// entity name (mentioned only by a pending episode) and its fact.
async fn pending_revision_fixture() -> (Fx, String, String, String) {
    let f = fx();
    ingest(&f.state, "note-1", NOTE).await;
    let v2 = NOTE.replace('.', "!");
    let before = f.ex.call_count();
    f.ex.fail_on.store(before + 3, Ordering::SeqCst);
    let v = try_call(
        "knowledge_process_document",
        doc_params("note-1", &v2),
        &f.state,
    )
    .await;
    assert!(
        v.get("error").is_some(),
        "extraction was meant to fail: {v}"
    );
    let fact = fact_of(&f.ex.calls()[before].body);
    assert!(fact.ends_with('!'), "{fact}");
    (f, v2, format!("{fact} src"), fact)
}

/// #883 FR-002/FR-003, SC-002: an entity mentioned only by an episode of a pending revision
/// reports no episodes and no support before commit, and the episode after commit.
#[tokio::test]
async fn find_entities_include_episodes_excludes_pending_revision_until_commit() {
    let (f, v2, name, _fact) = pending_revision_fixture().await;

    let pending = find_entity(&f.state, &name).await;
    assert_eq!(pending["episode_uuids"], json!([]), "{pending}");
    assert_eq!(pending["source_descriptions"], json!([]), "{pending}");
    assert_eq!(pending["support"]["primary"], 0, "{pending}");
    assert_eq!(pending["corroboration"], 0, "{pending}");

    // Retry commits the revision: the episode becomes a live mention.
    f.ex.fail_on.store(0, Ordering::SeqCst);
    ingest(&f.state, "note-1", &v2).await;
    let committed = find_entity(&f.state, &name).await;
    assert_eq!(
        committed["episode_uuids"].as_array().unwrap().len(),
        1,
        "{committed}"
    );
    assert_eq!(committed["support"]["primary"], 1, "{committed}");
}

/// #883 edge case: while a revision replacing a live one is pending, the earlier revision's
/// episodes stay listed (belief/search keep them live); after commit they are superseded and
/// drop out, and the new revision's appear.
#[tokio::test]
async fn pending_revision_keeps_previous_revision_episodes_listed_until_commit() {
    let (f, v2, new_name, fact) = pending_revision_fixture().await;
    let old_name = format!("{} src", fact.replace('!', "."));

    let old = find_entity(&f.state, &old_name).await;
    assert_eq!(old["episode_uuids"].as_array().unwrap().len(), 1, "{old}");
    assert_eq!(old["support"]["primary"], 1, "{old}");

    f.ex.fail_on.store(0, Ordering::SeqCst);
    ingest(&f.state, "note-1", &v2).await;

    let old = find_entity(&f.state, &old_name).await;
    assert_eq!(old["episode_uuids"], json!([]), "superseded: {old}");
    assert_eq!(old["support"]["primary"], 0, "{old}");
    let new = find_entity(&f.state, &new_name).await;
    assert_eq!(new["episode_uuids"].as_array().unwrap().len(), 1, "{new}");
}

/// #883 FR-004: edge `support` follows the same predicate as entity `support`.
#[tokio::test]
async fn edge_support_excludes_pending_revision_until_commit() {
    let (f, v2, _name, fact) = pending_revision_fixture().await;
    let edge_support = |state: Arc<AppState>, fact: String| async move {
        let r = call(
            "knowledge_find_relationships",
            json!({"query": fact, "group_ids": [G], "num_results": 50}),
            &state,
        )
        .await;
        r["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["fact"] == fact.as_str())
            .map(|e| e["support"]["primary"].clone())
    };
    // Pending: the edge is either not returned or carries no support — never `1`.
    let pending = edge_support(Arc::clone(&f.state), fact.clone()).await;
    assert!(
        pending.is_none() || pending == Some(json!(0)),
        "pending edge support: {pending:?}"
    );

    f.ex.fail_on.store(0, Ordering::SeqCst);
    ingest(&f.state, "note-1", &v2).await;
    assert_eq!(
        edge_support(Arc::clone(&f.state), fact).await,
        Some(json!(1))
    );
}

/// #883: the ingest veto (`entities_with_corroborating_mention`) deliberately keeps the
/// supersession-only predicate, so a pending *primary* mention still shields its entity.
#[tokio::test]
async fn corroborating_mention_veto_still_counts_pending_primary_revision() {
    let (f, _v2, name, _fact) = pending_revision_fixture().await;
    let entity = find_entity(&f.state, &name).await;
    let uuid = entity["uuid"].as_str().unwrap().to_string();
    let now_us = chrono::Utc::now().timestamp_micros();
    let shielded =
        f.db.connect()
            .unwrap()
            .entities_with_corroborating_mention(std::slice::from_ref(&uuid), now_us)
            .unwrap();
    assert!(shielded.contains(&uuid), "{shielded:?}");
}
