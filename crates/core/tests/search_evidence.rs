//! Per-result search evidence and the `min_similarity` floor (issue #629) on
//! `knowledge_find_entities` and `knowledge_find_relationships`.
//!
//! Uses `NameMapEmbedder` with one-hot item vectors and the query vector `[0.1, 0.3, 0.6, 1.0]`,
//! so each item's cosine similarity to the query is known exactly: `q[i] / |q|` with
//! `|q| = sqrt(1.46)`, i.e. alpha 0.083, bravo 0.248, charlie 0.497, delta 0.828. The query text
//! shares no words with any item, so BM25 contributes nothing unless a test wants it to.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::{Embedder, NameMapEmbedder},
    extractor::ConfigurableExtractor,
    handlers,
    ipc::IpcRequest,
    telemetry::NoopSink,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const GRP: &str = "test-group";
const QUERY: &str = "zulu unrelated query";
/// Query similarity rises with the one-hot axis index, so axis 3 (the last inserted) ranks first.
const QUERY_VECTOR: [f32; DIM] = [0.1, 0.3, 0.6, 1.0];
const EXPECTED: [&str; 4] = ["delta", "charlie", "bravo", "alpha"];

fn one_hot(axis: usize) -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[axis] = 1.0;
    v
}

fn state_with(dir: &TempDir, embedder: Arc<dyn Embedder>) -> Arc<AppState> {
    let db = Arc::new(Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.build_indices_and_constraints().unwrap();
    }
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder,
        extractor: Arc::new(ConfigurableExtractor::new(vec![])),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink: Arc::new(NoopSink),
        db_path: "test.db".to_string(),
        wal_root: None,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built: Arc::new(AtomicBool::new(true)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: None,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

async fn dispatch(method: &str, params: Value, state: Arc<AppState>) -> Value {
    let request = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    let response = serde_json::to_value(handlers::dispatch(request, state, None).await).unwrap();
    assert!(
        response.get("error").is_none(),
        "{method} failed: {}",
        response["error"]
    );
    response["result"].clone()
}

const SIMS: [f64; 4] = [0.0828, 0.2483, 0.4966, 0.8276];

fn assert_close(actual: &Value, expected: f64) {
    let a = actual
        .as_f64()
        .unwrap_or_else(|| panic!("expected number, got {actual}"));
    assert!((a - expected).abs() < 1e-3, "{a} != {expected}");
}

const ENTITIES: [(&str, &str); 4] = [
    ("alpha", "first placeholder note"),
    ("bravo", "second placeholder note"),
    ("charlie", "third placeholder note"),
    ("delta", "fourth placeholder note"),
];

fn entity_vectors() -> HashMap<String, Vec<f32>> {
    let mut vectors = HashMap::new();
    for (axis, (name, summary)) in ENTITIES.iter().enumerate() {
        vectors.insert(name.to_string(), one_hot(axis));
        vectors.insert(summary.to_string(), one_hot(axis));
    }
    vectors.insert(QUERY.to_string(), QUERY_VECTOR.to_vec());
    vectors
}

async fn seed_entities(state: &Arc<AppState>) {
    for (name, summary) in ENTITIES {
        dispatch(
            "knowledge_assert_entity",
            json!({ "name": name, "summary": summary, "group_id": GRP }),
            Arc::clone(state),
        )
        .await;
    }
}

fn names(result: &Value) -> Vec<&str> {
    result["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect()
}

async fn find_entities(state: &Arc<AppState>, extra: Value) -> Value {
    let mut params = json!({ "query": QUERY, "group_ids": [GRP], "num_results": 4 });
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    dispatch("knowledge_find_entities", params, Arc::clone(state)).await
}

#[tokio::test]
async fn find_entities_reports_vector_evidence_with_null_for_absent_paths() {
    let dir = TempDir::new().unwrap();
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, entity_vectors())));
    seed_entities(&state).await;

    let result = find_entities(&state, json!({})).await;
    let nodes = result["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 4);
    for n in nodes {
        let s = &n["search"];
        assert_eq!(s["text_match"], false, "{n}");
        assert!(s["bm25_score"].is_null(), "{n}");
        assert!(s["rrf_score"].as_f64().unwrap() > 0.0, "{n}");
    }
    // Results are delta, charlie, bravo, alpha; every similarity is cosine (1 - distance).
    for (i, n) in nodes.iter().enumerate() {
        let axis = 3 - i;
        assert_close(&n["search"]["name_similarity"], SIMS[axis]);
        assert_close(&n["search"]["summary_similarity"], SIMS[axis]);
    }
    // Row fields stay at the top level next to `search`.
    assert_eq!(nodes[0]["name"], "delta");
    assert!(nodes[0]["uuid"].is_string());
}

#[tokio::test]
async fn find_entities_unset_floor_matches_the_pre_change_order() {
    let dir = TempDir::new().unwrap();
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, entity_vectors())));
    seed_entities(&state).await;

    let unset = find_entities(&state, json!({})).await;
    assert_eq!(names(&unset), EXPECTED);
    // A null floor is the same as no floor, and a floor of 0 keeps every positive similarity.
    let null_floor = find_entities(&state, json!({ "min_similarity": null })).await;
    assert_eq!(names(&null_floor), EXPECTED);
    let zero = find_entities(&state, json!({ "min_similarity": 0.0 })).await;
    assert_eq!(names(&zero), EXPECTED);
    assert_eq!(unset["nodes"], null_floor["nodes"]);
}

