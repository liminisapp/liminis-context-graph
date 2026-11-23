//! Issue #859 / ADR-0859: the hard deletes (`knowledge_delete_by_source`,
//! `knowledge_delete_chunk_episode`) report the entities they leave with no mentioning episode, and
//! with `prune_orphans: true` delete them together with the edges left with no episodes. Pruning
//! is reproduced by WAL replay and is scoped to the groups named in the call.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    ConfigurableExtractor, Db, ExtractedEdge, ExtractedEntity, ExtractionResult, Extractor,
    WalReplayer, WalWriter,
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
    // Per-group writers are created lazily under `wal_root` (issue #378), one stream per group.
    let writers: HashMap<String, WalWriter> = HashMap::new();
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

/// One single-fact extraction per `(doc-ish label, fact)`: distinct entities per fact so edges
/// never collide.
fn one(label: &str, fact: &str) -> ExtractionResult {
    extraction(&[(&format!("{label}-src"), &format!("{label}-dst"), fact)])
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

fn next_submission() -> usize {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

async fn ingest(state: &Arc<AppState>, group: &str, chunk_id: &str, doc: Option<&str>) {
    let mut params = json!({
        "chunk_text": format!("text of {chunk_id} (submission {})", next_submission()),
        "chunk_id": chunk_id, "source_file": "doc.md", "group_id": group,
        "reference_time": "2026-08-04T00:00:00Z",
    });
    if let Some(d) = doc {
        params["doc_id"] = json!(d);
    }
    let r = call("knowledge_process_chunk", params, state).await;
    assert_eq!(r["success"], true, "{r}");
}

/// `uuid → name` of every entity in `group`.
async fn entity_names(state: &Arc<AppState>, group: &str) -> BTreeMap<String, String> {
    let r = call(
        "knowledge_get_nodes_by_group",
        json!({"group_ids": [group]}),
        state,
    )
    .await;
    r["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["uuid"].as_str().unwrap().to_string(),
                n["name"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn uuid_of(names: &BTreeMap<String, String>, name: &str) -> String {
    names
        .iter()
        .find(|(_, n)| n.as_str() == name)
        .unwrap_or_else(|| panic!("no entity {name:?} in {names:?}"))
        .0
        .clone()
}

fn name_set(names: &BTreeMap<String, String>) -> Vec<&str> {
    let mut v: Vec<&str> = names.values().map(String::as_str).collect();
    v.sort();
    v
}

async fn edge_facts(state: &Arc<AppState>, group: &str) -> Vec<String> {
    let r = call(
        "knowledge_get_edges_by_group",
        json!({"group_ids": [group]}),
        state,
    )
    .await;
    let mut v: Vec<String> = r["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["fact"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

/// Replays `group`'s own WAL stream into a fresh database (once or `times` over).
async fn replayed(root: &Path, groups: &[&str], times: usize) -> (Arc<AppState>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("r.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        for _ in 0..times {
            for g in groups {
                let stream = lcg_core::wal_group::group_wal_dir(root, g).unwrap();
                WalReplayer::new(&stream)
                    .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
                    .unwrap();
            }
        }
    }
    let state = make_state(db, None, Arc::new(ConfigurableExtractor::new(vec![])));
    (state, dir)
}

fn orphan_uuids(r: &Value) -> Vec<String> {
    r["orphaned_entity_uuids"]
        .as_array()
        .unwrap_or_else(|| panic!("no orphaned_entity_uuids in {r}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// Two chunks sharing entity `x`; `y` appears only in c1 and `z` only in c2.
async fn two_chunks(wal: Option<&Path>) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let (state, db, dir) = state_with(
        vec![
            extraction(&[("x", "y", "x relates y")]),
            extraction(&[("x", "z", "x relates z")]),
        ],
        wal.map(Path::to_path_buf),
    );
    ingest(&state, G, "c1", Some("D")).await;
    ingest(&state, G, "c2", Some("D")).await;
    (state, db, dir)
}

/// US2 AC1/AC2/AC3: orphans are reported, kept, and only entities that lost a mention count.
#[tokio::test]
async fn delete_chunk_reports_orphans_and_keeps_them() {
    let (state, _db, _d) = two_chunks(None).await;
    let before = entity_names(&state, G).await;
    assert_eq!(name_set(&before), vec!["x", "y", "z"]);
    let y = uuid_of(&before, "y");

    let r = call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "c1", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(r["deleted_count"], 1, "{r}");
    assert_eq!(
        orphan_uuids(&r),
        vec![y.clone()],
        "x is still mentioned by c2: {r}"
    );
    assert_eq!(r["orphaned_entity_count"], 1);
    assert_eq!(r["pruned_entity_count"], 0);
    assert_eq!(r["pruned_edge_count"], 0);
    // Nothing beyond the episode was deleted (FR-011).
    assert_eq!(entity_names(&state, G).await, before);
    assert_eq!(
        edge_facts(&state, G).await,
        vec!["x relates y", "x relates z"]
    );

    // Deleting a chunk that strands nothing reports an empty list.
    let r = call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "nope", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(r["deleted_count"], 0);
    assert_eq!(r["orphaned_entity_uuids"], json!([]));
    assert_eq!(r["orphaned_entity_count"], 0);
}

#[tokio::test]
async fn delete_by_source_reports_all_stranded_entities() {
    let (state, _db, _d) = two_chunks(None).await;
    let names = entity_names(&state, G).await;
    let r = call(
        "knowledge_delete_by_source",
        json!({"source_file": "doc.md", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(r["deleted_count"], 2, "{r}");
    let mut got = orphan_uuids(&r);
    got.sort();
    let mut want: Vec<String> = names.keys().cloned().collect();
    want.sort();
    assert_eq!(got, want, "{r}");
    assert_eq!(r["orphaned_entity_count"], 3);
    assert_eq!(
        entity_names(&state, G).await,
        names,
        "kept without prune_orphans"
    );
}

/// Edge case: an entity mentioned by a superseded (not deleted) episode is not an orphan.
#[tokio::test]
async fn superseded_mention_keeps_entity_from_being_an_orphan() {
    let (state, _db, _d) = two_chunks(None).await;
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "c1", "group_ids": [G]}),
        &state,
    )
    .await;
    let names = entity_names(&state, G).await;
    let z = uuid_of(&names, "z");
    let r = call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "c2", "group_ids": [G], "prune_orphans": true}),
        &state,
    )
    .await;
    // x and y are still mentioned by the superseded c1 episode; only z is stranded.
    assert_eq!(orphan_uuids(&r), vec![z], "{r}");
    let after = entity_names(&state, G).await;
    assert_eq!(name_set(&after), vec!["x", "y"], "history stays readable");
    // The superseded episode's own edge is untouched.
    assert_eq!(edge_facts(&state, G).await, vec!["x relates y"]);
}

/// US3 AC1/AC2/AC4 and the WAL rebuild (AC3, FR-013).
#[tokio::test]
async fn prune_orphans_removes_entities_and_episodeless_edges_and_replays() {
    let wal = TempDir::new().unwrap();
    let (state, _db, _d) = two_chunks(Some(wal.path())).await;
    let names = entity_names(&state, G).await;
    let y = uuid_of(&names, "y");
    let r = call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "c1", "group_ids": [G], "prune_orphans": true}),
        &state,
    )
    .await;
    assert_eq!(orphan_uuids(&r), vec![y], "{r}");
    assert_eq!(r["pruned_entity_count"], 1, "{r}");
    assert_eq!(r["pruned_edge_count"], 1, "{r}");

    let after = entity_names(&state, G).await;
    assert_eq!(name_set(&after), vec!["x", "z"]);
    assert_eq!(edge_facts(&state, G).await, vec!["x relates z"]);
    let found = call(
        "knowledge_find_entities",
        json!({"query": "y", "num_results": 10, "group_ids": [G]}),
        &state,
    )
    .await;
    assert!(!found.to_string().contains("\"name\":\"y\""), "{found}");

    // FR-013: rebuilding from the WAL keeps them gone.
    let (state2, _d2) = replayed(wal.path(), &[G], 1).await;
    assert_eq!(entity_names(&state2, G).await, after);
    assert_eq!(edge_facts(&state2, G).await, vec!["x relates z"]);
}

#[tokio::test]
async fn prune_by_source_leaves_no_dangling_edge() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = two_chunks(Some(wal.path())).await;
    let r = call(
        "knowledge_delete_by_source",
        json!({"source_file": "doc.md", "group_ids": [G], "prune_orphans": true}),
        &state,
    )
    .await;
    assert_eq!(r["pruned_entity_count"], 3, "{r}");
    assert_eq!(r["pruned_edge_count"], 2, "{r}");
    assert!(entity_names(&state, G).await.is_empty());
    assert!(edge_facts(&state, G).await.is_empty());
    let conn = db.connect().unwrap();
    for q in [
        "MATCH (rn:RelatesToNode_) RETURN count(*)",
        "MATCH (:Entity)-[r:RELATES_TO]->(:Entity) RETURN count(*)",
    ] {
        let rows: Vec<_> = conn.query_cypher_raw(q).unwrap().collect();
        assert_eq!(format!("{:?}", rows[0][0]), "Int64(0)", "{q}");
    }
    let (state2, _d2) = replayed(wal.path(), &[G], 1).await;
    assert!(entity_names(&state2, G).await.is_empty());
    assert!(edge_facts(&state2, G).await.is_empty());
}

/// An edge still supported by a remaining episode is kept, and so are its endpoints; an edge
/// whose every cited episode was deleted goes.
#[tokio::test]
async fn edge_with_a_surviving_episode_is_kept() {
    let (state, db, _d) = state_with(
        vec![
            extraction(&[("x", "y", "shared fact")]),
            extraction(&[("x", "y", "other fact")]),
        ],
        None,
    );
    ingest(&state, G, "c1", Some("D")).await;
    ingest(&state, G, "c2", Some("D")).await;
    // Make "shared fact" cite both episodes, as a fact corroborated by two chunks would.
    {
        let conn = db.connect().unwrap();
        let eps: Vec<(String, String)> = conn
            .query_cypher_raw("MATCH (ep:Episodic) RETURN ep.name, ep.uuid")
            .unwrap()
            .map(|r| (format!("{}", r[0]), format!("{}", r[1])))
            .collect();
        let uuid = |n: &str| eps.iter().find(|(name, _)| name == n).unwrap().1.clone();
        conn.run_cypher(&format!(
            "MATCH (rn:RelatesToNode_) WHERE rn.fact = 'shared fact' \
             SET rn.episodes = ['{}', '{}']",
            uuid("c1"),
            uuid("c2")
        ))
        .unwrap();
    }
    let r = call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "c1", "group_ids": [G], "prune_orphans": true}),
        &state,
    )
    .await;
    assert_eq!(r["orphaned_entity_count"], 0, "{r}");
    assert_eq!(r["pruned_entity_count"], 0, "{r}");
    assert_eq!(r["pruned_edge_count"], 0, "c2 still backs the edge: {r}");
    assert_eq!(name_set(&entity_names(&state, G).await), vec!["x", "y"]);
    assert_eq!(
        edge_facts(&state, G).await,
        vec!["other fact", "shared fact"]
    );

    // Now the last citing episode goes too: the edge is left with no episodes and is pruned.
    let r = call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "c2", "group_ids": [G], "prune_orphans": true}),
        &state,
    )
    .await;
    assert_eq!(r["pruned_edge_count"], 2, "{r}");
    assert_eq!(r["pruned_entity_count"], 2, "{r}");
    assert!(edge_facts(&state, G).await.is_empty());
}

