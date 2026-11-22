//! Issue #856: implicit default-group writes return an additive `warnings` entry.
//!
//! Drives `handlers::dispatch_with_policy` in-process (the single chokepoint IPC, standalone MCP
//! and attached MCP share). No defaults change; only the response gains a warning.

use std::collections::HashMap;
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
    ontology::{Ontology, OntologyMode, RelationTypeDef},
    telemetry::{NoopSink, TelemetrySink},
    types::EntityRow,
    write_policy::{self, WritableGroups, WriteClass, IMPLICIT_DEFAULT_GROUP_WARNING},
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
        workspace_root: Some(dir.path().to_path_buf()),
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: Some(Arc::new(Ontology {
            mode: OntologyMode::Open,
            entity_types: vec![],
            relation_types: vec![RelationTypeDef {
                name: "KNOWS".to_string(),
                description: None,
                source_types: vec![],
                target_types: vec![],
                aliases: vec![],
                inverse_aliases: vec![],
                symmetric: false,
                keywords: vec![],
                extract: true,
            }],
            ancestor_map: HashMap::new(),
        })),
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    });
    Fixture { _dir: dir, state }
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

/// Every defaulting write method (`WriteClass::GroupParam`). A coverage test below ties this to
/// `write_policy::CLASSIFICATION`, so a newly added defaulting writer must be added here.
const DEFAULTING_WRITERS: &[&str] = &[
    "knowledge_add_episode",
    "knowledge_process_chunk",
    "knowledge_process_document",
    "knowledge_assert_entity",
    "knowledge_assert_relationship",
    "knowledge_merge_entities",
    "knowledge_add_cross_group_edge",
    "knowledge_reprocess_entity_types",
    "knowledge_reprocess_relation_types",
];

/// Seed `Alice` and `Bob` in `liminis` and `other` so every method has something to act on in
/// whichever group a scenario targets.
fn seed(f: &Fixture) {
    let db = f.state.db.load_full().unwrap();
    let conn = db.connect().unwrap();
    for g in ["liminis", "other"] {
        conn.insert_entity(&entity("Alice", g)).unwrap();
        conn.insert_entity(&entity("Bob", g)).unwrap();
    }
}

/// Entity uuids of `group` (a cross-group edge's endpoints must belong to the edge's own group).
fn uuids_for(f: &Fixture, group: &str) -> Vec<String> {
    let db = f.state.db.load_full().unwrap();
    let conn = db.connect().unwrap();
    conn.get_entities_by_group_ids(Some(&[group]))
        .unwrap()
        .into_iter()
        .map(|e| e.uuid)
        .collect()
}

/// Minimal params that make `method` succeed (no `group_id`; scenarios add it).
fn params_for(method: &str, uuids: &[String], n: usize) -> Value {
    match method {
        "knowledge_add_episode" => json!({
            "name": format!("ep-{n}"),
            "episode_body": "Alice works at Acme Corp.",
            "source": "test",
            "source_description": "test source",
            "reference_time": "2024-06-01T12:00:00Z",
        }),
        "knowledge_process_chunk" => json!({
            "chunk_text": "Alice works at Acme Corp.",
            "chunk_id": format!("chunk-{n}"),
            "source_file": "test.txt",
            "reference_time": "2024-06-01T12:00:00Z",
        }),
        "knowledge_process_document" => json!({
            "doc_id": format!("doc-{n}"),
            "content": "# Note\n\nAlice works at Acme Corp.\n",
            "reference_time": "2024-06-01T12:00:00Z",
        }),
        "knowledge_assert_entity" => json!({"name": "Carol", "summary": "a person"}),
        "knowledge_assert_relationship" => json!({
            "source_name": "Alice",
            "target_name": "Bob",
            "predicate": "KNOWS",
            "fact": format!("Alice knows Bob {n}"),
        }),
        "knowledge_merge_entities" => json!({
            "canonical_name": "Alice",
            "merge_all_by_name": true,
            "dry_run": true,
        }),
        "knowledge_add_cross_group_edge" => json!({
            "name": "KNOWS",
            "source": {"uuid": uuids[0]},
            "target": {"uuid": uuids[1]},
            "fact": format!("cross edge {n}"),
        }),
        "knowledge_reprocess_entity_types" | "knowledge_reprocess_relation_types" => {
            json!({"dry_run": true})
        }
        other => panic!("no params for {other}"),
    }
}

