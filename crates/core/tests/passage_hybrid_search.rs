//! Issue #789 / ADR-0789: hybrid BM25 + vector passage search with RRF and a top-rank bonus.
//!
//! Episodes are inserted directly with chosen one-hot embeddings and `NameMapEmbedder` maps each
//! query to a chosen vector, so the vector ranking is fully determined. Lexical ranking comes
//! from the real `episode_content` FTS index.

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
    types::EpisodicRow,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "g1";
const TS: &str = "2026-08-04 00:00:00";

fn one_hot(axis: usize) -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[axis] = 1.0;
    v
}

struct Ep {
    uuid: &'static str,
    group: &'static str,
    origin: Option<&'static str>,
    content: &'static str,
    axis: usize,
}

fn ep(uuid: &'static str, content: &'static str, axis: usize) -> Ep {
    Ep {
        uuid,
        group: G,
        origin: None,
        content,
        axis,
    }
}

fn make_state(embedder: Arc<dyn Embedder>, eps: &[Ep]) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        for e in eps {
            conn.insert_episodic(&EpisodicRow {
                uuid: e.uuid.to_string(),
                name: e.uuid.to_string(),
                group_id: e.group.to_string(),
                created_at: TS.to_string(),
                source: "text".to_string(),
                source_description: "note".to_string(),
                content: e.content.to_string(),
                content_embedding: one_hot(e.axis),
                valid_at: TS.to_string(),
                entity_edges: vec![],
                attributes: "{}".to_string(),
                origin: e.origin.map(str::to_string),
                ..Default::default()
            })
            .unwrap();
        }
        conn.build_indices_and_constraints().unwrap();
    }
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(Arc::clone(&db))),
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
    });
    (state, db, dir)
}

fn embedder(queries: &[(&str, Vec<f32>)]) -> Arc<dyn Embedder> {
    let map: HashMap<String, Vec<f32>> = queries
        .iter()
        .map(|(q, v)| (q.to_string(), v.clone()))
        .collect();
    Arc::new(NameMapEmbedder::new(DIM, map))
}

async fn try_search(state: &Arc<AppState>, params: Value) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: "knowledge_search_passages".to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, Arc::clone(state), None).await).unwrap()
}

async fn search(state: &Arc<AppState>, params: Value) -> Vec<Value> {
    let v = try_search(state, params).await;
    assert!(v.get("error").is_none(), "search errored: {v}");
    v["result"]["passages"].as_array().unwrap().clone()
}

fn uuids(ps: &[Value]) -> Vec<String> {
    ps.iter()
        .map(|p| p["uuid"].as_str().unwrap().to_string())
        .collect()
}

fn params(query: &str, mode: &str) -> Value {
    json!({"query": query, "group_ids": [G], "min_score": 0.0, "num_results": 10, "mode": mode})
}

/// Query vector: closest to axis 0, then 1, then 2, then 3 (all non-zero so every episode is a
/// vector candidate).
fn graded_query() -> Vec<f32> {
    vec![1.0, 0.6, 0.3, 0.1]
}

#[tokio::test]
async fn exact_term_note_ranks_higher_in_hybrid_than_vector() {
    // `far` is the semantically most distant episode but is the only one containing the
    // distinctive term.
    let eps = [
        ep("near0", "unrelated gardening notes", 0),
        ep("near1", "more gardening and weather", 1),
        ep("near2", "another note about cooking", 2),
        ep("far", "the quokkaplex migration runbook", 3),
    ];
    let (state, _db, _d) = make_state(embedder(&[("quokkaplex", graded_query())]), &eps);
    let vector = uuids(&search(&state, params("quokkaplex", "vector")).await);
    assert_eq!(vector, vec!["near0", "near1", "near2", "far"]);
    let hybrid = uuids(&search(&state, params("quokkaplex", "hybrid")).await);
    assert_eq!(hybrid.len(), 4, "semantic-only matches are still returned");
    let pos = |v: &[String], u: &str| v.iter().position(|x| x == u).unwrap();
    assert!(pos(&hybrid, "far") < pos(&vector, "far"), "{hybrid:?}");
    assert_eq!(hybrid[0], "far", "rank-1 lexical hit leads: {hybrid:?}");
}

#[tokio::test]
async fn no_lexical_match_equals_vector_ordering() {
    let eps = [
        ep("a", "alpha text", 0),
        ep("b", "beta text", 1),
        ep("c", "gamma text", 2),
    ];
    let (state, _db, _d) = make_state(embedder(&[("zzzunmatched", graded_query())]), &eps);
    let vector = uuids(&search(&state, params("zzzunmatched", "vector")).await);
    let hybrid = uuids(&search(&state, params("zzzunmatched", "hybrid")).await);
    assert_eq!(hybrid, vector);
    assert!(!hybrid.is_empty());
}

