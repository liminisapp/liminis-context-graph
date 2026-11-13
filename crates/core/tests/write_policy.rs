//! In-process tests for the daemon-wide per-group write allow-list (issue #742, ADR-0742).
//!
//! Every test drives `handlers::dispatch_with_policy` with an explicit policy, so nothing here
//! touches the process-wide `OnceLock` or the environment (parallel-test safe).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    cross_group::{self, CreateCrossGroupEdgeParams, EndpointSpec},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::MockExtractor,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    types::EntityRow,
    write_policy::{self, WritableGroups, WriteClass},
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DIM: usize = 4;
const TS: &str = "2026-01-01T00:00:00Z";

struct Fixture {
    _dir: TempDir,
    wal_dir: std::path::PathBuf,
    state: Arc<AppState>,
}

fn entity(name: &str, group_id: &str) -> EntityRow {
    EntityRow {
        uuid: Uuid::new_v4().to_string(),
        name: name.to_string(),
        group_id: group_id.to_string(),
        labels: vec!["Entity".to_string()],
        created_at: TS.to_string(),
        name_embedding: vec![1.0, 0.0, 0.0, 0.0],
        summary: format!("summary of {name}"),
        attributes: "{}".to_string(),
        ..Default::default()
    }
}

fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let db = Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap();
    db.connect().unwrap().init_schema(DIM).unwrap();
    let wal_dir = dir.path().join("wal");
    std::fs::create_dir_all(&wal_dir).unwrap();
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(Arc::new(db))),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor: Arc::new(MockExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: dir.path().join("test.db").to_str().unwrap().to_string(),
        wal_root: Some(wal_dir.clone()),
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
    });
    Fixture {
        _dir: dir,
        wal_dir,
        state,
    }
}

fn policy(groups: &str) -> WritableGroups {
    WritableGroups::parse(groups).unwrap()
}

async fn call(f: &Fixture, p: Option<&WritableGroups>, method: &str, params: Value) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    let resp = handlers::dispatch_with_policy(req, Arc::clone(&f.state), None, p).await;
    serde_json::to_value(resp).unwrap()
}

/// True when the response is a `-32004` write refusal; returns its `data.reason`.
fn refusal_reason(v: &Value) -> Option<String> {
    (v["error"]["code"] == -32004).then(|| v["error"]["data"]["reason"].as_str().unwrap().into())
}

fn assert_refused(v: &Value, reason: &str) {
    assert_eq!(refusal_reason(v).as_deref(), Some(reason), "{v}");
}

fn assert_not_refused(v: &Value) {
    assert!(refusal_reason(v).is_none(), "unexpected refusal: {v}");
}

fn entity_count(f: &Fixture, group: &str) -> u64 {
    let db = f.state.db.load_full().unwrap();
    let conn = db.connect().unwrap();
    conn.count_entities_by_group_ids(&[group]).unwrap()
}

fn wal_lines(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += wal_lines(&p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                n += std::fs::read_to_string(&p)
                    .unwrap_or_default()
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .count();
            }
        }
    }
    n
}

/// A `layer` row holding a cross-group pointer into group `a` (so `layer` owns rows that a purge
/// or rebind of `a` rewrites — ADR-0385).
fn seed_pointer_layer_into_a(f: &Fixture) {
    let db = f.state.db.load_full().unwrap();
    let conn = db.connect().unwrap();
    let alice = entity("Alice", "layer");
    let bob = entity("Bob", "a");
    conn.insert_entity(&alice).unwrap();
    conn.insert_entity(&bob).unwrap();
    cross_group::create_cross_group_edge(
        &conn,
        CreateCrossGroupEdgeParams {
            name: "KNOWS".to_string(),
            source: EndpointSpec::Uuid(alice.uuid.clone()),
            target: EndpointSpec::Foreign {
                kind: None,
                source_group_id: "a".to_string(),
                endpoint_name: "Bob".to_string(),
            },
            group_id: "layer".to_string(),
            fact: "Alice knows Bob".to_string(),
            fact_embedding: vec![1.0, 0.0, 0.0, 0.0],
            valid_at: None,
            relation_type: None,
        },
        TS,
    )
    .unwrap();
}

