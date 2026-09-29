//! Kind-scoped entity identity (issue #615, ADR-0615): `Entity.kind` and
//! `lookup_key = group_id ␟ kind ␟ lower(trim(name))`.
//!
//! Covers SC-001..SC-007 and the D2 (reads broad / writes scoped), D3 (`lookup_key` derived:
//! stripped on write, recomputed on replay), D4 (kind-pinned cross-group pointers) and D5
//! (merges never cross kinds) decisions.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::corrections::{merge_entities, MergeEntitiesParams};
use lcg_core::cross_group::{
    create_cross_group_edge, rebind_pointers_forced, resolve_endpoint_kind,
    CreateCrossGroupEdgeParams, EndpointSpec,
};
use lcg_core::pointer::{self, BindingState};
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::MockExtractor,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    Db, EntityRow, WalReplayer, WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "liminis";

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("kind.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
        conn.create_entity_lookup_key_index().unwrap();
    }
    (db, dir)
}

fn make_state(db: Arc<Db>, wal_dir: Option<PathBuf>) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let wal_writer = wal_dir
        .as_ref()
        .and_then(|d| WalWriter::new(d, 10_000, 5 * 1024 * 1024).ok());
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor: Arc::new(MockExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(tokio::sync::RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: wal_dir,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(
            wal_writer
                .into_iter()
                .map(|w| (G.to_string(), w))
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

async fn call(method: &str, params: Value, state: &Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, Arc::clone(state), None).await).unwrap()
}

async fn assert_entity(state: &Arc<AppState>, params: Value) -> Value {
    let v = call("knowledge_assert_entity", params, state).await;
    assert!(v.get("error").is_none(), "unexpected error: {v}");
    v["result"].clone()
}

fn entity(uuid: &str, name: &str, kind: &str, group: &str, created: &str) -> EntityRow {
    EntityRow {
        uuid: uuid.to_string(),
        name: name.to_string(),
        group_id: group.to_string(),
        labels: vec!["Entity".to_string()],
        kind: kind.to_string(),
        created_at: created.to_string(),
        name_embedding: vec![1.0, 0.0, 0.0, 0.0],
        summary: format!("{kind} {name}"),
        attributes: "{}".to_string(),
        ..Default::default()
    }
}

fn nodes_named(db: &Db, name: &str) -> Vec<EntityRow> {
    let conn = db.connect().unwrap();
    let mut rows: Vec<EntityRow> = conn
        .get_entities_by_group_ids(Some(&[G]))
        .unwrap()
        .into_iter()
        .filter(|e| e.name.eq_ignore_ascii_case(name))
        .collect();
    rows.sort_by(|a, b| a.kind.cmp(&b.kind));
    rows
}

// ── SC-001 / SC-002 ───────────────────────────────────────────────────────────

#[tokio::test]
async fn same_name_different_kinds_coexist_and_channel_is_untouched() {
    let (db, _dir) = make_db();
    let state = make_state(Arc::clone(&db), None);

    let channel = assert_entity(
        &state,
        json!({"name": "adr", "kind": "KnowledgeChannel", "summary": "the channel",
               "labels": ["Entity", "KnowledgeChannel"], "attributes": {"owner": "docs"}}),
    )
    .await;
    assert_eq!(channel["created"], true);
    let topic = assert_entity(&state, json!({"name": "adr", "kind": "Topic"})).await;
    assert_eq!(topic["created"], true);
    assert_ne!(channel["entity_uuid"], topic["entity_uuid"]);

    let rows = nodes_named(&db, "adr");
    assert_eq!(rows.len(), 2, "two distinct entities, not one overwritten");
    let ch = rows.iter().find(|r| r.kind == "KnowledgeChannel").unwrap();
    assert_eq!(ch.summary, "the channel");
    assert!(ch.attributes.contains("docs"));
    assert_eq!(ch.labels, vec!["Entity", "KnowledgeChannel"]);
    let tp = rows.iter().find(|r| r.kind == "Topic").unwrap();
    assert_eq!(tp.labels, vec!["Entity", "Topic"], "kind is appended to labels");

    // Re-asserting the channel updates it in place (idempotent per (group, kind, name)).
    let again = assert_entity(&state, json!({"name": "ADR", "kind": "KnowledgeChannel"})).await;
    assert_eq!(again["created"], false);
    assert_eq!(again["entity_uuid"], channel["entity_uuid"]);
    assert_eq!(nodes_named(&db, "adr").len(), 2);
}

#[tokio::test]
async fn relationship_between_same_named_kinds_links_two_distinct_nodes() {
    let (db, _dir) = make_db();
    let state = make_state(Arc::clone(&db), None);
    let ch = assert_entity(&state, json!({"name": "adr", "kind": "KnowledgeChannel"})).await;
    let tp = assert_entity(&state, json!({"name": "adr", "kind": "Topic"})).await;

    let v = call(
        "knowledge_assert_relationship",
        json!({"source_name": "adr", "source_kind": "KnowledgeChannel",
               "target_name": "adr", "target_kind": "Topic", "predicate": "COVERS"}),
        &state,
    )
    .await;
    assert!(v.get("error").is_none(), "{v}");

    let conn = db.connect().unwrap();
    let edges = conn.get_edges_by_group_ids(Some(&[G])).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].source_node_uuid, ch["entity_uuid"].as_str().unwrap());
    assert_eq!(edges[0].target_node_uuid, tp["entity_uuid"].as_str().unwrap());
    assert_ne!(edges[0].source_node_uuid, edges[0].target_node_uuid, "not a self-loop");
}

