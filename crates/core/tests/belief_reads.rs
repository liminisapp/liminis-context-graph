//! Issue #706 / ADR-0706: the two correction reads. `knowledge_get_correction_changes` shows what
//! a superseded episode supported versus what its replacement supports; `knowledge_get_affected_work`
//! lists the episodes written inside a dropped fact's belief window that mention its endpoints.
//! Both derive from #705's records and write nothing.

use std::collections::HashMap;
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
    ConfigurableExtractor, Db, ExtractedEdge, ExtractedEntity, ExtractionResult, Extractor,
    RelatesToEdge, WalReplayer, WalWriter,
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

fn state_with(
    queue: Vec<ExtractionResult>,
    wal_dir: Option<PathBuf>,
) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let (db, dir) = make_db();
    let state = make_state(
        Arc::clone(&db),
        wal_dir,
        Arc::new(ConfigurableExtractor::new(queue)),
    );
    (state, db, dir)
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

fn rfc3339(us: i64) -> String {
    chrono::DateTime::from_timestamp_micros(us)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// `(ingested_at_us of the edge with this fact, superseded_at_us of its only producing episode)`.
fn t1_t2(db: &Db, fact_text: &str) -> (i64, i64) {
    let conn = db.connect().unwrap();
    let uuid = edge_uuid(db, fact_text);
    let rows = conn
        .get_edge_belief_rows(std::slice::from_ref(&uuid))
        .unwrap();
    let row = &rows[&uuid];
    let eps = conn.get_episode_supersession(&row.episodes).unwrap();
    (
        row.ingested_at_us.unwrap(),
        eps[&row.episodes[0]].superseded_at_us.unwrap(),
    )
}

fn edge_uuid(db: &Db, fact: &str) -> String {
    let conn = db.connect().unwrap();
    conn.get_edges_by_group_ids(None)
        .unwrap()
        .into_iter()
        .find(|e| e.fact == fact)
        .unwrap_or_else(|| panic!("no edge with fact {fact:?}"))
        .uuid
}

fn ep(r: &Value) -> String {
    r["episode_uuid"].as_str().unwrap().to_string()
}

fn uuids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|v| v["uuid"].as_str().unwrap().to_string())
        .collect()
}

