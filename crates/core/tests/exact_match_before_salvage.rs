//! Issue #666: an off-list edge endpoint that exists *exactly* in the stored graph resolves to
//! that entity and is never cosine-salvaged onto a merely similar batch entity.
//!
//! The stored entity is seeded under an `identity: true` kind so Phase B (which is kind-scoped,
//! ADR-0616) does not merge it with the similar batch entity — salvage is kind-blind, which is
//! what made the defect reachable.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::NameMapEmbedder,
    episode::{self, AddEpisodeResult},
    extractor::ConfigurableExtractor,
    ontology::{compute_ancestor_map, EntityTypeDef, Ontology, OntologyMode},
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEdge, ExtractedEntity, ExtractionResult, SourceType},
};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "grp";
const OTHER: &str = "other-grp";

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("exact.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(DIM).unwrap();
    (db, dir)
}

fn ontology() -> Ontology {
    let mut entity_types: Vec<EntityTypeDef> = [("Person", true), ("Technology", false)]
        .iter()
        .map(|(n, id)| EntityTypeDef {
            name: n.to_string(),
            description: None,
            parent: None,
            identity: *id,
            extract: true,
        })
        .collect();
    lcg_core::ontology::validate_and_clean_parents(&mut entity_types);
    let ancestor_map = compute_ancestor_map(&entity_types);
    Ontology {
        mode: OntologyMode::Open,
        entity_types,
        relation_types: vec![],
        ancestor_map,
    }
}

/// Foo, foo and Fooz share one vector (cosine 1.0, no identifier-veto token difference);
/// Bar is orthogonal.
fn make_state(db: Arc<Db>, extractions: Vec<ExtractionResult>) -> Arc<AppState> {
    let mut map = HashMap::new();
    for n in ["Foo", "foo", "Fooz"] {
        map.insert(n.to_string(), vec![1.0, 0.0, 0.0, 0.0]);
    }
    map.insert("Bar".to_string(), vec![0.0, 1.0, 0.0, 0.0]);
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(NameMapEmbedder::new(DIM, map)),
        extractor: Arc::new(ConfigurableExtractor::new(extractions)),
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
        ontology: Some(Arc::new(ontology())),
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn ent(name: &str, ty: &str) -> ExtractedEntity {
    ExtractedEntity {
        name: name.to_string(),
        entity_type: ty.to_string(),
        summary: String::new(),
        original_entity_type: None,
    }
}

fn edge(src: &str, dst: &str) -> ExtractedEdge {
    ExtractedEdge {
        source_name: src.to_string(),
        target_name: dst.to_string(),
        fact: format!("{src} relates to {dst}"),
        relation_type: Some("RELATES".to_string()),
        ..Default::default()
    }
}

fn extraction(entities: Vec<ExtractedEntity>, edges: Vec<ExtractedEdge>) -> ExtractionResult {
    ExtractionResult { entities, edges }
}

async fn ingest(state: &Arc<AppState>, group: &str, n: usize) -> AddEpisodeResult {
    episode::add_episode(
        Arc::clone(state),
        &format!("ep-{n}"),
        "body",
        "test",
        "test source",
        "2026-01-01T00:00:00Z",
        group,
        SourceType::Text,
        None,
        "",
    )
    .await
    .unwrap()
}

/// UUID of the entity called `name` of the given kind in `group`.
fn uuid_of(db: &Db, group: &str, name: &str, kind: &str) -> String {
    let conn = db.connect().unwrap();
    conn.get_entities_by_group_ids(Some(&[group]))
        .unwrap()
        .into_iter()
        .find(|e| e.name == name && e.kind == kind)
        .unwrap_or_else(|| panic!("no {kind} entity {name:?} in {group}"))
        .uuid
}

fn source_uuids(db: &Db) -> Vec<(String, String)> {
    let conn = db.connect().unwrap();
    conn.get_edges_by_group_ids(Some(&[G]))
        .unwrap()
        .into_iter()
        .map(|e| (e.source_node_uuid, e.target_node_uuid))
        .collect()
}

#[tokio::test]
async fn exact_stored_match_beats_similar_batch_entity() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        vec![
            extraction(vec![ent("Foo", "Person")], vec![]),
            extraction(
                vec![ent("Fooz", "Technology"), ent("Bar", "Technology")],
                vec![edge("Foo", "Bar")],
            ),
        ],
    );
    ingest(&state, G, 1).await;
    let r = ingest(&state, G, 2).await;

    assert_eq!(r.edges_extracted, 1);
    assert_eq!(r.dedup_paths.salvage_vetoed, 0);
    let foo = uuid_of(&db, G, "Foo", "Person");
    let bar = uuid_of(&db, G, "Bar", "Entity");
    assert_eq!(source_uuids(&db), vec![(foo, bar)]);
}

