// Per-request cancellation and `process_chunk` phase progress (issue #861, ADR-0861).
//
// Same style as `cancel_shutdown.rs`, but the token is a *child* of `state.cancel_token`, handed
// to `dispatch_cancellable` the way the connection layer does on a client disconnect:
//   - cancelling the child cancels only that request and never counts as a shutdown cancel;
//   - cancelling the parent (shutdown) still reaches the child;
//   - a commit whose write lock is already obtainable finishes even if the token is cancelled;
//   - a request cancelled before the write lock leaves no episode, no WAL record, no held lock and
//     no in-flight registration, so a retry extracts and commits once;
//   - `process_chunk` streams `embed`, `extract`, `commit` before the terminal result.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    error::Error,
    extractor::{ExtractOptions, MockExtractor},
    handlers,
    ipc::IpcRequest,
    telemetry::NoopSink,
    types::ExtractionOutcome,
    Extractor,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

// ── Extractors ────────────────────────────────────────────────────────────────

/// Delegates to `MockExtractor`, counting `extract` calls; optionally sleeps first (a slow LLM)
/// and optionally cancels a token once it has been reached (a disconnect mid-extraction).
struct TestExtractor {
    calls: Arc<AtomicUsize>,
    delay: Duration,
    cancel_on_extract: Option<CancellationToken>,
    /// When set, `extract` takes the group write lock and parks the guard here, so the request
    /// (which has already passed its own brief read-lock use) blocks at the commit's write-lock
    /// acquisition until the test drops the guard. A deterministic "commit not yet started" seam.
    hold_write_lock: Option<(Arc<RwLock<()>>, HeldGuard)>,
}

type HeldGuard = Arc<Mutex<Option<tokio::sync::OwnedRwLockWriteGuard<()>>>>;

impl Extractor for TestExtractor {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        let nth = self.calls.fetch_add(1, Ordering::SeqCst);
        let delay = self.delay;
        let cancel = self.cancel_on_extract.clone();
        // Only the first attempt parks the lock; a retry must be free to commit.
        let hold = self.hold_write_lock.clone().filter(|_| nth == 0);
        Box::pin(async move {
            if let Some((lock, slot)) = hold {
                let g = lock.write_owned().await;
                *slot.lock().unwrap() = Some(g);
            }
            if let Some(c) = cancel {
                c.cancel();
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            MockExtractor.extract(opts).await
        })
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        _allowed_types: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let count = entities.len();
        Box::pin(async move { Ok(vec![String::new(); count]) })
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        _allowed_types: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let count = edges.len();
        Box::pin(async move { Ok(vec![String::new(); count]) })
    }
}

// ── Fixture ───────────────────────────────────────────────────────────────────

struct Fixture {
    state: Arc<AppState>,
    calls: Arc<AtomicUsize>,
    held: HeldGuard,
    wal_dir: TempDir,
    _db_dir: TempDir,
}

fn fixture(delay: Duration, cancel_on_extract: Option<CancellationToken>) -> Fixture {
    fixture_with(delay, cancel_on_extract, false)
}

fn fixture_with(
    delay: Duration,
    cancel_on_extract: Option<CancellationToken>,
    hold_lock: bool,
) -> Fixture {
    let db_dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(db_dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(4).unwrap();
        conn.create_vector_indexes().unwrap();
    }
    let wal_dir = TempDir::new().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let write_lock = Arc::new(RwLock::new(()));
    let held: HeldGuard = Arc::new(Mutex::new(None));
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(4)),
        extractor: Arc::new(TestExtractor {
            calls: Arc::clone(&calls),
            delay,
            cancel_on_extract,
            hold_write_lock: hold_lock.then(|| (Arc::clone(&write_lock), Arc::clone(&held))),
        }),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock,
        sink: Arc::new(NoopSink),
        db_path: "test.db".to_string(),
        wal_root: Some(wal_dir.path().to_path_buf()),
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
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
        embedding_cache: std::sync::Arc::new(lcg_core::EmbeddingCache::new()),
    });
    Fixture {
        state,
        calls,
        held,
        wal_dir,
        _db_dir: db_dir,
    }
}

fn req(method: &str, params: Value) -> IpcRequest {
    IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    }
}

fn chunk_params(text: &str) -> Value {
    json!({
        "chunk_text": text, "chunk_id": "c1", "source_file": "doc.md",
        "group_id": "g", "reference_time": "2026-08-04T00:00:00Z",
    })
}

fn episode_count(state: &AppState) -> u64 {
    state
        .db
        .load_full()
        .unwrap()
        .connect()
        .unwrap()
        .count_nodes("Episodic")
        .unwrap()
}

fn files_under(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                files_under(&p)
            } else {
                1
            }
        })
        .sum()
}

fn is_cancelled_error(v: &Value) -> bool {
    v.get("error").is_some() && v["error"].to_string().to_lowercase().contains("cancel")
}