#[tokio::test]
async fn find_entities_floor_drops_distant_neighbours() {
    let dir = TempDir::new().unwrap();
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, entity_vectors())));
    seed_entities(&state).await;

    let r = find_entities(&state, json!({ "min_similarity": 0.4 })).await;
    assert_eq!(names(&r), ["delta", "charlie"]);
    let r = find_entities(&state, json!({ "min_similarity": 0.5 })).await;
    assert_eq!(names(&r), ["delta"]);
    let r = find_entities(&state, json!({ "min_similarity": 0.99 })).await;
    assert!(names(&r).is_empty());
    assert_eq!(r["count"], 0);
    // Out-of-range values are clamped, not rejected.
    let r = find_entities(&state, json!({ "min_similarity": 5 })).await;
    assert!(names(&r).is_empty());
    let r = find_entities(&state, json!({ "min_similarity": -5 })).await;
    assert_eq!(names(&r), EXPECTED);
}

#[tokio::test]
async fn find_entities_floor_applies_per_vector_path() {
    // `delta` is near the query by name but its summary points away from it, so the summary
    // path fails the floor while the name path passes.
    let dir = TempDir::new().unwrap();
    let mut vectors = entity_vectors();
    vectors.insert("fourth placeholder note".to_string(), one_hot(0));
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, vectors)));
    seed_entities(&state).await;

    let r = find_entities(&state, json!({ "min_similarity": 0.5 })).await;
    assert_eq!(names(&r), ["delta"]);
    let s = &r["nodes"][0]["search"];
    assert_close(&s["name_similarity"], SIMS[3]);
    assert!(s["summary_similarity"].is_null(), "{s}");
    assert_eq!(s["text_match"], false);
}