fn fact_texts(list: &Value) -> Vec<String> {
    let mut v: Vec<String> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["fact"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

async fn changes(state: &Arc<AppState>, episode_uuid: &str) -> Value {
    call(
        "knowledge_get_correction_changes",
        json!({"episode_uuid": episode_uuid}),
        state,
    )
    .await
}

async fn affected(state: &Arc<AppState>, edge_uuid: &str) -> Value {
    call(
        "knowledge_get_affected_work",
        json!({"edge_uuid": edge_uuid}),
        state,
    )
    .await
}

/// SC-001, SC-002, SC-003, US1 AC1/AC3, US2 AC1/AC2 and the half-open window: one episode inside
/// the belief window, one before it, one after it, one inside but unrelated.
#[tokio::test]
async fn revision_diff_and_affected_work_window() {
    let (state, db, _d) = state_with(
        vec![
            extraction(&[("Alice", "Acme", "Alice shipped at Acme")]), // pre
            extraction(&[
                ("Alice", "Acme", "Alice runs Acme"), // kept by the correction
                ("Alice", "Acme", "Alice joined Acme in 2019"), // F, dropped
            ]), // E1
            extraction(&[("Alice", "Zed", "Alice advised Zed")]),      // inside
            extraction(&[("Quinn", "Rho", "Quinn met Rho")]),          // inside, unrelated
            extraction(&[
                ("Alice", "Acme", "Alice runs Acme"),
                ("Alice", "Acme", "Alice joined Acme in 2021"), // F', added
            ]), // E2
            extraction(&[("Acme", "Zed", "Acme bought Zed")]),         // post
        ],
        None,
    );
    let pre = chunk(&state, G, "pre", json!({})).await;
    let e1 = chunk(&state, G, "x", json!({})).await;
    let inside = chunk(&state, G, "mid", json!({})).await;
    let unrelated = chunk(&state, G, "unrel", json!({})).await;
    let e2 = chunk(&state, G, "x", json!({})).await;
    let post = chunk(&state, G, "post", json!({})).await;

    // US1 AC1 / AC3: the shared fact is on neither side.
    let c = changes(&state, &ep(&e1)).await;
    assert_eq!(c["status"], "superseded");
    assert_eq!(fact_texts(&c["removed"]), vec!["Alice joined Acme in 2019"]);
    assert_eq!(fact_texts(&c["added"]), vec!["Alice joined Acme in 2021"]);
    // The dropped fact has no other live producer, so it is genuinely no longer believed.
    assert_eq!(c["removed"][0]["still_believed"], false);
    assert_eq!(
        (c["removed_count"].clone(), c["added_count"].clone()),
        (json!(1), json!(1))
    );
    assert_eq!(c["replacement"]["superseded_by"], ep(&e2));
    assert_eq!(c["episode"]["reason"], "revision");
    for f in c["removed"]
        .as_array()
        .unwrap()
        .iter()
        .chain(c["added"].as_array().unwrap())
    {
        assert!(
            f["uuid"].is_string() && f["source_node_uuid"].is_string(),
            "FR-008: {f}"
        );
    }

    // US2 AC1: only the episode inside [T1, T2) that mentions an endpoint.
    let f_uuid = edge_uuid(&db, "Alice joined Acme in 2019");
    let a = affected(&state, &f_uuid).await;
    assert_eq!(a["interval"]["state"], "closed", "{a}");
    assert_eq!(a["basis"], "entity_overlap");
    assert_eq!(uuids(&a["candidates"]), vec![ep(&inside)], "{a}");
    assert_eq!(a["count"], 1);
    assert!(a.get("episode_uuids").is_none());
    let out: Vec<String> = [&pre, &e1, &unrelated, &e2, &post]
        .iter()
        .map(|r| ep(r))
        .collect();
    for u in &out {
        assert!(
            !uuids(&a["candidates"]).contains(u),
            "{u} must not be a candidate"
        );
    }

    // The window endpoints agree with the stored records, to the microsecond.
    let (t1, t2) = t1_t2(&db, "Alice joined Acme in 2019");
    assert_eq!(a["interval"]["start"], rfc3339(t1));
    assert_eq!(a["interval"]["end"], rfc3339(t2));
}

/// US1 AC2, FR-005: `chunk_removed` has no replacement; the read says so and is not ambiguous.
/// US1 AC4: a live episode has nothing to compare.
#[tokio::test]
async fn chunk_removed_has_no_replacement_and_live_episode_has_nothing_to_compare() {
    let (state, _db, _d) = state_with(
        vec![
            extraction(&[("A", "B", "gone fact")]),
            extraction(&[("C", "D", "live fact")]),
        ],
        None,
    );
    let gone = chunk(&state, G, "x", json!({})).await;
    let live = chunk(&state, G, "y", json!({})).await;
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "x", "group_ids": [G]}),
        &state,
    )
    .await;

    let c = changes(&state, &ep(&gone)).await;
    assert_eq!(c["status"], "superseded");
    assert!(c["replacement"].is_null());
    assert_eq!(fact_texts(&c["removed"]), vec!["gone fact"]);
    assert_eq!(c["added"], json!([]));
    assert_eq!(c["episode"]["reason"], "chunk_removed");
    assert!(c["note"]
        .as_str()
        .unwrap()
        .contains("No replacement exists"));

    let l = changes(&state, &ep(&live)).await;
    assert_eq!(l["status"], "not_superseded");
    assert_eq!(
        (l["removed"].clone(), l["added"].clone()),
        (json!([]), json!([]))
    );
}

/// Split units: the replacement side is the whole revision, not only the final unit.
#[tokio::test]
async fn multi_unit_revision_replacement_includes_earlier_units() {
    let (state, _db, _d) = state_with(
        vec![
            extraction(&[("A", "B", "old")]),
            extraction(&[("A", "B", "unit 1")]),
            extraction(&[("A", "B", "unit 2")]),
        ],
        None,
    );
    let old = chunk(&state, G, "x", json!({})).await;
    let u1 = chunk(&state, G, "x", json!({"supersede": false})).await;
    let u2 = chunk(
        &state,
        G,
        "x",
        json!({"same_revision_episode_uuids": [u1["episode_uuid"].clone()]}),
    )
    .await;
    let c = changes(&state, &ep(&old)).await;
    assert_eq!(fact_texts(&c["removed"]), vec!["old"]);
    assert_eq!(fact_texts(&c["added"]), vec!["unit 1", "unit 2"]);
    assert_eq!(
        c["replacement"]["unit_episode_uuids"],
        json!([ep(&u1), ep(&u2)])
    );
    assert_eq!(c["replacement"]["superseded_by"], ep(&u2));
}