// ── SC-003 / SC-004 (D2) ──────────────────────────────────────────────────────

#[tokio::test]
async fn name_only_reads_report_ambiguity_and_exact_kind_never_does() {
    let (db, _dir) = make_db();
    let state = make_state(Arc::clone(&db), None);
    let ch = assert_entity(&state, json!({"name": "adr", "kind": "KnowledgeChannel"})).await;
    let tp = assert_entity(&state, json!({"name": "adr", "kind": "Topic"})).await;

    // resolve_entity, name-only: ambiguity error listing both candidates as structured data.
    let v = call("knowledge_resolve_entity", json!({"name": "adr"}), &state).await;
    assert_eq!(v["error"]["code"], -32002, "{v}");
    assert_eq!(v["error"]["data"]["reason"], "ambiguous_entity");
    let cands = v["error"]["data"]["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 2);
    let kinds: Vec<&str> = cands.iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"KnowledgeChannel") && kinds.contains(&"Topic"));
    let uuids: Vec<&str> = cands.iter().map(|c| c["uuid"].as_str().unwrap()).collect();
    assert!(uuids.contains(&ch["entity_uuid"].as_str().unwrap()));
    assert!(uuids.contains(&tp["entity_uuid"].as_str().unwrap()));

    // Exact-kind lookup: never ambiguous.
    let v = call(
        "knowledge_resolve_entity",
        json!({"name": "adr", "kind": "Topic"}),
        &state,
    )
    .await;
    assert_eq!(v["result"]["node"]["uuid"], tp["entity_uuid"]);
    assert_eq!(v["result"]["node"]["kind"], "Topic");

    // A name-only relationship endpoint resolution is ambiguous too.
    assert_entity(&state, json!({"name": "solo", "kind": "Topic"})).await;
    let v = call(
        "knowledge_assert_relationship",
        json!({"source_name": "adr", "target_name": "solo", "predicate": "COVERS"}),
        &state,
    )
    .await;
    assert_eq!(v["error"]["code"], -32002, "{v}");
    let conn = db.connect().unwrap();
    assert_eq!(conn.get_edges_by_group_ids(Some(&[G])).unwrap().len(), 0);

    // find/list: multi-row reads return both kinds; a kind filter narrows them.
    let v = call("knowledge_list_entities", json!({"group_ids": [G]}), &state).await;
    assert_eq!(v["result"]["count"], 3);
    let v = call(
        "knowledge_list_entities",
        json!({"group_ids": [G], "kind": "KnowledgeChannel"}),
        &state,
    )
    .await;
    assert_eq!(v["result"]["count"], 1);
    assert_eq!(v["result"]["nodes"][0]["kind"], "KnowledgeChannel");
}

