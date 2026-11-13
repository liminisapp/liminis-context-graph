// Issue #741 (reproduction from #739): rebuilding the groups of a cross-group edge from WAL must
// yield the same graph in either order, and must never report a hop-less edge's pointer as
// `bound`.
//
// Group naming is concrete on purpose, as in #739: group `layer` OWNS the edge (its WAL holds the
// `RelatesToNode_` CREATE and the hop writes), and the edge's target endpoint is the entity
// `ADR-1` in group `up` (in `cross_group.rs`'s terms, the pointer's `source_group_id` is `up`).
//
// Wrong order = rebuild `layer` first: the hop writes find no `ADR-1` and no-op, while the
// replayed `attributes` still claim `bound`. Real IPC dispatch throughout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::MockExtractor,
    handlers,
    ipc::IpcRequest,
    pointer::{self, BindingState, EndpointSide},
    telemetry::{NoopSink, TelemetrySink},
    EntityRow,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const LAYER: &str = "layer";
const UP: &str = "up";

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("order_test.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(4).unwrap();
    (db, dir)
}

fn make_state(db: Arc<Db>, wal_root: PathBuf) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(4)),
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
        embedding_cache: std::sync::Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn entity_line(seq: u64, uuid: &str, name: &str, group: &str) -> String {
    format!(
        r#"{{"seq":{seq},"ts":"2026-05-22T00:00:00.000000+00:00","db":"","cypher":"MERGE (n:Entity {{uuid: '{uuid}'}}) ON CREATE SET n.name = '{name}', n.group_id = '{group}', n.labels = ['t'], n.created_at = timestamp('2026-05-22 00:00:00'), n.name_embedding = [1.0, 0.0, 0.0, 0.0], n.summary = 's', n.attributes = '{{}}'","params":{{}}}}"#
    )
}

fn write_group_wal(wal_root: &Path, group: &str, file: &str, lines: &[String]) {
    let dir = wal_root.join(group);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(file), lines.join("\n") + "\n").unwrap();
    // A hand-written stream needs a generation marker, as a published one would carry.
    std::fs::write(
        dir.join(".wal-generation.json"),
        format!(r#"{{"generation": "{group}-gen-1"}}"#),
    )
    .unwrap();
}

async fn dispatch_val(id: i64, method: &str, params: Value, state: Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(id),
        method: method.to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, state, None).await).unwrap()
}

/// Streaming (foreground) rebuild: a progress sender takes the synchronous code path.
async fn rebuild_sync(params: Value, state: Arc<AppState>) -> Value {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: "knowledge_rebuild_from_wal".to_string(),
        params,
    };
    let v = serde_json::to_value(handlers::dispatch(req, state, Some(tx)).await).unwrap();
    assert!(v.get("error").is_none(), "unexpected error: {v}");
    assert_eq!(v["result"]["success"], true, "rebuild must succeed: {v}");
    v
}

/// Background-job rebuild: submit, then poll `knowledge_rebuild_status` to completion.
async fn rebuild_job(params: Value, state: Arc<AppState>) -> Value {
    let submit = dispatch_val(1, "knowledge_rebuild_from_wal", params, Arc::clone(&state)).await;
    let job_id = submit["result"]["job_id"]
        .as_str()
        .unwrap_or_else(|| panic!("expected job_id: {submit}"))
        .to_string();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let s = dispatch_val(
            2,
            "knowledge_rebuild_status",
            json!({"job_id": job_id.as_str()}),
            Arc::clone(&state),
        )
        .await;
        match s["result"]["status"].as_str().unwrap_or("?") {
            "completed" => return s["result"]["result"].clone(),
            "failed" => panic!("rebuild job failed: {s}"),
            _ => assert!(
                std::time::Instant::now() < deadline,
                "job did not complete: {s}"
            ),
        }
    }
}

struct Fixture {
    wal_root: PathBuf,
    note: String,
    adr: String,
    edge: String,
    // Keep tempdirs alive for the test's lifetime.
    _wal_dir: TempDir,
    _origin_db_dir: TempDir,
}

