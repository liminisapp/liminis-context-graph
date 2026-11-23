//! Issue #884 / ADR-0884: `knowledge_status {include_episode_breakdown: true}` reports, per group,
//! the live / superseded / pending episode counts and the entities and edges supported only by
//! superseded episodes (the ADR-0828 reopening-trigger measurements).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::UnconfiguredExtractor,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    ConfigurableExtractor, Db, EpisodicRow, ExtractedEdge, ExtractedEntity, ExtractionResult,
    Extractor, WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "liminis";

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        // Indexes exist before any supersession `SET`: lbug cannot `SET` an indexed column, so
        // this proves the new (unindexed) columns are settable on an FTS/HNSW-indexed node.
        conn.create_vector_indexes().unwrap();
    }
    (db, dir)
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

/// One extraction: `facts` are `(source, target, fact)` triples; entities are derived from them.
fn extraction(facts: &[(&str, &str, &str)]) -> ExtractionResult {
    let mut names: Vec<&str> = facts.iter().flat_map(|(s, t, _)| [*s, *t]).collect();
    names.sort();
    names.dedup();
    ExtractionResult {
        entities: names
            .into_iter()
            .map(|n| ExtractedEntity {
                name: n.to_string(),
                entity_type: "Thing".to_string(),
                summary: format!("{n} summary"),
                original_entity_type: None,
            })
            .collect(),
        edges: facts
            .iter()
            .map(|(s, t, f)| ExtractedEdge {
                source_name: s.to_string(),
                target_name: t.to_string(),
                fact: f.to_string(),
                relation_type: Some("RELATED_TO".to_string()),
                valid_at: None,
                invalid_at: None,
                original_relation_type: None,
            })
            .collect(),
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

async fn chunk(state: &Arc<AppState>, group: &str, chunk_id: &str, extra: Value) -> Value {
    let mut params = json!({
        "chunk_text": format!("text of {chunk_id}"), "chunk_id": chunk_id,
        "source_file": "doc.md", "group_id": group,
        "reference_time": "2026-08-04T00:00:00Z",
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    let r = call("knowledge_process_chunk", params, state).await;
    assert_eq!(r["success"], true, "{r}");
    r
}

fn state_with(queue: Vec<ExtractionResult>) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let (db, dir) = make_db();
    let state = make_state(
        Arc::clone(&db),
        None,
        Arc::new(ConfigurableExtractor::new(queue)),
    );
    (state, db, dir)
}

fn degraded_state() -> Arc<AppState> {
    let (db, _dir) = make_db();
    let state = make_state(db, None, Arc::new(UnconfiguredExtractor));
    state.db.store(None);
    *state.degraded_reason.lock().unwrap() = Some("corrupt wal".to_string());
    state
}

const PENDING_UUID: &str = "pending-ep-1";

fn insert_raw_episode(db: &Db, uuid: &str, group: &str, pending: bool) {
    let conn = db.connect().unwrap();
    conn.insert_episodic(&EpisodicRow {
        uuid: uuid.into(),
        name: uuid.into(),
        group_id: group.into(),
        created_at: "2026-01-01 00:00:00".into(),
        source: "text".into(),
        source_description: "t".into(),
        content: "raw".into(),
        content_embedding: vec![0.0; DIM],
        valid_at: "2026-01-01 00:00:00".into(),
        attributes: "{}".into(),
        revision_id: pending.then(|| "rev-1".to_string()),
        // `None` + `revision_id` = an uncommitted revision (ADR-0785).
        revision_committed_at: None,
        ..Default::default()
    })
    .unwrap();
}

async fn status(state: &Arc<AppState>) -> Value {
    call(
        "knowledge_status",
        json!({"include_episode_breakdown": true}),
        state,
    )
    .await
}

fn group<'a>(breakdown: &'a Value, gid: &str) -> &'a Value {
    breakdown["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["group_id"] == gid)
        .unwrap_or_else(|| panic!("group {gid} missing from {breakdown}"))
}

fn counts(g: &Value) -> (u64, u64, u64, u64, u64) {
    (
        g["live"].as_u64().unwrap(),
        g["superseded"].as_u64().unwrap(),
        g["pending"].as_u64().unwrap(),
        g["only_superseded_entities"].as_u64().unwrap(),
        g["only_superseded_edges"].as_u64().unwrap(),
    )
}

/// Builds the ADR-0828 scenario in group `liminis`: an A→B→A oscillation on chunk `x` (Globex is
/// only ever mentioned by the superseded middle revision; Acme by a live one), a removed chunk
/// `y` with no successor, and one uncommitted-revision episode. Group `other` has live episodes
/// only.
async fn scenario() -> (Arc<AppState>, Arc<Db>, TempDir) {
    let (state, db, dir) = state_with(vec![
        extraction(&[("Alice", "Acme", "Alice works at Acme")]),
        extraction(&[("Alice", "Globex", "Alice works at Globex")]),
        extraction(&[("Alice", "Acme", "Alice works at Acme")]),
        extraction(&[("Zed", "Quux", "Zed knows Quux")]),
        extraction(&[("Bob", "Initech", "Bob works at Initech")]),
    ]);
    // Each revision carries distinct text: an identical resubmission is a no-op (#858).
    chunk(&state, G, "x", json!({"chunk_text": "revision 1 of x"})).await;
    chunk(&state, G, "x", json!({"chunk_text": "revision 2 of x"})).await;
    chunk(&state, G, "x", json!({"chunk_text": "revision 3 of x"})).await;
    chunk(&state, G, "y", json!({})).await;
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "y", "group_ids": [G]}),
        &state,
    )
    .await;
    chunk(&state, "other", "z", json!({})).await;
    insert_raw_episode(&db, PENDING_UUID, G, true);
    // Unattributed episode (no chunk lineage): never superseded, so live.
    insert_raw_episode(&db, "unattributed-1", "other", false);
    (state, db, dir)
}

