// Identifier-mismatch veto on the *hybrid* dedup path (issue #650, ADR-0650).
//
// `episode::hybrid_threshold()` caches `LIMINIS_DEDUP_HYBRID_THRESHOLD` in a process-wide
// `OnceLock`, so this file (its own test binary) forces the hybrid path by setting the
// threshold to 1 before any ingest: the first episode sees an empty group and takes brute
// force; every later episode sees ≥ 1 entity and takes the hybrid path, whose missing indexes
// are built by the ADR-0025 auto-heal.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::NameMapEmbedder,
    episode,
    extractor::ConfigurableExtractor,
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEntity, ExtractionResult, SourceType},
};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const EMB_DIM: usize = 4;
const REF_TIME: &str = "2026-01-01T00:00:00Z";
const GROUP: &str = "test-grp";

fn make_state(
    ext: ConfigurableExtractor,
    embedder: NameMapEmbedder,
) -> (Arc<AppState>, Arc<Db>, TempDir) {
    std::env::set_var("LIMINIS_DEDUP_HYBRID_THRESHOLD", "1");
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(EMB_DIM).unwrap();
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(Arc::clone(&db))),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(embedder),
        extractor: Arc::new(ext),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: None,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "test".to_string(),
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
    (state, db, dir)
}

fn one_entity(name: &str) -> ExtractionResult {
    ExtractionResult {
        entities: vec![ExtractedEntity {
            name: name.to_string(),
            entity_type: "Person".to_string(),
            summary: format!("{name} summary"),
            original_entity_type: None,
        }],
        edges: vec![],
    }
}

async fn ingest(state: &Arc<AppState>, ep: &str) -> episode::AddEpisodeResult {
    episode::add_episode(
        Arc::clone(state),
        ep,
        "body",
        "test",
        "test source",
        REF_TIME,
        GROUP,
        SourceType::Text,
        None,
        "",
    )
    .await
    .unwrap()
}

async fn run_pair(first: &str, second: &str) -> (usize, episode::AddEpisodeResult) {
    let mut m: HashMap<String, Vec<f32>> = HashMap::new();
    m.insert(first.to_string(), vec![1.0, 0.0, 0.0, 0.0]);
    m.insert(second.to_string(), vec![0.9239, 0.3827, 0.0, 0.0]);
    let ext = ConfigurableExtractor::new(vec![one_entity(first), one_entity(second)]);
    let (state, db, _dir) = make_state(ext, NameMapEmbedder::new(EMB_DIM, m));
    ingest(&state, "ep-a").await;
    let res = ingest(&state, "ep-b").await;
    let count = db.connect().unwrap().entity_count_in_group(GROUP).unwrap();
    (count, res)
}

#[tokio::test]
async fn hybrid_path_vetoes_identifier_differences_and_keeps_aliases() {
    for (a, b) in [
        ("ADR 2018", "ADR 2019"),
        ("lcg 0.15.0", "lcg 0.16.2"),
        ("RFC 9110", "RFC 9111"),
        ("Q3 2025 roadmap", "Q4 2025 roadmap"),
        ("issue #611", "issue #612"),
        ("Project Aurora", "Project Aurora v2"),
    ] {
        let (count, res) = run_pair(a, b).await;
        assert_eq!(count, 2, "hybrid: {a:?} / {b:?} must stay two entities");
        assert_eq!(res.dedup_paths.vetoed, 1, "hybrid: {a:?} / {b:?} vetoed");
    }
    for (a, b) in [("PostgreSQL", "Postgres"), ("New York", "New York City")] {
        let (count, res) = run_pair(a, b).await;
        assert_eq!(count, 1, "hybrid: {a:?} / {b:?} must still merge");
        assert_eq!(res.dedup_paths.embedding_merge, 1);
    }
}
