// Issue #649 (fixes the #646 regression): a full-text index built by lbug 0.20 (lcg <= 0.15.x)
// cannot be maintained by lbug 0.21 (lcg 0.16.x) for terms containing any non-ASCII character --
// deletes/updates fail with `FTS index '<idx>' is inconsistent`, and `QUERY_FTS_INDEX` silently
// returns zero rows -- while the storage version (47) is identical, so nothing detects it.
//
// The fixture (crates/core/tests/fixtures/fts_lbug020_db/t.db.tar.gz) was written by the
// published v0.15.0 binary; see its README.md for the exact build steps and contents. These tests
// run against the real bundled FTS extension on every CI platform.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::{Conn, Db},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::MockExtractor,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// The fixture's embedding dimension (see its README).
const DIM: usize = 768;
const MARKER_QUERY: &str = "MATCH (s:SchemaState {key: 'fts_built_by_lbug'}) RETURN s.status";

/// Extracts the fixture into a fresh temp dir: `t.db` plus `wal/liminis/*.jsonl`.
fn extract_fixture() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let archive =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fts_lbug020_db/t.db.tar.gz");
    assert!(
        archive.exists(),
        "fixture not found at {}",
        archive.display()
    );
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(dir.path())
        .status()
        .expect("failed to invoke tar");
    assert!(status.success(), "tar extraction of the fixture failed");
    let db_path = dir.path().join("t.db");
    assert!(db_path.exists(), "t.db missing after extraction");
    (dir, db_path)
}

fn rows(conn: &Conn<'_>, cypher: &str) -> Vec<Vec<String>> {
    conn.cypher_query(cypher)
        .unwrap_or_else(|e| panic!("{cypher}: {e}"))
}

fn fts(conn: &Conn<'_>, table: &str, index: &str, term: &str) -> Vec<Vec<String>> {
    rows(
        conn,
        &format!("CALL QUERY_FTS_INDEX('{table}', '{index}', '{term}') RETURN node.uuid"),
    )
}

fn marker(conn: &Conn<'_>) -> Option<String> {
    rows(conn, MARKER_QUERY)
        .into_iter()
        .next()
        .map(|r| r[0].clone())
}

fn state_for(db: Arc<Db>, root: &Path, db_path: &Path) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let wal_writer = WalWriter::new(root.join("wal").join("liminis"), 10_000, 0).ok();
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor: Arc::new(MockExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: db_path.to_str().unwrap().to_string(),
        wal_root: Some(root.join("wal")),
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(
            wal_writer
                .into_iter()
                .map(|w| ("liminis".to_string(), w))
                .collect(),
        )),
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

async fn dispatch(method: &str, params: Value, state: &Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, Arc::clone(state), None).await).unwrap()
}

fn arrow_edge_uuid(conn: &Conn<'_>) -> String {
    rows(
        conn,
        "MATCH (r:RelatesToNode_) WHERE r.fact CONTAINS '→' RETURN r.uuid",
    )
    .into_iter()
    .next()
    .expect("fixture has a → edge")[0]
        .clone()
}

/// The fixture really is stale under lbug 0.21: before any rebuild, a delete through the live
/// index fails with the `inconsistent` error (the exact engine text is classified), and an
/// ASCII-only row is unaffected. Guards against the fixture silently becoming a no-op.
#[test]
fn fixture_index_is_stale_under_current_lbug() {
    let (_dir, db_path) = extract_fixture();
    let db = Db::open(db_path.to_str().unwrap()).unwrap();
    // No `init_schema`: the open-time rebuild must not have run.
    let conn = db.connect().unwrap();
    let uuid = arrow_edge_uuid(&conn);

    // `query_cypher_raw` bypasses the backstop, exposing the raw engine error.
    let err = conn
        .query_cypher_raw(&format!(
            "MATCH (r:RelatesToNode_ {{uuid: '{uuid}'}}) DETACH DELETE r"
        ))
        .map(|it| it.count())
        .expect_err("deleting a → edge through a lbug-0.20-built FTS index must fail");
    assert!(
        lcg_core::error::is_fts_inconsistent_error(&err),
        "real engine error text must be classified: {err}"
    );
}