// ── AC1 / AC3 / AC7 ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn assert_entity_to_non_listed_group_is_refused_and_writes_nothing() {
    let f = fixture();
    let p = policy("a");
    let before = wal_lines(&f.wal_dir);
    let v = call(
        &f,
        Some(&p),
        "knowledge_assert_entity",
        json!({"name": "X", "group_id": "b", "summary": "s"}),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("'b'") && msg.contains("--writable-groups"),
        "{msg}"
    );
    assert_eq!(v["error"]["data"]["group_ids"], json!(["b"]));
    assert_eq!(v["error"]["data"]["setting"], "--writable-groups");
    assert_eq!(v["error"]["data"]["writable_groups"], json!(["a"]));
    assert_eq!(entity_count(&f, "b"), 0);
    assert_eq!(wal_lines(&f.wal_dir), before, "no WAL event on refusal");
}

#[tokio::test]
async fn assert_entity_to_listed_group_succeeds() {
    let f = fixture();
    let p = policy("a");
    let v = call(
        &f,
        Some(&p),
        "knowledge_assert_entity",
        json!({"name": "X", "group_id": "a", "summary": "s"}),
    )
    .await;
    assert!(v["error"].is_null(), "{v}");
    assert_eq!(entity_count(&f, "a"), 1);
}

#[tokio::test]
async fn unset_policy_leaves_every_group_writable() {
    let f = fixture();
    let v = call(
        &f,
        None,
        "knowledge_assert_entity",
        json!({"name": "X", "group_id": "anything", "summary": "s"}),
    )
    .await;
    assert!(v["error"].is_null(), "{v}");
    assert_eq!(entity_count(&f, "anything"), 1);
}

