//! Issue #645 / #666 end-to-end: a corpus naming the sibling family `X-24001` … `X-24007` with
//! cross-references keeps every edge endpoint on its own sibling and creates no self-loops, even
//! with embeddings that make every sibling a perfect cosine match for every other.

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
    types::{ExtractedEdge, ExtractedEntity, ExtractionResult, SourceType},
};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "grp";

fn names() -> Vec<String> {
    (1..=7).map(|i| format!("X-2400{i}")).collect()
}

fn ent(name: &str) -> ExtractedEntity {
    ExtractedEntity {
        name: name.to_string(),
        entity_type: "Entity".to_string(),
        summary: format!("{name} summary"),
        original_entity_type: None,
    }
}

fn edge(src: &str, dst: &str) -> ExtractedEdge {
    ExtractedEdge {
        source_name: src.to_string(),
        target_name: dst.to_string(),
        fact: format!("{src} references {dst}"),
        relation_type: None,
        valid_at: None,
        invalid_at: None,
        original_relation_type: None,
    }
}

async fn ingest(state: &Arc<AppState>, n: usize) -> episode::AddEpisodeResult {
    episode::add_episode(
        Arc::clone(state),
        &format!("ep-{n}"),
        "body",
        "test",
        "test source",
        "2026-01-01T00:00:00Z",
        G,
        SourceType::Text,
        None,
        "",
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn sibling_family_has_no_repointed_endpoints_and_no_self_loops() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("sib.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(DIM).unwrap();

    // Every sibling embeds identically: cosine 1.0 against every other sibling.
    let mut map = HashMap::new();
    for n in names() {
        map.insert(n, vec![1.0, 0.0, 0.0, 0.0]);
    }
    let all = names();
    let all_refs: Vec<&str> = all.iter().map(String::as_str).collect();

    // Episode 1 ingests the family; episode 2 re-mentions only X-24001 and cross-references the
    // whole ring, so six of seven edges name off-list endpoints (exact stored hits after #666).
    let ring: Vec<(String, String)> = (0..7)
        .map(|i| (all[i].clone(), all[(i + 1) % 7].clone()))
        .collect();
    let ring_edges: Vec<ExtractedEdge> = ring.iter().map(|(a, b)| edge(a, b)).collect();
    let ext = ConfigurableExtractor::new(vec![
        ExtractionResult {
            entities: all_refs.iter().map(|n| ent(n)).collect(),
            edges: vec![],
        },
        ExtractionResult {
            entities: vec![ent("X-24001")],
            edges: ring_edges,
        },
    ]);
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(Arc::clone(&db))),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(NameMapEmbedder::new(DIM, map)),
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

    let first = ingest(&state, 1).await;
    assert_eq!(first.dedup_paths.embedding_merge, 0);
    let second = ingest(&state, 2).await;
    assert_eq!(second.edges_extracted, 7, "{second:?}");
    assert_eq!(second.edges_dropped_unresolvable, 0);
    assert_eq!(second.edges_dropped_self_loop, 0);

    let conn = db.connect().unwrap();
    let rows = conn.get_entities_by_group_ids(Some(&[G])).unwrap();
    assert_eq!(rows.len(), 7, "siblings must not merge");
    let uuid_to_name: HashMap<String, String> =
        rows.into_iter().map(|e| (e.uuid, e.name)).collect();

    let mut got: Vec<(String, String)> = conn
        .get_edges_by_group_ids(Some(&[G]))
        .unwrap()
        .into_iter()
        .map(|e| {
            assert_ne!(e.source_node_uuid, e.target_node_uuid, "self-loop edge");
            (
                uuid_to_name[&e.source_node_uuid].clone(),
                uuid_to_name[&e.target_node_uuid].clone(),
            )
        })
        .collect();
    got.sort();
    let mut want = ring;
    want.sort();
    assert_eq!(got, want, "every endpoint stays on its own sibling");
}