#[tokio::test]
async fn punctuation_and_operator_queries_never_error() {
    let eps = [ep("a", "alpha text", 0), ep("b", "beta text", 1)];
    let q = graded_query();
    let queries = [
        "?!...",
        "a OR",
        "*",
        "\"unterminated",
        "title:alpha",
        "(((",
        "the",
    ];
    let pairs: Vec<(&str, Vec<f32>)> = queries.iter().map(|s| (*s, q.clone())).collect();
    let (state, _db, _d) = make_state(embedder(&pairs), &eps);
    for query in queries {
        let hybrid = search(&state, params(query, "hybrid")).await;
        let vector = search(&state, params(query, "vector")).await;
        assert_eq!(uuids(&hybrid), uuids(&vector), "query {query:?}");
    }
}

#[tokio::test]
async fn vector_mode_is_unchanged_and_exposes_no_similarity_field() {
    let eps = [ep("a", "alpha text", 0), ep("b", "beta text", 1)];
    let (state, _db, _d) = make_state(embedder(&[("alpha", graded_query())]), &eps);
    let vector = search(&state, params("alpha", "vector")).await;
    // Cosine scores, no `similarity` key.
    assert!(vector[0].get("similarity").is_none());
    let s0 = vector[0]["score"].as_f64().unwrap();
    assert!(s0 > 0.5 && s0 <= 1.0, "cosine score expected, got {s0}");
    // Omitting mode uses the default, which is vector unless LCG_PASSAGE_SEARCH_MODE is set.
    let mut p = params("alpha", "vector");
    p.as_object_mut().unwrap().remove("mode");
    let default = search(&state, p).await;
    assert_eq!(default, vector);
}

#[tokio::test]
async fn hybrid_result_carries_fused_score_and_similarity() {
    let eps = [ep("a", "alpha text", 0), ep("lex", "needleword here", 3)];
    let (state, _db, _d) = make_state(embedder(&[("needleword", one_hot(0))]), &eps);
    let hybrid = search(&state, params("needleword", "hybrid")).await;
    let by: HashMap<&str, &Value> = hybrid
        .iter()
        .map(|p| (p["uuid"].as_str().unwrap(), p))
        .collect();
    // Vector matched `a` (cosine 1.0); `lex` has orthogonal embedding (cosine 0.0 → still a
    // vector candidate at min_score 0) .
    assert!((by["a"]["similarity"].as_f64().unwrap() - 1.0).abs() < 1e-6);
    assert!(
        by["a"]["score"].as_f64().unwrap() < 0.2,
        "fused, not cosine"
    );
}

#[tokio::test]
async fn min_score_floors_vector_side_only_and_admits_lexical_only_hits() {
    let eps = [
        ep("near", "alpha text", 0),
        ep("far", "needleword content", 3),
    ];
    let (state, _db, _d) = make_state(embedder(&[("needleword", one_hot(0))]), &eps);
    let mut p = params("needleword", "hybrid");
    p["min_score"] = json!(0.5);
    let hybrid = search(&state, p.clone()).await;
    let ids = uuids(&hybrid);
    assert!(ids.contains(&"near".to_string()), "{ids:?}");
    let far = hybrid.iter().find(|x| x["uuid"] == "far").unwrap();
    assert!(far.get("similarity").is_none(), "lexical-only: no cosine");
    // Vector mode with the same floor excludes it.
    p["mode"] = json!("vector");
    assert_eq!(uuids(&search(&state, p).await), vec!["near"]);
}

#[tokio::test]
async fn origins_and_group_filters_apply_to_the_lexical_path() {
    let mut derived = ep("derived-lex", "needleword derived page", 1);
    derived.origin = Some("derived");
    let mut other_group = ep("other-group", "needleword elsewhere", 2);
    other_group.group = "g2";
    let eps = [
        ep("primary-lex", "needleword primary note", 0),
        derived,
        other_group,
    ];
    let (state, _db, _d) = make_state(embedder(&[("needleword", one_hot(0))]), &eps);
    let mut p = params("needleword", "hybrid");
    p["origins"] = json!(["primary"]);
    let ids = uuids(&search(&state, p).await);
    assert_eq!(ids, vec!["primary-lex"], "origin filter");
    // Group filter: only g1 episodes, whatever their origin.
    let ids = uuids(&search(&state, params("needleword", "hybrid")).await);
    assert!(!ids.contains(&"other-group".to_string()), "{ids:?}");
    assert!(ids.contains(&"derived-lex".to_string()));
    // Other group alone.
    let mut p = params("needleword", "hybrid");
    p["group_ids"] = json!(["g2"]);
    assert_eq!(uuids(&search(&state, p).await), vec!["other-group"]);
}