#[tokio::test]
async fn find_entities_bm25_hit_stays_eligible_under_the_floor() {
    let dir = TempDir::new().unwrap();
    let mut vectors = entity_vectors();
    // The exact-name query embeds far from every entity (negative similarity everywhere).
    vectors.insert("quokka".to_string(), vec![-1.0; DIM]);
    vectors.insert("quokka station".to_string(), one_hot(0));
    vectors.insert("marsupial outpost".to_string(), one_hot(0));
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, vectors)));
    seed_entities(&state).await;
    dispatch(
        "knowledge_assert_entity",
        json!({ "name": "quokka station", "summary": "marsupial outpost", "group_id": GRP }),
        Arc::clone(&state),
    )
    .await;

    let r = dispatch(
        "knowledge_find_entities",
        json!({ "query": "quokka", "group_ids": [GRP], "num_results": 4, "min_similarity": 0.5 }),
        Arc::clone(&state),
    )
    .await;
    assert_eq!(names(&r), ["quokka station"], "{r}");
    let s = &r["nodes"][0]["search"];
    assert_eq!(s["text_match"], true);
    assert!(s["bm25_score"].as_f64().is_some());
    assert!(s["name_similarity"].is_null(), "{s}");
    assert!(s["summary_similarity"].is_null(), "{s}");

    // Without a floor the same entity also reports its vector evidence (a real, negative cosine).
    let r = dispatch(
        "knowledge_find_entities",
        json!({ "query": "quokka", "group_ids": [GRP], "num_results": 4 }),
        Arc::clone(&state),
    )
    .await;
    let hit = r["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "quokka station")
        .unwrap();
    assert_eq!(hit["search"]["text_match"], true);
    assert_close(&hit["search"]["name_similarity"], -0.5);
}

#[tokio::test]
async fn find_entities_zero_summary_embedding_never_yields_non_finite_evidence() {
    let dir = TempDir::new().unwrap();
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, entity_vectors())));
    // No summary => zero-vector `summary_embedding` placeholder.
    dispatch(
        "knowledge_assert_entity",
        json!({ "name": "delta", "group_id": GRP }),
        Arc::clone(&state),
    )
    .await;

    for extra in [json!({}), json!({ "min_similarity": 0.0 })] {
        let r = find_entities(&state, extra).await;
        let text = serde_json::to_string(&r).unwrap();
        assert!(!text.contains("NaN") && !text.contains("inf"), "{text}");
        for n in r["nodes"].as_array().unwrap() {
            let sim = &n["search"]["summary_similarity"];
            assert!(sim.is_null() || sim.as_f64().unwrap().is_finite(), "{n}");
        }
    }
}

#[tokio::test]
async fn find_entities_kind_path_carries_evidence_and_honours_the_floor() {
    let dir = TempDir::new().unwrap();
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, entity_vectors())));
    for (i, (name, summary)) in ENTITIES.iter().enumerate() {
        let kind = if i >= 2 { "Company" } else { "Topic" };
        dispatch(
            "knowledge_assert_entity",
            json!({ "name": name, "summary": summary, "group_id": GRP, "kind": kind }),
            Arc::clone(&state),
        )
        .await;
    }

    let r = find_entities(&state, json!({ "kind": "Company" })).await;
    assert_eq!(names(&r), ["delta", "charlie"]);
    for n in r["nodes"].as_array().unwrap() {
        assert_eq!(n["kind"], "Company");
        assert!(n["search"]["rrf_score"].as_f64().is_some(), "{n}");
        assert!(n["search"]["name_similarity"].as_f64().is_some(), "{n}");
    }
    let r = find_entities(&state, json!({ "kind": "Company", "min_similarity": 0.6 })).await;
    assert_eq!(names(&r), ["delta"]);
    let r = find_entities(&state, json!({ "kind": "Topic", "min_similarity": 0.6 })).await;
    assert!(names(&r).is_empty());
}

#[tokio::test]
async fn find_entities_rejects_non_numeric_min_similarity() {
    let dir = TempDir::new().unwrap();
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, entity_vectors())));
    for method in ["knowledge_find_entities", "knowledge_find_relationships"] {
        let request = IpcRequest {
            jsonrpc: "2.0".to_string(),
            id: json!(1),
            method: method.to_string(),
            params: json!({ "query": QUERY, "min_similarity": "high" }),
        };
        let response =
            serde_json::to_value(handlers::dispatch(request, Arc::clone(&state), None).await)
                .unwrap();
        assert!(response.get("error").is_some(), "{method}: {response}");
    }
}

const EDGES: [(&str, &str); 4] = [
    ("alpha", "first placeholder statement"),
    ("bravo", "second placeholder statement"),
    ("charlie", "third placeholder statement"),
    ("delta", "fourth placeholder statement"),
];

