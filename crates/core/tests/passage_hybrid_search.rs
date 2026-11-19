//! Issue #789 / ADR-0789: hybrid BM25 + vector passage search with weighted RRF and belief-aware
//! re-ranking.
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
    doc_id: Option<&'static str>,
    /// `(superseded_at, kind, superseded_by, reason)` for a superseded episode.
    superseded: Option<(&'static str, &'static str, &'static str, &'static str)>,
}

fn ep(uuid: &'static str, content: &'static str, axis: usize) -> Ep {
    Ep {
        uuid,
        group: G,
        origin: None,
        content,
        axis,
        doc_id: None,
        superseded: None,
    }
}

/// A `primary`/`derived` document passage: `doc_id` set (and `origin` as given).
fn doc(
    uuid: &'static str,
    doc_id: &'static str,
    origin: &'static str,
    content: &'static str,
    axis: usize,
) -> Ep {
    let mut e = ep(uuid, content, axis);
    e.doc_id = Some(doc_id);
    e.origin = Some(origin);
    e
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
                doc_id: e.doc_id.map(str::to_string),
                superseded_at: e.superseded.map(|s| s.0.to_string()),
                superseded_by_kind: e.superseded.map(|s| s.1.to_string()),
                superseded_by: e.superseded.map(|s| s.2.to_string()),
                supersession_reason: e.superseded.map(|s| s.3.to_string()),
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
    // Raw vector (belief-aware off, #806): cosine scores, no `similarity` key.
    let vector = search(&state, belief("alpha", "vector", false)).await;
    // Cosine scores, no `similarity` key.
    assert!(vector[0].get("similarity").is_none());
    let s0 = vector[0]["score"].as_f64().unwrap();
    assert!(s0 > 0.5 && s0 <= 1.0, "cosine score expected, got {s0}");
    // Omitting mode uses the default, which is vector unless LCG_PASSAGE_SEARCH_MODE is set.
    let mut p = belief("alpha", "vector", false);
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
    for mode in ["hybrid", "vector"] {
        let mut p = params("alpha", mode);
        p["belief_aware"] = json!("yes");
        assert!(try_search(&state, p).await.get("error").is_some(), "{mode}");
    }
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

/// Hybrid params with the belief rules explicitly on or off.
fn belief(query: &str, mode: &str, on: bool) -> Value {
    let mut p = params(query, mode);
    p["belief_aware"] = json!(on);
    p
}

/// The BM25 list alone, as uuids: the precondition that makes a belief test meaningful.
fn lexical_order(db: &Db, query: &str) -> Vec<String> {
    let conn = db.connect().unwrap();
    lcg_core::search::lexical_passage_list(&conn, query, Some(&[G]), None, 10)
        .unwrap()
        .into_iter()
        .map(|(u, _)| u)
        .collect()
}

/// A restating (`derived`) page that repeats the query's wording, so BM25 puts it first, and the
/// primary note it cites, which is longer and so scores lower lexically.
fn derived_and_source() -> [Ep; 2] {
    [
        doc(
            "page",
            "page",
            "derived",
            "quokkaplex summary of [[src]]",
            0,
        ),
        doc(
            "src",
            "src",
            "primary",
            "the quokkaplex migration plan with many more words about the rollout schedule",
            1,
        ),
    ]
}

#[tokio::test]
async fn hybrid_ranks_a_derived_page_below_the_primary_note_it_cites() {
    let (state, db, _d) = make_state(
        embedder(&[("quokkaplex", graded_query())]),
        &derived_and_source(),
    );
    assert_eq!(lexical_order(&db, "quokkaplex"), vec!["page", "src"]);
    // Without the rule the restating page is first (BM25 and vector both favour it).
    let off = search(&state, belief("quokkaplex", "hybrid", false)).await;
    assert_eq!(uuids(&off), vec!["page", "src"], "precondition");
    // With it, the page falls below its source, and the scores stay in rank order.
    let on = search(&state, belief("quokkaplex", "hybrid", true)).await;
    assert_eq!(uuids(&on), vec!["src", "page"]);
    let scores: Vec<f64> = on.iter().map(|p| p["score"].as_f64().unwrap()).collect();
    assert!(scores[0] > scores[1], "{scores:?}");
    // Hybrid defaults to the rule being on.
    let default = search(&state, params("quokkaplex", "hybrid")).await;
    assert_eq!(uuids(&default), vec!["src", "page"]);
}

/// The pre-#806 default vector output: the untouched raw path, called directly.
async fn raw_vector(state: &Arc<AppState>, query: &str) -> Value {
    let passages = lcg_core::search::search_passages(
        state.db.load_full().unwrap(),
        Arc::clone(&state.embedder),
        query,
        Some(vec![G.to_string()]),
        10,
        0.0,
        None,
    )
    .await
    .unwrap();
    serde_json::to_value(passages).unwrap()
}

/// The pre-#806 hybrid output with belief-aware ranking off, called directly.
async fn raw_hybrid(state: &Arc<AppState>, query: &str) -> Value {
    let passages = lcg_core::search::search_passages_hybrid(
        state.db.load_full().unwrap(),
        Arc::clone(&state.embedder),
        query,
        Some(vec![G.to_string()]),
        10,
        0.0,
        None,
        false,
    )
    .await
    .unwrap();
    serde_json::to_value(passages).unwrap()
}

/// Issue #806: vector mode is belief-aware by default; `belief_aware: false` restores the previous
/// default byte-for-byte.
#[tokio::test]
async fn vector_mode_is_belief_aware_by_default_and_opts_out_with_false() {
    let (state, _db, _d) = make_state(
        embedder(&[("quokkaplex", graded_query())]),
        &derived_and_source(),
    );
    let default = search(&state, params("quokkaplex", "vector")).await;
    let on = search(&state, belief("quokkaplex", "vector", true)).await;
    let off = search(&state, belief("quokkaplex", "vector", false)).await;
    // The derived page now sits below the primary note it cites.
    assert_eq!(uuids(&default), vec!["src", "page"]);
    assert_eq!(default, on, "default equals explicit belief_aware: true");
    assert_eq!(uuids(&off), vec!["page", "src"]);
    assert_ne!(default, off);
    // Opting out is exactly what the pre-#806 default returned (order, ids and scores).
    assert_eq!(Value::Array(off), raw_vector(&state, "quokkaplex").await);
    assert!(default[0].get("similarity").is_none(), "still vector mode");
}

/// A client that sends neither `mode` nor `belief_aware` (SC-004).
#[tokio::test]
async fn no_mode_and_no_belief_aware_equals_explicit_vector_belief() {
    let (state, _db, _d) = make_state(
        embedder(&[("quokkaplex", graded_query())]),
        &derived_and_source(),
    );
    let mut bare = params("quokkaplex", "vector");
    bare.as_object_mut().unwrap().remove("mode");
    let default = search(&state, bare).await;
    let explicit = search(&state, belief("quokkaplex", "vector", true)).await;
    assert_eq!(default, explicit);
    assert_eq!(uuids(&default), vec!["src", "page"]);
}

#[tokio::test]
async fn belief_aware_null_is_treated_as_omitted() {
    let (state, _db, _d) = make_state(
        embedder(&[("quokkaplex", graded_query())]),
        &derived_and_source(),
    );
    for mode in ["vector", "hybrid"] {
        let mut p = params("quokkaplex", mode);
        p["belief_aware"] = Value::Null;
        let with_null = search(&state, p).await;
        let omitted = search(&state, params("quokkaplex", mode)).await;
        assert_eq!(with_null, omitted, "{mode}");
        assert_eq!(uuids(&with_null), vec!["src", "page"], "{mode}");
    }
}

#[tokio::test]
async fn non_boolean_belief_aware_is_still_a_validation_error() {
    let (state, _db, _d) = make_state(
        embedder(&[("quokkaplex", graded_query())]),
        &derived_and_source(),
    );
    let mut p = params("quokkaplex", "vector");
    p["belief_aware"] = json!("yes");
    let v = try_search(&state, p).await;
    assert!(v.get("error").is_some(), "{v}");
}

/// Hybrid was already belief-aware by default (#789); `belief_aware: false` still gives the
/// unchanged weighted-RRF output.
#[tokio::test]
async fn hybrid_belief_aware_false_is_unchanged_weighted_rrf() {
    let (state, _db, _d) = make_state(
        embedder(&[("quokkaplex", graded_query())]),
        &derived_and_source(),
    );
    let off = search(&state, belief("quokkaplex", "hybrid", false)).await;
    assert_eq!(uuids(&off), vec!["page", "src"]);
    assert_eq!(Value::Array(off), raw_hybrid(&state, "quokkaplex").await);
    let default = search(&state, params("quokkaplex", "hybrid")).await;
    let on = search(&state, belief("quokkaplex", "hybrid", true)).await;
    assert_eq!(default, on);
    assert_eq!(uuids(&default), vec!["src", "page"]);
}

/// With nothing to demote, belief-aware and raw vector rankings coincide in ids, order and scores.
#[tokio::test]
async fn without_anchors_belief_aware_and_raw_vector_agree() {
    let eps = [
        ep("a", "alpha text", 0),
        ep("b", "beta text", 1),
        ep("c", "gamma text", 2),
    ];
    let (state, _db, _d) = make_state(embedder(&[("alpha", graded_query())]), &eps);
    let default = search(&state, params("alpha", "vector")).await;
    let off = search(&state, belief("alpha", "vector", false)).await;
    assert_eq!(uuids(&default), vec!["a", "b", "c"]);
    assert_eq!(default, off);
}

#[tokio::test]
async fn hybrid_derived_page_is_kept_when_its_source_was_not_retrieved() {
    // The cited note is in another group, so it is not a candidate: nothing to rank below.
    let mut src = doc("src", "src", "primary", "the quokkaplex plan", 1);
    src.group = "g2";
    let eps = [
        doc(
            "page",
            "page",
            "derived",
            "quokkaplex summary of [[src]]",
            0,
        ),
        src,
    ];
    let (state, _db, _d) = make_state(embedder(&[("quokkaplex", graded_query())]), &eps);
    let on = search(&state, belief("quokkaplex", "hybrid", true)).await;
    assert_eq!(uuids(&on), vec!["page"]);
}

fn stale_and_current(
    superseded: (&'static str, &'static str, &'static str, &'static str),
) -> [Ep; 2] {
    let mut old = doc(
        "old",
        "old",
        "primary",
        "quokkaplex decision: use plan A",
        0,
    );
    old.superseded = Some(superseded);
    [
        old,
        doc(
            "new",
            "new",
            "primary",
            "quokkaplex decision: use plan B after review of the rollout schedule",
            1,
        ),
    ]
}

#[tokio::test]
async fn hybrid_ranks_a_superseded_document_below_the_document_that_replaced_it() {
    let eps = stale_and_current((
        "2026-01-01 00:00:00",
        "document",
        "new",
        "document_superseded",
    ));
    let (state, db, _d) = make_state(embedder(&[("quokkaplex", graded_query())]), &eps);
    assert_eq!(lexical_order(&db, "quokkaplex"), vec!["old", "new"]);
    let off = search(&state, belief("quokkaplex", "hybrid", false)).await;
    assert_eq!(uuids(&off), vec!["old", "new"], "precondition");
    let on = search(&state, belief("quokkaplex", "hybrid", true)).await;
    assert_eq!(uuids(&on), vec!["new", "old"]);
    // The stale passage is demoted, not dropped (superseded episodes are still returned).
    assert_eq!(on.len(), 2);
}

#[tokio::test]
async fn hybrid_ranks_a_revised_episode_below_its_revision() {
    let eps = stale_and_current(("2026-01-01 00:00:00", "episode", "new", "revision"));
    let (state, _db, _d) = make_state(embedder(&[("quokkaplex", graded_query())]), &eps);
    let on = search(&state, belief("quokkaplex", "hybrid", true)).await;
    assert_eq!(uuids(&on), vec!["new", "old"]);
}

#[tokio::test]
async fn a_pending_document_supersession_does_not_demote() {
    // ADR-0707: a future effective instant leaves the document believed until it is reached.
    let eps = stale_and_current((
        "2099-01-01 00:00:00",
        "document",
        "new",
        "document_superseded",
    ));
    let (state, _db, _d) = make_state(embedder(&[("quokkaplex", graded_query())]), &eps);
    let on = search(&state, belief("quokkaplex", "hybrid", true)).await;
    assert_eq!(uuids(&on), vec!["old", "new"]);
}

#[tokio::test]
async fn a_removed_chunk_has_no_successor_and_is_not_demoted() {
    let eps = stale_and_current(("2026-01-01 00:00:00", "", "", "chunk_removed"));
    let (state, _db, _d) = make_state(embedder(&[("quokkaplex", graded_query())]), &eps);
    let on = search(&state, belief("quokkaplex", "hybrid", true)).await;
    assert_eq!(uuids(&on), vec!["old", "new"]);
}

#[tokio::test]
async fn belief_rules_respect_the_origins_and_group_filters() {
    // With `origins: [primary]` the derived page is not a candidate at all, and the primary note
    // still comes back; with the other group selected neither does.
    let (state, _db, _d) = make_state(
        embedder(&[("quokkaplex", graded_query())]),
        &derived_and_source(),
    );
    let mut p = belief("quokkaplex", "hybrid", true);
    p["origins"] = json!(["primary"]);
    assert_eq!(uuids(&search(&state, p).await), vec!["src"]);
    let mut p = belief("quokkaplex", "hybrid", true);
    p["group_ids"] = json!(["g2"]);
    assert!(search(&state, p).await.is_empty());
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

// ── Caller-supplied query variants through the request's pipeline (issue #802, ADR-0802) ──

fn with_variants(mut p: Value, queries: Value) -> Value {
    p["queries"] = queries;
    p
}

fn matched_queries(p: &Value) -> Vec<u64> {
    p["matched_queries"]
        .as_array()
        .unwrap_or_else(|| panic!("no matched_queries on {p}"))
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect()
}

#[tokio::test]
async fn a_variants_keywords_reach_bm25_in_hybrid_mode_but_not_in_vector_mode() {
    // `far` shares no vector direction with any query and no word with the primary query; only
    // the variant's keyword reaches it, and only through the lexical retriever.
    let eps = [
        ep("near", "unrelated words", 0),
        ep("far", "quokkaplex migration", 3),
    ];
    let (state, _db, _d) = make_state(
        embedder(&[("primary question", one_hot(0)), ("quokkaplex", one_hot(2))]),
        &eps,
    );
    let mut base = params("primary question", "hybrid");
    base["min_score"] = json!(0.5);
    assert_eq!(uuids(&search(&state, base.clone()).await), vec!["near"]);

    let hybrid = search(&state, with_variants(base.clone(), json!(["quokkaplex"]))).await;
    assert_eq!(uuids(&hybrid), vec!["near", "far"], "{hybrid:?}");
    assert_eq!(matched_queries(&hybrid[0]), vec![0]);
    assert_eq!(matched_queries(&hybrid[1]), vec![1]);
    // A lexical-only hit carries no cosine, as in single-query hybrid.
    assert!(hybrid[1].get("similarity").is_none(), "{hybrid:?}");
    assert!(hybrid[0]["similarity"].as_f64().unwrap() > 0.99);

    // The same request in vector mode never consults BM25.
    let mut vp = base;
    vp["mode"] = json!("vector");
    let vector = search(&state, with_variants(vp, json!(["quokkaplex"]))).await;
    assert_eq!(uuids(&vector), vec!["near"], "{vector:?}");
}

#[tokio::test]
async fn belief_ranking_is_honoured_once_on_the_fused_list_in_both_modes() {
    // `page` (derived) restates the query and is first for the primary; the variant retrieves its
    // cited source `src` first. Fused without belief the page still leads; with it the page falls
    // below its source, in vector mode and in hybrid mode (both default on since #806).
    let (state, _db, _d) = make_state(
        embedder(&[
            ("quokkaplex", graded_query()),
            ("migration plan", one_hot(1)),
        ]),
        &derived_and_source(),
    );
    for mode in ["vector", "hybrid"] {
        let off = search(
            &state,
            with_variants(belief("quokkaplex", mode, false), json!(["migration plan"])),
        )
        .await;
        assert_eq!(uuids(&off), vec!["page", "src"], "{mode}: precondition");
        let on = search(
            &state,
            with_variants(belief("quokkaplex", mode, true), json!(["migration plan"])),
        )
        .await;
        assert_eq!(uuids(&on), vec!["src", "page"], "{mode}");
        // Scores stay in rank order after the demotion, and the evidence survives it.
        let scores: Vec<f64> = on.iter().map(|p| p["score"].as_f64().unwrap()).collect();
        assert!(scores[0] > scores[1], "{mode}: {scores:?}");
        assert_eq!(matched_queries(&on[0]), vec![0, 1], "{mode}");
    }
    // Both modes default to the rule being on (#806), exactly as without variants.
    let hybrid = search(
        &state,
        with_variants(params("quokkaplex", "hybrid"), json!(["migration plan"])),
    )
    .await;
    assert_eq!(uuids(&hybrid), vec!["src", "page"]);
    let vector = search(
        &state,
        with_variants(params("quokkaplex", "vector"), json!(["migration plan"])),
    )
    .await;
    assert_eq!(uuids(&vector), vec!["src", "page"]);
}

#[tokio::test]
async fn five_agreeing_variants_cannot_outrank_the_primarys_top_hit() {
    // Five variants all put `v` first; the primary puts `p` first. The primary keeps at least
    // two-thirds of the fusion weight, so `p` stays ahead however many variants agree.
    let eps = [ep("p", "alpha", 0), ep("v", "beta", 1)];
    let (state, _db, _d) = make_state(
        embedder(&[
            ("q", one_hot(0)),
            ("v1", one_hot(1)),
            ("v2", one_hot(1)),
            ("v3", one_hot(1)),
            ("v4", one_hot(1)),
            ("v5", one_hot(1)),
        ]),
        &eps,
    );
    for n in 1..=5usize {
        let queries: Vec<String> = (1..=n).map(|i| format!("v{i}")).collect();
        let r = search(&state, with_variants(params("q", "vector"), json!(queries))).await;
        assert_eq!(uuids(&r)[0], "p", "n = {n}: {r:?}");
        // min_score is 0, so `v` is also a (low-ranked) candidate of the primary query.
        assert_eq!(matched_queries(&r[1]), (0..=n as u64).collect::<Vec<_>>());
    }
}