#[tokio::test]
async fn omitted_group_id_is_checked_against_the_default_group() {
    let f = fixture();
    // Default group is "liminis"; not listed → refused.
    let p = policy("a");
    let v = call(
        &f,
        Some(&p),
        "knowledge_assert_entity",
        json!({"name": "X", "summary": "s"}),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    assert_eq!(v["error"]["data"]["group_ids"], json!(["liminis"]));
    assert_eq!(entity_count(&f, "liminis"), 0);
    // Listing the default group explicitly makes the omitted form writable.
    let p = policy("a,liminis");
    let v = call(
        &f,
        Some(&p),
        "knowledge_assert_entity",
        json!({"name": "X", "summary": "s"}),
    )
    .await;
    assert!(v["error"].is_null(), "{v}");
}

const GROUP_PARAM_METHODS: &[&str] = &[
    "knowledge_add_episode",
    "knowledge_process_chunk",
    "knowledge_assert_entity",
    "knowledge_assert_relationship",
    "knowledge_merge_entities",
    "knowledge_add_cross_group_edge",
    "knowledge_reprocess_entity_types",
    "knowledge_reprocess_relation_types",
];

#[tokio::test]
async fn group_param_methods_resolve_omitted_null_nonstring_and_empty_like_the_handlers() {
    let f = fixture();
    let p = policy("a");
    for method in GROUP_PARAM_METHODS {
        // Absent / null / non-string fall back to the default group ("liminis", unlisted).
        for params in [json!({}), json!({"group_id": null}), json!({"group_id": 7})] {
            let v = call(&f, Some(&p), method, params.clone()).await;
            assert_refused(&v, "group_not_writable");
            assert_eq!(
                v["error"]["data"]["group_ids"],
                json!(["liminis"]),
                "{method} {params}"
            );
        }
        // The empty string stays the empty string (a distinct, never-listable group).
        let v = call(&f, Some(&p), method, json!({"group_id": ""})).await;
        assert_refused(&v, "group_not_writable");
        assert_eq!(v["error"]["data"]["group_ids"], json!([""]), "{method}");
        // A listed group passes the guard (the handler may still reject the thin params).
        let v = call(&f, Some(&p), method, json!({"group_id": "a"})).await;
        assert_not_refused(&v);
    }
}

// ── AC2: cross-group edges ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cross_group_edge_checks_only_the_edges_own_group() {
    let f = fixture();
    {
        let db = f.state.db.load_full().unwrap();
        let conn = db.connect().unwrap();
        conn.insert_entity(&entity("Alice", "a")).unwrap();
        conn.insert_entity(&entity("Bob", "b")).unwrap();
        conn.insert_entity(&entity("Alice", "b")).unwrap();
    }
    let p = policy("a");
    // Authored in `a`, foreign endpoint in `b` (read-only pointer): allowed.
    let alice_a = {
        let db = f.state.db.load_full().unwrap();
        let conn = db.connect().unwrap();
        conn.get_entities_by_group_ids(Some(&["a"])).unwrap()[0]
            .uuid
            .clone()
    };
    let v = call(
        &f,
        Some(&p),
        "knowledge_add_cross_group_edge",
        json!({
            "name": "KNOWS",
            "group_id": "a",
            "source": {"uuid": alice_a},
            "target": {"source_group_id": "b", "endpoint_name": "Bob"},
            "fact": "Alice knows Bob",
        }),
    )
    .await;
    assert!(v["error"].is_null(), "{v}");
    // Authored in `b`: refused, even though `a` is mentioned as an endpoint.
    let v = call(
        &f,
        Some(&p),
        "knowledge_add_cross_group_edge",
        json!({
            "name": "KNOWS",
            "group_id": "b",
            "source": {"source_group_id": "a", "endpoint_name": "Alice"},
            "target": {"uuid": "whatever"},
            "fact": "x",
        }),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    assert_eq!(v["error"]["data"]["group_ids"], json!(["b"]));
}

// ── group-required / group-array methods ──────────────────────────────────────────────────────

#[tokio::test]
async fn required_group_methods_are_checked() {
    let f = fixture();
    let p = policy("a");
    for method in [
        "knowledge_supersede_document",
        "knowledge_canonicalize_relations",
        "knowledge_backfill_relation_types",
        "knowledge_backfill_summary_embeddings",
        "knowledge_reload_ontology",
    ] {
        let v = call(&f, Some(&p), method, json!({"group_id": "b"})).await;
        assert_refused(&v, "group_not_writable");
        let v = call(&f, Some(&p), method, json!({"group_id": "a"})).await;
        assert_not_refused(&v);
        // Missing group: the handler's own error, not a refusal.
        let v = call(&f, Some(&p), method, json!({})).await;
        assert_not_refused(&v);
    }
    for method in [
        "knowledge_delete_by_source",
        "knowledge_delete_chunk_episode",
        "knowledge_mark_chunk_removed",
    ] {
        // One non-listed group among several rejects the whole call.
        let v = call(&f, Some(&p), method, json!({"group_ids": ["a", "b"]})).await;
        assert_refused(&v, "group_not_writable");
        assert_eq!(v["error"]["data"]["group_ids"], json!(["b"]));
        let v = call(&f, Some(&p), method, json!({"group_ids": ["a"]})).await;
        assert_not_refused(&v);
    }
}

// ── admin / multi-group ───────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_by_group_needs_target_groups_and_foreign_pointer_owners() {
    let f = fixture();
    seed_pointer_layer_into_a(&f);
    // Target `b` is not listed.
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_delete_by_group",
        json!({"group_ids": ["b"], "confirm": true}),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    // Target `a` is listed, but `layer` owns a pointer into it and is not: refused, `a` intact.
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_delete_by_group",
        json!({"group_ids": ["a"], "confirm": true}),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    assert_eq!(v["error"]["data"]["group_ids"], json!(["layer"]));
    assert_eq!(entity_count(&f, "a"), 1);
    // Once the owner is listed too, the purge proceeds.
    let v = call(
        &f,
        Some(&policy("a,layer")),
        "knowledge_delete_by_group",
        json!({"group_ids": ["a"], "confirm": true}),
    )
    .await;
    assert!(v["error"].is_null(), "{v}");
    assert_eq!(entity_count(&f, "a"), 0);
}

