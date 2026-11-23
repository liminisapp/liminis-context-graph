//! Issue #292 / ADR-0292: a routine `knowledge_process_chunk` ingest never supersedes (or deletes)
//! an episode it cannot attribute to its own prior writes. Attribution is the `Episodic.write_path`
//! provenance marker: `chunk` (this handler) and NULL (legacy, pre-marker) are eligible; every
//! other value is left alone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    handlers,
    ipc::IpcRequest,
    schema,
    telemetry::{NoopSink, TelemetrySink},
    ConfigurableExtractor, Db, EpisodicRow, ExtractedEdge, ExtractedEntity, ExtractionResult,
    WalReplayer, WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "liminis";

fn make_state(
    wal_dir: Option<std::path::PathBuf>,
    n_extractions: usize,
) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
    }
    let writers: HashMap<String, WalWriter> = wal_dir
        .as_ref()
        .and_then(|d| WalWriter::new(d, 10_000, 5 * 1024 * 1024).ok())
        .map(|w| (G.to_string(), w))
        .into_iter()
        .collect();
    let queue: Vec<ExtractionResult> = (0..n_extractions)
        .map(|i| extraction(&format!("fact {i}")))
        .collect();
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(Arc::clone(&db))),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor: Arc::new(ConfigurableExtractor::new(queue)),
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
    });
    (state, db, dir)
}

