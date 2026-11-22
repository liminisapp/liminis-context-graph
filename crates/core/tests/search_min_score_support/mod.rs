//! Shared fixture for the `search_min_score_*` test binaries (issue #812). Each binary is its own
//! process because `lcg_core::search_config::install` can only be called once per process.
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

pub const DIM: usize = 4;
pub const G: &str = "g1";
pub const TS: &str = "2026-08-04 00:00:00";

pub fn one_hot(axis: usize) -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[axis] = 1.0;
    v
}

pub struct Ep {
    pub uuid: &'static str,
    pub group: &'static str,
    pub origin: Option<&'static str>,
    pub content: &'static str,
    pub axis: usize,
    pub doc_id: Option<&'static str>,
    /// `(superseded_at, kind, superseded_by, reason)` for a superseded episode.
    pub superseded: Option<(&'static str, &'static str, &'static str, &'static str)>,
}

pub fn ep(uuid: &'static str, content: &'static str, axis: usize) -> Ep {
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
pub fn doc(
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

pub fn make_state(embedder: Arc<dyn Embedder>, eps: &[Ep]) -> (Arc<AppState>, Arc<Db>, TempDir) {
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
        search_stages: Default::default(),
        chunk_inflight: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    });
    (state, db, dir)
}

pub fn embedder(queries: &[(&str, Vec<f32>)]) -> Arc<dyn Embedder> {
    let map: HashMap<String, Vec<f32>> = queries
        .iter()
        .map(|(q, v)| (q.to_string(), v.clone()))
        .collect();
    Arc::new(NameMapEmbedder::new(DIM, map))
}

pub async fn try_search(state: &Arc<AppState>, params: Value) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: "knowledge_search_passages".to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, Arc::clone(state), None).await).unwrap()
}

pub async fn status(state: &Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: "knowledge_status".to_string(),
        params: json!({}),
    };
    let v = serde_json::to_value(handlers::dispatch(req, Arc::clone(state), None).await).unwrap();
    assert!(v.get("error").is_none(), "status errored: {v}");
    v["result"].clone()
}

pub async fn search(state: &Arc<AppState>, params: Value) -> Vec<Value> {
    let v = try_search(state, params).await;
    assert!(v.get("error").is_none(), "search errored: {v}");
    v["result"]["passages"].as_array().unwrap().clone()
}

pub fn uuids(ps: &[Value]) -> Vec<String> {
    let mut u: Vec<String> = ps
        .iter()
        .map(|p| p["uuid"].as_str().unwrap().to_string())
        .collect();
    u.sort();
    u
}

/// Query with cosines 0.75 / 0.55 / 0.35 / ~0.11 against passages on axes 0..=3 (unit norm).
pub fn graded_query() -> Vec<f32> {
    vec![0.75, 0.55, 0.35, 0.111_803_4]
}

/// Four passages, one per axis, with no lexical overlap with the query text "zzqueryzz" — so in
/// hybrid mode only the vector retriever contributes candidates.
pub fn four_passages() -> Vec<Ep> {
    vec![
        ep("p75", "alpha text one", 0),
        ep("p55", "bravo text two", 1),
        ep("p35", "charlie text three", 2),
        ep("p11", "delta text four", 3),
    ]
}

pub fn params(mode: &str, min_score: Option<Value>) -> Value {
    let mut p = json!({"query": "zzqueryzz", "group_ids": [G], "num_results": 10, "mode": mode});
    if let Some(m) = min_score {
        p["min_score"] = m;
    }
    p
}
