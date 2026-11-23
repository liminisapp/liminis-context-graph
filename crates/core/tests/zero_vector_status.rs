//! `knowledge_status` surfaces the durable per-group zero-vector record (issue #808, ADR-0808):
//! exact counts past the log cap, per-kind breakdown, 0 when clean, persistence across restart,
//! and a full rebuild (`force_clear`) resetting the record to that rebuild's own failures.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::{EmbedRole, Embedder},
    extractor::MockExtractor,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    wal_group, Error,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const G: &str = "g-zero";
const DIM: usize = 4;

/// Embeds every text successfully, except those starting with `bad`, which fail with a
/// non-transient (HTTP 400-shaped) error.
struct PoisonEmbedder;

impl Embedder for PoisonEmbedder {
    fn embed<'a>(
        &'a self,
        text: &'a str,
        _role: EmbedRole,
    ) -> BoxFuture<'a, Result<Vec<f32>, Error>> {
        Box::pin(async move {
            if text.starts_with("bad") {
                Err(Error::Ipc(
                    "UDS embedder returned status 400 Bad Request".to_string(),
                ))
            } else {
                Ok(vec![text.len() as f32 + 1.0, 1.0, 1.0, 1.0])
            }
        })
    }

    fn dim(&self) -> usize {
        DIM
    }
}

fn open_db(path: &std::path::Path) -> Arc<Db> {
    let db = Arc::new(Db::open(path.to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(DIM).unwrap();
    db
}

fn make_state(db: Arc<Db>, wal_root: std::path::PathBuf) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(PoisonEmbedder),
        extractor: Arc::new(MockExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: Some(wal_root),
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
        search_stages: Default::default(),
        chunk_inflight: Default::default(),
    })
}

fn req(id: i64, method: &str, params: Value) -> IpcRequest {
    IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(id),
        method: method.to_string(),
        params,
    }
}

async fn dispatch_val(id: i64, method: &str, params: Value, state: Arc<AppState>) -> Value {
    serde_json::to_value(handlers::dispatch(req(id, method, params), state, None).await).unwrap()
}

/// Streaming dispatch: a progress sender selects the synchronous foreground replay path.
async fn rebuild_foreground(params: Value, state: Arc<AppState>) -> Value {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let v = serde_json::to_value(
        handlers::dispatch(
            req(1, "knowledge_rebuild_from_wal", params),
            state,
            Some(tx),
        )
        .await,
    )
    .unwrap();
    assert!(v.get("error").is_none(), "rebuild failed: {v}");
    v
}

/// Non-streaming dispatch: submits a background job and polls it to completion.
async fn rebuild_background(params: Value, state: Arc<AppState>) -> Value {
    let submit = dispatch_val(1, "knowledge_rebuild_from_wal", params, Arc::clone(&state)).await;
    let job_id = submit["result"]["job_id"]
        .as_str()
        .unwrap_or_else(|| panic!("expected job_id: {submit}"))
        .to_string();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let st = dispatch_val(
            2,
            "knowledge_rebuild_status",
            json!({"job_id": job_id}),
            Arc::clone(&state),
        )
        .await;
        match st["result"]["status"].as_str().unwrap_or("?") {
            "completed" => return st,
            "failed" => panic!("background rebuild failed: {st}"),
            _ => assert!(std::time::Instant::now() < deadline, "rebuild job hung"),
        }
    }
}

fn wal_line(seq: u64, uuid: &str, name: &str) -> String {
    json!({
        "seq": seq,
        "ts": "2026-05-22T00:00:00.000000+00:00",
        "db": "",
        "cypher": "MERGE (n:Entity {uuid: $uuid}) SET n.name = $name, n.group_id = $group_id, \
                   n.name_embedding = $name_embedding",
        "params": { "uuid": uuid, "name": name, "group_id": G },
    })
    .to_string()
}

