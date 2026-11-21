// Engine-level tests for the built-in test providers (issue #851): the real ingest, resolution,
// search and lint paths running against `TestEmbedder` + `TestExtractor` with no network.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    episode, handlers,
    ipc::IpcRequest,
    ontology::{Ontology, OntologyMode, RelationTypeDef},
    telemetry::{NoopSink, TelemetrySink},
    types::SourceType,
    TestEmbedder, TestExtractor, TEST_EMBEDDER_MODEL, TEST_EMBEDDING_DEFAULT_DIM,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

fn make_state(ontology: Option<Ontology>) -> (Arc<AppState>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(TEST_EMBEDDING_DEFAULT_DIM).unwrap();
    }
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(TestEmbedder::default()),
        extractor: Arc::new(TestExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: None,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: TEST_EMBEDDER_MODEL.to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: ontology.map(Arc::new),
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    });
    (state, dir)
}

async fn ingest(state: &Arc<AppState>, name: &str, body: &str) {
    episode::add_episode(
        Arc::clone(state),
        name,
        body,
        "test",
        "test source",
        "2026-01-01T00:00:00Z",
        "g",
        SourceType::Text,
        None,
        "",
    )
    .await
    .unwrap();
}

async fn call(state: &Arc<AppState>, method: &str, params: Value) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    let resp = handlers::dispatch(req, Arc::clone(state), None).await;
    let v = serde_json::to_value(resp).unwrap();
    assert!(v.get("error").is_none(), "{method} errored: {v}");
    v["result"].clone()
}

async fn entity_names(state: &Arc<AppState>) -> Vec<String> {
    let r = call(
        state,
        "knowledge_get_nodes_by_group",
        json!({"group_ids": ["g"]}),
    )
    .await;
    let mut n: Vec<String> = r["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    n.sort();
    n
}

async fn edges(state: &Arc<AppState>) -> Vec<Value> {
    let r = call(
        state,
        "knowledge_get_edges_by_group",
        json!({"group_ids": ["g"]}),
    )
    .await;
    r["edges"].as_array().unwrap().clone()
}

#[tokio::test]
async fn fact_lines_produce_expected_entities_and_edges() {
    let (state, _d) = make_state(None);
    ingest(
        &state,
        "doc",
        "# Notes\nsome prose\nAlice -> Acme: Alice works at Acme\nWiki -[HOSTED_ON]-> ServerA: Wiki runs on ServerA\n",
    )
    .await;
    assert_eq!(
        entity_names(&state).await,
        ["Acme", "Alice", "ServerA", "Wiki"]
    );
    let es = edges(&state).await;
    assert_eq!(es.len(), 2);
    let by_fact = |f: &str| es.iter().find(|e| e["fact"] == f).unwrap().clone();
    assert_eq!(
        by_fact("Alice works at Acme")["relation_type"],
        "RELATED_TO"
    );
    assert_eq!(
        by_fact("Wiki runs on ServerA")["relation_type"],
        "HOSTED_ON"
    );
}

#[tokio::test]
async fn chunk_without_fact_lines_extracts_nothing() {
    let (state, _d) = make_state(None);
    ingest(&state, "prose", "Just prose.\nNo facts here: honest.").await;
    assert!(entity_names(&state).await.is_empty());
    assert!(edges(&state).await.is_empty());
}

#[tokio::test]
async fn same_fact_in_two_documents_resolves_to_same_entities() {
    let (state, _d) = make_state(None);
    ingest(&state, "a", "Alice -> Acme: Alice works at Acme").await;
    ingest(&state, "b", "Alice -> Acme: Alice works at Acme").await;
    assert_eq!(entity_names(&state).await, ["Acme", "Alice"]);
}

#[tokio::test]
async fn unrelated_entities_are_never_merged() {
    let (state, _d) = make_state(None);
    let lines = [
        ("Server A", "Server B"),
        ("Alice Smith", "Alice Jones"),
        ("Project Alpha", "Project Beta"),
        ("Zorblax", "Quimby"),
        ("Wiki", "Blog"),
    ];
    for (i, (s, d)) in lines.iter().enumerate() {
        ingest(
            &state,
            &format!("d{i}"),
            &format!("{s} -> {d}: {s} relates to {d}"),
        )
        .await;
    }
    let mut expected: Vec<String> = lines
        .iter()
        .flat_map(|(s, d)| [s.to_string(), d.to_string()])
        .collect();
    expected.sort();
    assert_eq!(entity_names(&state).await, expected);
}

#[tokio::test]
async fn strict_ontology_reclassifies_unknown_relation() {
    let ontology = Ontology {
        mode: OntologyMode::Strict,
        entity_types: vec![],
        relation_types: vec![RelationTypeDef {
            name: "WORKS_AT".to_string(),
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
    };
    let (state, _d) = make_state(Some(ontology));
    ingest(
        &state,
        "a",
        "Wiki -[HOSTED_ON]-> ServerA: Wiki runs on ServerA",
    )
    .await;
    let es = edges(&state).await;
    assert_eq!(es.len(), 1);
    assert_eq!(es[0]["relation_type"], "UNCLASSIFIED");
}

#[tokio::test]
async fn single_valued_relation_lint_flags_two_destinations() {
    let (state, _d) = make_state(None);
    ingest(
        &state,
        "a",
        "Wiki -[HOSTED_ON]-> ServerA: Wiki runs on ServerA",
    )
    .await;
    ingest(
        &state,
        "b",
        "Wiki -[HOSTED_ON]-> ServerB: Wiki runs on ServerB",
    )
    .await;
    let r = call(
        &state,
        "knowledge_lint",
        json!({"categories": ["contradiction"], "single_valued_relations": ["HOSTED_ON"]}),
    )
    .await;
    let findings = r.to_string();
    assert!(
        findings.contains("ServerA") && findings.contains("ServerB"),
        "expected a contradiction citing both servers: {r}"
    );
}

#[tokio::test]
async fn search_ranks_word_overlap_first() {
    let (state, _d) = make_state(None);
    ingest(
        &state,
        "a",
        "Alice -> Acme: Alice works at Acme\nBob -> Cello: Bob plays the cello",
    )
    .await;
    call(&state, "knowledge_build_indices", json!({})).await;
    let r = call(
        &state,
        "knowledge_find_relationships",
        json!({"query": "Alice works Acme", "num_results": 5}),
    )
    .await;
    let s = r.to_string();
    assert!(s.contains("Alice works at Acme"), "{r}");
}

#[tokio::test]
async fn status_and_health_report_test_providers() {
    let (state, _d) = make_state(None);
    for m in ["knowledge_status", "health_check"] {
        let r = call(&state, m, json!({})).await;
        assert_eq!(
            r["providers"],
            json!({"embedder": "test", "extractor": "test"}),
            "{m}: {r}"
        );
    }
    let st = call(&state, "knowledge_status", json!({})).await;
    assert_eq!(st["embedding_model"], TEST_EMBEDDER_MODEL);
}
