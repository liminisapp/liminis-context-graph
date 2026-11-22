//! Issue #796: optional rerank and query-expansion stages for `knowledge_search_passages`.
//!
//! Exercises the whole handler through `dispatch` with a mock reranker and mock expander:
//! the default path is unchanged, opt-in per call and via config, N honoured, graceful
//! degradation, belief-aware ordering, and the four rerank × expand combinations.

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
    query_expand::{MockQueryExpander, QueryVariant, VariantKind},
    rerank::MockReranker,
    search_stages::{SearchStages, SearchStagesConfig},
    telemetry::{NoopSink, TelemetrySink},
    ConfigurableExtractor, Db, ExtractedEdge, ExtractedEntity, ExtractionResult, WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "liminis";

fn extraction(i: usize) -> ExtractionResult {
    let (s, t) = (format!("S{i}"), format!("T{i}"));
    ExtractionResult {
        entities: [&s, &t]
            .into_iter()
            .map(|n| ExtractedEntity {
                name: n.clone(),
                entity_type: "Thing".to_string(),
                summary: format!("{n} summary"),
                original_entity_type: None,
            })
            .collect(),
        edges: vec![ExtractedEdge {
            source_name: s,
            target_name: t,
            fact: format!("fact {i}"),
            relation_type: Some("RELATED_TO".to_string()),
            valid_at: None,
            invalid_at: None,
            original_relation_type: None,
        }],
    }
}

struct Fixture {
    state: Arc<AppState>,
    _db_dir: TempDir,
    _wal_dir: TempDir,
}