fn with_group(mut params: Value, group: Option<Value>) -> Value {
    if let Some(g) = group {
        params["group_id"] = g;
    }
    params
}

fn warnings_of(v: &Value) -> Vec<String> {
    v["result"]["warnings"]
        .as_array()
        .map(|a| a.iter().map(|w| w.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn every_defaulting_writer_warns_when_group_id_is_omitted_null_or_non_string() {
    let f = fixture();
    seed(&f);
    let mut n = 0;
    for method in DEFAULTING_WRITERS {
        for group in [None, Some(Value::Null), Some(json!(7))] {
            n += 1;
            let params = with_group(
                params_for(method, &uuids_for(&f, "liminis"), n),
                group.clone(),
            );
            let v = call(&f, None, method, params).await;
            assert!(v["error"].is_null(), "{method} {group:?}: {v}");
            assert_ne!(v["result"]["success"], json!(false), "{method}: {v}");
            assert_eq!(
                warnings_of(&v),
                vec![IMPLICIT_DEFAULT_GROUP_WARNING.to_string()],
                "{method} {group:?}: {v}"
            );
        }
    }
}

#[tokio::test]
async fn explicit_group_never_warns_including_liminis_and_empty() {
    let f = fixture();
    seed(&f);
    let mut n = 100;
    for method in DEFAULTING_WRITERS {
        for g in ["other", "liminis"] {
            n += 1;
            let params = with_group(params_for(method, &uuids_for(&f, g), n), Some(json!(g)));
            let v = call(&f, None, method, params).await;
            assert!(v["error"].is_null(), "{method} {g}: {v}");
            assert!(
                v["result"].get("warnings").is_none(),
                "{method} {g} must not warn: {v}"
            );
        }
        // `""` is a distinct group, not the default: no default-group warning either.
        n += 1;
        let params = with_group(
            params_for(method, &uuids_for(&f, "other"), n),
            Some(json!("")),
        );
        let v = call(&f, None, method, params).await;
        assert!(
            !warnings_of(&v).contains(&IMPLICIT_DEFAULT_GROUP_WARNING.to_string()),
            "{method} \"\": {v}"
        );
    }
}

#[tokio::test]
async fn warning_set_matches_the_group_param_classification() {
    let mut classified: Vec<&str> = write_policy::CLASSIFICATION
        .iter()
        .filter(|(_, c)| *c == WriteClass::GroupParam)
        .map(|(m, _)| *m)
        .collect();
    let mut tested: Vec<&str> = DEFAULTING_WRITERS.to_vec();
    classified.sort_unstable();
    tested.sort_unstable();
    assert_eq!(
        classified, tested,
        "every defaulting writer needs a warning test (and vice versa)"
    );
}

#[tokio::test]
async fn process_chunk_without_group_writes_to_liminis_and_warns() {
    let f = fixture();
    let v = call(
        &f,
        None,
        "knowledge_process_chunk",
        json!({
            "chunk_text": "Alice works at Acme Corp.",
            "chunk_id": "c-default",
            "source_file": "t.txt",
            "reference_time": "2024-06-01T12:00:00Z",
        }),
    )
    .await;
    assert_eq!(v["result"]["success"], json!(true), "{v}");
    assert_eq!(
        warnings_of(&v),
        vec![IMPLICIT_DEFAULT_GROUP_WARNING.to_string()]
    );
    let db = f.state.db.load_full().unwrap();
    let conn = db.connect().unwrap();
    assert!(
        !conn
            .get_entities_by_group_ids(Some(&["liminis"]))
            .unwrap()
            .is_empty(),
        "the data must still land in the default group"
    );
}

#[tokio::test]
async fn refused_implicit_default_write_returns_the_refusal_not_a_warning() {
    let f = fixture();
    seed(&f);
    let p = policy("other");
    for method in DEFAULTING_WRITERS {
        let v = call(
            &f,
            Some(&p),
            method,
            params_for(method, &uuids_for(&f, "liminis"), 900),
        )
        .await;
        assert_eq!(v["error"]["code"], json!(-32004), "{method}: {v}");
        assert!(v.get("result").is_none() || v["result"].is_null(), "{v}");
    }
}

#[tokio::test]
async fn soft_failure_and_error_responses_carry_no_warning() {
    let f = fixture();
    // Bad scope: `Ok({"success": false})` soft failure.
    let v = call(
        &f,
        None,
        "knowledge_reprocess_entity_types",
        json!({"scope": "bogus"}),
    )
    .await;
    assert_eq!(v["result"]["success"], json!(false), "{v}");
    assert!(v["result"].get("warnings").is_none(), "{v}");
    // Hard error (missing required param).
    let v = call(&f, None, "knowledge_process_chunk", json!({})).await;
    assert!(!v["error"].is_null(), "{v}");
    assert!(v.get("result").is_none() || v["result"].is_null(), "{v}");
}

#[tokio::test]
async fn non_defaulting_methods_never_gain_a_warnings_key() {
    let f = fixture();
    seed(&f);
    for (method, params) in [
        ("knowledge_status", json!({})),
        ("knowledge_find_entities", json!({"query": "Alice"})),
        ("knowledge_walk", json!({"start": {"name": "Alice"}})),
        ("knowledge_get_ontology", json!({})),
    ] {
        let v = call(&f, None, method, params).await;
        assert!(v["result"].get("warnings").is_none(), "{method}: {v}");
    }
}

// ── docs table vs dispatch classification (SC-003, FR-013) ────────────────────────────────────

const REFERENCE: &str = include_str!("../../../docs/ipc-mcp-reference.md");
const CLASSES: &[&str] = &[
    "default group + warning",
    "default group",
    "every group",
    "refused",
    "not applicable",
];

/// Rows of the "Omitted-group behaviour per method" table: `(method, class)`.
fn documented_rows() -> Vec<(String, String)> {
    let start = REFERENCE
        .find("### Omitted-group behaviour per method")
        .expect("omitted-group section missing from docs/ipc-mcp-reference.md");
    let section = &REFERENCE[start..];
    let end = section[4..].find("\n### ").map_or(section.len(), |i| i + 4);
    section[..end]
        .lines()
        .filter_map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            let method = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
            Some((method.to_string(), cells.get(2)?.to_string()))
        })
        .collect()
}

#[test]
fn docs_table_covers_every_dispatched_method_exactly_once() {
    let rows = documented_rows();
    let mut documented: Vec<&str> = rows.iter().map(|(m, _)| m.as_str()).collect();
    documented.sort_unstable();
    let mut dup = documented.clone();
    dup.dedup();
    assert_eq!(dup.len(), documented.len(), "a method is documented twice");
    let mut classified: Vec<&str> = write_policy::CLASSIFICATION
        .iter()
        .map(|(m, _)| *m)
        .collect();
    classified.sort_unstable();
    assert_eq!(
        documented, classified,
        "docs/ipc-mcp-reference.md omitted-group table must list every classified method, no more"
    );
    for (m, c) in &rows {
        assert!(CLASSES.contains(&c.as_str()), "{m}: unknown class {c:?}");
    }
}

#[test]
fn docs_warns_rows_equal_the_defaulting_writers() {
    let mut warns: Vec<String> = documented_rows()
        .into_iter()
        .filter(|(_, c)| c == "default group + warning")
        .map(|(m, _)| m)
        .collect();
    warns.sort_unstable();
    let mut expected: Vec<String> = DEFAULTING_WRITERS.iter().map(|m| m.to_string()).collect();
    expected.sort_unstable();
    assert_eq!(warns, expected);
}
