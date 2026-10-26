//! Integration tests for issue #651: entity merge consolidates summaries instead of
//! concatenating them, and always re-embeds `summary_embedding`.
//!
//! Covers SC-001 (bounded, non-concatenated summary after repeated merges), SC-002 (stored
//! embedding equals the embedding of the stored summary, live and after a WAL rebuild), SC-003
//! (bounded fallback with no / failing extractor), SC-004 (replay reproduces summaries with zero
//! consolidation calls), SC-005 (consolidation runs outside the write lock) and SC-006 (empty or
//! already-contained incoming summaries cost no extractor call and no re-embed), plus the
//! within-chunk chaining and cancellation edge cases.
//!
//! Merges are driven through `knowledge_add_episode` with a `PassthroughDedupAdapter`, so every
//! repeat of an entity name is an exact-name merge into the first insert.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::{CountingEmbedder, Embedder, HashEmbedder},
    error::Error,
    extractor::{ExtractOptions, Extractor},
    handlers,
    ipc::IpcRequest,
    replay::WalReplayer,
    summary_merge::MERGED_SUMMARY_CAP,
    telemetry::NoopSink,
    types::{ExtractedEntity, ExtractionOutcome, ExtractionResult, DEFAULT_KIND},
    WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;

const DIM: usize = 8;
const GRP: &str = "test-group";
const NAME: &str = "Acme";

// ── Stub extractor ────────────────────────────────────────────────────────────

type ConsolidateFn = Box<dyn Fn(&str, &str, usize) -> Result<String, Error> + Send + Sync>;

/// Returns queued extraction results in FIFO order, and answers `consolidate_summary` with a
/// caller-supplied closure (`None` = the trait default, i.e. "no consolidation available").
struct StubExtractor {
    queue: Mutex<VecDeque<ExtractionResult>>,
    consolidate: Option<ConsolidateFn>,
    calls: AtomicUsize,
    /// `(existing, incoming)` as seen by each consolidation call, in order.
    seen: Mutex<Vec<(String, String)>>,
    /// When set, every call records whether `try_write()` on this lock succeeded mid-call.
    probe_lock: Option<Arc<RwLock<()>>>,
    lock_free_during_call: AtomicBool,
    /// When set, the call announces it started and then never returns (cancellation test).
    hang: Option<Arc<Notify>>,
}

impl StubExtractor {
    fn new(results: Vec<ExtractionResult>, consolidate: Option<ConsolidateFn>) -> Self {
        Self {
            queue: Mutex::new(results.into_iter().collect()),
            consolidate,
            calls: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
            probe_lock: None,
            lock_free_during_call: AtomicBool::new(false),
            hang: None,
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Extractor for StubExtractor {
    fn extract<'a>(
        &'a self,
        _opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        let r = self.queue.lock().unwrap().pop_front().unwrap_or_default();
        Box::pin(async move { Ok(r.into()) })
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        _allowed_types: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let n = entities.len();
        Box::pin(async move { Ok(vec![String::new(); n]) })
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        _allowed_types: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let n = edges.len();
        Box::pin(async move { Ok(vec![String::new(); n]) })
    }

    fn consolidate_summary<'a>(
        &'a self,
        _entity_name: &'a str,
        existing: &'a str,
        incoming: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            self.seen
                .lock()
                .unwrap()
                .push((existing.to_string(), incoming.to_string()));
            if let Some(lock) = &self.probe_lock {
                if lock.try_write().is_ok() {
                    self.lock_free_during_call.store(true, Ordering::SeqCst);
                }
            }
            if let Some(started) = &self.hang {
                started.notify_one();
                std::future::pending::<()>().await;
            }
            match &self.consolidate {
                Some(f) => f(existing, incoming, n),
                None => Err(Error::Config("no consolidation".to_string())),
            }
        })
    }
}

// ── Harness ───────────────────────────────────────────────────────────────────

