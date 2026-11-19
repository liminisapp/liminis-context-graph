//! Issue #802 / ADR-0802: caller-supplied query variants on `knowledge_search_passages`.
//!
//! A scripted embedder maps keywords in a text to orthogonal unit vectors, so each test controls
//! exactly which passage each query retrieves. Covers: variant-only matches and their evidence,
//! primary-before-variant ordering, `group_ids` / `origins` / `min_score` applying to every
//! variant, one batched embed call, validation errors through the handler, and that `queries`
//! absent / `null` / `[]` all give a response identical to the pre-feature one.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::{EmbedRole, Embedder},
    error::Error,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    ConfigurableExtractor, Db,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "liminis";
const OTHER: &str = "other";

/// Keyword → axis. Text with none of them maps to the fourth axis.
fn vector_for(text: &str) -> Vec<f32> {
    if text.contains("ALPHA") {
        vec![1.0, 0.0, 0.0, 0.0]
    } else if text.contains("BETA") {
        vec![0.0, 1.0, 0.0, 0.0]
    } else if text.contains("GAMMA") {
        vec![0.0, 0.0, 1.0, 0.0]
    } else {
        vec![0.0, 0.0, 0.0, 1.0]
    }
}

#[derive(Default)]
struct Scripted {
    singles: AtomicUsize,
    batches: AtomicUsize,
    /// The role and texts of each batched call.
    batch_log: Mutex<Vec<(EmbedRole, Vec<String>)>>,
}

impl Embedder for Scripted {
    fn embed<'a>(
        &'a self,
        text: &'a str,
        _role: EmbedRole,
    ) -> BoxFuture<'a, Result<Vec<f32>, Error>> {
        self.singles.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(vector_for(text)) })
    }

    fn embed_batch<'a>(
        &'a self,
        texts: &'a [&'a str],
        role: EmbedRole,
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, Error>> {
        self.batches.fetch_add(1, Ordering::SeqCst);
        self.batch_log
            .lock()
            .unwrap()
            .push((role, texts.iter().map(|t| t.to_string()).collect()));
        Box::pin(async move { Ok(texts.iter().map(|t| vector_for(t)).collect()) })
    }

    fn dim(&self) -> usize {
        DIM
    }
}