/// SC-001 / acceptance 1-4: opening the fixture rebuilds the FTS indexes once, after which
/// deleting/updating non-ASCII rows works and non-ASCII search returns the expected rows.
#[tokio::test]
async fn open_rebuilds_stale_fts_then_writes_and_search_work() {
    let (dir, db_path) = extract_fixture();
    let db = Arc::new(Db::open(db_path.to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        assert_eq!(marker(&conn), None, "a 0.15-built database has no marker");
        conn.init_schema(DIM).unwrap(); // the real open path: rebuilds + marks
        assert_eq!(marker(&conn).as_deref(), Some(lbug::VERSION));

        // Search: non-ASCII terms return rows again (zero rows against the stale index).
        assert_eq!(
            fts(&conn, "RelatesToNode_", "edge_name_and_fact", "→").len(),
            1
        );
        assert_eq!(
            fts(&conn, "Entity", "node_name_and_summary", "Zürich").len(),
            2
        );
        assert_eq!(
            fts(&conn, "Entity", "node_name_and_summary", "東京").len(),
            1
        );
        assert_eq!(fts(&conn, "Episodic", "episode_content", "—").len(), 1);

        // Writes: delete the → edge.
        let uuid = arrow_edge_uuid(&conn);
        conn.run_cypher(&format!(
            "MATCH (r:RelatesToNode_ {{uuid: '{uuid}'}}) DETACH DELETE r"
        ))
        .unwrap();
        assert!(fts(&conn, "RelatesToNode_", "edge_name_and_fact", "→").is_empty());
        assert_eq!(
            conn.fts_repair_count(),
            0,
            "the open-time rebuild means the backstop never fires"
        );
    }
    let state = state_for(Arc::clone(&db), dir.path(), &db_path);

    // Writes: delete the — episode through the real handler.
    let v = dispatch(
        "knowledge_delete_episode",
        json!({"episode_uuid": "ep-nonascii"}),
        &state,
    )
    .await;
    assert!(v.get("error").is_none(), "delete episode failed: {v}");

    // Writes: update a non-ASCII entity (re-assert with a new summary).
    let v = dispatch(
        "knowledge_assert_entity",
        json!({"name": "Zürich", "summary": "Largest city in Switzerland"}),
        &state,
    )
    .await;
    assert!(v.get("error").is_none(), "update entity failed: {v}");

    let conn = db.connect().unwrap();
    assert!(fts(&conn, "Episodic", "episode_content", "—").is_empty());
    assert_eq!(
        fts(&conn, "Entity", "node_name_and_summary", "Switzerland").len(),
        1
    );
}

/// SC-002: a second open of a rebuilt database keeps the marker and keeps working.
#[test]
fn second_open_keeps_marker() {
    let (_dir, db_path) = extract_fixture();
    {
        let db = Db::open(db_path.to_str().unwrap()).unwrap();
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        assert_eq!(marker(&conn).as_deref(), Some(lbug::VERSION));
    }
    let db = Db::open(db_path.to_str().unwrap()).unwrap();
    let conn = db.connect().unwrap();
    conn.init_schema(DIM).unwrap();
    assert_eq!(marker(&conn).as_deref(), Some(lbug::VERSION));
    assert_eq!(
        fts(&conn, "Entity", "node_name_and_summary", "東京").len(),
        1
    );
}

/// SC-004 / FR-006: with the open-time rebuild bypassed (bare `connect`, no `init_schema`, as a
/// pinned 0.16.0-0.16.2 database would be), a write that hits the stale index is repaired by the
/// statement-level backstop: all 3 indexes are rebuilt, the statement is retried once and
/// succeeds, and the repair is counted.
#[test]
fn backstop_rebuilds_all_three_indexes_and_retries() {
    let (_dir, db_path) = extract_fixture();
    let db = Db::open(db_path.to_str().unwrap()).unwrap();
    let conn = db.connect().unwrap();
    let uuid = arrow_edge_uuid(&conn);
    assert_eq!(conn.fts_repair_count(), 0);
    let _ = conn.drain_mutations(); // discard the recorded uuid lookup

    conn.run_cypher(&format!(
        "MATCH (r:RelatesToNode_ {{uuid: '{uuid}'}}) DETACH DELETE r"
    ))
    .expect("backstop must repair and retry");
    // The repair DDL and marker write never entered the WAL buffer: only the retried statement.
    let recorded = conn.drain_mutations();
    assert_eq!(recorded.len(), 1, "unexpected WAL entries: {recorded:?}");
    assert!(recorded[0].0.contains("DETACH DELETE"));

    assert_eq!(conn.fts_repair_count(), 1);
    assert!(conn.fts_last_repair_unix_ms().is_some());
    assert_eq!(marker(&conn).as_deref(), Some(lbug::VERSION));
    // All three indexes were rebuilt, not only the one named in the error: non-ASCII search
    // works on every table, and a delete through each other index now succeeds without a repair.
    assert_eq!(
        fts(&conn, "Entity", "node_name_and_summary", "Zürich").len(),
        2
    );
    assert_eq!(fts(&conn, "Episodic", "episode_content", "—").len(), 1);
    conn.run_cypher("MATCH (e:Episodic {uuid: 'ep-nonascii'}) DETACH DELETE e")
        .unwrap();
    conn.run_cypher("MATCH (e:Entity) WHERE e.name = '東京' DETACH DELETE e")
        .unwrap();
    assert_eq!(
        conn.fts_repair_count(),
        1,
        "no further repair should be needed"
    );
}

/// FR-006: the `knowledge_query_cypher` escape hatch (`Conn::cypher_query`) is covered by the same
/// backstop as `raw_query` / `exec_params`.
#[test]
fn backstop_covers_cypher_query_mutations() {
    let (_dir, db_path) = extract_fixture();
    let db = Db::open(db_path.to_str().unwrap()).unwrap();
    let conn = db.connect().unwrap();
    let uuid = arrow_edge_uuid(&conn);
    conn.cypher_query(&format!(
        "MATCH (r:RelatesToNode_ {{uuid: '{uuid}'}}) DETACH DELETE r"
    ))
    .expect("cypher_query must repair and retry");
    assert_eq!(conn.fts_repair_count(), 1);
    assert_eq!(marker(&conn).as_deref(), Some(lbug::VERSION));
}

/// FR-007 / acceptance 3: `knowledge_rebuild_from_wal {from_seq: 0, force_clear: true}` against
/// stale FTS indexes (open-time rebuild bypassed) completes with no failed mutations -- the
/// purge no longer deletes through a live index.
#[tokio::test]
async fn force_clear_rebuild_from_wal_completes_with_stale_fts() {
    let (dir, db_path) = extract_fixture();
    let db = Arc::new(Db::open(db_path.to_str().unwrap()).unwrap());
    let state = state_for(Arc::clone(&db), dir.path(), &db_path);

    let v = dispatch(
        "knowledge_rebuild_from_wal",
        json!({"from_seq": 0, "force_clear": true}),
        &state,
    )
    .await;
    assert_eq!(v["result"]["success"], true, "rebuild did not start: {v}");
    let job_id = v["result"]["job_id"].as_str().expect("job_id").to_string();

    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let s = dispatch(
            "knowledge_rebuild_status",
            json!({"job_id": job_id}),
            &state,
        )
        .await;
        match s["result"]["status"].as_str().unwrap_or("?") {
            "completed" => {
                assert_eq!(s["result"]["result"]["failed_lines"], 0, "{s}");
                break;
            }
            "failed" => panic!("rebuild failed: {s}"),
            _ => assert!(
                std::time::Instant::now() < deadline,
                "rebuild timed out: {s}"
            ),
        }
    }

    let db = state.db.load_full().expect("db present after rebuild");
    let conn = db.connect().unwrap();
    assert_eq!(rows(&conn, "MATCH (e:Entity) RETURN count(*)")[0][0], "4");
    assert_eq!(
        fts(&conn, "Entity", "node_name_and_summary", "東京").len(),
        1
    );
    assert_eq!(fts(&conn, "Episodic", "episode_content", "—").len(), 1);
}