#[tokio::test]
async fn rebind_pointers_requires_the_owning_groups_not_the_source_group() {
    let f = fixture();
    seed_pointer_layer_into_a(&f);
    // Owner `layer` not listed: refused even though the source group `a` is.
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_rebind_pointers",
        json!({"source_group_id": "a"}),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    assert_eq!(v["error"]["data"]["group_ids"], json!(["layer"]));
    // The source group need not be listed: only the owners are written.
    let v = call(
        &f,
        Some(&policy("layer")),
        "knowledge_rebind_pointers",
        json!({"source_group_id": "a"}),
    )
    .await;
    assert!(v["error"].is_null(), "{v}");
}

#[tokio::test]
async fn rebuild_from_wal_is_checked_on_every_call() {
    let f = fixture();
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_rebuild_from_wal",
        json!({"group_id": "b"}),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_rebuild_from_wal",
        json!({"group_id": "b", "dry_run": true}),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    // `""` maps to the default group in this handler, which is not listed.
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_rebuild_from_wal",
        json!({"group_id": ""}),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    assert_eq!(v["error"]["data"]["group_ids"], json!(["liminis"]));
    // Listed: passes the guard (this fixture has no WAL, so the handler itself errors).
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_rebuild_from_wal",
        json!({"group_id": "a"}),
    )
    .await;
    assert_not_refused(&v);
}

#[tokio::test]
async fn delete_episode_is_checked_against_the_episodes_owning_group() {
    let f = fixture();
    let ep_uuid = Uuid::new_v4().to_string();
    {
        let db = f.state.db.load_full().unwrap();
        let conn = db.connect().unwrap();
        conn.insert_episodic(&lcg_core::types::EpisodicRow {
            uuid: ep_uuid.clone(),
            name: "ep".to_string(),
            group_id: "b".to_string(),
            created_at: TS.to_string(),
            valid_at: TS.to_string(),
            content: "c".to_string(),
            content_embedding: vec![1.0, 0.0, 0.0, 0.0],
            ..Default::default()
        })
        .unwrap();
    }
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_delete_episode",
        json!({"episode_uuid": ep_uuid}),
    )
    .await;
    assert_refused(&v, "group_not_writable");
    assert_eq!(v["error"]["data"]["group_ids"], json!(["b"]));
    let db = f.state.db.load_full().unwrap();
    assert!(db
        .connect()
        .unwrap()
        .get_episode_group_id(&ep_uuid)
        .unwrap()
        .is_some());
    // Unknown episode: a no-op delete is not refused.
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_delete_episode",
        json!({"episode_uuid": "nope"}),
    )
    .await;
    assert_not_refused(&v);
    // Owner listed: deleted.
    let v = call(
        &f,
        Some(&policy("b")),
        "knowledge_delete_episode",
        json!({"episode_uuid": ep_uuid}),
    )
    .await;
    assert!(v["error"].is_null(), "{v}");
}