/// Chains: asking about E1 compares E1 with E2 (its direct superseder), not with E3.
#[tokio::test]
async fn chain_compares_with_the_direct_superseder() {
    let (state, db, _d) = state_with(
        vec![
            extraction(&[("A", "B", "v1")]),
            extraction(&[("A", "B", "v2")]),
            extraction(&[("A", "B", "v3")]),
        ],
        None,
    );
    let e1 = chunk(&state, G, "x", json!({})).await;
    let e2 = chunk(&state, G, "x", json!({})).await;
    chunk(&state, G, "x", json!({})).await;
    let c1 = changes(&state, &ep(&e1)).await;
    assert_eq!(fact_texts(&c1["removed"]), vec!["v1"]);
    assert_eq!(fact_texts(&c1["added"]), vec!["v2"]);
    assert_eq!(c1["replacement"]["superseded_by"], ep(&e2));
    let c2 = changes(&state, &ep(&e2)).await;
    assert_eq!(fact_texts(&c2["removed"]), vec!["v2"]);
    assert_eq!(fact_texts(&c2["added"]), vec!["v3"]);
    // v2 was believed from its own ingest until E3 superseded E2.
    let a = affected(&state, &edge_uuid(&db, "v2")).await;
    assert_eq!(a["interval"]["state"], "closed");
}

/// FR-006, SC-003: every interval case that is not closed says which case applies, returns no
/// candidates, and still carries the entity-overlap basis.
#[tokio::test]
async fn non_closed_intervals_are_named_not_empty() {
    let (state, db, _d) = state_with(vec![extraction(&[("A", "B", "live fact")])], None);
    chunk(&state, G, "x", json!({})).await;
    let live = affected(&state, &edge_uuid(&db, "live fact")).await;
    assert_eq!(live["interval"]["state"], "open");
    assert_eq!(live["candidates"], json!([]));
    assert_eq!(live["basis"], "entity_overlap");

    let conn = db.connect().unwrap();
    let ents = conn.get_entities_by_group_ids(None).unwrap();
    let mk = |uuid: &str, fact: &str, episodes: Vec<String>, warrant: Option<&str>| RelatesToEdge {
        uuid: uuid.into(),
        name: "X".into(),
        source_node_uuid: ents[0].uuid.clone(),
        target_node_uuid: ents[1].uuid.clone(),
        group_id: G.into(),
        fact: fact.into(),
        fact_embedding: vec![1.0, 0.0, 0.0, 0.0],
        created_at: "2026-01-01 00:00:00".into(),
        attributes: "{}".into(),
        episodes,
        warrant: warrant.map(str::to_string),
        ..Default::default()
    };
    conn.insert_relates_to_edge(&mk("legacy", "legacy fact", vec![], None))
        .unwrap();
    conn.insert_relates_to_edge(&mk("warranted", "asserted fact", vec![], Some("assertion")))
        .unwrap();
    conn.insert_relates_to_edge(&mk(
        "orphan",
        "orphan fact",
        vec!["no-such-episode".into()],
        None,
    ))
    .unwrap();

    for (uuid, state_name, cause) in [
        ("legacy", "no_record", "legacy"),
        ("orphan", "no_record", "support_deleted"),
        ("warranted", "not_defined", "warranted"),
    ] {
        let a = affected(&state, uuid).await;
        assert_eq!(a["interval"]["state"], state_name, "{uuid}: {a}");
        assert_eq!(a["interval"]["cause"], cause, "{uuid}: {a}");
        assert_eq!(a["candidates"], json!([]), "{uuid}");
        assert_eq!(a["basis"], "entity_overlap", "{uuid}");
    }
}

/// A removed fact that a second, live episode (outside the replacement) also produced is still
/// believed: the changed-facts read flags it, and the affected-work read agrees the interval is open.
#[tokio::test]
async fn removed_fact_with_another_live_producer_is_still_believed() {
    let (state, db, _d) = state_with(
        vec![
            extraction(&[("A", "B", "old fact")]),   // E1
            extraction(&[("C", "D", "other fact")]), // live, unrelated chunk
            extraction(&[("A", "B", "new fact")]),   // E2
        ],
        None,
    );
    let e1 = chunk(&state, G, "x", json!({})).await;
    let other = chunk(&state, G, "other", json!({})).await;

    // Edges are not deduplicated, so a shared edge is planted directly: produced by both episodes.
    let conn = db.connect().unwrap();
    let ents = conn.get_entities_by_group_ids(None).unwrap();
    conn.insert_relates_to_edge(&RelatesToEdge {
        uuid: "shared".into(),
        name: "X".into(),
        source_node_uuid: ents[0].uuid.clone(),
        target_node_uuid: ents[1].uuid.clone(),
        group_id: G.into(),
        fact: "shared fact".into(),
        fact_embedding: vec![1.0, 0.0, 0.0, 0.0],
        created_at: "2026-01-01 00:00:00".into(),
        attributes: "{}".into(),
        episodes: vec![ep(&e1), ep(&other)],
        ..Default::default()
    })
    .unwrap();
    chunk(&state, G, "x", json!({})).await; // E2 supersedes E1

    let c = changes(&state, &ep(&e1)).await;
    let flag = |fact: &str| {
        c["removed"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["fact"] == fact)
            .unwrap_or_else(|| panic!("{fact:?} not in removed: {c}"))["still_believed"]
            .clone()
    };
    assert_eq!(flag("shared fact"), true);
    assert_eq!(flag("old fact"), false);

    let a = affected(&state, "shared").await;
    assert_eq!(a["interval"]["state"], "open", "{a}");
    assert_eq!(a["candidates"], json!([]));
}