fn open_db(dir: &TempDir, name: &str) -> Arc<Db> {
    let db = Arc::new(Db::open(dir.path().join(name).to_str().unwrap()).unwrap());
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
    extractor: Arc<dyn Extractor>,
    write_lock: Arc<RwLock<()>>,
    wal_dir: Option<&std::path::Path>,
) -> Arc<AppState> {
    let mut writers = HashMap::new();
    if let Some(dir) = wal_dir {
        writers.insert(GRP.to_string(), WalWriter::new(dir, 10_000, 0).unwrap());
    }
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder,
        extractor,
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock,
        sink: Arc::new(NoopSink),
        db_path: "test.db".to_string(),
        wal_root: wal_dir.map(|d| d.to_path_buf()),
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(writers)),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built: Arc::new(AtomicBool::new(true)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: None,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn entity(summary: &str) -> ExtractedEntity {
    ExtractedEntity {
        name: NAME.to_string(),
        entity_type: "Entity".to_string(),
        summary: summary.to_string(),
        original_entity_type: None,
    }
}

/// One extraction result per episode, each naming `NAME` once with the given summary.
fn one_entity_per_episode(summaries: &[&str]) -> Vec<ExtractionResult> {
    summaries
        .iter()
        .map(|s| ExtractionResult {
            entities: vec![entity(s)],
            edges: vec![],
        })
        .collect()
}

async fn dispatch_raw(method: &str, params: Value, state: Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, state, None).await).unwrap()
}

async fn add_episode_raw(state: &Arc<AppState>, i: usize) -> Value {
    dispatch_raw(
        "knowledge_add_episode",
        json!({
            "name": format!("episode-{i}"),
            "episode_body": "ignored by the stub extractor",
            "source": "text",
            "reference_time": "2026-01-01T00:00:00Z",
            "group_id": GRP,
        }),
        Arc::clone(state),
    )
    .await
}

async fn add_episode(state: &Arc<AppState>, i: usize) {
    let v = add_episode_raw(state, i).await;
    assert!(v.get("error").is_none(), "add_episode failed: {v}");
}

fn stored_summary(db: &Db) -> String {
    let conn = db.connect().unwrap();
    conn.get_entity_by_name_ci(NAME, GRP, DEFAULT_KIND)
        .unwrap()
        .expect("entity exists")
        .summary
}