/// Records #739's WAL: `note` in `layer`, `ADR-1` in `up` (seq 1, after a decoy at seq 0 so an
/// incremental `from_seq: 1` rebuild is meaningful), and an `ANNOTATES` cross-group edge owned
/// by `layer` created through the live API on an origin DB, so `layer`'s WAL holds the
/// `RelatesToNode_` CREATE with `bound` pointer attributes plus the hop writes.
async fn record_fixture() -> Fixture {
    let wal_dir = TempDir::new().unwrap();
    let wal_root = wal_dir.path().to_path_buf();
    let note = Uuid::new_v4().to_string();
    let adr = Uuid::new_v4().to_string();
    write_group_wal(
        &wal_root,
        UP,
        "20260522_000000_aaaaaa_0000.jsonl",
        &[
            entity_line(0, "decoy-up", "decoy", UP),
            entity_line(1, &adr, "ADR-1", UP),
        ],
    );
    write_group_wal(
        &wal_root,
        LAYER,
        "20260522_000000_bbbbbb_0000.jsonl",
        &[entity_line(0, &note, "note", LAYER)],
    );

    let (db, origin_db_dir) = make_db();
    let state = make_state(db, wal_root.clone());
    rebuild_sync(json!({"group_id": UP, "from_seq": 0}), Arc::clone(&state)).await;
    rebuild_sync(
        json!({"group_id": LAYER, "from_seq": 0}),
        Arc::clone(&state),
    )
    .await;
    let add = dispatch_val(
        3,
        "knowledge_add_cross_group_edge",
        json!({
            "name": "ANNOTATES",
            "group_id": LAYER,
            "source": {"uuid": note},
            "target": {"source_group_id": UP, "endpoint_name": "ADR-1"},
            "fact": "note annotates ADR-1",
        }),
        Arc::clone(&state),
    )
    .await;
    assert!(add.get("error").is_none(), "{add}");
    assert_eq!(
        add["result"]["cross_group_pointers"]["dst"]["binding_state"], "bound",
        "origin edge must start bound: {add}"
    );
    let edge = add["result"]["uuid"].as_str().unwrap().to_string();
    Fixture {
        wal_root,
        note,
        adr,
        edge,
        _wal_dir: wal_dir,
        _origin_db_dir: origin_db_dir,
    }
}

/// A consumer: a fresh empty DB over the recorded WAL root.
fn consumer(fx: &Fixture) -> (Arc<AppState>, TempDir) {
    let (db, dir) = make_db();
    (make_state(db, fx.wal_root.clone()), dir)
}

async fn pointer_counts(state: &Arc<AppState>) -> (u64, u64, u64) {
    let v = dispatch_val(9, "knowledge_status", json!({}), Arc::clone(state)).await;
    let c = &v["result"]["cross_group_pointers"];
    (
        c["bound"].as_u64().unwrap_or_else(|| panic!("{v}")),
        c["unbound"].as_u64().unwrap(),
        c["ambiguous"].as_u64().unwrap(),
    )
}

/// `knowledge_get_entity_neighbors` on the note, scoped to `layer`: returns (has ADR-1 node,
/// has ANNOTATES edge).
async fn neighbors(fx: &Fixture, state: &Arc<AppState>) -> (bool, bool) {
    let v = dispatch_val(
        10,
        "knowledge_get_entity_neighbors",
        json!({"entity_uuid": fx.note, "group_ids": [LAYER]}),
        Arc::clone(state),
    )
    .await;
    assert!(v.get("error").is_none(), "{v}");
    let has_node = v["result"]["nodes"]
        .as_array()
        .map(|a| a.iter().any(|n| n["name"] == "ADR-1"))
        .unwrap_or(false);
    let has_edge = v["result"]["edges"]
        .as_array()
        .map(|a| a.iter().any(|e| e["name"] == "ANNOTATES"))
        .unwrap_or(false);
    (has_node, has_edge)
}

fn edge_dst_and_state(fx: &Fixture, state: &Arc<AppState>) -> (String, BindingState) {
    let db = state.db.load_full().unwrap();
    let conn = db.connect().unwrap();
    let e = conn
        .get_relates_to_by_uuids(std::slice::from_ref(&fx.edge))
        .unwrap()
        .into_iter()
        .next()
        .expect("edge node must exist after layer replay");
    let ptr = pointer::read_pointers(&e.attributes)
        .get(EndpointSide::Dst)
        .cloned()
        .unwrap();
    (e.target_node_uuid, ptr.binding_state)
}

fn assert_repair_counts(rebind: &Value, ctx: &str) {
    assert!(
        rebind.is_object(),
        "{ctx}: cross_group_rebind must be non-null: {rebind}"
    );
    assert_eq!(
        rebind["bound"], 1,
        "{ctx}: pointer must be repaired: {rebind}"
    );
}