/// SC-001: per-group live / superseded / pending equal the shared predicate applied by hand.
/// SC-002: the three counts partition each group's episodes.
#[tokio::test]
async fn episode_counts_match_the_shared_liveness_predicate() {
    let (state, db, _d) = scenario().await;
    let s = status(&state).await;
    let b = &s["episode_breakdown"];
    assert!(b["as_of_us"].as_i64().unwrap() > 0, "{b}");

    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 1000, None, None).unwrap();
    let uuids: Vec<String> = eps.iter().map(|(e, _)| e.uuid.clone()).collect();
    let sup = conn.get_episode_supersession(&uuids).unwrap();
    let now = b["as_of_us"].as_i64().unwrap();
    let mut expected: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    for ep in sup.values() {
        let e = expected.entry(ep.group_id.clone()).or_default();
        if ep.is_pending() {
            e.2 += 1;
        } else if ep.live_at(now) {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    for (gid, (live, superseded, pending)) in &expected {
        let g = group(b, gid);
        assert_eq!(
            (
                g["live"].as_u64(),
                g["superseded"].as_u64(),
                g["pending"].as_u64()
            ),
            (Some(*live), Some(*superseded), Some(*pending)),
            "group {gid}: {b}"
        );
        let total = eps.iter().filter(|(e, _)| &e.group_id == gid).count() as u64;
        assert_eq!(live + superseded + pending, total, "partition of {gid}");
    }
    assert_eq!(b["groups"].as_array().unwrap().len(), expected.len());

    // Concrete expectations for the scenario.
    let g = group(b, G);
    assert_eq!(
        (
            g["live"].clone(),
            g["superseded"].clone(),
            g["pending"].clone()
        ),
        (json!(1), json!(3), json!(1)),
        "{g}"
    );
    // Groups are separate and a clean group reports explicit zeros (present, not omitted).
    let o = group(b, "other");
    assert_eq!(counts(o), (2, 0, 0, 0, 0), "{o}");
}

/// SC-003: Globex (mentioned only by the superseded middle revision) and the removed chunk's
/// Zed / Quux are only-superseded; Acme (also mentioned by a live revision), Alice and the
/// pending episode's absent entities are not. Edges are counted by producer.
#[tokio::test]
async fn only_superseded_entities_and_edges_follow_the_oscillation_scenario() {
    let (state, db, _d) = scenario().await;
    let s = status(&state).await;
    let g = group(&s["episode_breakdown"], G);
    assert_eq!(g["only_superseded_entities"], 3, "Globex, Zed, Quux: {g}");

    // Reference edge count: edges whose belief is Superseded, by hand.
    let conn = db.connect().unwrap();
    let edges = conn.get_edges_by_group_ids(Some(&[G])).unwrap();
    let uuids: Vec<String> = edges.iter().map(|e| e.uuid.clone()).collect();
    let rows = conn.get_edge_belief_rows(&uuids).unwrap();
    let all_eps: Vec<String> = rows.values().flat_map(|r| r.episodes.clone()).collect();
    let sup = conn.get_episode_supersession(&all_eps).unwrap();
    let now = s["episode_breakdown"]["as_of_us"].as_i64().unwrap();
    let expected = rows
        .values()
        .filter(|r| {
            lcg_core::belief::evaluate(r, &sup, now) == lcg_core::belief::BeliefState::Superseded
        })
        .count() as u64;
    assert!(expected >= 2, "Globex and Zed-Quux edges at least");
    assert_eq!(g["only_superseded_edges"], expected, "{g}");
    let facts: Vec<&str> = edges.iter().map(|e| e.fact.as_str()).collect();
    assert!(facts.contains(&"Alice works at Globex"));
}

/// An entity name that also lives in a live episode is never only-superseded, even when the
/// superseded episode is the older one.
#[tokio::test]
async fn shared_entity_with_a_live_mention_is_not_counted() {
    let (state, _db, _d) = state_with(vec![
        extraction(&[("Alice", "Acme", "Alice works at Acme")]),
        extraction(&[("Alice", "Acme", "Alice leads Acme")]),
    ]);
    chunk(&state, G, "x", json!({"chunk_text": "revision 1 of x"})).await;
    chunk(&state, G, "x", json!({"chunk_text": "revision 2 of x"})).await;
    let s = status(&state).await;
    let g = group(&s["episode_breakdown"], G);
    assert_eq!(counts(g).0, 1);
    assert_eq!(counts(g).1, 1);
    assert_eq!(g["only_superseded_entities"], 0, "{g}");
}

/// Opt-out: no new key and no new query; opt-in preserves every other field's presence.
#[tokio::test]
async fn breakdown_is_opt_in_and_additive() {
    let (state, _db, _d) = scenario().await;
    let plain = call("knowledge_status", json!({}), &state).await;
    assert!(plain.get("episode_breakdown").is_none(), "{plain}");
    assert!(plain.get("episode_breakdown_unavailable").is_none());
    // Non-boolean / false values behave as absent.
    for v in [json!(false), json!("true"), json!(1), json!(null)] {
        let r = call(
            "knowledge_status",
            json!({"include_episode_breakdown": v}),
            &state,
        )
        .await;
        assert!(r.get("episode_breakdown").is_none(), "{r}");
    }
    let with = status(&state).await;
    for (k, v) in plain.as_object().unwrap() {
        assert!(with.get(k).is_some(), "opt-in dropped {k}");
        if k == "last_index_time" || k == "index_created_at" {
            continue;
        }
        assert_eq!(&with[k], v, "opt-in changed {k}");
    }
    assert_eq!(with["episode_count"], plain["episode_count"]);
}

#[tokio::test]
async fn empty_graph_reports_an_empty_group_list() {
    let (state, _db, _d) = state_with(vec![]);
    let s = status(&state).await;
    assert_eq!(s["episode_breakdown"]["groups"], json!([]), "{s}");
}

/// FR-011: degraded mode keeps its status shape; the breakdown is null with a reason.
#[tokio::test]
async fn degraded_mode_marks_the_breakdown_unavailable() {
    let state = degraded_state();
    let s = status(&state).await;
    assert_eq!(s["degraded"], true, "{s}");
    assert!(s["episode_breakdown"].is_null(), "{s}");
    assert!(s["episode_breakdown_unavailable"].is_string(), "{s}");
}

/// FR-013: the scan writes nothing.
#[tokio::test]
async fn breakdown_is_read_only() {
    let (state, db, _d) = scenario().await;
    let before = {
        let conn = db.connect().unwrap();
        (
            conn.retrieve_episodes_page(None, 1000, None, None)
                .unwrap()
                .len(),
            conn.get_edges_by_group_ids(None).unwrap().len(),
        )
    };
    let a = status(&state).await["episode_breakdown"]["groups"].clone();
    let b = status(&state).await["episode_breakdown"]["groups"].clone();
    assert_eq!(a, b, "idempotent");
    let conn = db.connect().unwrap();
    assert_eq!(
        before,
        (
            conn.retrieve_episodes_page(None, 1000, None, None)
                .unwrap()
                .len(),
            conn.get_edges_by_group_ids(None).unwrap().len(),
        )
    );
}