/// Asserts the entity's stored `summary_embedding` is the embedding of `summary`: the HNSW
/// nearest neighbour of `embed(summary)` is the entity at (near-)zero cosine distance. Needs the
/// summary index to exist (it does for the live DB; the rebuilt DB creates it first).
async fn assert_embedding_matches(db: &Db, embedder: &dyn Embedder, summary: &str) {
    let v = embedder.embed(summary).await.unwrap();
    let lit = format!(
        "[{}]",
        v.iter()
            .map(|x| format!("{x:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let conn = db.connect().unwrap();
    let rows: Vec<(String, f64)> = conn
        .query_cypher_raw(&format!(
            "CALL QUERY_VECTOR_INDEX('Entity', 'entity_summary_embedding_idx', {lit}, 1) \
             RETURN node.name, distance"
        ))
        .unwrap()
        .map(|r| (r[0].to_string(), r[1].to_string().parse().unwrap()))
        .collect();
    assert_eq!(rows.len(), 1, "one entity expected");
    assert_eq!(rows[0].0, NAME);
    assert!(
        rows[0].1 < 1e-4,
        "summary_embedding is stale: distance to embed(stored summary) = {}",
        rows[0].1
    );
}

fn hash_embedder() -> Arc<dyn Embedder> {
    Arc::new(HashEmbedder::new(DIM))
}

/// Replays the WAL in `wal_dir` into a fresh DB (recomputing vectors from text with `embedder`),
/// builds the summary index, and returns the DB.
async fn rebuild_from_wal(
    wal_dir: &std::path::Path,
    embedder: Arc<dyn Embedder>,
    dir: &TempDir,
) -> Db {
    let wal_dir = wal_dir.to_path_buf();
    let db_path = dir.path().join("rebuilt.db");
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let db = Db::open(db_path.to_str().unwrap()).unwrap();
        {
            let conn = db.connect().unwrap();
            conn.init_schema(DIM).unwrap();
            let embed_fn: lcg_core::replay::RecomputeEmbedFn =
                Box::new(move |texts: &[&str]| handle.block_on(embedder.embed_batch(texts)));
            let stats = WalReplayer::new(&wal_dir)
                .replay(&conn, embed_fn, DIM)
                .expect("replay succeeds");
            assert_eq!(stats.failed_lines, 0, "replay must be clean");
            conn.create_entity_summary_embedding_index().unwrap();
        }
        db
    })
    .await
    .unwrap()
}

fn contradictory() -> [&'static str; 5] {
    [
        "Acme is a startup building rockets. The new design is added.",
        "Acme remains unchanged. The rejected design was dropped.",
        "Acme adopted the original design after all.",
        "Acme is now headquartered in Ohio.",
        "Acme was acquired by Globex in 2026.",
    ]
}

// ── SC-001 ────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn five_contradictory_merges_yield_one_bounded_consolidated_summary() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir, "live.db");
    let inputs = contradictory();
    let ex = Arc::new(StubExtractor::new(
        one_entity_per_episode(&inputs),
        Some(Box::new(|_, _, n| {
            Ok(format!("Consolidated description v{n}."))
        })),
    ));
    let state = make_state(
        db.clone(),
        hash_embedder(),
        ex.clone(),
        Arc::new(RwLock::new(())),
        None,
    );
    for i in 0..inputs.len() {
        add_episode(&state, i).await;
    }

    // First episode inserts; the other four each merge → four consolidation calls.
    assert_eq!(ex.calls(), 4);
    let summary = stored_summary(&db);
    assert_eq!(summary, "Consolidated description v4.");
    assert!(summary.chars().count() <= MERGED_SUMMARY_CAP);
    for s in inputs {
        assert!(!summary.contains(s), "summary must not be a concatenation");
    }
    // Each consolidation builds on the previous result and receives the new incoming text.
    let seen = ex.seen.lock().unwrap().clone();
    assert_eq!(seen[0].0, inputs[0]);
    assert_eq!(seen[1].0, "Consolidated description v1.");
    assert_eq!(seen[3].0, "Consolidated description v3.");
    assert_eq!(seen[3].1, inputs[4]);
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_extractor_output_is_capped_at_a_sentence_boundary() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir, "live.db");
    let ex = Arc::new(StubExtractor::new(
        one_entity_per_episode(&["First.", "Second."]),
        Some(Box::new(|_, _, _| {
            Ok(format!("Lead sentence. {}", "filler ".repeat(300)))
        })),
    ));
    let state = make_state(
        db.clone(),
        hash_embedder(),
        ex,
        Arc::new(RwLock::new(())),
        None,
    );
    add_episode(&state, 0).await;
    add_episode(&state, 1).await;
    let summary = stored_summary(&db);
    assert!(summary.chars().count() <= MERGED_SUMMARY_CAP);
    assert_eq!(summary, "Lead sentence.");
}

// ── SC-002 / SC-004 ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn embedding_matches_stored_summary_live_and_after_wal_rebuild_with_zero_llm_calls() {
    let dir = TempDir::new().unwrap();
    let wal = TempDir::new().unwrap();
    let db = open_db(&dir, "live.db");
    let embedder = hash_embedder();
    let inputs = contradictory();
    let ex = Arc::new(StubExtractor::new(
        one_entity_per_episode(&inputs),
        Some(Box::new(|_, _, n| {
            Ok(format!("Consolidated description v{n}."))
        })),
    ));
    let state = make_state(
        db.clone(),
        Arc::clone(&embedder),
        ex.clone(),
        Arc::new(RwLock::new(())),
        Some(wal.path()),
    );
    for i in 0..inputs.len() {
        add_episode(&state, i).await;
    }
    let live_summary = stored_summary(&db);
    assert_eq!(live_summary, "Consolidated description v4.");
    assert_embedding_matches(&db, embedder.as_ref(), &live_summary).await;

    // Rebuild from the WAL: the recorded summary is applied verbatim, the vector is recomputed
    // from it, and no extractor is ever involved.
    let calls_before_replay = ex.calls();
    let rebuilt_dir = TempDir::new().unwrap();
    let rebuilt = rebuild_from_wal(wal.path(), Arc::clone(&embedder), &rebuilt_dir).await;
    assert_eq!(stored_summary(&rebuilt), live_summary);
    assert_embedding_matches(&rebuilt, embedder.as_ref(), &live_summary).await;
    assert_eq!(
        ex.calls(),
        calls_before_replay,
        "replay must not call an LLM"
    );
}