/// AC1 + AC2: rebuild `layer` then `up` (streaming path).
#[tokio::test]
async fn wrong_order_streaming_rebuild_is_repaired() {
    let fx = record_fixture().await;
    let (state, _d) = consumer(&fx);

    let layer_v = rebuild_sync(
        json!({"group_id": LAYER, "from_seq": 0}),
        Arc::clone(&state),
    )
    .await;
    assert!(
        layer_v["result"]["cross_group_rebind"].is_object(),
        "cross_group_rebind is reported on every rebuild: {layer_v}"
    );
    // AC2: no false health between the two rebuilds.
    assert_eq!(pointer_counts(&state).await, (0, 1, 0));
    assert_eq!(
        edge_dst_and_state(&fx, &state),
        (String::new(), BindingState::Unbound)
    );
    assert_eq!(neighbors(&fx, &state).await, (false, false));

    let up_v = rebuild_sync(json!({"group_id": UP, "from_seq": 0}), Arc::clone(&state)).await;
    assert_repair_counts(&up_v["result"]["cross_group_rebind"], "up rebuild");

    assert_eq!(neighbors(&fx, &state).await, (true, true));
    assert_eq!(pointer_counts(&state).await, (1, 0, 0));
    assert_eq!(
        edge_dst_and_state(&fx, &state),
        (fx.adr.clone(), BindingState::Bound)
    );
}

/// Same as above through the background-job rebuild path.
#[tokio::test]
async fn wrong_order_background_job_rebuild_is_repaired() {
    let fx = record_fixture().await;
    let (state, _d) = consumer(&fx);

    let layer_r = rebuild_job(
        json!({"group_id": LAYER, "from_seq": 0}),
        Arc::clone(&state),
    )
    .await;
    assert!(layer_r["cross_group_rebind"].is_object(), "{layer_r}");
    assert_eq!(pointer_counts(&state).await, (0, 1, 0));

    let up_r = rebuild_job(json!({"group_id": UP, "from_seq": 0}), Arc::clone(&state)).await;
    assert_repair_counts(&up_r["cross_group_rebind"], "up job rebuild");
    assert_eq!(neighbors(&fx, &state).await, (true, true));
    assert_eq!(pointer_counts(&state).await, (1, 0, 0));
}

/// An incremental (`from_seq > 0`) rebuild of `up` repairs too.
#[tokio::test]
async fn wrong_order_incremental_rebuild_of_target_is_repaired() {
    let fx = record_fixture().await;
    let (state, _d) = consumer(&fx);

    rebuild_sync(
        json!({"group_id": LAYER, "from_seq": 0}),
        Arc::clone(&state),
    )
    .await;
    assert_eq!(pointer_counts(&state).await, (0, 1, 0));

    // ADR-1 is seq 1; seq 0 is a decoy — a from_seq=1 replay is a non-full rebuild.
    let up_v = rebuild_sync(json!({"group_id": UP, "from_seq": 1}), Arc::clone(&state)).await;
    assert_repair_counts(
        &up_v["result"]["cross_group_rebind"],
        "incremental up rebuild",
    );
    assert_eq!(neighbors(&fx, &state).await, (true, true));
}

/// AC3: right order yields the same final graph; `cross_group_rebind` is reported on both.
#[tokio::test]
async fn right_order_is_unchanged_and_reports_rebind() {
    let fx = record_fixture().await;
    let (state, _d) = consumer(&fx);

    let up_v = rebuild_sync(json!({"group_id": UP, "from_seq": 0}), Arc::clone(&state)).await;
    assert!(up_v["result"]["cross_group_rebind"].is_object(), "{up_v}");
    let layer_v = rebuild_sync(
        json!({"group_id": LAYER, "from_seq": 0}),
        Arc::clone(&state),
    )
    .await;
    let rebind = &layer_v["result"]["cross_group_rebind"];
    assert!(rebind.is_object(), "{layer_v}");
    // Nothing points into `layer`, and the hop exists: no pointer needed repair.
    assert_eq!(rebind["checked"], 0, "{layer_v}");

    assert_eq!(neighbors(&fx, &state).await, (true, true));
    assert_eq!(pointer_counts(&state).await, (1, 0, 0));
    assert_eq!(
        edge_dst_and_state(&fx, &state),
        (fx.adr.clone(), BindingState::Bound)
    );
}