#[tokio::test]
async fn single_non_default_kind_resolves_on_name_only_read() {
    let (db, _dir) = make_db();
    let state = make_state(Arc::clone(&db), None);
    let tp = assert_entity(&state, json!({"name": "lonely", "kind": "Topic"})).await;
    let v = call("knowledge_resolve_entity", json!({"name": "lonely"}), &state).await;
    assert_eq!(v["result"]["node"]["uuid"], tp["entity_uuid"]);
    let v = call("knowledge_resolve_entity", json!({"name": "nobody"}), &state).await;
    assert_eq!(v["result"]["found"], false);
}

#[tokio::test]
async fn name_only_write_is_scoped_to_the_default_kind() {
    let (db, _dir) = make_db();
    let state = make_state(Arc::clone(&db), None);
    assert_entity(
        &state,
        json!({"name": "adr", "kind": "KnowledgeChannel", "summary": "chan"}),
    )
    .await;
    assert_entity(&state, json!({"name": "adr", "kind": "Topic", "summary": "topic"})).await;

    let created = assert_entity(&state, json!({"name": "adr", "summary": "plain"})).await;
    assert_eq!(created["created"], true, "no kind-Entity node existed: a new one is created");

    let rows = nodes_named(&db, "adr");
    assert_eq!(rows.len(), 3);
    let by_kind = |k: &str| rows.iter().find(|r| r.kind == k).unwrap();
    assert_eq!(by_kind("Entity").summary, "plain");
    assert_eq!(by_kind("KnowledgeChannel").summary, "chan");
    assert_eq!(by_kind("Topic").summary, "topic");

    // A second name-only write updates the kind-Entity node only.
    let upd = assert_entity(&state, json!({"name": "adr", "summary": "plain2"})).await;
    assert_eq!(upd["created"], false);
    let rows = nodes_named(&db, "adr");
    assert_eq!(rows.iter().find(|r| r.kind == "Entity").unwrap().summary, "plain2");
    assert_eq!(rows.iter().find(|r| r.kind == "Topic").unwrap().summary, "topic");
}

#[tokio::test]
async fn rename_guard_is_kind_scoped_and_entity_uuid_kind_is_immutable() {
    let (db, _dir) = make_db();
    let state = make_state(Arc::clone(&db), None);
    let team = assert_entity(&state, json!({"name": "pset", "kind": "Team"})).await;
    assert_entity(&state, json!({"name": "other", "kind": "Topic"})).await;

    // Renaming the Team to a name only a *different kind* uses is not a collision.
    let v = call(
        "knowledge_assert_entity",
        json!({"entity_uuid": team["entity_uuid"], "name": "other"}),
        &state,
    )
    .await;
    assert!(v.get("error").is_none(), "cross-kind rename must not collide: {v}");

    // A supplied kind must match the uuid's kind: kind is immutable.
    let v = call(
        "knowledge_assert_entity",
        json!({"entity_uuid": team["entity_uuid"], "name": "other", "kind": "Topic"}),
        &state,
    )
    .await;
    assert!(v["error"]["message"].as_str().unwrap().contains("immutable"), "{v}");
}

#[tokio::test]
async fn invalid_kinds_are_rejected() {
    let (db, _dir) = make_db();
    let state = make_state(Arc::clone(&db), None);
    for bad in [json!(""), json!("   "), json!("a\u{1f}b"), json!(7)] {
        let v = call(
            "knowledge_assert_entity",
            json!({"name": "x", "kind": bad}),
            &state,
        )
        .await;
        assert!(v.get("error").is_some(), "kind {bad} must be rejected: {v}");
    }
    assert!(nodes_named(&db, "x").is_empty());
    // Whitespace around a valid kind is trimmed, not rejected.
    assert_entity(&state, json!({"name": "x", "kind": " Topic "})).await;
    assert_eq!(nodes_named(&db, "x")[0].kind, "Topic");
}