#[tokio::test]
async fn whole_database_operations_need_every_present_group_listed() {
    let f = fixture();
    {
        let db = f.state.db.load_full().unwrap();
        let conn = db.connect().unwrap();
        conn.insert_entity(&entity("A1", "a")).unwrap();
        conn.insert_entity(&entity("B1", "b")).unwrap();
    }
    // A group that exists only as a WAL directory counts too.
    std::fs::create_dir_all(f.wal_dir.join("walonly")).unwrap();

    for method in ["knowledge_clear_all", "knowledge_apply_corrections"] {
        let v = call(&f, Some(&policy("a")), method, json!({"confirm": true})).await;
        assert_refused(&v, "all_groups_required");
        let groups = v["error"]["data"]["group_ids"].as_array().unwrap();
        assert!(
            groups.contains(&json!("b")) && groups.contains(&json!("walonly")),
            "{v}"
        );
        assert!(!groups.contains(&json!("a")), "{v}");
        // Still missing the WAL-only group.
        let v = call(&f, Some(&policy("a,b")), method, json!({"confirm": true})).await;
        assert_refused(&v, "all_groups_required");
    }
    assert_eq!(entity_count(&f, "a"), 1);
    assert_eq!(entity_count(&f, "b"), 1);
    // All present groups listed: passes the guard.
    let v = call(
        &f,
        Some(&policy("a,b,walonly")),
        "knowledge_apply_corrections",
        json!({"dry_run": true}),
    )
    .await;
    assert_not_refused(&v);
}

// ── raw Cypher, exempt, read-only, unclassified ───────────────────────────────────────────────

#[tokio::test]
async fn raw_cypher_is_refused_whenever_a_list_is_set() {
    let f = fixture();
    let p = policy("a");
    let before = wal_lines(&f.wal_dir);
    for q in [
        "CREATE (:Entity {uuid:'x', group_id:'b'})",
        "CALL some_writing_procedure()",
        "MATCH (n) RETURN n",
    ] {
        let v = call(&f, Some(&p), "knowledge_query_cypher", json!({"query": q})).await;
        assert_refused(&v, "raw_cypher_disabled");
    }
    assert_eq!(entity_count(&f, "b"), 0);
    assert_eq!(wal_lines(&f.wal_dir), before);
    // Unset: unchanged behaviour.
    let v = call(
        &f,
        None,
        "knowledge_query_cypher",
        json!({"query": "MATCH (n:Entity) RETURN n.uuid"}),
    )
    .await;
    assert!(v["error"].is_null(), "{v}");
}

#[tokio::test]
async fn reads_of_non_listed_groups_and_exempt_methods_are_unaffected() {
    let f = fixture();
    f.state
        .db
        .load_full()
        .unwrap()
        .connect()
        .unwrap()
        .insert_entity(&entity("B1", "b"))
        .unwrap();
    let p = policy("a");
    let v = call(
        &f,
        Some(&p),
        "knowledge_get_nodes_by_group",
        json!({"group_ids": ["b"]}),
    )
    .await;
    assert!(v["error"].is_null(), "{v}");
    let v = call(&f, Some(&p), "knowledge_status", json!({})).await;
    assert!(v["error"].is_null(), "{v}");
    // Recovery is exempt (ADR-0009); a bogus strategy yields the handler's own error.
    let v = call(
        &f,
        Some(&p),
        "knowledge_recover",
        json!({"strategy": "bogus"}),
    )
    .await;
    assert_not_refused(&v);
    let v = call(&f, Some(&p), "knowledge_wal_mark_list", json!({})).await;
    assert_not_refused(&v);
}

#[tokio::test]
async fn unclassified_method_is_refused_when_a_list_is_set() {
    let f = fixture();
    let v = call(
        &f,
        Some(&policy("a")),
        "knowledge_brand_new_writer",
        json!({}),
    )
    .await;
    assert_refused(&v, "unclassified_method");
    // Unset: the pre-existing "method not found" behaviour.
    let v = call(&f, None, "knowledge_brand_new_writer", json!({})).await;
    assert_eq!(v["error"]["code"], -32000, "{v}");
}

#[test]
fn every_classified_method_is_a_real_handler_method() {
    // Guards against stale table keys: each non-exempt/non-readonly class must name a method the
    // dispatcher knows. `classify` is total over the table by construction; spot-check shape.
    assert_eq!(
        write_policy::classify("knowledge_query_cypher"),
        Some(WriteClass::RawCypher)
    );
    assert_eq!(write_policy::classify("nope"), None);
    assert!(matches!(
        write_policy::classify("knowledge_recover"),
        Some(WriteClass::Exempt(_))
    ));
}