fn write_wal(wal_root: &std::path::Path, lines: &[String]) {
    let dir = wal_group::group_wal_dir(wal_root, G).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    // A known generation, as every real publisher stamps one (see wal_generation_reset.rs).
    std::fs::write(
        dir.join(".wal-generation.json"),
        r#"{"generation":"gen-1"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("0000.jsonl"),
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
}

async fn zero_rows(state: &Arc<AppState>) -> Value {
    let v = dispatch_val(9, "knowledge_status", json!({}), Arc::clone(state)).await;
    assert!(v.get("error").is_none(), "{v}");
    v["result"]["wal_groups"][G]["zero_vector_rows"].clone()
}

fn assert_counts(v: &Value, total: u64, name: u64, fact: u64, summary: u64, content: u64) {
    assert_eq!(v["total"], total, "{v}");
    assert_eq!(v["by_kind"]["name"], name, "{v}");
    assert_eq!(v["by_kind"]["fact"], fact, "{v}");
    assert_eq!(v["by_kind"]["summary"], summary, "{v}");
    assert_eq!(v["by_kind"]["content"], content, "{v}");
}

fn corpus(bad: usize, good: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for i in 0..bad {
        lines.push(wal_line(i as u64, &format!("b{i}"), &format!("bad-{i}")));
    }
    for i in 0..good {
        lines.push(wal_line(
            (bad + i) as u64,
            &format!("g{i}"),
            &format!("fine-{i}"),
        ));
    }
    lines
}

#[tokio::test]
async fn status_reports_exact_counts_past_log_cap_and_survives_restart() {
    let db_dir = TempDir::new().unwrap();
    let wal_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("zero.db");
    write_wal(wal_dir.path(), &corpus(25, 5));

    {
        let state = make_state(open_db(&db_path), wal_dir.path().to_path_buf());
        // Field present and all zero before any failure.
        assert_counts(&zero_rows(&state).await, 0, 0, 0, 0, 0);

        rebuild_foreground(json!({"group_id": G, "from_seq": 0}), Arc::clone(&state)).await;
        // 25 > the 10-row log cap: the count is exact, not capped.
        assert_counts(&zero_rows(&state).await, 25, 25, 0, 0, 0);
    }

    // Restart: a fresh Db handle and AppState over the same files.
    let state = make_state(open_db(&db_path), wal_dir.path().to_path_buf());
    assert_counts(&zero_rows(&state).await, 25, 25, 0, 0, 0);
}

#[tokio::test]
async fn clean_replay_reports_zero_and_full_rebuild_resets_to_its_own_failures() {
    let db_dir = TempDir::new().unwrap();
    let wal_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("zero.db");
    write_wal(wal_dir.path(), &corpus(25, 5));
    let state = make_state(open_db(&db_path), wal_dir.path().to_path_buf());

    // Background path records too.
    rebuild_background(json!({"group_id": G, "from_seq": 0}), Arc::clone(&state)).await;
    assert_counts(&zero_rows(&state).await, 25, 25, 0, 0, 0);

    // The WAL now has only 3 bad rows: a full rebuild (`force_clear`) leaves only its own
    // failures, not the earlier 25.
    write_wal(wal_dir.path(), &corpus(3, 4));
    rebuild_foreground(
        json!({"group_id": G, "from_seq": 0, "force_clear": true}),
        Arc::clone(&state),
    )
    .await;
    assert_counts(&zero_rows(&state).await, 3, 3, 0, 0, 0);

    // And a fully clean full rebuild reads 0 again.
    write_wal(wal_dir.path(), &corpus(0, 6));
    rebuild_background(
        json!({"group_id": G, "from_seq": 0, "force_clear": true}),
        Arc::clone(&state),
    )
    .await;
    assert_counts(&zero_rows(&state).await, 0, 0, 0, 0, 0);
}

#[tokio::test]
async fn record_api_removal_decrements_the_reported_count() {
    let db_dir = TempDir::new().unwrap();
    let wal_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("zero.db");
    write_wal(wal_dir.path(), &corpus(4, 0));
    let db = open_db(&db_path);
    let state = make_state(Arc::clone(&db), wal_dir.path().to_path_buf());
    rebuild_foreground(json!({"group_id": G, "from_seq": 0}), Arc::clone(&state)).await;
    assert_counts(&zero_rows(&state).await, 4, 4, 0, 0, 0);

    let conn = db.connect().unwrap();
    let listed = conn.list_zero_vector_rows(G, None).unwrap();
    assert_eq!(listed.len(), 4);
    assert_eq!(conn.remove_zero_vector_rows(G, &listed[..1]).unwrap(), 1);
    assert_counts(&zero_rows(&state).await, 3, 3, 0, 0, 0);
}
