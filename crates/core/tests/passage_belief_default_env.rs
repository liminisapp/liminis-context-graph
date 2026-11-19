//! Issue #806: the `LCG_PASSAGE_BELIEF_AWARE` workspace switch for `knowledge_search_passages`.
//!
//! This lives in its own integration binary (its own process) with a single test function,
//! because it mutates process-global environment that the service reads per request; in
//! `passage_hybrid_search.rs` it would race the parallel tests there.

#![allow(dead_code)]

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

fn with_belief(mode: &str, belief: Option<bool>) -> Value {
    let mut p = params("quokkaplex", mode);
    if let Some(b) = belief {
        p["belief_aware"] = json!(b);
    }
    p
}

const VAR: &str = "LCG_PASSAGE_BELIEF_AWARE";

#[tokio::test]
async fn belief_aware_env_switch() {
    let (state, _db, _d) = make_state(
        embedder(&[("quokkaplex", graded_query())]),
        &derived_and_source(),
    );
    let page_first = vec!["page", "src"];
    let src_first = vec!["src", "page"];

    // Unset: belief-aware by default, in both modes.
    std::env::remove_var(VAR);
    let default_on = search(&state, with_belief("vector", None)).await;
    assert_eq!(uuids(&default_on), src_first);
    assert_eq!(
        uuids(&search(&state, with_belief("hybrid", None)).await),
        src_first
    );
    let raw = search(&state, with_belief("vector", Some(false))).await;
    assert_eq!(uuids(&raw), page_first);

    // `off` (any recognised spelling) restores the previous vector default byte-for-byte.
    for off in ["off", "OFF", "0", "false", " Off "] {
        std::env::set_var(VAR, off);
        let got = search(&state, with_belief("vector", None)).await;
        assert_eq!(got, raw, "{off:?}");
        // In hybrid it yields weighted RRF without the belief rules.
        let hy = search(&state, with_belief("hybrid", None)).await;
        assert_eq!(uuids(&hy), page_first, "hybrid {off:?}");
    }

    // An explicit request value wins over the switch, in both directions.
    std::env::set_var(VAR, "off");
    let forced_on = search(&state, with_belief("vector", Some(true))).await;
    assert_eq!(forced_on, default_on);
    std::env::set_var(VAR, "on");
    let forced_off = search(&state, with_belief("vector", Some(false))).await;
    assert_eq!(forced_off, raw);
    assert_eq!(
        search(&state, with_belief("vector", None)).await,
        default_on
    );

    // An unrecognised or empty value is ignored: default behaviour applies, no failure.
    for bad in ["maybe", "", "   ", "disable"] {
        std::env::set_var(VAR, bad);
        let got = search(&state, with_belief("vector", None)).await;
        assert_eq!(got, default_on, "{bad:?}");
    }
    std::env::remove_var(VAR);
}