#[tokio::test]
async fn exact_stored_match_is_case_insensitive() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        vec![
            extraction(vec![ent("Foo", "Person")], vec![]),
            extraction(
                vec![ent("Fooz", "Technology"), ent("Bar", "Technology")],
                vec![edge("foo", "Bar")],
            ),
        ],
    );
    ingest(&state, G, 1).await;
    let r = ingest(&state, G, 2).await;

    assert_eq!(r.edges_extracted, 1);
    let foo = uuid_of(&db, G, "Foo", "Person");
    let bar = uuid_of(&db, G, "Bar", "Entity");
    assert_eq!(source_uuids(&db), vec![(foo, bar)]);
}

#[tokio::test]
async fn same_name_in_another_group_is_not_an_exact_hit() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        vec![
            extraction(vec![ent("Foo", "Person")], vec![]),
            extraction(
                vec![ent("Fooz", "Technology"), ent("Bar", "Technology")],
                vec![edge("Foo", "Bar")],
            ),
        ],
    );
    ingest(&state, OTHER, 1).await;
    let r = ingest(&state, G, 2).await;

    // Salvage proceeds as before: the off-list 'Foo' is rewritten onto the batch's 'Fooz'.
    assert_eq!(r.edges_extracted, 1);
    let fooz = uuid_of(&db, G, "Fooz", "Entity");
    let bar = uuid_of(&db, G, "Bar", "Entity");
    assert_eq!(source_uuids(&db), vec![(fooz, bar)]);
}

#[tokio::test]
async fn no_exact_match_still_salvages() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        vec![extraction(
            vec![ent("Fooz", "Technology"), ent("Bar", "Technology")],
            vec![edge("Foo", "Bar")],
        )],
    );
    let r = ingest(&state, G, 1).await;

    assert_eq!(r.edges_extracted, 1);
    assert_eq!(r.edges_dropped_unresolvable, 0);
    let fooz = uuid_of(&db, G, "Fooz", "Entity");
    let bar = uuid_of(&db, G, "Bar", "Entity");
    assert_eq!(source_uuids(&db), vec![(fooz, bar)]);
}

#[tokio::test]
async fn ambiguous_exact_hit_is_dropped_not_salvaged() {
    let (db, _d) = make_db();
    // 'Foo' is stored under two eligible kinds (default and the identity kind Person).
    let state = make_state(
        Arc::clone(&db),
        vec![
            extraction(vec![ent("Foo", "Person")], vec![]),
            extraction(vec![ent("Foo", "Technology")], vec![]),
            extraction(
                vec![ent("Fooz", "Technology"), ent("Bar", "Technology")],
                vec![edge("Foo", "Bar")],
            ),
        ],
    );
    ingest(&state, G, 1).await;
    ingest(&state, G, 2).await;
    let r = ingest(&state, G, 3).await;

    assert_eq!(r.edges_extracted, 0);
    assert_eq!(r.edges_dropped_unresolvable, 1, "{r:?}");
    assert!(source_uuids(&db).is_empty());
}