// ── SC-005 / D3: WAL ──────────────────────────────────────────────────────────

fn wal_text(dir: &std::path::Path) -> String {
    let mut all = String::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
            all.push_str(&fs::read_to_string(p).unwrap());
        }
    }
    all
}

fn replay_into_fresh(wal_dir: &std::path::Path) -> (Db, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Db::open(dir.path().join("replay.db").to_str().unwrap()).unwrap();
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        WalReplayer::new(wal_dir)
            .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
            .unwrap();
        lcg_core::schema::backfill_entity_lookup_keys(&conn).unwrap();
        conn.create_entity_lookup_key_index().unwrap();
    }
    (db, dir)
}

#[tokio::test]
async fn new_wal_records_carry_kind_but_never_lookup_key_and_replay_recomputes_it() {
    let (db, _dir) = make_db();
    let wal_dir = TempDir::new().unwrap();
    let state = make_state(Arc::clone(&db), Some(wal_dir.path().to_path_buf()));
    assert_entity(&state, json!({"name": "adr", "kind": "KnowledgeChannel"})).await;
    assert_entity(&state, json!({"name": "adr", "kind": "Topic"})).await;
    // An update writes a `SET ... e.lookup_key = $lookup_key` record too.
    assert_entity(&state, json!({"name": "adr", "kind": "Topic", "summary": "v2"})).await;

    let text = wal_text(wal_dir.path());
    assert!(text.contains("$lookup_key"), "template still names the derived param");
    assert!(
        !text.contains("\"lookup_key\""),
        "no WAL record may carry a lookup_key param value: {text}"
    );
    assert!(text.contains("\"kind\":\"Topic\""));

    let (db2, _d2) = replay_into_fresh(wal_dir.path());
    let conn = db2.connect().unwrap();
    let ch = conn.get_entity_by_name_ci("adr", G, "KnowledgeChannel").unwrap().unwrap();
    let tp = conn.get_entity_by_name_ci("adr", G, "Topic").unwrap().unwrap();
    assert_ne!(ch.uuid, tp.uuid);
    assert_eq!(tp.summary, "v2");
    assert!(conn.get_entity_by_name_ci("adr", G, "Entity").unwrap().is_none());
    let rows = conn
        .cypher_query("MATCH (e:Entity) RETURN e.lookup_key ORDER BY e.lookup_key")
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn old_wal_with_literal_lookup_key_and_no_kind_replays_as_default_kind() {
    let wal_dir = TempDir::new().unwrap();
    let create = json!({
        "seq": 1, "ts": "2026-01-01T00:00:00.000000+00:00", "db": "",
        "cypher": "CREATE (:Entity {uuid: $uuid, name: $name, group_id: $group_id, \
                   labels: $labels, created_at: $created_at, name_embedding: $name_embedding, \
                   summary: $summary, attributes: $attributes, \
                   summary_embedding: $summary_embedding, lookup_key: $lookup_key})",
        "params": {
            "uuid": "old-1", "name": "Old Thing", "group_id": "liminis",
            "labels": ["Entity"], "created_at": "2026-01-01T00:00:00Z",
            "name_embedding": [1.0, 0.0, 0.0, 0.0], "summary": "s", "attributes": "{}",
            "summary_embedding": [0.0, 0.0, 0.0, 0.0],
            // An old-format (two-field) literal key: must be ignored, never trusted.
            "lookup_key": "liminis\u{1f}old thing"
        }
    });
    // An old update record carries no group_id/kind at all.
    let update = json!({
        "seq": 2, "ts": "2026-01-01T00:00:01.000000+00:00", "db": "",
        "cypher": "MATCH (e:Entity {uuid: $uuid}) SET e.name = $name, e.labels = $labels, \
                   e.summary = $summary, e.attributes = $attributes, e.lookup_key = $lookup_key",
        "params": {
            "uuid": "old-1", "name": "Old Thing", "labels": ["Entity"], "summary": "s2",
            "attributes": "{}", "lookup_key": "liminis\u{1f}old thing"
        }
    });
    fs::write(
        wal_dir.path().join("20260519_000000_aaa111_0000.jsonl"),
        format!("{create}\n{update}\n"),
    )
    .unwrap();

    let (db, _d) = replay_into_fresh(wal_dir.path());
    let conn = db.connect().unwrap();
    let row = conn
        .get_entity_by_name_ci("old thing", G, "Entity")
        .unwrap()
        .expect("old record replays as kind Entity and resolves through the new key");
    assert_eq!(row.uuid, "old-1");
    assert_eq!(row.kind, "Entity");
    assert_eq!(row.summary, "s2");
    let keys = conn
        .cypher_query("MATCH (e:Entity) RETURN e.lookup_key, e.kind")
        .unwrap();
    assert_eq!(keys.len(), 1);
}

#[tokio::test]
async fn dump_round_trip_preserves_kinds() {
    let (db, _dir) = make_db();
    {
        let conn = db.connect().unwrap();
        conn.insert_entity(&entity("a", "adr", "KnowledgeChannel", G, "2026-01-01 00:00:00"))
            .unwrap();
        conn.insert_entity(&entity("b", "adr", "Topic", G, "2026-01-02 00:00:00"))
            .unwrap();
        conn.insert_entity(&entity("c", "adr", "Entity", G, "2026-01-03 00:00:00"))
            .unwrap();
    }
    let state = make_state(Arc::clone(&db), None);
    let target = TempDir::new().unwrap();
    let dump_dir = target.path().join("dump");
    let v = call(
        "knowledge_dump_wal",
        json!({"target_dir": dump_dir.to_str().unwrap()}),
        &state,
    )
    .await;
    assert!(v.get("error").is_none(), "{v}");
    let (db2, _d2) = replay_into_fresh(&dump_dir);
    let conn = db2.connect().unwrap();
    for (name, kind, uuid) in [
        ("adr", "KnowledgeChannel", "a"),
        ("adr", "Topic", "b"),
        ("adr", "Entity", "c"),
    ] {
        let row = conn.get_entity_by_name_ci(name, G, kind).unwrap().unwrap();
        assert_eq!(row.uuid, uuid);
        assert_eq!(row.kind, kind);
    }
}

// ── D4: pointers ──────────────────────────────────────────────────────────────

fn pointer_edge(
    db: &Db,
    target_kind: Option<&str>,
) -> (String, lcg_core::RelatesToEdge) {
    let conn = db.connect().unwrap();
    let edge = create_cross_group_edge(
        &conn,
        CreateCrossGroupEdgeParams {
            name: "REFERS".to_string(),
            source: EndpointSpec::Uuid("hub".to_string()),
            target: EndpointSpec::Foreign {
                source_group_id: "src".to_string(),
                endpoint_name: "adr".to_string(),
                kind: target_kind.map(str::to_string),
            },
            group_id: "layer".to_string(),
            fact: "hub refers to adr".to_string(),
            fact_embedding: vec![0.0; DIM],
            valid_at: None,
            relation_type: None,
        },
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    (edge.uuid.clone(), edge)
}

fn dst_state(db: &Db, edge_uuid: &str) -> (BindingState, Option<String>, Option<String>) {
    let conn = db.connect().unwrap();
    let e = conn
        .get_relates_to_by_uuids(&[edge_uuid.to_string()])
        .unwrap()
        .remove(0);
    let p = pointer::read_pointers(&e.attributes).dst.unwrap();
    (p.binding_state, p.resolved_uuid, p.endpoint_kind)
}

#[test]
fn pointer_without_kind_goes_ambiguous_and_kind_pinned_pointer_stays_bound() {
    let (db, _dir) = make_db();
    {
        let conn = db.connect().unwrap();
        conn.insert_entity(&entity("hub", "hub", "Entity", "layer", "2026-01-01 00:00:00"))
            .unwrap();
        conn.insert_entity(&entity("t1", "adr", "Topic", "src", "2026-01-01 00:00:00"))
            .unwrap();
    }
    let (loose, _) = pointer_edge(&db, None);
    let (pinned, _) = pointer_edge(&db, Some("Topic"));
    assert_eq!(dst_state(&db, &loose).0, BindingState::Bound, "one kind: binds as today");
    let (st, uuid, kind) = dst_state(&db, &pinned);
    assert_eq!((st, uuid.as_deref(), kind.as_deref()), (BindingState::Bound, Some("t1"), Some("Topic")));

    // A second kind of the same name appears in the source group.
    {
        let conn = db.connect().unwrap();
        conn.insert_entity(&entity("c1", "adr", "KnowledgeChannel", "src", "2026-01-02 00:00:00"))
            .unwrap();
        rebind_pointers_forced(&conn, "src", "2026-01-03T00:00:00Z").unwrap();
    }
    assert_eq!(dst_state(&db, &loose).0, BindingState::Ambiguous);
    let (st, uuid, _) = dst_state(&db, &pinned);
    assert_eq!((st, uuid.as_deref()), (BindingState::Bound, Some("t1")));

    // Pinned to the other kind binds to that entity.
    let (chan, _) = pointer_edge(&db, Some("KnowledgeChannel"));
    assert_eq!(dst_state(&db, &chan).1.as_deref(), Some("c1"));
    // A kind that does not exist is unbound, whatever else shares the name.
    let conn = db.connect().unwrap();
    let (st, uuid) = resolve_endpoint_kind(&conn, "src", "adr", Some("Team")).unwrap();
    assert_eq!((st, uuid), (BindingState::Unbound, None));
}

#[tokio::test]
async fn add_cross_group_edge_accepts_kind_for_foreign_endpoints_only() {
    let (db, _dir) = make_db();
    let state = make_state(Arc::clone(&db), None);
    {
        let conn = db.connect().unwrap();
        conn.insert_entity(&entity("hub", "hub", "Entity", "layer", "2026-01-01 00:00:00"))
            .unwrap();
        conn.insert_entity(&entity("t1", "adr", "Topic", "src", "2026-01-01 00:00:00"))
            .unwrap();
        conn.insert_entity(&entity("c1", "adr", "KnowledgeChannel", "src", "2026-01-01 00:00:00"))
            .unwrap();
    }
    let v = call(
        "knowledge_add_cross_group_edge",
        json!({"name": "REFERS", "group_id": "layer", "fact": "f",
               "source": {"uuid": "hub"},
               "target": {"source_group_id": "src", "endpoint_name": "adr"},
               "target_kind": "Topic"}),
        &state,
    )
    .await;
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["result"]["cross_group_pointers"]["dst"]["endpoint_kind"], "Topic", "{v}");
    assert_eq!(v["result"]["cross_group_pointers"]["dst"]["resolved_uuid"], "t1", "{v}");

    // A kind on a {uuid} endpoint is a validation error.
    let v = call(
        "knowledge_add_cross_group_edge",
        json!({"name": "REFERS", "group_id": "layer", "fact": "f",
               "source": {"uuid": "hub"}, "source_kind": "Topic",
               "target": {"source_group_id": "src", "endpoint_name": "adr"}}),
        &state,
    )
    .await;
    assert!(v.get("error").is_some(), "{v}");
}

// ── D5: merges ────────────────────────────────────────────────────────────────

fn merge_params(canonical: &str, alias: &str) -> MergeEntitiesParams {
    MergeEntitiesParams {
        canonical_uuid: Some(canonical.to_string()),
        canonical_name: None,
        alias_uuids: vec![alias.to_string()],
        alias_names: vec![],
        merge_all_by_name: false,
        kind: None,
        group_id: G.to_string(),
        dry_run: false,
    }
}

#[test]
fn merge_refuses_to_cross_kinds_and_modifies_nothing() {
    let (db, _dir) = make_db();
    let conn = db.connect().unwrap();
    conn.insert_entity(&entity("t", "adr", "Topic", G, "2026-01-01 00:00:00")).unwrap();
    conn.insert_entity(&entity("c", "adr", "KnowledgeChannel", G, "2026-01-02 00:00:00"))
        .unwrap();

    for dry_run in [true, false] {
        let mut p = merge_params("t", "c");
        p.dry_run = dry_run;
        let r = merge_entities(&conn, &p, "2026-02-01T00:00:00Z");
        assert!(!r.success, "dry_run={dry_run}");
        let msg = r.errors.join(" ");
        assert!(msg.contains("'Topic'") && msg.contains("'KnowledgeChannel'"), "{msg}");
    }
    let c = conn.get_entity_by_uuid("c").unwrap().unwrap();
    assert!(!c.labels.contains(&"Merged".to_string()), "alias must be untouched");
    assert_eq!(nodes_named(&db, "adr").len(), 2);
}

#[test]
fn same_kind_merge_is_unchanged_and_merge_all_by_name_stays_in_kind() {
    let (db, _dir) = make_db();
    let conn = db.connect().unwrap();
    conn.insert_entity(&entity("t1", "adr", "Topic", G, "2026-01-01 00:00:00")).unwrap();
    conn.insert_entity(&entity("t2", "adr", "Topic", G, "2026-01-02 00:00:00")).unwrap();
    conn.insert_entity(&entity("c1", "adr", "KnowledgeChannel", G, "2026-01-03 00:00:00"))
        .unwrap();

    let r = merge_entities(&conn, &merge_params("t1", "t2"), "2026-02-01T00:00:00Z");
    assert!(r.success, "{:?}", r.errors);
    assert_eq!(r.merged_count, 1);

    // merge_all_by_name from a Topic canonical never sweeps the KnowledgeChannel.
    conn.insert_entity(&entity("t3", "adr", "Topic", G, "2026-01-04 00:00:00")).unwrap();
    let mut p = merge_params("t1", "unused");
    p.alias_uuids.clear();
    p.merge_all_by_name = true;
    let r = merge_entities(&conn, &p, "2026-02-02T00:00:00Z");
    assert!(r.success, "{:?}", r.errors);
    let c1 = conn.get_entity_by_uuid("c1").unwrap().unwrap();
    assert!(!c1.labels.contains(&"Merged".to_string()));

    // A kind-less canonical_name matching several kinds is an ambiguity error.
    let p = MergeEntitiesParams {
        canonical_uuid: None,
        canonical_name: Some("adr".to_string()),
        alias_uuids: vec!["t3".to_string()],
        alias_names: vec![],
        merge_all_by_name: false,
        kind: None,
        group_id: G.to_string(),
        dry_run: true,
    };
    let r = merge_entities(&conn, &p, "2026-02-03T00:00:00Z");
    assert!(!r.success);
    assert!(r.errors.join(" ").contains("ambiguous"), "{:?}", r.errors);
}

// ── Extraction stays in the default-kind namespace (FR-010) ───────────────────

#[test]
fn dedup_candidate_queries_never_offer_a_non_default_kind() {
    let (db, _dir) = make_db();
    let conn = db.connect().unwrap();
    conn.insert_entity(&entity("t", "adr", "Topic", G, "2026-01-01 00:00:00")).unwrap();
    // Identical embedding: brute-force similarity would match the Topic if it were a candidate.
    let hit = conn
        .brute_force_similar_entity(&[1.0, 0.0, 0.0, 0.0], G, 0.5)
        .unwrap();
    assert!(hit.is_none(), "a Topic must never be an extraction dedup candidate");
    conn.insert_entity(&entity("e", "adr2", "Entity", G, "2026-01-02 00:00:00")).unwrap();
    let hit = conn
        .brute_force_similar_entity(&[1.0, 0.0, 0.0, 0.0], G, 0.5)
        .unwrap();
    assert_eq!(hit.unwrap().uuid, "e");
}