/// An asserted entity never lost a mention here, so it is never reported or pruned.
#[tokio::test]
async fn asserted_entities_are_untouched() {
    let (state, _db, _d) = two_chunks(None).await;
    call(
        "knowledge_assert_entity",
        json!({"name": "Asserted", "group_id": G, "summary": "by hand"}),
        &state,
    )
    .await;
    let r = call(
        "knowledge_delete_by_source",
        json!({"source_file": "doc.md", "group_ids": [G], "prune_orphans": true}),
        &state,
    )
    .await;
    assert_eq!(r["pruned_entity_count"], 3, "{r}");
    assert_eq!(name_set(&entity_names(&state, G).await), vec!["Asserted"]);
}

/// FR-014: `prune_orphans` stays inside the named groups, and each group's prune lands on its own
/// WAL stream.
#[tokio::test]
async fn prune_is_scoped_to_the_named_groups_and_their_streams() {
    let wal = TempDir::new().unwrap();
    let (state, _db, _d) = state_with(
        vec![
            extraction(&[("x", "y", "x relates y")]),
            extraction(&[("x", "y", "x relates y")]),
        ],
        Some(wal.path().to_path_buf()),
    );
    ingest(&state, "g1", "c1", Some("D")).await;
    ingest(&state, "g2", "c1", Some("D")).await;
    let r = call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "c1", "group_ids": ["g1"], "prune_orphans": true}),
        &state,
    )
    .await;
    assert_eq!(r["pruned_entity_count"], 2, "{r}");
    assert!(entity_names(&state, "g1").await.is_empty());
    assert_eq!(name_set(&entity_names(&state, "g2").await), vec!["x", "y"]);
    assert_eq!(edge_facts(&state, "g2").await, vec!["x relates y"]);

    // g1's own stream reproduces the prune; g2's stream still has its rows.
    let (state2, _d2) = replayed(wal.path(), &["g1", "g2"], 1).await;
    assert!(entity_names(&state2, "g1").await.is_empty());
    assert!(edge_facts(&state2, "g1").await.is_empty());
    assert_eq!(name_set(&entity_names(&state2, "g2").await), vec!["x", "y"]);
    assert_eq!(edge_facts(&state2, "g2").await, vec!["x relates y"]);
}

#[tokio::test]
async fn prune_orphans_must_be_a_boolean() {
    let (state, _db, _d) = two_chunks(None).await;
    let v = try_call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "c1", "group_ids": [G], "prune_orphans": "yes"}),
        &state,
    )
    .await;
    assert!(v.get("error").is_some(), "{v}");
    assert_eq!(
        name_set(&entity_names(&state, G).await),
        vec!["x", "y", "z"]
    );
}