fn fixture(chunks: usize) -> Fixture {
    let db_dir = TempDir::new().unwrap();
    let wal_dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(db_dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
    }
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let writers: HashMap<String, WalWriter> = WalWriter::new(wal_dir.path(), 10_000, 5 << 20)
        .ok()
        .map(|w| (G.to_string(), w))
        .into_iter()
        .collect();
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor: Arc::new(ConfigurableExtractor::new(
            (0..chunks).map(extraction).collect(),
        )),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(tokio::sync::RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: Some(wal_dir.path().to_path_buf()),
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 << 20,
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
    Fixture {
        state,
        _db_dir: db_dir,
        _wal_dir: wal_dir,
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

async fn put(state: &Arc<AppState>, chunk_id: &str, doc: Option<&str>, ref_time: &str) {
    let mut params = json!({
        "chunk_text": format!("text of {chunk_id}"), "chunk_id": chunk_id,
        "source_file": format!("notes/{chunk_id}.md"), "group_id": G,
        "reference_time": ref_time,
    });
    if let Some(d) = doc {
        params["doc_id"] = json!(d);
    }
    call("knowledge_process_chunk", params, state).await;
}

async fn search(state: &Arc<AppState>, extra: Value) -> Value {
    let mut params =
        json!({"query": "text", "group_ids": [G], "min_score": 0.0, "num_results": 10});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    call("knowledge_search_passages", params, state).await
}

fn names(resp: &Value) -> Vec<String> {
    resp["passages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap_or("").to_string())
        .collect()
}

fn contents(resp: &Value) -> Vec<String> {
    resp["passages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["content"].as_str().unwrap().to_string())
        .collect()
}

/// Logit by chunk name embedded in the passage content ("text of <chunk>").
fn by_chunk(table: &'static [(&'static str, f64)]) -> MockReranker {
    MockReranker::new(move |_q, d| {
        table
            .iter()
            .find(|(c, _)| d.ends_with(&format!("text of {c}")) || d == format!("text of {c}"))
            .map_or(-10.0, |(_, s)| *s)
    })
}

fn install(
    state: &Arc<AppState>,
    config: SearchStagesConfig,
    reranker: Option<Arc<MockReranker>>,
    expander: Option<Arc<MockQueryExpander>>,
) {
    state.set_search_stages(SearchStages::new(
        config,
        reranker.map(|r| r as Arc<dyn lcg_core::rerank::Reranker>),
        expander.map(|e| e as Arc<dyn lcg_core::query_expand::QueryExpander>),
    ));
}

/// The cross-encoder alone decides the order, so these tests pin the stage plumbing rather than
/// the position-aware blend (which has its own tests below and in `passage_rerank`).
fn ce_only() -> SearchStagesConfig {
    SearchStagesConfig {
        blend: lcg_core::passage_rerank::BlendConfig::cross_encoder_only(),
        ..Default::default()
    }
}

const TABLE: &[(&str, f64)] = &[("a", -3.0), ("b", 4.0), ("c", 0.0), ("d", 2.0)];

async fn four(fx: &Fixture) {
    for c in ["a", "b", "c", "d"] {
        put(&fx.state, c, None, "2026-08-04T00:00:00Z").await;
    }
}

#[tokio::test]
async fn default_call_is_unchanged_even_with_stages_configured() {
    let fx = fixture(4);
    four(&fx).await;
    let before = search(&fx.state, json!({})).await;

    let rr = Arc::new(by_chunk(TABLE));
    let ex = Arc::new(MockQueryExpander::failing());
    install(
        &fx.state,
        SearchStagesConfig::default(),
        Some(rr.clone()),
        Some(ex.clone()),
    );
    let after = search(&fx.state, json!({})).await;

    assert_eq!(before, after, "default response must be byte-identical");
    assert_eq!(rr.calls(), 0);
    assert_eq!(ex.calls(), 0);
    let obj = after.as_object().unwrap();
    assert!(obj.contains_key("passages") && obj.contains_key("count"));
    assert!(!obj.contains_key("rerank") && !obj.contains_key("expansion"));
    for p in after["passages"].as_array().unwrap() {
        assert!(
            p.get("rerank_score").is_none() && p.get("belief").is_none(),
            "{p}"
        );
    }
}

#[tokio::test]
async fn rerank_true_reorders_by_the_cross_encoder() {
    let fx = fixture(4);
    four(&fx).await;
    let rr = Arc::new(by_chunk(TABLE));
    install(&fx.state, ce_only(), Some(rr.clone()), None);

    let resp = search(&fx.state, json!({"rerank": true})).await;
    let content = contents(&resp);
    assert_eq!(
        content,
        vec!["text of b", "text of d", "text of c", "text of a"]
    );
    assert_eq!(resp["rerank"]["applied"], json!(true));
    assert_eq!(resp["rerank"]["candidates"], json!(4));
    // Opt-in responses carry the stage's wall-clock time.
    assert!(resp["rerank"]["elapsed_ms"].is_u64(), "{resp}");
    assert_eq!(resp["passages"][0]["rerank_score"], json!(4.0));
    assert_eq!(rr.calls(), 1);
    // Reranked passages carry their belief state when known (live → current).
    assert_eq!(resp["passages"][0]["belief"], json!("current"));
    assert!(resp.get("expansion").is_none());
}

#[tokio::test]
async fn rerank_candidates_bounds_the_head_and_tail_keeps_first_stage_order() {
    let fx = fixture(4);
    four(&fx).await;
    let first_stage = contents(&search(&fx.state, json!({})).await);
    let rr = Arc::new(by_chunk(TABLE));
    install(&fx.state, ce_only(), Some(rr.clone()), None);

    let resp = search(&fx.state, json!({"rerank": true, "rerank_candidates": 2})).await;
    assert_eq!(rr.last_documents(), first_stage[..2].to_vec());
    assert_eq!(resp["rerank"]["candidates"], json!(2));
    let content = contents(&resp);
    assert_eq!(content.len(), 4, "no duplicates, nothing lost: {content:?}");
    // Tail (positions 2..) is exactly the first-stage tail, in order.
    assert_eq!(content[2..], first_stage[2..]);
    // Head is the first-stage head, reordered by score.
    let mut head = content[..2].to_vec();
    head.sort();
    let mut expect = first_stage[..2].to_vec();
    expect.sort();
    assert_eq!(head, expect);

    // k smaller than N truncates the reranked list.
    let resp = search(&fx.state, json!({"rerank": true, "num_results": 1})).await;
    assert_eq!(contents(&resp), vec!["text of b"]);
}

#[tokio::test]
async fn config_default_and_per_call_override_both_ways() {
    let fx = fixture(4);
    four(&fx).await;
    let rr = Arc::new(by_chunk(TABLE));
    install(
        &fx.state,
        SearchStagesConfig {
            rerank_default: true,
            ..Default::default()
        },
        Some(rr.clone()),
        None,
    );
    // Default on: runs without a per-call flag.
    let resp = search(&fx.state, json!({})).await;
    assert_eq!(resp["rerank"]["applied"], json!(true));
    assert_eq!(rr.calls(), 1);
    // Explicit false overrides the config.
    let resp = search(&fx.state, json!({"rerank": false})).await;
    assert!(resp.get("rerank").is_none());
    assert_eq!(rr.calls(), 1);

    // Default off: explicit true runs for that call only.
    install(
        &fx.state,
        SearchStagesConfig::default(),
        Some(rr.clone()),
        None,
    );
    assert!(search(&fx.state, json!({})).await.get("rerank").is_none());
    assert_eq!(
        search(&fx.state, json!({"rerank": true})).await["rerank"]["applied"],
        json!(true)
    );
    assert_eq!(rr.calls(), 2);
}

/// Caller-supplied `queries` (#802) compose with rerank (#796): the user's own query runs the
/// variants pipeline as the first stage, and the cross-encoder then reorders its result.
#[tokio::test]
async fn caller_supplied_queries_compose_with_rerank() {
    let fx = fixture(4);
    four(&fx).await;
    let rr = Arc::new(by_chunk(TABLE));
    install(&fx.state, ce_only(), Some(rr.clone()), None);
    let resp = search(
        &fx.state,
        json!({"rerank": true, "queries": ["alternative phrasing", "another one"]}),
    )
    .await;
    assert_eq!(resp["rerank"]["applied"], json!(true));
    assert_eq!(rr.calls(), 1);
    assert_eq!(
        contents(&resp),
        vec!["text of b", "text of d", "text of c", "text of a"]
    );
    // The first stage ran the variants pipeline: results say which queries retrieved them.
    assert!(
        resp["passages"][0].get("matched_queries").is_some(),
        "{resp}"
    );
}

#[tokio::test]
async fn unreachable_or_unconfigured_reranker_degrades_to_first_stage() {
    let fx = fixture(4);
    four(&fx).await;
    let first_stage = contents(&search(&fx.state, json!({})).await);

    // Nothing configured at all.
    let resp = search(&fx.state, json!({"rerank": true})).await;
    assert_eq!(contents(&resp), first_stage);
    assert_eq!(
        resp["rerank"],
        json!({"applied": false, "candidates": 0, "reason": "not_configured"})
    );

    // Configured but failing.
    install(
        &fx.state,
        SearchStagesConfig::default(),
        Some(Arc::new(MockReranker::failing())),
        None,
    );
    let resp = search(&fx.state, json!({"rerank": true})).await;
    assert_eq!(contents(&resp), first_stage);
    assert_eq!(resp["rerank"]["applied"], json!(false));
    assert_eq!(resp["rerank"]["reason"], json!("unavailable"));
    for p in resp["passages"].as_array().unwrap() {
        assert!(p.get("rerank_score").is_none());
    }
}

#[tokio::test]
async fn no_candidates_does_not_call_the_reranker() {
    let fx = fixture(0);
    let rr = Arc::new(by_chunk(TABLE));
    install(
        &fx.state,
        SearchStagesConfig::default(),
        Some(rr.clone()),
        None,
    );
    let resp = search(&fx.state, json!({"rerank": true})).await;
    assert_eq!(resp["count"], json!(0));
    assert_eq!(resp["rerank"]["applied"], json!(false));
    assert_eq!(resp["rerank"]["reason"], json!("no_candidates"));
    assert_eq!(rr.calls(), 0);
}

#[tokio::test]
async fn superseded_document_cannot_outrank_its_replacement() {
    let fx = fixture(2);
    put(&fx.state, "old", Some("DOC1"), "2026-01-01T00:00:00Z").await;
    put(&fx.state, "new", Some("DOC2"), "2026-06-01T00:00:00Z").await;
    call(
        "knowledge_supersede_document",
        json!({"group_id": G, "superseding_doc_id": "DOC2", "superseded_doc_id": "DOC1",
               "effective_at": "2026-06-01T00:00:00Z"}),
        &fx.state,
    )
    .await;

    // The superseded note matches the query far better than its replacement.
    let rr = Arc::new(by_chunk(&[("old", 9.0), ("new", -6.0)]));
    install(&fx.state, SearchStagesConfig::default(), Some(rr), None);
    let resp = search(&fx.state, json!({"rerank": true})).await;
    assert_eq!(contents(&resp), vec!["text of new", "text of old"]);
    assert_eq!(resp["passages"][0]["belief"], json!("current"));
    assert_eq!(resp["passages"][1]["belief"], json!("superseded"));
    // `score` stays the first-stage cosine; the cross-encoder logit is reported separately.
    assert_eq!(resp["passages"][1]["rerank_score"], json!(9.0));
}

#[tokio::test]
async fn origins_filter_still_applies_with_rerank() {
    let fx = fixture(2);
    put(&fx.state, "a", None, "2026-08-04T00:00:00Z").await;
    let mut params = json!({
        "chunk_text": "text of b", "chunk_id": "b", "source_file": "notes/b.md",
        "group_id": G, "reference_time": "2026-08-04T00:00:00Z", "origin": "derived",
    });
    params["origin"] = json!("derived");
    call("knowledge_process_chunk", params, &fx.state).await;
    install(
        &fx.state,
        SearchStagesConfig::default(),
        Some(Arc::new(by_chunk(TABLE))),
        None,
    );
    let resp = search(&fx.state, json!({"rerank": true, "origins": ["primary"]})).await;
    assert_eq!(contents(&resp), vec!["text of a"]);
}

fn variants() -> Vec<QueryVariant> {
    vec![
        QueryVariant {
            kind: VariantKind::Lexical,
            text: "keywords here".into(),
        },
        QueryVariant {
            kind: VariantKind::Hyde,
            text: "a hypothetical answer".into(),
        },
    ]
}

#[tokio::test]
async fn expansion_alone_fuses_variants_and_never_reranks() {
    let fx = fixture(4);
    four(&fx).await;
    let rr = Arc::new(by_chunk(TABLE));
    let ex = Arc::new(MockQueryExpander::with_variants(variants()));
    install(
        &fx.state,
        SearchStagesConfig::default(),
        Some(rr.clone()),
        Some(ex.clone()),
    );

    let resp = search(&fx.state, json!({"expand": true})).await;
    assert_eq!(resp["expansion"]["applied"], json!(true));
    assert_eq!(resp["expansion"]["variants"], json!(2));
    assert!(resp["expansion"]["elapsed_ms"].is_u64(), "{resp}");
    assert!(resp.get("rerank").is_none());
    assert_eq!(ex.calls(), 1);
    assert_eq!(rr.calls(), 0);
    // Fusion must not duplicate a passage that several variant lists return. (The mock embedder
    // gives every chunk one vector, so each list holds the same four documents; this pins
    // de-duplication by uuid rather than distinct-variant retrieval.)
    let ids: std::collections::HashSet<&str> = resp["passages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["uuid"].as_str().expect("passage uuid"))
        .collect();
    assert_eq!(ids.len(), 4, "fused list has no duplicate uuids: {resp}");
    assert_eq!(resp["passages"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn all_four_rerank_expand_combinations_run_exactly_the_requested_stages() {
    let fx = fixture(4);
    four(&fx).await;
    let rr = Arc::new(by_chunk(TABLE));
    let ex = Arc::new(MockQueryExpander::with_variants(variants()));
    install(&fx.state, ce_only(), Some(rr.clone()), Some(ex.clone()));

    for (rerank, expand) in [(false, false), (true, false), (false, true), (true, true)] {
        let (rc, ec) = (rr.calls(), ex.calls());
        let resp = search(&fx.state, json!({"rerank": rerank, "expand": expand})).await;
        assert_eq!(
            rr.calls() - rc,
            usize::from(rerank),
            "rerank={rerank} expand={expand}"
        );
        assert_eq!(
            ex.calls() - ec,
            usize::from(expand),
            "rerank={rerank} expand={expand}"
        );
        assert_eq!(resp.get("rerank").is_some(), rerank);
        assert_eq!(resp.get("expansion").is_some(), expand);
    }
    // Expansion + rerank: fusion first, then the cross-encoder decides the final order.
    let resp = search(&fx.state, json!({"rerank": true, "expand": true})).await;
    assert_eq!(contents(&resp)[0], "text of b");
    assert_eq!(resp["rerank"]["applied"], json!(true));
    assert_eq!(resp["expansion"]["applied"], json!(true));
}

#[tokio::test]
async fn unavailable_expander_falls_back_to_the_original_query() {
    let fx = fixture(4);
    four(&fx).await;
    let plain = contents(&search(&fx.state, json!({})).await);

    // Not configured.
    let resp = search(&fx.state, json!({"expand": true})).await;
    assert_eq!(contents(&resp), plain);
    assert_eq!(resp["expansion"]["applied"], json!(false));
    assert_eq!(resp["expansion"]["reason"], json!("not_configured"));

    // Failing, and returning nothing usable.
    for (ex, reason) in [
        (MockQueryExpander::failing(), "unavailable"),
        (MockQueryExpander::with_variants(vec![]), "no_variants"),
    ] {
        install(
            &fx.state,
            SearchStagesConfig::default(),
            None,
            Some(Arc::new(ex)),
        );
        let resp = search(&fx.state, json!({"expand": true})).await;
        assert_eq!(contents(&resp), plain);
        assert_eq!(resp["expansion"]["reason"], json!(reason));
    }
}

#[tokio::test]
async fn non_boolean_flags_are_rejected() {
    let fx = fixture(0);
    for key in ["rerank", "expand"] {
        let v = try_call(
            "knowledge_search_passages",
            json!({"query": "q", key: "yes"}),
            &fx.state,
        )
        .await;
        assert!(v.get("error").is_some(), "{key}: {v}");
    }
}

#[tokio::test]
async fn non_integer_or_null_rerank_candidates_is_rejected_but_out_of_range_is_clamped() {
    let fx = fixture(0);
    for bad in [json!("10"), json!(-5), json!(2.5), json!(null)] {
        let v = try_call(
            "knowledge_search_passages",
            json!({"query": "q", "rerank": true, "rerank_candidates": bad}),
            &fx.state,
        )
        .await;
        assert!(v.get("error").is_some(), "{bad}: {v}");
    }
    for ok in [json!(0), json!(9999)] {
        let v = try_call(
            "knowledge_search_passages",
            json!({"query": "q", "rerank": true, "rerank_candidates": ok}),
            &fx.state,
        )
        .await;
        assert!(v.get("error").is_none(), "{ok}: {v}");
    }
}

#[tokio::test]
async fn staged_search_self_heals_missing_indices_even_when_concurrent_searches_race() {
    // The original query and every expansion variant run concurrently; each fails with a
    // missing-index error. Whichever builds the indices first, none of them may give up
    // because it saw the flag already set: the search must succeed after the build.
    let fx = fixture(4);
    four(&fx).await;
    {
        let db = fx.state.db.load_full().unwrap();
        db.connect().unwrap().drop_vector_indexes();
    }
    install(
        &fx.state,
        SearchStagesConfig::default(),
        None,
        Some(Arc::new(MockQueryExpander::with_variants(variants()))),
    );
    let resp = search(&fx.state, json!({"expand": true})).await;
    assert_eq!(resp["expansion"]["applied"], json!(true), "{resp}");
    assert_eq!(names(&resp).len(), 4, "{resp}");
}

#[tokio::test]
async fn default_blend_keeps_the_first_stage_top_result_against_a_mediocre_cross_encoder() {
    let fx = fixture(4);
    four(&fx).await;
    let first_stage = contents(&search(&fx.state, json!({})).await);
    // The mock embedder gives every chunk the same vector, so the first-stage order is only a
    // tie-break. Whatever it is, give the first-stage top result the *lowest* logit and the others
    // slightly higher ones: the cross-encoder alone then puts it last, so the test only passes if
    // the position-aware blend is really mixing the first-stage rank in.
    let top = first_stage[0].clone();
    let rr = Arc::new(MockReranker::new(
        move |_q, d| {
            if d == top {
                -0.2
            } else {
                0.1
            }
        },
    ));
    install(&fx.state, ce_only(), Some(rr.clone()), None);
    let ce = search(&fx.state, json!({"rerank": true})).await;
    assert_ne!(
        contents(&ce)[0],
        first_stage[0],
        "the cross-encoder alone must not keep the top result: {ce}"
    );
    install(
        &fx.state,
        SearchStagesConfig::default(),
        Some(rr.clone()),
        None,
    );
    let resp = search(&fx.state, json!({"rerank": true})).await;
    assert_eq!(contents(&resp)[0], first_stage[0]);
    assert_eq!(resp["rerank"]["applied"], json!(true));
}