/// Both orders converge on the same final edge/pointer state.
#[tokio::test]
async fn both_orders_converge_on_the_same_graph() {
    let fx = record_fixture().await;
    let (right, _d1) = consumer(&fx);
    let (wrong, _d2) = consumer(&fx);
    for (g, s) in [(UP, &right), (LAYER, &right), (LAYER, &wrong), (UP, &wrong)] {
        rebuild_sync(json!({"group_id": g, "from_seq": 0}), Arc::clone(s)).await;
    }
    assert_eq!(
        edge_dst_and_state(&fx, &right),
        edge_dst_and_state(&fx, &wrong)
    );
    assert_eq!(neighbors(&fx, &right).await, neighbors(&fx, &wrong).await);
    assert_eq!(pointer_counts(&right).await, pointer_counts(&wrong).await);
}

/// Target group never rebuilt: the layer rebuild succeeds, the pointer stays honestly
/// `unbound`, no error.
#[tokio::test]
async fn target_never_rebuilt_leaves_pointer_unbound() {
    let fx = record_fixture().await;
    let (state, _d) = consumer(&fx);
    let v = rebuild_sync(
        json!({"group_id": LAYER, "from_seq": 0}),
        Arc::clone(&state),
    )
    .await;
    assert!(v["result"]["cross_group_rebind"].is_object(), "{v}");
    assert_eq!(pointer_counts(&state).await, (0, 1, 0));
    assert_eq!(neighbors(&fx, &state).await, (false, false));
}

/// FR-009: `knowledge_rebind_pointers` on a pointer left `unbound` by the layer rebuild
/// re-resolves it rather than skipping it on the staleness gate.
#[tokio::test]
async fn rebind_pointers_resolves_demoted_pointer_without_staleness_skip() {
    let fx = record_fixture().await;
    let (state, _d) = consumer(&fx);
    rebuild_sync(
        json!({"group_id": LAYER, "from_seq": 0}),
        Arc::clone(&state),
    )
    .await;
    assert_eq!(pointer_counts(&state).await, (0, 1, 0));

    // Make the target present WITHOUT a rebuild (so no automatic repair has run), then drive
    // the operator path. Applied position of `up` is set high to prove the gate cannot skip.
    {
        let db = state.db.load_full().unwrap();
        let conn = db.connect().unwrap();
        conn.insert_entity(&EntityRow {
            uuid: fx.adr.clone(),
            name: "ADR-1".to_string(),
            group_id: UP.to_string(),
            labels: vec!["Entity".to_string()],
            created_at: "2026-05-22 00:00:00".to_string(),
            name_embedding: vec![1.0, 0.0, 0.0, 0.0],
            summary: "s".to_string(),
            attributes: "{}".to_string(),
            ..Default::default()
        })
        .unwrap();
        conn.set_wal_position(UP, 1, None, None).unwrap();
    }
    let v = dispatch_val(
        5,
        "knowledge_rebind_pointers",
        json!({"source_group_id": UP}),
        Arc::clone(&state),
    )
    .await;
    assert_eq!(v["result"]["staleness_skipped"], 0, "{v}");
    assert_eq!(v["result"]["bound"], 1, "{v}");
    assert_eq!(
        edge_dst_and_state(&fx, &state),
        (fx.adr.clone(), BindingState::Bound)
    );
}

/// A `force_clear` rebuild of `up` with `layer` already built (right order): the purge flips
/// `layer`'s pointer to `unbound`; the post-rebuild rebind now re-binds it.
#[tokio::test]
async fn force_clear_rebuild_of_target_rebinds_pointer() {
    let fx = record_fixture().await;
    let (state, _d) = consumer(&fx);
    rebuild_sync(json!({"group_id": UP, "from_seq": 0}), Arc::clone(&state)).await;
    rebuild_sync(
        json!({"group_id": LAYER, "from_seq": 0}),
        Arc::clone(&state),
    )
    .await;
    assert_eq!(pointer_counts(&state).await, (1, 0, 0));

    let v = rebuild_sync(
        json!({"group_id": UP, "from_seq": 0, "force_clear": true}),
        Arc::clone(&state),
    )
    .await;
    assert_repair_counts(&v["result"]["cross_group_rebind"], "force_clear up rebuild");
    assert_eq!(pointer_counts(&state).await, (1, 0, 0));
    assert_eq!(neighbors(&fx, &state).await, (true, true));
}