// ── SC-003 ────────────────────────────────────────────────────────────────────

async fn run_fallback_scenario(consolidate: Option<ConsolidateFn>) {
    let dir = TempDir::new().unwrap();
    let wal = TempDir::new().unwrap();
    let db = open_db(&dir, "live.db");
    let embedder = hash_embedder();
    let sentences: Vec<String> = (0..8)
        .map(|i| {
            format!(
                "Statement number {i} says {}.",
                "something notable ".repeat(8)
            )
        })
        .collect();
    let refs: Vec<&str> = sentences.iter().map(|s| s.as_str()).collect();
    let ex = Arc::new(StubExtractor::new(
        one_entity_per_episode(&refs),
        consolidate,
    ));
    let state = make_state(
        db.clone(),
        Arc::clone(&embedder),
        ex,
        Arc::new(RwLock::new(())),
        Some(wal.path()),
    );
    for i in 0..refs.len() {
        // A failing consolidation must never fail the chunk (FR-008).
        add_episode(&state, i).await;
    }
    let summary = stored_summary(&db);
    assert!(summary.chars().count() <= MERGED_SUMMARY_CAP, "{summary}");
    assert!(
        summary.ends_with(refs[7]),
        "fallback keeps the most recent text: {summary}"
    );
    assert!(summary.ends_with('.'), "cut at a sentence boundary");
    assert_embedding_matches(&db, embedder.as_ref(), &summary).await;

    // The WAL carries the resulting summary, so a rebuild agrees.
    let rebuilt_dir = TempDir::new().unwrap();
    let rebuilt = rebuild_from_wal(wal.path(), Arc::clone(&embedder), &rebuilt_dir).await;
    assert_eq!(stored_summary(&rebuilt), summary);
}

#[tokio::test(flavor = "multi_thread")]
async fn no_extractor_falls_back_to_bounded_merge_and_refreshes_embedding() {
    run_fallback_scenario(None).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn failing_extractor_falls_back_to_bounded_merge_and_refreshes_embedding() {
    run_fallback_scenario(Some(Box::new(|_, _, _| {
        Err(Error::Ipc("boom".to_string()))
    })))
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_extractor_output_falls_back() {
    run_fallback_scenario(Some(Box::new(|_, _, _| Ok("   ".to_string())))).await;
}

// ── SC-005 ────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn consolidation_runs_outside_the_write_lock() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir, "live.db");
    let write_lock = Arc::new(RwLock::new(()));
    let mut ex = StubExtractor::new(
        one_entity_per_episode(&["One.", "Two."]),
        Some(Box::new(|_, _, _| Ok("Merged.".to_string()))),
    );
    ex.probe_lock = Some(Arc::clone(&write_lock));
    let ex = Arc::new(ex);
    let state = make_state(db, hash_embedder(), ex.clone(), write_lock, None);
    add_episode(&state, 0).await;
    add_episode(&state, 1).await;
    assert_eq!(ex.calls(), 1);
    assert!(
        ex.lock_free_during_call.load(Ordering::SeqCst),
        "the write lock must be free while the consolidation call is in flight"
    );
}

// ── SC-006 ────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn empty_or_contained_incoming_makes_no_call_and_no_reembed() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir, "live.db");
    let counting = Arc::new(CountingEmbedder::new(hash_embedder()));
    let embedder: Arc<dyn Embedder> = counting.clone();
    let original = "Acme builds rockets. It is based in Ohio.";
    let ex = Arc::new(StubExtractor::new(
        one_entity_per_episode(&[original, "", "It is based in Ohio.", original]),
        Some(Box::new(|_, _, _| Ok("SHOULD NOT BE USED.".to_string()))),
    ));
    let state = make_state(
        db.clone(),
        Arc::clone(&embedder),
        ex.clone(),
        Arc::new(RwLock::new(())),
        None,
    );
    add_episode(&state, 0).await;
    let batches_after_insert = counting.batch_call_count();
    for i in 1..4 {
        add_episode(&state, i).await;
    }
    assert_eq!(
        ex.calls(),
        0,
        "no extractor call for empty/contained incoming"
    );
    assert_eq!(stored_summary(&db), original);
    assert_embedding_matches(&db, embedder.as_ref(), original).await;
    // Each later episode still runs the pre-lock name/summary embedding pass (2 batch calls plus
    // the edge-fact batch); the merge re-embed adds none, so the count is the same per episode as
    // a no-merge chunk would cost — compare against a per-episode baseline of 3.
    assert_eq!(
        counting.batch_call_count() - batches_after_insert,
        3 * 3,
        "no merge re-embed batch may be issued when the summary is unchanged"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn existing_empty_summary_uses_incoming_without_a_call() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir, "live.db");
    let embedder = hash_embedder();
    let ex = Arc::new(StubExtractor::new(
        one_entity_per_episode(&["", "Now it has a summary."]),
        Some(Box::new(|_, _, _| Ok("SHOULD NOT BE USED.".to_string()))),
    ));
    let state = make_state(
        db.clone(),
        Arc::clone(&embedder),
        ex.clone(),
        Arc::new(RwLock::new(())),
        None,
    );
    add_episode(&state, 0).await;
    add_episode(&state, 1).await;
    assert_eq!(ex.calls(), 0);
    let summary = stored_summary(&db);
    assert_eq!(summary, "Now it has a summary.");
    assert_embedding_matches(&db, embedder.as_ref(), &summary).await;
}