/// Drains whatever progress frames are already queued, as `(phase)` strings.
fn drain_phases(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(f) = rx.try_recv() {
        assert_eq!(f["type"], "progress");
        out.push(f["phase"].as_str().unwrap().to_string());
    }
    out
}

// ── Cancel before commit ──────────────────────────────────────────────────────

/// A per-request cancel during a slow extraction returns Cancelled, writes nothing, and is not
/// a shutdown cancel: the parent token stays live and `cancelled_chunks` stays 0.
#[tokio::test]
async fn request_cancel_during_extraction_leaves_no_trace() {
    let fx = fixture(Duration::from_secs(60), None);
    let child = fx.state.cancel_token.child_token();
    let s = Arc::clone(&fx.state);
    let c = child.clone();
    let handle = tokio::spawn(async move {
        let r = req("knowledge_process_chunk", chunk_params("slow chunk"));
        handlers::dispatch_cancellable(r, s, None, c).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    child.cancel();

    let resp = tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("request should exit promptly after its token is cancelled")
        .unwrap();
    let v = serde_json::to_value(resp).unwrap();
    assert!(is_cancelled_error(&v), "expected a cancelled error: {v}");
    assert_eq!(episode_count(&fx.state), 0, "no episode");
    assert_eq!(files_under(fx.wal_dir.path()), 0, "no WAL record");
    assert_eq!(fx.state.cancelled_chunks.load(Ordering::Relaxed), 0);
    assert!(!fx.state.cancel_token.is_cancelled());
    assert!(
        fx.state.write_lock.try_write().is_ok(),
        "write lock must not be left held"
    );
}

/// Shutdown (the parent token) still cascades to the per-request child and counts as a shutdown
/// cancel.
#[tokio::test]
async fn shutdown_cancel_reaches_child_token_and_counts() {
    let fx = fixture(Duration::from_secs(60), None);
    let child = fx.state.cancel_token.child_token();
    let s = Arc::clone(&fx.state);
    let handle = tokio::spawn(async move {
        let r = req("knowledge_process_chunk", chunk_params("slow chunk"));
        handlers::dispatch_cancellable(r, s, None, child).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    fx.state.cancel_token.cancel();

    let resp = tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("request should exit promptly on shutdown")
        .unwrap();
    assert!(is_cancelled_error(&serde_json::to_value(resp).unwrap()));
    assert_eq!(fx.state.cancelled_chunks.load(Ordering::Relaxed), 1);
}

/// A cancel that lands while the request waits for the write lock (the last cancellation point)
/// leaves nothing behind, releases the in-flight registration, and a retry on a fresh token
/// extracts and commits exactly once.
#[tokio::test]
async fn cancel_before_commit_then_retry_extracts_once() {
    let fx = fixture_with(Duration::ZERO, None, true);
    let child = fx.state.cancel_token.child_token();
    let (tx, mut rx) = unbounded_channel();
    let s = Arc::clone(&fx.state);
    let c = child.clone();
    let handle = tokio::spawn(async move {
        let r = req("knowledge_process_chunk", chunk_params("retry chunk"));
        handlers::dispatch_cancellable(r, s, Some(tx), c).await
    });
    // Wait for the `commit` frame: the request is now parked on the held write lock.
    loop {
        let f = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("commit frame")
            .expect("channel open");
        if f["phase"] == "commit" {
            break;
        }
    }
    child.cancel();
    let resp = tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("cancelled request exits while the lock is still held elsewhere")
        .unwrap();
    assert!(is_cancelled_error(&serde_json::to_value(resp).unwrap()));
    drop(fx.held.lock().unwrap().take());
    assert_eq!(episode_count(&fx.state), 0);
    assert_eq!(files_under(fx.wal_dir.path()), 0);
    assert_eq!(fx.calls.load(Ordering::SeqCst), 1);

    // Retry on a "new connection": a fresh token, same chunk.
    let retry = handlers::dispatch_cancellable(
        req("knowledge_process_chunk", chunk_params("retry chunk")),
        Arc::clone(&fx.state),
        None,
        fx.state.cancel_token.child_token(),
    )
    .await;
    let v = serde_json::to_value(retry).unwrap();
    assert_eq!(v["result"]["success"], true, "{v}");
    assert_eq!(
        fx.calls.load(Ordering::SeqCst),
        2,
        "one extraction per attempt"
    );
    assert_eq!(episode_count(&fx.state), 1, "committed once");
}

// ── Commit, once started, finishes ────────────────────────────────────────────

/// A token cancelled in the same instant the write lock becomes available does not stop the
/// commit: the lock arm is `biased`, so a write that can proceed always finishes (FR-006).
/// Current-thread runtime: dropping the guard and cancelling happen with no await in between.
#[tokio::test]
async fn cancel_at_commit_boundary_still_commits() {
    let fx = fixture_with(Duration::ZERO, None, true);
    let child = fx.state.cancel_token.child_token();
    let (tx, mut rx) = unbounded_channel();
    let s = Arc::clone(&fx.state);
    let c = child.clone();
    let handle = tokio::spawn(async move {
        let r = req("knowledge_process_chunk", chunk_params("commit chunk"));
        handlers::dispatch_cancellable(r, s, Some(tx), c).await
    });
    loop {
        let f = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("commit frame")
            .expect("channel open");
        if f["phase"] == "commit" {
            break;
        }
    }
    drop(fx.held.lock().unwrap().take());
    child.cancel();
    let resp = handle.await.unwrap();
    let v = serde_json::to_value(resp).unwrap();
    assert_eq!(v["result"]["success"], true, "commit must finish: {v}");
    assert_eq!(episode_count(&fx.state), 1);
    assert!(files_under(fx.wal_dir.path()) > 0, "WAL record written");
}

// ── Progress ──────────────────────────────────────────────────────────────────

/// With a progress sender the phases arrive in order, before the terminal result.
#[tokio::test]
async fn process_chunk_streams_phases_in_order() {
    let fx = fixture(Duration::ZERO, None);
    let (tx, mut rx) = unbounded_channel();
    let resp = handlers::dispatch_cancellable(
        req("knowledge_process_chunk", chunk_params("progress chunk")),
        Arc::clone(&fx.state),
        Some(tx),
        fx.state.cancel_token.child_token(),
    )
    .await;
    let v = serde_json::to_value(resp).unwrap();
    assert_eq!(v["result"]["success"], true, "{v}");
    assert_eq!(drain_phases(&mut rx), vec!["embed", "extract", "commit"]);
}

/// Without a token (no sender) the response is the plain result and nothing else is produced;
/// plain `dispatch` keeps working unchanged.
#[tokio::test]
async fn process_chunk_without_token_is_unchanged() {
    let fx = fixture(Duration::ZERO, None);
    let resp = handlers::dispatch(
        req("knowledge_process_chunk", chunk_params("plain chunk")),
        Arc::clone(&fx.state),
        None,
    )
    .await;
    let v = serde_json::to_value(resp).unwrap();
    assert_eq!(v["result"]["success"], true, "{v}");
    assert_eq!(episode_count(&fx.state), 1);
}

/// The #858 unchanged no-op and a validation failure both return a well-formed terminal response
/// with no progress frame for a phase that never ran.
#[tokio::test]
async fn noop_and_validation_failure_emit_no_frames() {
    let fx = fixture(Duration::ZERO, None);
    let first = handlers::dispatch(
        req("knowledge_process_chunk", chunk_params("same chunk")),
        Arc::clone(&fx.state),
        None,
    )
    .await;
    assert_eq!(
        serde_json::to_value(first).unwrap()["result"]["success"],
        true
    );

    let (tx, mut rx) = unbounded_channel();
    let again = handlers::dispatch_cancellable(
        req("knowledge_process_chunk", chunk_params("same chunk")),
        Arc::clone(&fx.state),
        Some(tx),
        fx.state.cancel_token.child_token(),
    )
    .await;
    let v = serde_json::to_value(again).unwrap();
    assert_eq!(v["result"]["status"], "unchanged", "{v}");
    assert!(drain_phases(&mut rx).is_empty());

    let (tx, mut rx) = unbounded_channel();
    let bad = handlers::dispatch_cancellable(
        req("knowledge_process_chunk", json!({"chunk_text": "x"})),
        Arc::clone(&fx.state),
        Some(tx),
        fx.state.cancel_token.child_token(),
    )
    .await;
    assert!(serde_json::to_value(bad).unwrap().get("error").is_some());
    assert!(drain_phases(&mut rx).is_empty());
}

// ── process_document ──────────────────────────────────────────────────────────

/// `knowledge_process_document` honours the per-request token at its pre-commit points: a cancel
/// during extraction returns Cancelled, commits no revision, and is not a shutdown cancel.
#[tokio::test]
async fn process_document_cancelled_before_commit() {
    let child_holder = CancellationToken::new();
    // The extractor cancels the per-request token the moment it is reached (a disconnect
    // mid-extraction); the parent shutdown token is never touched.
    let fx = fixture(Duration::ZERO, Some(child_holder.clone()));
    let resp = handlers::dispatch_cancellable(
        req(
            "knowledge_process_document",
            json!({
                "doc_id": "d1", "group_id": "g",
                "content": "# Title\n\nSome body text for the document.\n",
                "reference_time": "2026-08-04T00:00:00Z",
            }),
        ),
        Arc::clone(&fx.state),
        None,
        child_holder.clone(),
    )
    .await;
    let v = serde_json::to_value(resp).unwrap();
    assert!(is_cancelled_error(&v), "expected a cancelled error: {v}");
    assert!(!fx.state.cancel_token.is_cancelled());
    assert_eq!(fx.state.cancelled_chunks.load(Ordering::Relaxed), 0);
    assert_eq!(fx.calls.load(Ordering::SeqCst), 1);
    // The cancel was observed before the unit committed (the Phase A / pre-Phase-B check), so
    // no episode was written.
    assert_eq!(episode_count(&fx.state), 0);
}