fn extraction(fact: &str) -> ExtractionResult {
    ExtractionResult {
        entities: ["A", "B"]
            .iter()
            .map(|n| ExtractedEntity {
                name: n.to_string(),
                entity_type: "Thing".to_string(),
                summary: format!("{n} summary"),
                original_entity_type: None,
            })
            .collect(),
        edges: vec![ExtractedEdge {
            source_name: "A".to_string(),
            target_name: "B".to_string(),
            fact: fact.to_string(),
            relation_type: Some("RELATED_TO".to_string()),
            valid_at: None,
            invalid_at: None,
            original_relation_type: None,
        }],
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

async fn chunk_in(
    state: &Arc<AppState>,
    group: &str,
    chunk_id: &str,
    text: &str,
    extra: Value,
) -> Value {
    let mut params = json!({
        "chunk_text": text, "chunk_id": chunk_id, "source_file": "notes", "group_id": group,
        "reference_time": "2026-08-04T00:00:00Z",
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    let r = call("knowledge_process_chunk", params, state).await;
    assert_eq!(r["success"], true, "{r}");
    r
}

async fn chunk(state: &Arc<AppState>, chunk_id: &str, text: &str) -> Value {
    chunk_in(state, G, chunk_id, text, json!({})).await
}

/// A foreign writer mirroring this handler's own `"{source_file}:{chunk_id}"` convention.
async fn add_episode(
    state: &Arc<AppState>,
    group: &str,
    name: &str,
    source_description: &str,
) -> String {
    let r = call(
        "knowledge_add_episode",
        json!({"name": name, "episode_body": "a caller's own episode", "group_id": group,
               "source_description": source_description,
               "reference_time": "2026-08-04T00:00:00Z"}),
        state,
    )
    .await;
    r["episode_uuid"].as_str().unwrap().to_string()
}

/// `(superseded?, write_path)` for one episode.
fn state_of(db: &Db, uuid: &str) -> (bool, Option<String>) {
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 100, None, None).unwrap();
    let (row, _) = eps
        .iter()
        .find(|(e, _)| e.uuid == uuid)
        .expect("episode exists");
    let wp = conn
        .get_episode_write_paths(&[uuid.to_string()])
        .unwrap()
        .remove(uuid)
        .unwrap();
    (row.superseded_at.is_some(), wp)
}

fn legacy_row(db: &Db, uuid: &str, name: &str, group: &str) {
    // A pre-marker row: `write_path` left at its NULL default.
    let conn = db.connect().unwrap();
    conn.insert_episodic(&EpisodicRow {
        uuid: uuid.into(),
        name: name.into(),
        group_id: group.into(),
        created_at: "2026-01-01 00:00:00".into(),
        source: "text".into(),
        source_description: format!("notes:{name}"),
        content: "legacy".into(),
        content_embedding: vec![0.0; DIM],
        valid_at: "2026-01-01 00:00:00".into(),
        attributes: "{}".into(),
        ..Default::default()
    })
    .unwrap();
}

/// US1 AC1 / FR-006 / SC-001: the issue's collision case. The foreign row mirrors the handler's
/// convention byte for byte, survives, and the call is a normal ingest that reports it.
#[tokio::test]
async fn mirrored_foreign_episode_survives_chunk_ingest() {
    let (state, db, _d) = make_state(None, 4);
    let foreign = add_episode(&state, G, "chunk-42", "notes:chunk-42").await;
    let r = chunk(&state, "chunk-42", "different text").await;
    assert_eq!(r["success"], true);
    assert_eq!(r["superseded_episode_uuids"], json!([]));
    assert_eq!(r["left_alone_episode_uuids"], json!([foreign.clone()]));
    assert_eq!(state_of(&db, &foreign), (false, Some("episode".into())));
    // The new episode is stamped as chunk-written and is live.
    let new = r["episode_uuid"].as_str().unwrap();
    assert_eq!(state_of(&db, new), (false, Some("chunk".into())));
    // Repeated ingests never touch it (SC-001).
    for _ in 0..2 {
        chunk(&state, "chunk-42", "yet different text").await;
    }
    assert_eq!(state_of(&db, &foreign), (false, Some("episode".into())));
}

/// FR-006: protection does not depend on the source_description mirroring the convention.
/// FR-007: nor on the incoming text differing from the foreign row's content.
#[tokio::test]
async fn unmirrored_foreign_episode_and_identical_text_survive() {
    let (state, db, _d) = make_state(None, 4);
    let foreign = add_episode(&state, G, "chunk-7", "manual note").await;
    let r = chunk(&state, "chunk-7", "a caller's own episode").await; // identical body text
    assert_eq!(r["left_alone_episode_uuids"], json!([foreign.clone()]));
    assert!(!state_of(&db, &foreign).0);
}

/// US3: a mixed lineage supersedes only the attributable (own) episodes.
#[tokio::test]
async fn mixed_lineage_supersedes_only_own_episodes() {
    let (state, db, _d) = make_state(None, 4);
    let own = chunk(&state, "x", "v1").await;
    let foreign = add_episode(&state, G, "x", "notes:x").await;
    let r = chunk(&state, "x", "v2").await;
    assert_eq!(
        r["superseded_episode_uuids"],
        json!([own["episode_uuid"].clone()])
    );
    assert_eq!(r["left_alone_episode_uuids"], json!([foreign.clone()]));
    assert!(state_of(&db, own["episode_uuid"].as_str().unwrap()).0);
    assert!(!state_of(&db, &foreign).0);
}

/// US2 / FR-005 / FR-010: own-only resubmission is unchanged, and an unambiguous response carries
/// no `left_alone_episode_uuids` key.
#[tokio::test]
async fn own_only_resubmission_is_unchanged() {
    let (state, db, _d) = make_state(None, 4);
    let r1 = chunk(&state, "x", "v1").await;
    let r2 = chunk(&state, "x", "v2").await;
    assert_eq!(
        r2["superseded_episode_uuids"],
        json!([r1["episode_uuid"].clone()])
    );
    assert!(r1.get("left_alone_episode_uuids").is_none());
    assert!(r2.get("left_alone_episode_uuids").is_none());
    assert!(state_of(&db, r1["episode_uuid"].as_str().unwrap()).0);
    assert_eq!(
        state_of(&db, r2["episode_uuid"].as_str().unwrap()),
        (false, Some("chunk".into()))
    );
}

/// US2 AC2: a multi-unit revision still keeps its earlier units live; a foreign row is untouched.
#[tokio::test]
async fn multi_unit_revision_with_foreign_row() {
    let (state, db, _d) = make_state(None, 6);
    let old = chunk(&state, "x", "old").await;
    let foreign = add_episode(&state, G, "x", "notes:x").await;
    let u1 = chunk_in(&state, G, "x", "unit 1", json!({"supersede": false})).await;
    assert_eq!(u1["superseded_episode_uuids"], json!([]));
    assert!(
        u1.get("left_alone_episode_uuids").is_none(),
        "no ambiguity reported when not superseding"
    );
    let u2 = chunk_in(
        &state,
        G,
        "x",
        "unit 2",
        json!({"same_revision_episode_uuids": [u1["episode_uuid"].clone()]}),
    )
    .await;
    assert_eq!(
        u2["superseded_episode_uuids"],
        json!([old["episode_uuid"].clone()])
    );
    assert_eq!(u2["left_alone_episode_uuids"], json!([foreign.clone()]));
    assert!(!state_of(&db, u1["episode_uuid"].as_str().unwrap()).0);
    assert!(!state_of(&db, &foreign).0);
}

/// FR-009 / US4: a legacy NULL-marker row named like the chunk is still superseded. This pins the
/// documented residual exposure of pre-upgrade rows to the name collision.
#[tokio::test]
async fn legacy_null_marker_row_is_still_superseded() {
    let (state, db, _d) = make_state(None, 2);
    legacy_row(&db, "legacy-1", "x", G);
    assert_eq!(state_of(&db, "legacy-1"), (false, None));
    let r = chunk(&state, "x", "v1").await;
    assert_eq!(r["superseded_episode_uuids"], json!(["legacy-1"]));
    assert!(r.get("left_alone_episode_uuids").is_none());
    assert!(state_of(&db, "legacy-1").0);
}

/// Supersession is group-scoped: a same-named foreign row in another group is never touched (and
/// is not even reported).
#[tokio::test]
async fn foreign_row_in_another_group_is_untouched() {
    let (state, db, _d) = make_state(None, 2);
    let other = add_episode(&state, "other", "x", "notes:x").await;
    let r = chunk(&state, "x", "v1").await;
    assert!(r.get("left_alone_episode_uuids").is_none());
    assert_eq!(state_of(&db, &other), (false, Some("episode".into())));
}

/// A derived chunk ingest is not refused by a foreign live *primary* row (it would not revise it),
/// but is still refused by an own primary row (#752 behaviour is unchanged for attributable rows).
#[tokio::test]
async fn origin_guard_ignores_foreign_rows_but_still_applies_to_own() {
    let (state, db, _d) = make_state(None, 4);
    let foreign = add_episode(&state, G, "x", "notes:x").await;
    let r = chunk_in(&state, G, "x", "a page", json!({"origin": "derived"})).await;
    assert_eq!(r["left_alone_episode_uuids"], json!([foreign.clone()]));
    assert!(!state_of(&db, &foreign).0);

    // An own primary episode still blocks a derived revise.
    chunk(&state, "y", "primary text").await;
    let refused = try_call(
        "knowledge_process_chunk",
        json!({"chunk_text": "a page", "chunk_id": "y", "source_file": "notes", "group_id": G,
               "reference_time": "2026-08-04T00:00:00Z", "origin": "derived"}),
        &state,
    )
    .await;
    assert!(refused.get("error").is_some(), "{refused}");
}

/// FR-008: the explicit by-name operations keep their by-name contracts on a foreign row.
#[tokio::test]
async fn by_name_operations_still_act_on_foreign_rows() {
    let (state, db, _d) = make_state(None, 2);
    let f1 = add_episode(&state, G, "x", "notes:x").await;
    let r = call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "x", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(r["superseded_uuids"], json!([f1.clone()]));
    assert!(state_of(&db, &f1).0);

    let f2 = add_episode(&state, G, "z", "notes:z").await;
    let d = call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "z", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(d["deleted_count"], 1);
    let conn = db.connect().unwrap();
    assert!(conn.get_episode_write_paths(&[f2]).unwrap().is_empty());
}

fn replayed(source: &std::path::Path) -> (Db, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Db::open(dir.path().join("r.db").to_str().unwrap()).unwrap();
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        WalReplayer::new(source)
            .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
            .unwrap();
    }
    (db, dir)
}

/// FR-015 / SC-005: the marker survives WAL replay and a dump → replay round trip, including NULL
/// staying NULL.
#[tokio::test]
async fn write_path_survives_wal_replay_and_dump() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = make_state(Some(wal.path().to_path_buf()), 4);
    let own = chunk(&state, "x", "v1").await["episode_uuid"]
        .as_str()
        .unwrap()
        .to_string();
    let foreign = add_episode(&state, G, "x", "notes:x").await;
    legacy_row(&db, "legacy-1", "old", G);
    let all = vec![own.clone(), foreign.clone(), "legacy-1".to_string()];
    let expected = db.connect().unwrap().get_episode_write_paths(&all).unwrap();
    assert_eq!(expected[&own], Some("chunk".into()));
    assert_eq!(expected[&foreign], Some("episode".into()));
    assert_eq!(expected["legacy-1"], None);

    // WAL replay (the legacy row was inserted directly, so it is not in the WAL).
    let (db2, _d2) = replayed(wal.path());
    let got = db2
        .connect()
        .unwrap()
        .get_episode_write_paths(&all)
        .unwrap();
    assert_eq!(got[&own], expected[&own]);
    assert_eq!(got[&foreign], expected[&foreign]);

    // Dump → replay carries all three, NULL included.
    let out = TempDir::new().unwrap();
    let target = out.path().join("dump");
    let r = call(
        "knowledge_dump_wal",
        json!({"target_dir": target.to_str().unwrap()}),
        &state,
    )
    .await;
    assert_eq!(r["success"], true, "{r}");
    let (db3, _d3) = replayed(&target);
    assert_eq!(
        db3.connect()
            .unwrap()
            .get_episode_write_paths(&all)
            .unwrap(),
        expected
    );
}

/// A pre-feature WAL (no `write_path` property) replays to NULL, i.e. legacy.
#[tokio::test]
async fn pre_feature_wal_replays_as_legacy() {
    let wal = TempDir::new().unwrap();
    let (state, _db, _d) = make_state(Some(wal.path().to_path_buf()), 1);
    let own = chunk(&state, "x", "v1").await["episode_uuid"]
        .as_str()
        .unwrap()
        .to_string();
    let mut stack = vec![wal.path().to_path_buf()];
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            stack.extend(std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
        } else if let Ok(text) = std::fs::read_to_string(&p) {
            let stripped = text
                .replace(", write_path: $write_path", "")
                .replace(r#","write_path":"chunk""#, "");
            std::fs::write(&p, stripped).unwrap();
        }
    }
    let (db2, _d2) = replayed(wal.path());
    let got = db2
        .connect()
        .unwrap()
        .get_episode_write_paths(std::slice::from_ref(&own))
        .unwrap();
    assert_eq!(got[&own], None);
}

/// The migration adds `write_path` to a database created before this change, idempotently.
#[test]
fn migrate_adds_write_path_idempotently() {
    let dir = TempDir::new().unwrap();
    let db = Db::open(dir.path().join("old.db").to_str().unwrap()).unwrap();
    let conn = db.connect().unwrap();
    conn.run_cypher("CREATE NODE TABLE Entity (uuid STRING PRIMARY KEY, name STRING)")
        .unwrap();
    conn.run_cypher("CREATE NODE TABLE Episodic (uuid STRING PRIMARY KEY, name STRING)")
        .unwrap();
    conn.run_cypher(
        "CREATE NODE TABLE RelatesToNode_ (uuid STRING PRIMARY KEY, relation_type STRING, \
         episodes STRING[], expired_at TIMESTAMP)",
    )
    .unwrap();
    conn.run_cypher("CREATE REL TABLE MENTIONS (FROM Episodic TO Entity, group_id STRING)")
        .unwrap();
    schema::migrate(&conn, DIM);
    schema::migrate(&conn, DIM);
    conn.run_cypher("MATCH (n:Episodic) RETURN n.write_path LIMIT 0")
        .unwrap_or_else(|e| panic!("Episodic.write_path must bind after migrate: {e}"));
}