// ── Within-chunk chaining & cancellation ──────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn two_merges_into_one_entity_in_a_chunk_build_on_each_other() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir, "live.db");
    let embedder = hash_embedder();
    let results = vec![
        ExtractionResult {
            entities: vec![entity("Seed summary.")],
            edges: vec![],
        },
        ExtractionResult {
            entities: vec![entity("Alpha detail."), entity("Beta detail.")],
            edges: vec![],
        },
    ];
    let ex = Arc::new(StubExtractor::new(
        results,
        Some(Box::new(|existing, incoming, _| {
            Ok(format!("{existing} + {incoming}"))
        })),
    ));
    let state = make_state(
        db.clone(),
        Arc::clone(&embedder),
        ex.clone(),
        Arc::new(RwLock::new(())),
        None,
    );
    add_episode(&state, 0).await;
    add_episode(&state, 1).await;

    assert_eq!(ex.calls(), 2);
    let seen = ex.seen.lock().unwrap().clone();
    assert_eq!(seen[0].0, "Seed summary.");
    assert_eq!(
        seen[1].0, "Seed summary. + Alpha detail.",
        "the second merge consolidates the first merge's result, not the pre-chunk summary"
    );
    let summary = stored_summary(&db);
    assert_eq!(summary, "Seed summary. + Alpha detail. + Beta detail.");
    assert_embedding_matches(&db, embedder.as_ref(), &summary).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_during_consolidation_returns_cancelled() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir, "live.db");
    let started = Arc::new(Notify::new());
    let mut ex = StubExtractor::new(one_entity_per_episode(&["One.", "Two."]), None);
    ex.hang = Some(Arc::clone(&started));
    let state = make_state(
        db.clone(),
        hash_embedder(),
        Arc::new(ex),
        Arc::new(RwLock::new(())),
        None,
    );
    add_episode(&state, 0).await;

    let st = Arc::clone(&state);
    let handle = tokio::spawn(async move { add_episode_raw(&st, 1).await });
    started.notified().await;
    state.cancel_token.cancel();
    let v = handle.await.unwrap();
    assert!(v.get("error").is_some(), "cancelled chunk must error: {v}");
    assert_eq!(state.cancelled_chunks.load(Ordering::Relaxed), 1);
    assert_eq!(stored_summary(&db), "One.", "nothing was written");
}
