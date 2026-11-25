// Per-request deadline (`deadline_ms`) and joining a duplicate in-flight `process_chunk`
// (issue #914, ADR-0914). Same fixture style as `cancel_request.rs`.

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

// ── Extractor ─────────────────────────────────────────────────────────────────

/// What the extractor does on its nth call (0-based).
#[derive(Clone, Default)]
struct Script {
    /// Sleep before answering, per call index; calls past the end use the last entry (or none).
    delays: Vec<Duration>,
    /// Fail the first call (after its delay) with a structured `DbUnavailable` error.
    fail_first: bool,
    /// Panic on the first call.
    panic_first: bool,
    /// The first call takes the group write lock and parks the guard (see `cancel_request.rs`).
    hold_write_lock: bool,
}

struct TestExtractor {
    calls: Arc<AtomicUsize>,
    script: Script,
    write_lock: Arc<RwLock<()>>,
    held: HeldGuard,
}

type HeldGuard = Arc<Mutex<Option<tokio::sync::OwnedRwLockWriteGuard<()>>>>;

impl Extractor for TestExtractor {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        let nth = self.calls.fetch_add(1, Ordering::SeqCst);
        let s = self.script.clone();
        let delay = s
            .delays
            .get(nth)
            .or(s.delays.last())
            .copied()
            .unwrap_or(Duration::ZERO);
        let lock = Arc::clone(&self.write_lock);
        let held = Arc::clone(&self.held);
        Box::pin(async move {
            if nth == 0 && s.panic_first {
                panic!("scripted extractor panic");
            }
            if nth == 0 && s.hold_write_lock {
                let g = lock.write_owned().await;
                *held.lock().unwrap() = Some(g);
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            if nth == 0 && s.fail_first {
                return Err(Error::DbUnavailable("scripted failure".to_string()));
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

fn fixture(script: Script) -> Fixture {
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
            script,
            write_lock: Arc::clone(&write_lock),
            held: Arc::clone(&held),
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

fn slow_first(first: Duration) -> Script {
    Script {
        delays: vec![first, Duration::ZERO],
        ..Default::default()
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

fn chunk(text: &str) -> Value {
    json!({
        "chunk_text": text, "chunk_id": "c1", "source_file": "doc.md",
        "group_id": "g", "reference_time": "2026-08-04T00:00:00Z",
    })
}

fn with(mut p: Value, key: &str, v: Value) -> Value {
    p[key] = v;
    p
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

/// Sends one request on a fresh per-request token (a child of the shutdown token).
async fn send(fx: &Fixture, method: &str, params: Value) -> Value {
    send_on(
        fx,
        method,
        params,
        fx.state.cancel_token.child_token(),
        None,
    )
    .await
}

async fn send_on(
    fx: &Fixture,
    method: &str,
    params: Value,
    token: CancellationToken,
    progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
) -> Value {
    let resp =
        handlers::dispatch_cancellable(req(method, params), Arc::clone(&fx.state), progress, token)
            .await;
    serde_json::to_value(resp).unwrap()
}

fn spawn_chunk(
    fx: &Fixture,
    params: Value,
    token: CancellationToken,
) -> tokio::task::JoinHandle<Value> {
    let state = Arc::clone(&fx.state);
    tokio::spawn(async move {
        let resp = handlers::dispatch_cancellable(
            req("knowledge_process_chunk", params),
            state,
            None,
            token,
        )
        .await;
        serde_json::to_value(resp).unwrap()
    })
}

async fn wait_calls(fx: &Fixture, n: usize) {
    for _ in 0..1000 {
        if fx.calls.load(Ordering::SeqCst) >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("extractor never reached {n} calls");
}

async fn wait_inflight(fx: &Fixture, n: usize) {
    for _ in 0..1000 {
        if fx.state.chunk_inflight_count("g", "c1") >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("chunk_inflight never reached {n}");
}

fn is_cancelled_error(v: &Value) -> bool {
    v["error"]["code"] == -32000
        && v["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("cancelled"))
}

fn assert_deadline_error(v: &Value, phase: &str) {
    assert_eq!(v["error"]["code"], -32005, "{v}");
    assert_eq!(v["error"]["data"]["reason"], "deadline_exceeded", "{v}");
    assert_eq!(v["error"]["data"]["phase"], phase, "{v}");
    assert_eq!(v["error"]["data"]["committed"], false, "{v}");
}

fn assert_no_trace(fx: &Fixture) {
    assert_eq!(episode_count(&fx.state), 0, "no episode");
    assert_eq!(files_under(fx.wal_dir.path()), 0, "no WAL record");
    assert!(
        fx.state.write_lock.try_write().is_ok(),
        "write lock must not be left held"
    );
    assert_eq!(fx.state.chunk_inflight_count("g", "c1"), 0);
}

// ═════════════════════════════════════════════════════════════════════════════
// deadline_ms
// ═════════════════════════════════════════════════════════════════════════════

/// A slow extractor and a short deadline: `deadline_exceeded` before commit, naming the phase,
/// with nothing written, the lock free and no shutdown-cancel accounting.
#[tokio::test]
async fn deadline_before_commit_returns_structured_error_and_writes_nothing() {
    let fx = fixture(slow_first(Duration::from_secs(60)));
    let (tx, mut rx) = unbounded_channel();
    let v = tokio::time::timeout(
        Duration::from_secs(10),
        send_on(
            &fx,
            "knowledge_process_chunk",
            with(chunk("slow"), "deadline_ms", json!(150)),
            fx.state.cancel_token.child_token(),
            Some(tx),
        ),
    )
    .await
    .expect("a deadline must end a slow request promptly");
    assert_deadline_error(&v, "extract");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("nothing was written"));
    assert_no_trace(&fx);
    assert_eq!(fx.state.cancelled_chunks.load(Ordering::Relaxed), 0);
    assert!(!fx.state.cancel_token.is_cancelled());
    // Frames sent before the terminal error stay valid.
    let mut phases = Vec::new();
    while let Ok(f) = rx.try_recv() {
        phases.push(f["phase"].as_str().unwrap().to_string());
    }
    assert_eq!(phases, vec!["embed", "extract"]);

    // The request is safe to retry: it extracts and commits once.
    let retry = send(&fx, "knowledge_process_chunk", chunk("slow")).await;
    assert_eq!(retry["result"]["success"], true, "{retry}");
    assert_eq!(episode_count(&fx.state), 1);
}

/// A deadline comfortably longer than the work changes nothing and reports nothing.
#[tokio::test]
async fn deadline_met_carries_no_indication() {
    let fx = fixture(Script::default());
    let v = send(
        &fx,
        "knowledge_process_chunk",
        with(chunk("quick"), "deadline_ms", json!(600_000)),
    )
    .await;
    assert_eq!(v["result"]["success"], true, "{v}");
    assert!(v["result"].get("deadline_exceeded").is_none(), "{v}");
    assert!(v["result"].get("joined").is_none(), "{v}");
    assert_eq!(episode_count(&fx.state), 1);
}

/// A request waiting for the group write lock behind another writer is still before commit.
#[tokio::test]
async fn deadline_while_waiting_for_write_lock_is_abandoned() {
    let fx = fixture(Script {
        hold_write_lock: true,
        ..Default::default()
    });
    let v = tokio::time::timeout(
        Duration::from_secs(10),
        send(
            &fx,
            "knowledge_process_chunk",
            with(chunk("lock wait"), "deadline_ms", json!(300)),
        ),
    )
    .await
    .expect("deadline must fire while waiting for the lock");
    assert_deadline_error(&v, "awaiting_write_lock");
    drop(fx.held.lock().unwrap().take());
    assert_no_trace(&fx);
}

/// A deadline that passes once the commit has started does not tear it: the write completes and
/// the response says the deadline was exceeded. The commit is stalled deterministically by
/// holding `wal_writers` (taken inside the commit) until well after the deadline.
#[tokio::test]
async fn deadline_passing_during_commit_still_commits_and_flags_it() {
    let fx = fixture(Script::default());
    let state = Arc::clone(&fx.state);
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let g = state.wal_writers.lock().unwrap();
        locked_tx.send(()).unwrap();
        let _ = release_rx.recv();
        drop(g);
    });
    locked_rx.recv().unwrap();

    let (tx, mut rx) = unbounded_channel();
    let handle = {
        let state = Arc::clone(&fx.state);
        let token = fx.state.cancel_token.child_token();
        tokio::spawn(async move {
            let resp = handlers::dispatch_cancellable(
                req(
                    "knowledge_process_chunk",
                    with(chunk("commit stall"), "deadline_ms", json!(400)),
                ),
                state,
                Some(tx),
                token,
            )
            .await;
            serde_json::to_value(resp).unwrap()
        })
    };
    // The `commit` frame proves the write lock is held and the commit started.
    loop {
        let f = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("commit frame")
            .expect("channel open");
        if f["phase"] == "commit" {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(700)).await; // past the deadline, mid-commit
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    let v = handle.await.unwrap();
    assert_eq!(v["result"]["success"], true, "commit must finish: {v}");
    assert_eq!(v["result"]["deadline_exceeded"], true, "{v}");
    assert_eq!(episode_count(&fx.state), 1);
    assert!(files_under(fx.wal_dir.path()) > 0, "WAL record written");
}

/// Invalid `deadline_ms` is rejected before any work or registration.
#[tokio::test]
async fn invalid_deadline_ms_is_rejected_before_any_work() {
    let fx = fixture(Script::default());
    for bad in [json!(0), json!(-1), json!(1.5), json!("100"), json!(true)] {
        let v = send(
            &fx,
            "knowledge_process_chunk",
            with(chunk("x"), "deadline_ms", bad.clone()),
        )
        .await;
        assert_eq!(v["error"]["code"], -32000, "{bad}: {v}");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("deadline_ms"),
            "{bad}: {v}"
        );
        let d = send(
            &fx,
            "knowledge_process_document",
            json!({"doc_id": "d", "group_id": "g", "content": "# T\n\nbody\n", "deadline_ms": bad}),
        )
        .await;
        assert!(
            d["error"]["message"]
                .as_str()
                .unwrap()
                .contains("deadline_ms"),
            "{d}"
        );
    }
    assert_eq!(fx.calls.load(Ordering::SeqCst), 0, "no extraction");
    assert_no_trace(&fx);
}

/// An unchanged no-op returns before any cancellation point, so a deadline cannot touch it.
#[tokio::test]
async fn unchanged_noop_ignores_the_deadline() {
    let fx = fixture(Script::default());
    let first = send(&fx, "knowledge_process_chunk", chunk("same")).await;
    assert_eq!(first["result"]["success"], true);
    let again = send(
        &fx,
        "knowledge_process_chunk",
        with(chunk("same"), "deadline_ms", json!(1)),
    )
    .await;
    assert_eq!(again["result"]["status"], "unchanged", "{again}");
    assert!(
        again["result"].get("deadline_exceeded").is_none(),
        "{again}"
    );
}

/// A client disconnect (the per-request token cancelled) before the deadline is reported as the
/// cancellation it is, not as a deadline.
#[tokio::test]
async fn disconnect_before_deadline_is_a_plain_cancel() {
    let fx = fixture(slow_first(Duration::from_secs(60)));
    let token = fx.state.cancel_token.child_token();
    let h = spawn_chunk(
        &fx,
        with(chunk("dc"), "deadline_ms", json!(30_000)),
        token.clone(),
    );
    wait_calls(&fx, 1).await;
    token.cancel();
    let v = h.await.unwrap();
    assert!(is_cancelled_error(&v), "{v}");
    assert_eq!(fx.state.cancelled_chunks.load(Ordering::Relaxed), 0);
    assert_no_trace(&fx);
}

/// Shutdown still cancels, and still counts, with a deadline set.
#[tokio::test]
async fn shutdown_with_deadline_is_counted_as_shutdown() {
    let fx = fixture(slow_first(Duration::from_secs(60)));
    let h = spawn_chunk(
        &fx,
        with(chunk("sd"), "deadline_ms", json!(30_000)),
        fx.state.cancel_token.child_token(),
    );
    wait_calls(&fx, 1).await;
    fx.state.cancel_token.cancel();
    let v = h.await.unwrap();
    assert!(is_cancelled_error(&v), "{v}");
    assert_eq!(fx.state.cancelled_chunks.load(Ordering::Relaxed), 1);
}

const TWO_SECTION_DOC: &str = "# Alpha\n\nAlpha section body text about Ada Lovelace.\n\n# Beta\n\nBeta section body text about Grace Hopper.\n";

/// A document whose deadline passes before the revision commits returns `deadline_exceeded` with
/// `partial`, retires nothing and commits no revision; units already extracted stay.
#[tokio::test]
async fn document_deadline_is_partial_and_commits_no_revision() {
    // Call 0 (first unit) is quick; every later call is slow.
    let fx = fixture(Script {
        delays: vec![Duration::ZERO, Duration::from_secs(60)],
        ..Default::default()
    });
    let params = json!({
        "doc_id": "d1", "group_id": "g", "content": TWO_SECTION_DOC,
        "reference_time": "2026-08-04T00:00:00Z", "deadline_ms": 800,
    });
    let v = tokio::time::timeout(
        Duration::from_secs(30),
        send(&fx, "knowledge_process_document", params),
    )
    .await
    .expect("deadline must end the document ingest");
    assert_eq!(v["error"]["code"], -32005, "{v}");
    assert_eq!(v["error"]["data"]["reason"], "deadline_exceeded", "{v}");
    assert_eq!(v["error"]["data"]["partial"], true, "{v}");
    assert_eq!(v["error"]["data"]["phase"], "extracting", "{v}");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("did not complete"));
    assert_eq!(fx.state.cancelled_chunks.load(Ordering::Relaxed), 0);
    // The first unit committed and stays; the second was abandoned.
    assert_eq!(episode_count(&fx.state), 1, "committed unit stays in place");
    assert_eq!(fx.calls.load(Ordering::SeqCst), 2);
    let db = fx.state.db.load_full().unwrap();
    let conn = db.connect().unwrap();
    assert_eq!(
        conn.latest_doc_revision("g", "d1")
            .unwrap()
            .map(|r| r.revision_id),
        None,
        "no revision committed"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// join
// ═════════════════════════════════════════════════════════════════════════════

fn joined(v: &Value) -> bool {
    v["result"]["joined"] == true
}

/// Two identical concurrent requests: one extraction, two successes, the second marked joined,
/// the same episode, and the in-flight count is not raised by the follower.
#[tokio::test]
async fn identical_concurrent_requests_extract_once() {
    let fx = fixture(slow_first(Duration::from_millis(600)));
    let leader = spawn_chunk(&fx, chunk("same text"), fx.state.cancel_token.child_token());
    wait_calls(&fx, 1).await;
    let follower = spawn_chunk(&fx, chunk("same text"), fx.state.cancel_token.child_token());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        fx.state.chunk_inflight_count("g", "c1"),
        1,
        "a follower does not register in chunk_inflight"
    );
    let (l, f) = (leader.await.unwrap(), follower.await.unwrap());
    assert_eq!(l["result"]["success"], true, "{l}");
    assert_eq!(f["result"]["success"], true, "{f}");
    assert!(!joined(&l), "{l}");
    assert!(joined(&f), "{f}");
    assert_eq!(fx.calls.load(Ordering::SeqCst), 1, "one extraction");
    assert_eq!(l["result"]["episode_uuid"], f["result"]["episode_uuid"]);
    assert_eq!(episode_count(&fx.state), 1);
    assert_eq!(fx.state.chunk_inflight_count("g", "c1"), 0);
}

/// Many simultaneous identical requests elect exactly one leader.
#[tokio::test]
async fn many_simultaneous_requests_elect_one_leader() {
    let fx = fixture(slow_first(Duration::from_millis(800)));
    let handles: Vec<_> = (0..8)
        .map(|_| spawn_chunk(&fx, chunk("crowd"), fx.state.cancel_token.child_token()))
        .collect();
    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.unwrap());
    }
    assert!(
        results.iter().all(|r| r["result"]["success"] == true),
        "{results:?}"
    );
    assert_eq!(fx.calls.load(Ordering::SeqCst), 1, "one extraction");
    assert_eq!(results.iter().filter(|r| joined(r)).count(), 7);
    assert_eq!(episode_count(&fx.state), 1);
}

/// The leader's client disconnects before commit: the joined request does not inherit the
/// cancellation, takes over by running its own extraction, and exactly one episode results.
#[tokio::test]
async fn follower_survives_leader_disconnect() {
    let fx = fixture(slow_first(Duration::from_secs(60)));
    let leader_token = fx.state.cancel_token.child_token();
    let leader = spawn_chunk(&fx, chunk("takeover"), leader_token.clone());
    wait_calls(&fx, 1).await;
    let follower = spawn_chunk(&fx, chunk("takeover"), fx.state.cancel_token.child_token());
    tokio::time::sleep(Duration::from_millis(100)).await;
    leader_token.cancel();
    let l = leader.await.unwrap();
    let f = tokio::time::timeout(Duration::from_secs(10), follower)
        .await
        .expect("follower must not hang")
        .unwrap();
    assert!(is_cancelled_error(&l), "{l}");
    assert_eq!(f["result"]["success"], true, "{f}");
    assert_eq!(fx.calls.load(Ordering::SeqCst), 2, "one extra extraction");
    assert_eq!(episode_count(&fx.state), 1, "exactly one episode");
}

/// The leader abandoned by its own deadline: several followers all succeed; one of them becomes
/// the new leader and the rest join it, so the extractor ran twice in total.
#[tokio::test]
async fn followers_survive_leader_deadline() {
    let fx = fixture(slow_first(Duration::from_secs(60)));
    let leader = spawn_chunk(
        &fx,
        with(chunk("deadline takeover"), "deadline_ms", json!(500)),
        fx.state.cancel_token.child_token(),
    );
    wait_calls(&fx, 1).await;
    let followers: Vec<_> = (0..3)
        .map(|_| {
            spawn_chunk(
                &fx,
                chunk("deadline takeover"),
                fx.state.cancel_token.child_token(),
            )
        })
        .collect();
    let l = leader.await.unwrap();
    assert_deadline_error(&l, "extract");
    let mut fs = Vec::new();
    for f in followers {
        fs.push(
            tokio::time::timeout(Duration::from_secs(10), f)
                .await
                .expect("follower must not hang")
                .unwrap(),
        );
    }
    assert!(fs.iter().all(|f| f["result"]["success"] == true), "{fs:?}");
    assert!(
        fs.iter()
            .all(|f| f["result"].get("deadline_exceeded").is_none()),
        "followers must not inherit the leader's deadline: {fs:?}"
    );
    assert_eq!(fx.calls.load(Ordering::SeqCst), 2, "leader + one survivor");
    assert_eq!(fs.iter().filter(|f| joined(f)).count(), 2);
    assert_eq!(episode_count(&fx.state), 1);
}

/// A follower's own deadline applies only to it.
#[tokio::test]
async fn follower_deadline_does_not_cancel_leader() {
    let fx = fixture(slow_first(Duration::from_millis(1200)));
    let leader = spawn_chunk(
        &fx,
        chunk("own deadline"),
        fx.state.cancel_token.child_token(),
    );
    wait_calls(&fx, 1).await;
    let f = send(
        &fx,
        "knowledge_process_chunk",
        with(chunk("own deadline"), "deadline_ms", json!(200)),
    )
    .await;
    assert_deadline_error(&f, "awaiting_inflight");
    let l = leader.await.unwrap();
    assert_eq!(l["result"]["success"], true, "{l}");
    assert_eq!(fx.calls.load(Ordering::SeqCst), 1);
    assert_eq!(episode_count(&fx.state), 1);
}

/// A follower's own disconnect drops only it.
#[tokio::test]
async fn follower_disconnect_does_not_cancel_leader() {
    let fx = fixture(slow_first(Duration::from_millis(1000)));
    let leader = spawn_chunk(&fx, chunk("own dc"), fx.state.cancel_token.child_token());
    wait_calls(&fx, 1).await;
    let ftoken = fx.state.cancel_token.child_token();
    let follower = spawn_chunk(&fx, chunk("own dc"), ftoken.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;
    ftoken.cancel();
    assert!(is_cancelled_error(&follower.await.unwrap()));
    let l = leader.await.unwrap();
    assert_eq!(l["result"]["success"], true, "{l}");
    assert_eq!(fx.calls.load(Ordering::SeqCst), 1);
}

/// Different content, a different group, `supersede:false` and `same_revision_episode_uuids`
/// are never joined: each extracts for itself.
#[tokio::test]
async fn non_identical_requests_are_not_joined() {
    for variant in ["text", "group", "no_supersede", "keep"] {
        let fx = fixture(slow_first(Duration::from_millis(500)));
        let first = spawn_chunk(&fx, chunk("base"), fx.state.cancel_token.child_token());
        wait_calls(&fx, 1).await;
        let second_params = match variant {
            "text" => chunk("different"),
            "group" => with(chunk("base"), "group_id", json!("other")),
            "no_supersede" => with(chunk("base"), "supersede", json!(false)),
            _ => with(chunk("base"), "same_revision_episode_uuids", json!([])),
        };
        // `keep: []` is the empty list: it must still be treated as joinable-by-content only when
        // identical, so use a non-empty one for the "keep" variant.
        let second_params = if variant == "keep" {
            with(
                chunk("base"),
                "same_revision_episode_uuids",
                json!(["00000000-0000-0000-0000-000000000000"]),
            )
        } else {
            second_params
        };
        let second = spawn_chunk(&fx, second_params, fx.state.cancel_token.child_token());
        let (a, b) = (first.await.unwrap(), second.await.unwrap());
        assert!(!joined(&a) && !joined(&b), "{variant}: {a} {b}");
        assert_eq!(a["result"]["success"], true, "{variant}: {a}");
        assert_eq!(b["result"]["success"], true, "{variant}: {b}");
        assert_eq!(
            fx.calls.load(Ordering::SeqCst),
            2,
            "{variant}: own extraction"
        );
    }
}

/// A leader failure that is not a cancellation is shared with its structured code.
#[tokio::test]
async fn leader_failure_is_shared_with_followers() {
    let fx = fixture(Script {
        delays: vec![Duration::from_millis(500)],
        fail_first: true,
        ..Default::default()
    });
    let leader = spawn_chunk(&fx, chunk("fails"), fx.state.cancel_token.child_token());
    wait_calls(&fx, 1).await;
    let follower = spawn_chunk(&fx, chunk("fails"), fx.state.cancel_token.child_token());
    let (l, f) = (leader.await.unwrap(), follower.await.unwrap());
    for v in [&l, &f] {
        assert_eq!(v["error"]["code"], -32001, "{v}");
        assert_eq!(v["error"]["data"]["reason"], "scripted failure", "{v}");
    }
    assert_eq!(fx.calls.load(Ordering::SeqCst), 1, "no retry storm");
    assert_eq!(fx.state.chunk_inflight_count("g", "c1"), 0);
}

/// A leader that panics never leaves a follower waiting: the follower re-elects and succeeds.
#[tokio::test]
async fn follower_does_not_hang_when_leader_panics() {
    let fx = fixture(Script {
        delays: vec![Duration::from_millis(300), Duration::ZERO],
        panic_first: true,
        ..Default::default()
    });
    // The panic fires immediately on call 0; the follower must already be waiting, so hold the
    // leader at the unchanged-check read path with a held write lock until it has joined.
    let guard = fx.state.write_lock.write().await;
    let leader = spawn_chunk(&fx, chunk("panics"), fx.state.cancel_token.child_token());
    wait_inflight(&fx, 1).await;
    let follower = spawn_chunk(&fx, chunk("panics"), fx.state.cancel_token.child_token());
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(guard);
    assert!(
        leader.await.is_err(),
        "the scripted panic ends the leader task"
    );
    let f = tokio::time::timeout(Duration::from_secs(10), follower)
        .await
        .expect("follower must not hang")
        .unwrap();
    assert_eq!(f["result"]["success"], true, "{f}");
    assert_eq!(episode_count(&fx.state), 1);
}

/// A leader that ends as an `unchanged` no-op passes that result on, marked joined.
#[tokio::test]
async fn follower_of_unchanged_leader_gets_unchanged_joined() {
    let fx = fixture(Script::default());
    let first = send(&fx, "knowledge_process_chunk", chunk("stable")).await;
    assert_eq!(first["result"]["success"], true);
    // Stall the leader at the no-op's shared read lock so the follower joins while it is in flight.
    let guard = fx.state.write_lock.write().await;
    let leader = spawn_chunk(&fx, chunk("stable"), fx.state.cancel_token.child_token());
    wait_inflight(&fx, 1).await;
    let follower = spawn_chunk(&fx, chunk("stable"), fx.state.cancel_token.child_token());
    tokio::time::sleep(Duration::from_millis(150)).await;
    drop(guard);
    let (l, f) = (leader.await.unwrap(), follower.await.unwrap());
    assert_eq!(l["result"]["status"], "unchanged", "{l}");
    assert_eq!(f["result"]["status"], "unchanged", "{f}");
    assert!(joined(&f) && !joined(&l));
    assert_eq!(
        fx.calls.load(Ordering::SeqCst),
        1,
        "only the first ingest extracted"
    );
    assert_eq!(episode_count(&fx.state), 1);
}

/// Shutdown cancels a waiting follower too.
#[tokio::test]
async fn shutdown_cancels_waiting_followers() {
    let fx = fixture(slow_first(Duration::from_secs(60)));
    let leader = spawn_chunk(&fx, chunk("shutdown"), fx.state.cancel_token.child_token());
    wait_calls(&fx, 1).await;
    let follower = spawn_chunk(&fx, chunk("shutdown"), fx.state.cancel_token.child_token());
    tokio::time::sleep(Duration::from_millis(100)).await;
    fx.state.cancel_token.cancel();
    let (l, f) = (leader.await.unwrap(), follower.await.unwrap());
    assert!(is_cancelled_error(&l), "{l}");
    assert!(is_cancelled_error(&f), "{f}");
    assert_eq!(fx.state.chunk_inflight_count("g", "c1"), 0);
}