#[tokio::test]
async fn unknown_or_ill_typed_mode_is_rejected_and_empty_origins_still_rejected() {
    let (state, _db, _d) = make_state(embedder(&[]), &[ep("a", "alpha", 0)]);
    for bad in [json!("bm25"), json!("Hybrid"), json!(3), json!(true)] {
        let mut p = params("alpha", "vector");
        p["mode"] = bad.clone();
        let v = try_search(&state, p).await;
        assert!(v.get("error").is_some(), "mode {bad} accepted: {v}");
    }
    let mut p = params("alpha", "hybrid");
    p["top_rank_bonus"] = json!("yes");
    assert!(try_search(&state, p).await.get("error").is_some());
    let mut p = params("alpha", "hybrid");
    p["origins"] = json!([]);
    assert!(try_search(&state, p).await.get("error").is_some());
}

#[tokio::test]
async fn results_are_deduplicated_and_deterministic() {
    let eps = [
        ep("a", "shared term one", 0),
        ep("b", "shared term two", 1),
        ep("c", "shared term three", 2),
    ];
    let (state, _db, _d) = make_state(embedder(&[("shared", graded_query())]), &eps);
    let first = uuids(&search(&state, params("shared", "hybrid")).await);
    for _ in 0..3 {
        assert_eq!(
            uuids(&search(&state, params("shared", "hybrid")).await),
            first
        );
    }
    let mut dedup = first.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), first.len(), "no duplicates: {first:?}");
    assert_eq!(first.len(), 3);
}

#[tokio::test]
async fn num_results_truncates_hybrid() {
    let eps = [
        ep("a", "shared term one", 0),
        ep("b", "shared term two", 1),
        ep("c", "shared term three", 2),
    ];
    let (state, _db, _d) = make_state(embedder(&[("shared", graded_query())]), &eps);
    let mut p = params("shared", "hybrid");
    p["num_results"] = json!(2);
    assert_eq!(search(&state, p).await.len(), 2);
}

#[tokio::test]
async fn top_rank_bonus_is_switchable() {
    // `deep` ranks 3rd lexically (shorter docs score higher) and 1st by vector; construct so
    // bonus on/off produce valid, distinct scores for the same passage.
    let eps = [
        ep("a", "needleword", 1),
        ep("b", "needleword and more words here", 2),
        ep(
            "c",
            "needleword and many more extra words here to dilute",
            0,
        ),
    ];
    let (state, _db, _d) = make_state(embedder(&[("needleword", graded_query())]), &eps);
    let mut on = params("needleword", "hybrid");
    on["top_rank_bonus"] = json!(true);
    let mut off = params("needleword", "hybrid");
    off["top_rank_bonus"] = json!(false);
    let on = search(&state, on).await;
    let off = search(&state, off).await;
    assert_eq!(on.len(), 3);
    assert_eq!(off.len(), 3);
    let top_on = on[0]["score"].as_f64().unwrap();
    let top_off = off[0]["score"].as_f64().unwrap();
    assert!(
        top_on > top_off,
        "bonus raises the top score: {top_on} {top_off}"
    );
}

#[tokio::test]
async fn missing_fts_index_falls_back_to_vector() {
    let eps = [ep("a", "alpha text", 0), ep("b", "beta text", 1)];
    let (state, db, _d) = make_state(embedder(&[("alpha", graded_query())]), &eps);
    {
        let conn = db.connect().unwrap();
        conn.run_cypher("CALL DROP_FTS_INDEX('Episodic', 'episode_content')")
            .unwrap();
    }
    let hybrid = search(&state, params("alpha", "hybrid")).await;
    let vector = search(&state, params("alpha", "vector")).await;
    assert_eq!(uuids(&hybrid), uuids(&vector));
}

#[tokio::test]
async fn lexical_list_is_exposed_as_a_plain_uuid_score_list() {
    let eps = [
        ep("a", "needleword alpha", 0),
        ep("b", "nothing relevant", 1),
    ];
    let (_state, db, _d) = make_state(embedder(&[]), &eps);
    let conn = db.connect().unwrap();
    let list =
        lcg_core::search::lexical_passage_list(&conn, "needleword!", Some(&[G]), None, 10).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].0, "a");
    assert!(list[0].1 > 0.0);
}
