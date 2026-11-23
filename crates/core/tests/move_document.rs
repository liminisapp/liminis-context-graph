//! Issue #903 / ADR-0903: `knowledge_move_document` re-keys a document's live episodes from one
//! identity to another without re-extraction. These tests drive the real dispatcher over a real
//! database and WAL: no extractor call, reads cite the new path, `delete_by_source` follows,
//! rebuild-from-WAL and dump agree with the live graph, every refusal changes nothing (graph or
//! WAL), a batch is all-or-nothing, and the document-mode and supersession state moves with it.

use std::collections::{BTreeSet, HashMap};
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
const OTHER: &str = "other";

// ── Fixture ─────────────────────────────────────────────────────────────────

/// One fact per call (the body's last non-empty line); counts every call.
#[derive(Default)]
struct CountingExtractor {
    calls: AtomicUsize,
}

impl CountingExtractor {
    fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Extractor for CountingExtractor {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let fact = opts
            .episode_body
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        Box::pin(async move {
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
    ex: Arc<CountingExtractor>,
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

fn fx() -> Fx {
    let dir = TempDir::new().unwrap();
    let wal = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
    }
    let ex = Arc::new(CountingExtractor::default());
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

const NOTE: &str = "# Design note\n\nWe need a cache.\n\n## Goals\n\nShip the cache.\n\n## Options\n\nRedis or memcached.\n\n## Decision\n\nWe chose Redis.\n";

async fn ingest_in(
    state: &Arc<AppState>,
    group: &str,
    doc_id: &str,
    path: &str,
    content: &str,
) -> Value {
    call(
        "knowledge_process_document",
        json!({
            "doc_id": doc_id, "group_id": group, "content": content, "title": "Note",
            "source_path": path, "reference_time": "2026-11-01T00:00:00Z",
        }),
        state,
    )
    .await
}

async fn ingest(state: &Arc<AppState>, doc_id: &str, path: &str, content: &str) -> Value {
    ingest_in(state, G, doc_id, path, content).await
}

fn mv(from_doc: &str, from_path: &str, to_doc: &str, to_path: &str) -> Value {
    json!({
        "from": {"doc_id": from_doc, "source_path": from_path},
        "to": {"doc_id": to_doc, "source_path": to_path},
    })
}

async fn move_docs(state: &Arc<AppState>, moves: Vec<Value>) -> Value {
    try_call(
        "knowledge_move_document",
        json!({"group_id": G, "moves": moves}),
        state,
    )
    .await
}

fn live_at() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// The identity columns of every live episode of `group`, one line each, sorted.
fn identity_snapshot(db: &Db, group: &str) -> Vec<String> {
    let conn = db.connect().unwrap();
    let mut rows: Vec<String> = conn
        .move_candidates(Some(group), &live_at())
        .unwrap()
        .into_iter()
        .map(|c| {
            format!(
                "{} doc={:?} sf={:?} sd={} name={} chunk={:?}",
                c.uuid, c.doc_id, c.source_file, c.source_description, c.name, c.chunk_id
            )
        })
        .collect();
    rows.sort();
    rows
}

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

async fn entity_names(state: &Arc<AppState>) -> BTreeSet<String> {
    let r = call(
        "knowledge_get_nodes_by_group",
        json!({"group_ids": [G]}),
        state,
    )
    .await;
    r["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap().to_string())
        .collect()
}

fn wal_bytes(dir: &Path) -> Vec<u8> {
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        if p.is_dir() {
            for e in std::fs::read_dir(p).unwrap() {
                walk(&e.unwrap().path(), out);
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    files
        .iter()
        .flat_map(|f| std::fs::read(f).unwrap())
        .collect()
}

fn error_message(v: &Value) -> String {
    v["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

async fn replayed(source: &Path) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("r.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        WalReplayer::new(source)
            .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
            .unwrap();
    }
    let state = make_state(
        Arc::clone(&db),
        None,
        Arc::new(CountingExtractor::default()) as Arc<dyn Extractor>,
    );
    (state, db, dir)
}

// ── US1: one move ───────────────────────────────────────────────────────────

#[tokio::test]
async fn move_makes_no_extraction_and_every_read_cites_the_new_path() {
    let f = fx();
    ingest(&f.state, "meetings/standup.md", "meetings/standup.md", NOTE).await;
    let calls = f.ex.count();
    assert!(calls > 0);
    let facts = believed(&f.state).await;
    let entities = entity_names(&f.state).await;

    let r = move_docs(
        &f.state,
        vec![json!({
            "from": {"doc_id": "meetings/standup.md", "source_path": "meetings/standup.md"},
            "to": {"doc_id": "archive/standup.md", "source_path": "archive/standup.md"},
            "chunk_id_prefix": {"from": "meetings/standup.md::", "to": "archive/standup.md::"},
        })],
    )
    .await;
    assert_eq!(r["result"]["success"], true, "{r}");
    assert_eq!(r["result"]["moved"], 1);
    assert!(
        r["result"]["episodes_rekeyed"].as_u64().unwrap() >= 4,
        "{r}"
    );
    assert_eq!(
        r["result"]["moves"][0]["to"]["doc_id"],
        "archive/standup.md"
    );

    // SC-001: not one extractor call (the embedder is not on this path at all).
    assert_eq!(f.ex.count(), calls);
    // Entities and facts are exactly as before.
    assert_eq!(believed(&f.state).await, facts);
    assert_eq!(entity_names(&f.state).await, entities);

    // Every live episode carries the new identity, none the old.
    for line in identity_snapshot(&f.db, G) {
        assert!(line.contains("archive/standup.md"), "{line}");
        assert!(!line.contains("meetings/"), "{line}");
    }

    // Passage search cites the new path and never the old.
    let p = call(
        "knowledge_search_passages",
        json!({"query": "cache", "group_ids": [G], "min_score": 0.0}),
        &f.state,
    )
    .await;
    assert!(p["count"].as_u64().unwrap() > 0, "{p}");
    let text = p.to_string();
    assert!(text.contains("archive/standup.md"), "{text}");
    assert!(!text.contains("meetings/standup.md"), "{text}");

    // delete_by_source on the old path deletes nothing; on the new path it deletes the document.
    let old = call(
        "knowledge_delete_by_source",
        json!({"source_file": "meetings/standup.md", "group_ids": [G]}),
        &f.state,
    )
    .await;
    assert_eq!(old["deleted_count"], 0);
    let by_src = |src: &'static str| {
        let state = Arc::clone(&f.state);
        async move {
            call(
                "knowledge_get_entities_by_source",
                json!({"source": src}),
                &state,
            )
            .await["nodes"]
                .as_array()
                .unwrap()
                .len()
        }
    };
    assert!(by_src("archive/standup.md").await > 0);
    assert_eq!(by_src("meetings/standup.md").await, 0);
    let new = call(
        "knowledge_delete_by_source",
        json!({"source_file": "archive/standup.md", "group_ids": [G]}),
        &f.state,
    )
    .await;
    assert!(new["deleted_count"].as_u64().unwrap() >= 4, "{new}");
}

#[tokio::test]
async fn move_keeps_document_mode_baseline_so_reingest_does_not_re_extract() {
    let f = fx();
    ingest(&f.state, "a", "notes/a.md", NOTE).await;
    move_docs(&f.state, vec![mv("a", "notes/a.md", "b", "moved/b.md")]).await;
    let calls = f.ex.count();

    // Unchanged content at the new identity: a no-op, zero extraction.
    let r = ingest(&f.state, "b", "moved/b.md", NOTE).await;
    assert_eq!(f.ex.count(), calls, "{r}");

    // A changed chunk is diffed against the moved baseline: one extraction, not five.
    let edited = NOTE.replace("Redis or memcached.", "Redis or Valkey.");
    ingest(&f.state, "b", "moved/b.md", &edited).await;
    assert_eq!(f.ex.count(), calls + 1);
}

/// A destination `doc_id` with no live episode but leftover `DocRevision` rows (a hard delete
/// keeps them) must be refused: the move would otherwise merge two revision histories there.
#[tokio::test]
async fn destination_with_leftover_revision_history_is_refused() {
    let f = fx();
    ingest(&f.state, "b", "notes/b.md", NOTE).await;
    let del = call(
        "knowledge_delete_by_source",
        json!({"source_file": "notes/b.md", "group_ids": [G]}),
        &f.state,
    )
    .await;
    assert!(del["deleted_count"].as_u64().unwrap() >= 1, "{del}");
    ingest(&f.state, "a", "notes/a.md", &NOTE.replace("cache", "bus")).await;
    let before = identity_snapshot(&f.db, G);

    let r = move_docs(&f.state, vec![mv("a", "notes/a.md", "b", "moved/b.md")]).await;
    assert!(error_message(&r).contains("destination_has_history"), "{r}");
    assert_eq!(
        identity_snapshot(&f.db, G),
        before,
        "a refusal changes nothing"
    );
}

#[tokio::test]
async fn prefix_rewrite_renames_chunk_ids_and_descriptions() {
    let f = fx();
    ingest(&f.state, "a.md", "a.md", NOTE).await;
    let r = move_docs(
        &f.state,
        vec![json!({
            "from": {"doc_id": "a.md", "source_path": "a.md"},
            "to": {"doc_id": "b.md", "source_path": "b.md"},
            "chunk_id_prefix": {"from": "a.md::", "to": "b.md::"},
        })],
    )
    .await;
    assert_eq!(r["result"]["success"], true, "{r}");
    assert!(
        r["result"]["moves"][0]["chunk_ids_rewritten"]
            .as_u64()
            .unwrap()
            >= 4
    );
    for line in identity_snapshot(&f.db, G) {
        assert!(line.contains("name=b.md::"), "{line}");
        assert!(line.contains("sd=b.md:b.md::"), "{line}");
        assert!(!line.contains("a.md"), "{line}");
    }
    // The revision manifest follows, so the next ingest still pairs chunks by name.
    let calls = f.ex.count();
    ingest(&f.state, "b.md", "b.md", NOTE).await;
    assert_eq!(f.ex.count(), calls);
}

#[tokio::test]
async fn move_is_identical_after_wal_rebuild_and_dump() {
    let f = fx();
    ingest(&f.state, "a", "notes/a.md", NOTE).await;
    ingest(&f.state, "c", "notes/c.md", &NOTE.replace("cache", "queue")).await;
    move_docs(
        &f.state,
        vec![json!({
            "from": {"doc_id": "a", "source_path": "notes/a.md"},
            "to": {"doc_id": "b", "source_path": "moved/b.md"},
            "chunk_id_prefix": {"from": "a::", "to": "b::"},
        })],
    )
    .await;
    let live = identity_snapshot(&f.db, G);
    let facts = believed(&f.state).await;
    assert!(live.iter().any(|l| l.contains("name=b::")));

    let (s2, db2, _d2) = replayed(f.wal.path()).await;
    assert_eq!(identity_snapshot(&db2, G), live);
    assert_eq!(believed(&s2).await, facts);
    assert_eq!(
        db2.connect().unwrap().latest_doc_revision(G, "b").unwrap(),
        f.db.connect().unwrap().latest_doc_revision(G, "b").unwrap()
    );
    assert!(db2
        .connect()
        .unwrap()
        .latest_doc_revision(G, "a")
        .unwrap()
        .is_none());

    let out = TempDir::new().unwrap();
    let target = out.path().join("dump");
    let r = call(
        "knowledge_dump_wal",
        json!({"target_dir": target.to_str().unwrap()}),
        &f.state,
    )
    .await;
    assert_eq!(r["success"], true, "{r}");
    let (s3, db3, _d3) = replayed(&target).await;
    assert_eq!(identity_snapshot(&db3, G), live);
    assert_eq!(believed(&s3).await, facts);
}

#[tokio::test]
async fn origin_is_unchanged_and_dry_run_writes_nothing() {
    let f = fx();
    ingest(&f.state, "a", "notes/a.md", NOTE).await;
    let before = identity_snapshot(&f.db, G);
    let wal_before = wal_bytes(f.wal.path());

    let r = call(
        "knowledge_move_document",
        json!({"group_id": G, "dry_run": true,
               "moves": [mv("a", "notes/a.md", "b", "knowledge/b.md")]}),
        &f.state,
    )
    .await;
    assert_eq!(r["dry_run"], true);
    assert!(r["episodes_rekeyed"].as_u64().unwrap() > 0);
    assert_eq!(identity_snapshot(&f.db, G), before);
    assert_eq!(
        wal_bytes(f.wal.path()),
        wal_before,
        "a dry run logs nothing"
    );

    let origins = |db: &Db| -> Vec<Option<String>> {
        let conn = db.connect().unwrap();
        let mut v: Vec<_> = conn
            .retrieve_episodes_page(None, 1000, None, None)
            .unwrap()
            .into_iter()
            .map(|(e, _)| e.origin)
            .collect();
        v.sort();
        v
    };
    let o = origins(&f.db);
    move_docs(&f.state, vec![mv("a", "notes/a.md", "b", "knowledge/b.md")]).await;
    assert_eq!(origins(&f.db), o, "a move never changes origin");
}

#[tokio::test]
async fn a_doc_less_episode_moves_by_source_path_alone() {
    let f = fx();
    call(
        "knowledge_process_chunk",
        json!({"chunk_text": "plain chunk text", "chunk_id": "a.md::0", "source_file": "a.md",
               "group_id": G, "reference_time": "2026-08-04T00:00:00Z"}),
        &f.state,
    )
    .await;
    let r = move_docs(
        &f.state,
        vec![json!({"from": {"source_path": "a.md"}, "to": {"source_path": "b.md"}})],
    )
    .await;
    assert_eq!(r["result"]["episodes_rekeyed"], 1, "{r}");
    let snap = identity_snapshot(&f.db, G);
    assert!(
        snap[0].contains("sf=Some(\"b.md\")") && snap[0].contains("sd=b.md:a.md::0"),
        "{snap:?}"
    );
    let old = call(
        "knowledge_delete_by_source",
        json!({"source_file": "a.md", "group_ids": [G]}),
        &f.state,
    )
    .await;
    assert_eq!(old["deleted_count"], 0);
}

// ── US2 / US3: batches and refusals ─────────────────────────────────────────

#[tokio::test]
async fn a_batch_applies_every_move_in_one_wal_chunk() {
    let f = fx();
    for i in 0..5 {
        ingest(
            &f.state,
            &format!("d{i}"),
            &format!("old/d{i}.md"),
            &NOTE.replace("cache", &format!("cache{i}")),
        )
        .await;
    }
    let moves: Vec<Value> = (0..5)
        .map(|i| {
            mv(
                &format!("d{i}"),
                &format!("old/d{i}.md"),
                &format!("n{i}"),
                &format!("new/d{i}.md"),
            )
        })
        .collect();
    let r = move_docs(&f.state, moves).await;
    assert_eq!(r["result"]["moved"], 5, "{r}");
    for line in identity_snapshot(&f.db, G) {
        assert!(line.contains("new/d") && !line.contains("old/"), "{line}");
    }
}

async fn assert_refused(f: &Fx, moves: Vec<Value>, code: &str) {
    let snap = identity_snapshot(&f.db, G);
    let wal = wal_bytes(f.wal.path());
    let calls = f.ex.count();
    let r = move_docs(&f.state, moves).await;
    assert!(r.get("error").is_some(), "expected a refusal: {r}");
    let m = error_message(&r);
    assert!(
        m.contains("move_refused") && m.contains(code),
        "{code}: {m}"
    );
    assert_eq!(identity_snapshot(&f.db, G), snap, "{code}: graph changed");
    assert_eq!(wal_bytes(f.wal.path()), wal, "{code}: WAL changed");
    assert_eq!(f.ex.count(), calls);
}

#[tokio::test]
async fn refusals_leave_graph_and_wal_unchanged() {
    let f = fx();
    ingest(&f.state, "a", "notes/a.md", NOTE).await;
    ingest(&f.state, "b", "notes/b.md", &NOTE.replace("cache", "queue")).await;
    ingest_in(
        &f.state,
        OTHER,
        "x",
        "notes/x.md",
        &NOTE.replace("cache", "bus"),
    )
    .await;

    // Unknown source.
    assert_refused(
        &f,
        vec![mv("nope", "nope.md", "z", "z.md")],
        "unknown_source",
    )
    .await;
    // Destination already holds live episodes — a merge, not a move (by doc_id and by path).
    assert_refused(
        &f,
        vec![mv("a", "notes/a.md", "b", "elsewhere.md")],
        "occupied_destination",
    )
    .await;
    assert_refused(
        &f,
        vec![mv("a", "notes/a.md", "z", "notes/b.md")],
        "occupied_destination",
    )
    .await;
    // Cross-group: `x` lives in another group.
    assert_refused(&f, vec![mv("x", "notes/x.md", "y", "y.md")], "cross_group").await;
    // Identical.
    assert_refused(
        &f,
        vec![mv("a", "notes/a.md", "a", "notes/a.md")],
        "identical",
    )
    .await;
    // One bad item refuses the whole batch, including the valid move beside it.
    assert_refused(
        &f,
        vec![
            mv("a", "notes/a.md", "a2", "moved/a.md"),
            mv("nope", "nope.md", "z", "z.md"),
        ],
        "unknown_source",
    )
    .await;
    // Batch conflicts.
    assert_refused(
        &f,
        vec![
            mv("a", "notes/a.md", "m", "m.md"),
            mv("b", "notes/b.md", "m", "m.md"),
        ],
        "duplicate_destination",
    )
    .await;
    assert_refused(
        &f,
        vec![
            mv("a", "notes/a.md", "b", "notes/b.md"),
            mv("b", "notes/b.md", "c", "notes/c.md"),
        ],
        "destination_is_source",
    )
    .await;
    // A chunk-id rewrite colliding with a live chunk id elsewhere in the group.
    let collide = {
        let conn = f.db.connect().unwrap();
        let b = conn
            .move_candidates(Some(G), &live_at())
            .unwrap()
            .into_iter()
            .find(|c| c.doc_id.as_deref() == Some("b"))
            .unwrap();
        b.name
    };
    let a_name = {
        let conn = f.db.connect().unwrap();
        conn.move_candidates(Some(G), &live_at())
            .unwrap()
            .into_iter()
            .find(|c| c.doc_id.as_deref() == Some("a"))
            .unwrap()
            .name
    };
    assert_refused(
        &f,
        vec![json!({
            "from": {"doc_id": "a", "source_path": "notes/a.md"},
            "to": {"doc_id": "c", "source_path": "c.md"},
            "chunk_id_prefix": {"from": a_name, "to": collide},
        })],
        "chunk_id_collision",
    )
    .await;

    // Request-shape errors: empty batch, missing group, mismatched kinds.
    let r = move_docs(&f.state, vec![]).await;
    assert!(error_message(&r).contains("empty_batch"), "{r}");
    let r = try_call(
        "knowledge_move_document",
        json!({"moves": [mv("a", "notes/a.md", "q", "q.md")]}),
        &f.state,
    )
    .await;
    assert!(r.get("error").is_some(), "{r}");
    let r = move_docs(
        &f.state,
        vec![json!({"from": {"doc_id": "a"}, "to": {"source_path": "q.md"}})],
    )
    .await;
    assert!(r.get("error").is_some(), "{r}");
}

#[tokio::test]
async fn repeating_an_applied_move_is_refused_as_unknown_source() {
    let f = fx();
    ingest(&f.state, "a", "notes/a.md", NOTE).await;
    let m = mv("a", "notes/a.md", "b", "moved/b.md");
    assert_eq!(
        move_docs(&f.state, vec![m.clone()]).await["result"]["success"],
        true
    );
    assert_refused(&f, vec![m], "unknown_source").await;
}

#[tokio::test]
async fn superseded_history_moves_with_its_document_and_never_occupies() {
    let f = fx();
    ingest(&f.state, "a", "notes/a.md", NOTE).await;
    // Correct one chunk: the old revision's episode is superseded (no longer live).
    ingest(
        &f.state,
        "a",
        "notes/a.md",
        &NOTE.replace("Redis or memcached.", "Redis or Valkey."),
    )
    .await;
    let all = identity_snapshot(&f.db, G).len();
    let live = {
        let conn = f.db.connect().unwrap();
        conn.move_candidates(Some(G), &live_at())
            .unwrap()
            .iter()
            .filter(|c| c.live)
            .count()
    };
    assert!(
        all > live,
        "the correction left a superseded revision behind"
    );

    let r = move_docs(&f.state, vec![mv("a", "notes/a.md", "b", "moved/b.md")]).await;
    assert_eq!(
        r["result"]["episodes_rekeyed"].as_u64().unwrap() as usize,
        all,
        "{r}"
    );
    // History follows its document: nothing is left under the old identity, so deleting the old
    // path finds nothing and the vacated doc_id is free for a new document.
    assert!(identity_snapshot(&f.db, G)
        .iter()
        .all(|l| !l.contains("notes/a.md")));
    let old = call(
        "knowledge_delete_by_source",
        json!({"source_file": "notes/a.md", "group_ids": [G]}),
        &f.state,
    )
    .await;
    assert_eq!(old["deleted_count"], 0);
    ingest(&f.state, "a", "notes/a.md", "# Fresh\n\nUnrelated text.\n").await;

    // A document with only superseded history cannot be addressed by a move.
    let r = move_docs(&f.state, vec![mv("zzz", "zzz.md", "y", "y.md")]).await;
    assert!(error_message(&r).contains("unknown_source"), "{r}");
}

// ── Supersession state follows the doc_id ───────────────────────────────────

#[tokio::test]
async fn document_supersession_links_follow_a_moved_doc_id() {
    let f = fx();
    ingest(
        &f.state,
        "old",
        "notes/old.md",
        &NOTE.replace("cache", "memo"),
    )
    .await;
    ingest(
        &f.state,
        "new",
        "notes/new.md",
        &NOTE.replace("cache", "plan"),
    )
    .await;
    // Effective in the future: the old document stays live, so it can still be moved.
    call(
        "knowledge_supersede_document",
        json!({"group_id": G, "superseding_doc_id": "new", "superseded_doc_id": "old",
               "effective_at": "2099-01-01T00:00:00Z"}),
        &f.state,
    )
    .await;

    // Move the superseding document, then the superseded one, in one batch.
    let r = move_docs(
        &f.state,
        vec![
            mv("new", "notes/new.md", "new2", "moved/new.md"),
            mv("old", "notes/old.md", "old2", "moved/old.md"),
        ],
    )
    .await;
    assert_eq!(r["result"]["success"], true, "{r}");
    let conn = f.db.connect().unwrap();
    let recs = conn
        .get_doc_supersessions(&[(G.to_string(), "old2".to_string())])
        .unwrap();
    let rec = &recs[&(G.to_string(), "old2".to_string())];
    assert_eq!(rec.superseding_doc_id, "new2");
    assert!(conn
        .get_doc_supersessions(&[(G.to_string(), "old".to_string())])
        .unwrap()
        .is_empty());
    let uuids = conn.episode_uuids_superseded_by_doc(G, "new2").unwrap();
    assert!(!uuids.is_empty());
    assert!(conn
        .episode_uuids_superseded_by_doc(G, "new")
        .unwrap()
        .is_empty());

    // And it survives a rebuild.
    let (_s, db2, _d) = replayed(f.wal.path()).await;
    let conn2 = db2.connect().unwrap();
    let recs2 = conn2
        .get_doc_supersessions(&[(G.to_string(), "old2".to_string())])
        .unwrap();
    assert_eq!(
        recs2[&(G.to_string(), "old2".to_string())].superseding_doc_id,
        "new2"
    );
}

// ── Derived outputs ─────────────────────────────────────────────────────────

#[tokio::test]
async fn status_and_lint_name_the_new_identity_straight_away() {
    let f = fx();
    ingest(&f.state, "meetings/m.md", "meetings/m.md", NOTE).await;
    let before = call("knowledge_status", json!({}), &f.state).await;
    move_docs(
        &f.state,
        vec![mv(
            "meetings/m.md",
            "meetings/m.md",
            "archive/m.md",
            "archive/m.md",
        )],
    )
    .await;
    let after = call("knowledge_status", json!({}), &f.state).await;
    assert_eq!(before["episode_count"], after["episode_count"]);
    assert_eq!(before["entity_count"], after["entity_count"]);
    assert_eq!(before["relationship_count"], after["relationship_count"]);

    let lint = call("knowledge_lint", json!({"group_ids": [G]}), &f.state).await;
    assert!(!lint.to_string().contains("meetings/m.md"), "{lint}");
}

/// A move of a primary document is followed by `stale_derived_page`: the derived page's finding
/// is reported against the (new) document ids.
#[tokio::test]
async fn stale_derived_page_reports_the_moved_document() {
    let f = fx();
    let put = |chunk: &str, doc: &str, origin: Option<&str>, text: &str| {
        let mut p = json!({
            "chunk_text": text, "chunk_id": chunk, "source_file": format!("{doc}.md"),
            "group_id": G, "reference_time": "2026-08-04T00:00:00Z", "doc_id": doc,
        });
        if let Some(o) = origin {
            p["origin"] = json!(o);
        }
        p
    };
    call(
        "knowledge_process_chunk",
        put("note::0", "note", None, "alpha uses postgres"),
        &f.state,
    )
    .await;
    call(
        "knowledge_process_chunk",
        put("page::0", "page", Some("derived"), "alpha uses postgres"),
        &f.state,
    )
    .await;
    // The primary note is corrected: the page's support is now stale.
    call(
        "knowledge_process_chunk",
        put("note::0", "note", None, "alpha uses mysql instead"),
        &f.state,
    )
    .await;
    let lint_before = call(
        "knowledge_lint",
        json!({"group_ids": [G], "categories": ["stale_derived_page"]}),
        &f.state,
    )
    .await;

    move_docs(
        &f.state,
        vec![json!({
            "from": {"doc_id": "note", "source_path": "note.md"},
            "to": {"doc_id": "note-moved", "source_path": "moved/note.md"},
        })],
    )
    .await;
    let lint_after = call(
        "knowledge_lint",
        json!({"group_ids": [G], "categories": ["stale_derived_page"]}),
        &f.state,
    )
    .await;
    assert_eq!(lint_before["count"], lint_after["count"], "{lint_after}");
    let text = lint_after.to_string();
    assert!(
        !text.contains("\"note\""),
        "no finding may name the old doc_id: {text}"
    );
}

// ── Scale and concurrency ───────────────────────────────────────────────────

/// A folder rename is one call: 120 documents re-keyed together, no extraction.
#[tokio::test]
async fn a_folder_rename_is_one_call() {
    let f = fx();
    let n = 120;
    for i in 0..n {
        ingest(
            &f.state,
            &format!("old/d{i}.md"),
            &format!("old/d{i}.md"),
            &format!("# Doc {i}\n\nFact number {i} about the folder.\n"),
        )
        .await;
    }
    let calls = f.ex.count();
    let moves: Vec<Value> = (0..n)
        .map(|i| {
            json!({
                "from": {"doc_id": format!("old/d{i}.md"), "source_path": format!("old/d{i}.md")},
                "to": {"doc_id": format!("new/d{i}.md"), "source_path": format!("new/d{i}.md")},
                "chunk_id_prefix": {"from": format!("old/d{i}.md::"), "to": format!("new/d{i}.md::")},
            })
        })
        .collect();
    let r = move_docs(&f.state, moves).await;
    assert_eq!(r["result"]["moved"], n, "{r}");
    assert_eq!(f.ex.count(), calls);
    assert!(identity_snapshot(&f.db, G)
        .iter()
        .all(|l| !l.contains("old/")));
    let (_s, db2, _d) = replayed(f.wal.path()).await;
    assert_eq!(identity_snapshot(&db2, G), identity_snapshot(&f.db, G));
}

/// A document ingest racing a move into its `doc_id` never leaves two documents mixed under one
/// identity: either the move is refused (the destination was claimed first), or the move wins and
/// the ingest revises the moved document.
#[tokio::test]
async fn ingest_racing_a_move_into_the_destination_never_merges_silently() {
    let f = fx();
    ingest(&f.state, "a", "notes/a.md", NOTE).await;
    let st = Arc::clone(&f.state);
    let ing = tokio::spawn(async move {
        ingest(&st, "b", "moved/b.md", "# Other\n\nA different document.\n").await
    });
    let r = move_docs(&f.state, vec![mv("a", "notes/a.md", "b", "moved/b.md")]).await;
    ing.await.unwrap();
    let snap = identity_snapshot(&f.db, G);
    if r.get("error").is_some() {
        assert!(error_message(&r).contains("occupied_destination"), "{r}");
        assert!(snap.iter().any(|l| l.contains("doc=Some(\"a\")")));
    } else {
        // The move won; the ingest then revised document `b` as one document with one path.
        assert!(snap
            .iter()
            .filter(|l| l.contains("doc=Some(\"b\")"))
            .all(|l| l.contains("sf=Some(\"moved/b.md\")")));
    }
}