/// Groups: candidates never come from another group, even when a same-named entity exists there.
#[tokio::test]
async fn affected_work_stays_within_the_edge_group() {
    let (state, db, _d) = state_with(
        vec![
            extraction(&[("Alice", "Acme", "f1")]),
            extraction(&[("Alice", "Acme", "other group mentions Alice")]),
            extraction(&[("Alice", "Acme", "f2")]),
        ],
        None,
    );
    chunk(&state, G, "x", json!({})).await;
    let other = chunk(&state, "elsewhere", "o", json!({})).await;
    chunk(&state, G, "x", json!({})).await;
    let a = affected(&state, &edge_uuid(&db, "f1")).await;
    assert_eq!(a["interval"]["state"], "closed");
    assert_eq!(a["group_id"], G);
    assert!(!uuids(&a["candidates"]).contains(&ep(&other)), "{a}");
}

#[tokio::test]
async fn inputs_are_validated() {
    let (state, _db, _d) = state_with(vec![], None);
    for (method, params) in [
        ("knowledge_get_correction_changes", json!({})),
        (
            "knowledge_get_correction_changes",
            json!({"episode_uuid": 5}),
        ),
        (
            "knowledge_get_correction_changes",
            json!({"episode_uuid": "nope"}),
        ),
        ("knowledge_get_affected_work", json!({})),
        ("knowledge_get_affected_work", json!({"edge_uuid": ""})),
        ("knowledge_get_affected_work", json!({"edge_uuid": "nope"})),
    ] {
        let v = try_call(method, params.clone(), &state).await;
        assert!(v.get("error").is_some(), "{method} {params}: {v}");
    }
}

type Snap = (
    Vec<(String, Option<i64>, String, String)>,
    Vec<(String, Vec<String>, Option<String>, Option<String>)>,
);

/// Everything the reads could conceivably mutate, at microsecond precision.
fn snapshot(db: &Db) -> Snap {
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 1000, None, None).unwrap();
    let uuids: Vec<String> = eps.iter().map(|(e, _)| e.uuid.clone()).collect();
    let rows = conn.get_episode_rows(&uuids).unwrap();
    let mut episodes: Vec<_> = rows
        .values()
        .map(|r| {
            (
                r.uuid.clone(),
                r.superseded_at_us,
                r.superseded_by.clone().unwrap_or_default(),
                r.supersession_reason.clone().unwrap_or_default(),
            )
        })
        .collect();
    episodes.sort();
    let mut edges: Vec<_> = conn
        .get_edges_by_group_ids(None)
        .unwrap()
        .into_iter()
        .map(|e| (e.uuid, e.episodes, e.valid_at, e.invalid_at))
        .collect();
    edges.sort();
    (episodes, edges)
}

fn wal_bytes(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((p.to_string_lossy().to_string(), std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// SC-004 (nothing changes) and SC-005 (same answers after WAL replay).
#[tokio::test]
async fn reads_write_nothing_and_survive_wal_replay() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(
        vec![
            extraction(&[("Alice", "Acme", "F")]),
            extraction(&[("Alice", "Zed", "mid")]),
            extraction(&[("Alice", "Boston", "F2")]),
        ],
        Some(wal.path().to_path_buf()),
    );
    let e1 = chunk(&state, G, "x", json!({})).await;
    let mid = chunk(&state, G, "mid", json!({})).await;
    chunk(&state, G, "x", json!({})).await;
    let f = edge_uuid(&db, "F");

    let before = (snapshot(&db), wal_bytes(wal.path()));
    let c = changes(&state, &ep(&e1)).await;
    let a = affected(&state, &f).await;
    assert_eq!(uuids(&a["candidates"]), vec![ep(&mid)]);
    assert_eq!((snapshot(&db), wal_bytes(wal.path())), before, "SC-004");

    let dir2 = TempDir::new().unwrap();
    let db2 = Arc::new(Db::open(dir2.path().join("r.db").to_str().unwrap()).unwrap());
    {
        let conn = db2.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        WalReplayer::new(wal.path())
            .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
            .unwrap();
    }
    let state2 = make_state(db2, None, Arc::new(UnconfiguredExtractor));
    assert_eq!(changes(&state2, &ep(&e1)).await, c, "SC-005 changes");
    assert_eq!(affected(&state2, &f).await, a, "SC-005 affected work");
}
