//! Shared fixture for the summary-unit tests (issue #850): a keyword-axis embedder (so a summary
//! can match a query its passage does not), an `AppState` whose extractor summarizes by a
//! caller-supplied closure, and small helpers to ingest, search and inspect the store and WAL.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::{EmbedRole, Embedder},
    extractor::SummarizeFn,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    ConfigurableExtractor, Db, Error, WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

pub const DIM: usize = 6;
pub const G: &str = "liminis";
pub const OTHER: &str = "other";

/// Sets `LCG_SUMMARY_UNITS=on` once for the whole test binary. A binary that needs the switch
/// off must never call this (the on and off tests live in separate binaries).
pub fn enable_summary_units() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| std::env::set_var("LCG_SUMMARY_UNITS", "on"));
}

/// Upper-case keywords → axes; text with none of them maps to the last axis. Each keyword present
/// adds its axis, so `"KILN ROCKET"` sits between the two. Lower-case text never matches, which
/// is how a passage and its (upper-cased) summary get different vectors.
pub fn vector_for(text: &str) -> Vec<f32> {
    const KEYWORDS: [&str; 5] = ["KILN", "ROCKET", "GARDEN", "TIDE", "OWL"];
    let mut v = [0.0f32; DIM];
    for (i, k) in KEYWORDS.iter().enumerate() {
        if text.contains(k) {
            v[i] = 1.0;
        }
    }
    if v.iter().all(|x| *x == 0.0) {
        v[DIM - 1] = 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / norm).collect()
}

#[derive(Default)]
pub struct KeywordEmbedder;

impl Embedder for KeywordEmbedder {
    fn embed<'a>(
        &'a self,
        text: &'a str,
        _role: EmbedRole,
    ) -> BoxFuture<'a, Result<Vec<f32>, Error>> {
        Box::pin(async move { Ok(vector_for(text)) })
    }

    fn dim(&self) -> usize {
        DIM
    }
}

/// The default test summarizer: the body upper-cased, so its keywords become visible to the
/// embedder while the passage's stay invisible.
pub fn upper_summarizer(calls: Arc<AtomicUsize>) -> SummarizeFn {
    Arc::new(move |body: &str| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(format!("This note describes {}.", body.to_uppercase()))
    })
}

pub struct Fixture {
    pub state: Arc<AppState>,
    pub summarize_calls: Arc<AtomicUsize>,
    pub wal_dir: TempDir,
    _db_dir: TempDir,
}

pub fn fixture() -> Fixture {
    let calls = Arc::new(AtomicUsize::new(0));
    let f = upper_summarizer(Arc::clone(&calls));
    fixture_with(Some(f), calls)
}

pub fn fixture_with(summarize: Option<SummarizeFn>, calls: Arc<AtomicUsize>) -> Fixture {
    let db_dir = TempDir::new().unwrap();
    let wal_dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(db_dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
    }
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let mut extractor = ConfigurableExtractor::new(vec![]);
    if let Some(f) = summarize {
        extractor = extractor.with_summarize(f);
    }
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(KeywordEmbedder),
        extractor: Arc::new(extractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(tokio::sync::RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: Some(wal_dir.path().to_path_buf()),
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 << 20,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::<String, WalWriter>::new())),
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
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    });
    Fixture {
        state,
        summarize_calls: calls,
        wal_dir,
        _db_dir: db_dir,
    }
}

pub async fn try_call(method: &str, params: Value, state: &Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, Arc::clone(state), None).await).unwrap()
}

pub async fn call(method: &str, params: Value, state: &Arc<AppState>) -> Value {
    let v = try_call(method, params, state).await;
    assert!(v.get("error").is_none(), "{method} errored: {v}");
    v["result"].clone()
}

/// Ingests one chunk and returns its episode uuid.
pub async fn put(
    state: &Arc<AppState>,
    group: &str,
    chunk_id: &str,
    text: &str,
    doc_id: Option<&str>,
    ref_time: &str,
) -> String {
    let mut params = json!({
        "chunk_text": text, "chunk_id": chunk_id,
        "source_file": format!("notes/{chunk_id}.md"), "group_id": group,
        "reference_time": ref_time,
    });
    if let Some(d) = doc_id {
        params["doc_id"] = json!(d);
    }
    let r = call("knowledge_process_chunk", params, state).await;
    r["episode_uuid"].as_str().unwrap().to_string()
}

/// Every stored summary unit as `(uuid, episode_uuid, group_id, content)`, by episode uuid.
pub fn units(state: &Arc<AppState>) -> Vec<(String, String, String, String)> {
    let db = state.db.load_full().unwrap();
    let conn = db.connect().unwrap();
    let mut rows: Vec<(String, String, String, String)> = conn
        .cypher_query("MATCH (s:SummaryUnit) RETURN s.uuid, s.episode_uuid, s.group_id, s.content")
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone(), r[2].clone(), r[3].clone()))
        .collect();
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    rows
}

/// Every WAL line under `dir` (recursively), concatenated.
pub fn wal_text(dir: &Path) -> String {
    let mut out = String::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                out.push_str(&std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    out
}