fn make_state(embedder: Arc<Scripted>) -> (Arc<AppState>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
    }
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder,
        extractor: Arc::new(ConfigurableExtractor::new(vec![])),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(tokio::sync::RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: None,
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
    (state, dir)
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

async fn put(state: &Arc<AppState>, id: &str, text: &str, group: &str, origin: Option<&str>) {
    let mut params = json!({
        "chunk_text": text, "chunk_id": id, "source_file": format!("notes/{id}.md"),
        "group_id": group, "reference_time": "2026-08-04T00:00:00Z", "attributes": {},
    });
    if let Some(o) = origin {
        params["origin"] = json!(o);
    }
    let v = try_call("knowledge_process_chunk", params, state).await;
    assert!(v.get("error").is_none(), "ingest errored: {v}");
}

/// alpha / beta in group G, a derived beta in G, a beta in another group, and a gamma in G.
async fn corpus() -> (Arc<AppState>, Arc<Scripted>, TempDir) {
    let emb = Arc::new(Scripted::default());
    let (state, dir) = make_state(Arc::clone(&emb));
    put(&state, "alpha", "ALPHA note", G, None).await;
    put(&state, "beta", "BETA note", G, None).await;
    put(
        &state,
        "beta-derived",
        "BETA derived page",
        G,
        Some("derived"),
    )
    .await;
    put(&state, "beta-other", "BETA in another group", OTHER, None).await;
    put(&state, "gamma", "GAMMA note", G, None).await;
    (state, emb, dir)
}

async fn search(state: &Arc<AppState>, extra: Value) -> Value {
    let mut params = json!({"query": "ALPHA question", "min_score": 0.5, "num_results": 10});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    call("knowledge_search_passages", params, state).await
}

fn names(r: &Value) -> Vec<String> {
    r["passages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

fn matched(r: &Value, name: &str) -> Vec<u64> {
    r["passages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("{name} not in {r}"))["matched_queries"]
        .as_array()
        .unwrap_or_else(|| panic!("no matched_queries on {name}"))
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect()
}

#[tokio::test]
async fn variant_only_match_appears_with_only_variant_indices() {
    let (state, _e, _d) = corpus().await;
    let base = search(&state, json!({"group_ids": [G]})).await;
    assert_eq!(names(&base), vec!["alpha"]);

    let r = search(
        &state,
        json!({"group_ids": [G], "queries": ["BETA keywords"]}),
    )
    .await;
    let n = names(&r);
    assert_eq!(n[0], "alpha", "the primary's hit ranks first: {n:?}");
    assert!(n.contains(&"beta".to_string()), "{n:?}");
    assert_eq!(matched(&r, "alpha"), vec![0]);
    assert_eq!(matched(&r, "beta"), vec![1]);
    assert_eq!(r["count"], n.len());
}

#[tokio::test]
async fn a_passage_found_by_several_queries_appears_once_with_all_indices() {
    let (state, _e, _d) = corpus().await;
    let r = search(
        &state,
        json!({"group_ids": [G], "queries": ["BETA one", "ALPHA again", "BETA two"]}),
    )
    .await;
    let n = names(&r);
    assert_eq!(n.iter().filter(|x| *x == "alpha").count(), 1, "{n:?}");
    assert_eq!(matched(&r, "alpha"), vec![0, 2]);
    assert_eq!(matched(&r, "beta"), vec![1, 3]);
    // Found by two queries including the primary: ahead of everything found only by variants.
    assert_eq!(n[0], "alpha");
}

#[tokio::test]
async fn duplicate_variants_are_dropped_and_indices_stay_original() {
    let (state, _e, emb) = {
        let (s, e, d) = corpus().await;
        (s, d, e)
    };
    let before = emb.batch_log.lock().unwrap().len();
    let r = search(
        &state,
        json!({"group_ids": [G], "queries": ["ALPHA question", "BETA x", " BETA x "]}),
    )
    .await;
    // The first variant equals the primary and the third equals the second: neither is probed.
    assert_eq!(matched(&r, "alpha"), vec![0]);
    assert_eq!(matched(&r, "beta"), vec![2]);
    let log = emb.batch_log.lock().unwrap();
    assert_eq!(log.len(), before + 1);
    assert_eq!(log.last().unwrap().1, vec!["ALPHA question", "BETA x"]);
}

#[tokio::test]
async fn variants_are_embedded_in_one_batched_query_role_call() {
    let (state, emb, _d) = corpus().await;
    let (singles, batches) = (
        emb.singles.load(Ordering::SeqCst),
        emb.batches.load(Ordering::SeqCst),
    );
    search(
        &state,
        json!({"group_ids": [G], "queries": ["BETA a", "GAMMA b"]}),
    )
    .await;
    assert_eq!(emb.batches.load(Ordering::SeqCst), batches + 1);
    assert_eq!(
        emb.singles.load(Ordering::SeqCst),
        singles,
        "no per-query embeds"
    );
    let log = emb.batch_log.lock().unwrap();
    let (role, texts) = log.last().unwrap();
    assert_eq!(*role, EmbedRole::Query);
    assert_eq!(texts, &vec!["ALPHA question", "BETA a", "GAMMA b"]);
}

#[tokio::test]
async fn group_filter_applies_to_every_variant() {
    let (state, _e, _d) = corpus().await;
    let r = search(
        &state,
        json!({"group_ids": [G], "queries": ["BETA keywords"]}),
    )
    .await;
    assert!(!names(&r).contains(&"beta-other".to_string()), "{r}");
    // Without the filter the other group's note is reachable through the variant.
    let all = search(&state, json!({"queries": ["BETA keywords"]})).await;
    assert!(names(&all).contains(&"beta-other".to_string()), "{all}");
    // Scoped to the other group, the primary-matching note in G is absent too.
    let other = search(
        &state,
        json!({"group_ids": [OTHER], "queries": ["BETA keywords"]}),
    )
    .await;
    assert_eq!(names(&other), vec!["beta-other"]);
}

#[tokio::test]
async fn origins_filter_applies_to_every_variant() {
    let (state, _e, _d) = corpus().await;
    let r = search(
        &state,
        json!({"group_ids": [G], "origins": ["primary"], "queries": ["BETA keywords"]}),
    )
    .await;
    let n = names(&r);
    assert!(n.contains(&"beta".to_string()), "{n:?}");
    assert!(!n.contains(&"beta-derived".to_string()), "{n:?}");
    let derived = search(
        &state,
        json!({"group_ids": [G], "origins": ["derived"], "queries": ["BETA keywords"]}),
    )
    .await;
    // Only derived passages: the primary query's own hit (a primary note) is excluded as well.
    assert_eq!(names(&derived), vec!["beta-derived"]);
}

#[tokio::test]
async fn min_score_floor_applies_to_every_variant() {
    let (state, _e, _d) = corpus().await;
    // "DELTA" maps to the fourth axis, orthogonal to every note: similarity 0 < 0.5.
    let r = search(
        &state,
        json!({"group_ids": [G], "queries": ["DELTA nothing"]}),
    )
    .await;
    assert_eq!(names(&r), vec!["alpha"]);
    // With the floor lowered to 0 the variant's weak matches are admitted.
    let low = search(
        &state,
        json!({"group_ids": [G], "min_score": 0.0, "queries": ["DELTA nothing"]}),
    )
    .await;
    assert!(names(&low).len() > 1, "{low}");
    // With variants `score` is the fused rank score, in rank order; the cosine is `similarity`.
    let passages = low["passages"].as_array().unwrap();
    for p in passages {
        let sim = p["similarity"].as_f64().unwrap();
        assert!((0.0..=1.0 + 1e-6).contains(&sim), "{p}");
    }
    let scores: Vec<f64> = passages
        .iter()
        .map(|p| p["score"].as_f64().unwrap())
        .collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "{scores:?}");
}

#[tokio::test]
async fn absent_null_and_empty_queries_are_identical_to_no_queries() {
    let (state, _e, _d) = corpus().await;
    let base = search(&state, json!({"group_ids": [G]})).await;
    let with_empty = search(&state, json!({"group_ids": [G], "queries": []})).await;
    let with_null = search(&state, json!({"group_ids": [G], "queries": null})).await;
    assert_eq!(base, with_empty);
    assert_eq!(base, with_null);
    assert_eq!(
        serde_json::to_string(&base).unwrap(),
        serde_json::to_string(&with_empty).unwrap()
    );
    for p in base["passages"].as_array().unwrap() {
        assert!(p.get("matched_queries").is_none(), "{p}");
    }
}

#[tokio::test]
async fn only_duplicate_variants_fall_back_to_the_unchanged_path() {
    let (state, _e, _d) = corpus().await;
    let base = search(&state, json!({"group_ids": [G]})).await;
    let dup = search(
        &state,
        json!({"group_ids": [G], "queries": ["ALPHA question"]}),
    )
    .await;
    assert_eq!(base, dup);
}

#[tokio::test]
async fn invalid_queries_are_rejected_with_a_rule_naming_error() {
    let (state, emb, _d) = corpus().await;
    let batches = emb.batches.load(Ordering::SeqCst);
    let long = "x".repeat(2001);
    let cases: Vec<(Value, &str)> = vec![
        (json!(["a", "b", "c", "d", "e", "f"]), "at most 5"),
        (json!([""]), "non-empty"),
        (json!(["   "]), "non-empty"),
        (json!([long]), "at most 2000"),
        (json!("BETA"), "array of strings"),
        (json!([1]), "must be a string"),
    ];
    for (bad, want) in cases {
        let v = try_call(
            "knowledge_search_passages",
            json!({"query": "ALPHA q", "queries": bad}),
            &state,
        )
        .await;
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(msg.contains(want), "{bad}: expected {want:?} in {v}");
    }
    assert_eq!(
        emb.batches.load(Ordering::SeqCst),
        batches,
        "validation runs before any embedding"
    );
}
