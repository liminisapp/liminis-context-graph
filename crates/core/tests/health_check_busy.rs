// Issue #628: `health_check` reports `busy` instead of blocking behind the write lock.
//
// The busy path must answer without awaiting the write lock and without touching the DB.
// Hang detection uses a generous bound (1 s); the documented behaviour is "about 100 ms".

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::MockExtractor,
    handlers,
    ipc::IpcRequest,
    rebuild_job::{JobStatus, RebuildJob},
    telemetry::{NoopSink, TelemetrySink},
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const BOUND: Duration = Duration::from_secs(1);

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("health.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(4).unwrap();
    (db, dir)
}

fn make_state(db: Option<Arc<Db>>) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let degraded = db.is_none();
    Arc::new(AppState {
        db: ArcSwapOption::from(db),
        degraded_reason: Arc::new(Mutex::new(degraded.then(|| "corrupt wal".to_string()))),
        embedder: Arc::new(MockEmbedder::new(4)),
        extractor: Arc::new(MockExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: None,
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
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

/// Calls `health_check` under a timeout (a blocked handler fails the test) and returns `result`.
async fn health(state: &Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: "health_check".to_string(),
        params: json!({}),
    };
    let resp = tokio::time::timeout(BOUND, handlers::dispatch(req, state.clone(), None))
        .await
        .expect("health_check must not block behind the write lock");
    serde_json::to_value(resp).unwrap()["result"].clone()
}

fn insert_job(state: &AppState, id: &str, status: JobStatus, replayed: u64, age_secs: i64) {
    let mut job = RebuildJob::new(id.to_string());
    job.status = status;
    job.mutations_replayed = replayed;
    job.wal_files_processed = 2;
    job.wal_files_total = 5;
    job.start_time = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
    state
        .rebuild_jobs
        .lock()
        .unwrap()
        .insert(id.to_string(), job);
}

#[tokio::test]
async fn idle_reports_healthy_unchanged() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    assert_eq!(
        health(&state).await,
        json!({"ok": true, "healthy": true, "state": "healthy"})
    );
}

#[tokio::test]
async fn write_held_with_running_job_reports_rebuilding_with_progress() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    insert_job(&state, "job-1", JobStatus::Running, 100, 3);
    let _w = state.write_lock.write().await;

    let r = health(&state).await;
    assert_eq!(r["ok"], true);
    assert_eq!(r["healthy"], false);
    assert_eq!(r["state"], "busy");
    assert_eq!(r["activity"], "rebuilding");
    assert_eq!(r["job_id"], "job-1");
    assert_eq!(r["progress"]["mutations_replayed"], 100);
    assert_eq!(r["progress"]["wal_files_processed"], 2);
    assert_eq!(r["progress"]["wal_files_total"], 5);
    assert!(r["progress"]["elapsed_seconds"].as_f64().unwrap() >= 3.0);
}

#[tokio::test]
async fn progress_advances_between_calls() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    insert_job(&state, "job-1", JobStatus::Running, 100, 1);
    let _w = state.write_lock.write().await;

    let first = health(&state).await;
    state
        .rebuild_jobs
        .lock()
        .unwrap()
        .get_mut("job-1")
        .unwrap()
        .mutations_replayed = 250;
    let second = health(&state).await;

    let a = first["progress"]["mutations_replayed"].as_u64().unwrap();
    let b = second["progress"]["mutations_replayed"].as_u64().unwrap();
    assert!(b > a, "progress must advance: {a} -> {b}");
}

#[tokio::test]
async fn write_held_without_job_reports_writing() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    let _w = state.write_lock.write().await;

    let r = health(&state).await;
    assert_eq!(
        r,
        json!({"ok": true, "healthy": false, "state": "busy", "activity": "writing"})
    );
}

#[tokio::test]
async fn finished_jobs_do_not_count_as_rebuilding() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    insert_job(&state, "done", JobStatus::Completed, 10, 5);
    insert_job(&state, "failed", JobStatus::Failed, 10, 5);
    let _w = state.write_lock.write().await;

    let r = health(&state).await;
    assert_eq!(r["activity"], "writing");
    assert!(r.get("job_id").is_none());
    assert!(r.get("progress").is_none());
}

#[tokio::test]
async fn earliest_running_job_is_chosen() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    insert_job(&state, "newer", JobStatus::Running, 1, 2);
    insert_job(&state, "older", JobStatus::Running, 2, 60);
    let _w = state.write_lock.write().await;

    assert_eq!(health(&state).await["job_id"], "older");
}

#[tokio::test]
async fn contended_rebuild_jobs_mutex_falls_back_to_writing() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    insert_job(&state, "job-1", JobStatus::Running, 1, 1);
    let _w = state.write_lock.write().await;
    let _jobs = state.rebuild_jobs.lock().unwrap();

    let r = health(&state).await;
    assert_eq!(r["state"], "busy");
    assert_eq!(r["activity"], "writing");
    assert!(r.get("progress").is_none());
}

#[tokio::test]
async fn poisoned_rebuild_jobs_mutex_falls_back_to_writing() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    insert_job(&state, "job-1", JobStatus::Running, 1, 1);
    let jobs = state.rebuild_jobs.clone();
    let _ = std::thread::spawn(move || {
        let _g = jobs.lock().unwrap();
        panic!("poison rebuild_jobs");
    })
    .join();
    assert!(state.rebuild_jobs.is_poisoned());
    let _w = state.write_lock.write().await;

    let r = health(&state).await;
    assert_eq!(r["state"], "busy");
    assert_eq!(r["activity"], "writing");
}

/// FR-011: the busy path never connects to or queries the DB. The DB's backing files are removed
/// while the write lock is held (a mid-clear stand-in); `busy` must still be returned.
#[tokio::test]
async fn busy_path_does_not_touch_the_db() {
    let (db, dir) = make_db();
    let state = make_state(Some(db));
    let _w = state.write_lock.write().await;
    std::fs::remove_dir_all(dir.path()).unwrap();

    let r = health(&state).await;
    assert_eq!(r["state"], "busy");
    assert_eq!(r["activity"], "writing");
}

#[tokio::test]
async fn pending_writer_behind_reader_reports_busy() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    let reader = state.write_lock.read().await;
    let lock = state.write_lock.clone();
    let writer = tokio::spawn(async move {
        let _w = lock.write().await;
    });
    // Let the writer queue up behind the reader.
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(health(&state).await["state"], "busy");

    drop(reader);
    writer.await.unwrap();
}

#[tokio::test]
async fn degraded_is_never_masked_by_busy() {
    let state = make_state(None);
    let _w = state.write_lock.write().await;

    let r = health(&state).await;
    assert_eq!(r["ok"], false);
    assert_eq!(r["healthy"], false);
    assert_eq!(r["state"], "degraded");
    assert_eq!(r["reason"], "corrupt wal");
}

#[tokio::test]
async fn returns_to_healthy_after_writer_releases() {
    let (db, _dir) = make_db();
    let state = make_state(Some(db));
    {
        let _w = state.write_lock.write().await;
        assert_eq!(health(&state).await["state"], "busy");
    }
    assert_eq!(
        health(&state).await,
        json!({"ok": true, "healthy": true, "state": "healthy"})
    );
}