async fn seed_edges(state: &Arc<AppState>) {
    let endpoints =
        std::iter::once("hub".to_string()).chain(EDGES.iter().map(|(t, _)| format!("{t} node")));
    for name in endpoints {
        dispatch(
            "knowledge_assert_entity",
            json!({ "name": name, "group_id": GRP }),
            Arc::clone(state),
        )
        .await;
    }
    for (target, fact) in EDGES {
        dispatch(
            "knowledge_assert_relationship",
            json!({
                "source_name": "hub",
                "target_name": format!("{target} node"),
                "predicate": "RELATES_TO",
                "fact": fact,
                "group_id": GRP,
            }),
            Arc::clone(state),
        )
        .await;
    }
}

fn edge_vectors() -> HashMap<String, Vec<f32>> {
    let mut vectors = HashMap::new();
    vectors.insert("hub".to_string(), vec![0.5, 0.5, 0.5, 0.5]);
    for (axis, (target, fact)) in EDGES.iter().enumerate() {
        vectors.insert(format!("{target} node"), one_hot(axis));
        vectors.insert(fact.to_string(), one_hot(axis));
    }
    vectors.insert(QUERY.to_string(), QUERY_VECTOR.to_vec());
    vectors
}

fn facts(result: &Value) -> Vec<&str> {
    result["edges"]
        .as_array()
        .expect("edges array")
        .iter()
        .map(|e| e["fact"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn find_relationships_reports_evidence_and_honours_the_floor() {
    let dir = TempDir::new().unwrap();
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, edge_vectors())));
    seed_edges(&state).await;
    let find = |extra: Value| {
        let state = Arc::clone(&state);
        async move {
            let mut params = json!({ "query": QUERY, "group_ids": [GRP], "num_results": 4 });
            for (k, v) in extra.as_object().unwrap() {
                params[k] = v.clone();
            }
            dispatch("knowledge_find_relationships", params, state).await
        }
    };

    let unset = find(json!({})).await;
    let expected: Vec<&str> = EXPECTED
        .iter()
        .map(|n| EDGES.iter().find(|(t, _)| t == n).unwrap().1)
        .collect();
    assert_eq!(facts(&unset), expected);
    for (i, e) in unset["edges"].as_array().unwrap().iter().enumerate() {
        let s = &e["search"];
        assert_eq!(s["text_match"], false);
        assert!(s["bm25_score"].is_null());
        assert!(s["rrf_score"].as_f64().unwrap() > 0.0);
        assert_close(&s["fact_similarity"], SIMS[3 - i]);
    }

    let floored = find(json!({ "min_similarity": 0.5 })).await;
    assert_eq!(facts(&floored), [expected[0]]);
    let none = find(json!({ "min_similarity": 0.99 })).await;
    assert!(facts(&none).is_empty());
    assert_eq!(none["count"], 0);
    let zero = find(json!({ "min_similarity": 0 })).await;
    assert_eq!(facts(&zero), expected);
}

#[tokio::test]
async fn find_relationships_bm25_and_vector_hit_reports_both() {
    let dir = TempDir::new().unwrap();
    let mut vectors = edge_vectors();
    // A query whose text matches `delta`'s fact via BM25 and whose vector is close to it too.
    vectors.insert("fourth placeholder".to_string(), one_hot(3));
    let state = state_with(&dir, Arc::new(NameMapEmbedder::new(DIM, vectors)));
    seed_edges(&state).await;

    let r = dispatch(
        "knowledge_find_relationships",
        json!({ "query": "fourth placeholder", "group_ids": [GRP], "num_results": 4 }),
        Arc::clone(&state),
    )
    .await;
    let top = &r["edges"][0];
    assert_eq!(top["fact"], "fourth placeholder statement");
    let s = &top["search"];
    assert_eq!(s["text_match"], true);
    assert!(s["bm25_score"].as_f64().is_some(), "{s}");
    assert_close(&s["fact_similarity"], 1.0);
    assert!(s["rrf_score"].as_f64().unwrap() > 1.0 / 60.0);
}
