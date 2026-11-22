//! Literal-object edges and the misattachment diagnostic (#823, ADR-0823).
//!
//! Under an ontology, a fact stating a literal value about one entity ("Marlin Core has a monthly
//! budget of $4,200") is emitted with `source_name == target_name` and stored as a self-loop on
//! that entity, marked `literal_object`, instead of being pinned to a nearby unrelated entity.
//! An edge between two entities whose fact states a value but does not name both endpoints is
//! stored but marked `suspected_misattachment` (FR-007). Deterministic: no live model (FR-008).

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
    ontology::{compute_ancestor_map, EntityTypeDef, Ontology, OntologyMode, RelationTypeDef},
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEdge, ExtractedEntity, ExtractionResult, SourceType},
};
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "grp";
const BUDGET: &str = "Marlin Core has a monthly budget of $4,200";

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("lit.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(DIM).unwrap();
    (db, dir)
}

fn etype(name: &str) -> EntityTypeDef {
    EntityTypeDef {
        name: name.to_string(),
        description: None,
        parent: None,
        identity: false,
        extract: true,
    }
}

fn rtype(name: &str, src: &[&str], tgt: &[&str]) -> RelationTypeDef {
    RelationTypeDef {
        name: name.to_string(),
        description: None,
        source_types: src.iter().map(|s| s.to_string()).collect(),
        target_types: tgt.iter().map(|s| s.to_string()).collect(),
        inverse_aliases: vec![],
        symmetric: false,
        aliases: vec![],
        keywords: vec![],
        extract: true,
    }
}

/// Project, Person, Event; `HAS_MEASURE` (no signature), `SCHEDULED_FOR (any → Event)`,
/// `SETS_BUDGET (Person → Project)`.
fn ontology(mode: OntologyMode) -> Ontology {
    let entity_types = vec![etype("Project"), etype("Person"), etype("Event")];
    let ancestor_map = compute_ancestor_map(&entity_types);
    Ontology {
        mode,
        entity_types,
        relation_types: vec![
            rtype("HAS_MEASURE", &[], &[]),
            rtype("SCHEDULED_FOR", &[], &["Event"]),
            rtype("SETS_BUDGET", &["Person"], &["Project"]),
        ],
        ancestor_map,
    }
}

/// Orthogonal name vectors, so nothing dedup-merges or salvages unless a test maps two names to
/// the same vector on purpose.
fn base_vectors() -> HashMap<String, Vec<f32>> {
    let mut m = HashMap::new();
    m.insert("Marlin Core".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
    m.insert("Tomas Kowalski".to_string(), vec![0.0, 1.0, 0.0, 0.0]);
    m.insert("Basalt Supply".to_string(), vec![0.0, 0.0, 1.0, 0.0]);
    m
}

fn make_state(
    db: Arc<Db>,
    onto: Option<Ontology>,
    extractions: Vec<ExtractionResult>,
    vectors: HashMap<String, Vec<f32>>,
) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(NameMapEmbedder::new(DIM, vectors)),
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
        ontology: onto.map(Arc::new),
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn ent(name: &str, ty: &str) -> ExtractedEntity {
    ExtractedEntity {
        name: name.to_string(),
        entity_type: ty.to_string(),
        summary: format!("{ty} {name}"),
        original_entity_type: None,
    }
}

fn edge(src: &str, dst: &str, rt: &str, fact: &str) -> ExtractedEdge {
    ExtractedEdge {
        source_name: src.to_string(),
        target_name: dst.to_string(),
        fact: fact.to_string(),
        relation_type: Some(rt.to_string()),
        ..Default::default()
    }
}

fn marlin_and_tomas() -> Vec<ExtractedEntity> {
    vec![
        ent("Marlin Core", "Project"),
        ent("Tomas Kowalski", "Person"),
    ]
}

async fn ingest(state: &Arc<AppState>, ep: &str) -> AddEpisodeResult {
    episode::add_episode(
        Arc::clone(state),
        ep,
        "Marlin Core has a monthly budget of $4,200. Tomas Kowalski joined the team.",
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

/// A stored edge resolved to names: `(source, relation_type, target, fact, attributes)`.
type Stored = (String, String, String, String, Value);

fn stored_edges(db: &Arc<Db>) -> Vec<Stored> {
    let conn = db.connect().unwrap();
    let names: HashMap<String, String> = conn
        .get_entities_by_group_ids(Some(&[G]))
        .unwrap()
        .into_iter()
        .map(|e| (e.uuid, e.name))
        .collect();
    conn.get_edges_by_group_ids(Some(&[G]))
        .unwrap()
        .into_iter()
        .map(|e| {
            (
                names[&e.source_node_uuid].clone(),
                e.relation_type.clone().unwrap_or_default(),
                names[&e.target_node_uuid].clone(),
                e.fact.clone(),
                serde_json::from_str(&e.attributes).unwrap_or(Value::Null),
            )
        })
        .collect()
}

fn entity_names(db: &Arc<Db>) -> Vec<String> {
    let mut names: Vec<String> = db
        .connect()
        .unwrap()
        .get_entities_by_group_ids(Some(&[G]))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names
}

async fn run(
    onto: Option<Ontology>,
    entities: Vec<ExtractedEntity>,
    edges: Vec<ExtractedEdge>,
) -> (AddEpisodeResult, Vec<Stored>, Vec<String>) {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        onto,
        vec![ExtractionResult { entities, edges }],
        base_vectors(),
    );
    let r = ingest(&state, "ep-1").await;
    (r, stored_edges(&db), entity_names(&db))
}

fn flag(attrs: &Value, key: &str) -> bool {
    attrs.get(key).and_then(Value::as_bool).unwrap_or(false)
}

// (a) + (g) + (h): under an ontology, in strict and open mode alike, a value fact emitted as a
// self-reference with an unrelated entity present is stored on Marlin Core only, marked
// `literal_object`, and no node is named after the value (FR-002, FR-003, FR-005, FR-008).
#[tokio::test]
async fn value_fact_is_stored_as_self_loop_not_pinned_to_unrelated_entity() {
    for mode in [OntologyMode::Strict, OntologyMode::Open] {
        let (r, edges, names) = run(
            Some(ontology(mode.clone())),
            marlin_and_tomas(),
            vec![edge("Marlin Core", "Marlin Core", "HAS_MEASURE", BUDGET)],
        )
        .await;
        assert_eq!(edges.len(), 1, "{mode}: {edges:?}");
        let (src, rt, dst, fact, attrs) = &edges[0];
        assert_eq!((src.as_str(), dst.as_str()), ("Marlin Core", "Marlin Core"));
        assert_eq!(rt, "HAS_MEASURE");
        assert_eq!(fact, BUDGET);
        assert!(flag(attrs, "literal_object"), "{mode}: {attrs}");
        assert!(!flag(attrs, "suspected_misattachment"), "{mode}");
        assert!(
            edges
                .iter()
                .all(|e| e.0 != "Tomas Kowalski" && e.2 != "Tomas Kowalski"),
            "{mode}: no edge may pin the budget to Tomas"
        );
        assert_eq!(r.edges_extracted, 1);
        assert_eq!(r.edges_literal_object, 1, "{mode}");
        assert_eq!(r.edges_suspected_misattachment, 0);
        assert_eq!(r.edges_dropped_self_loop, 0);
        assert_eq!(
            names,
            vec!["Marlin Core".to_string(), "Tomas Kowalski".to_string()],
            "{mode}: no value-named node (FR-005)"
        );
    }
}

// (b): a fact naming both entities still yields an edge between them, unflagged (FR-004).
#[tokio::test]
async fn fact_naming_both_entities_keeps_the_edge_between_them() {
    for mode in [OntologyMode::Strict, OntologyMode::Open] {
        let (r, edges, _) = run(
            Some(ontology(mode.clone())),
            marlin_and_tomas(),
            vec![edge(
                "Tomas Kowalski",
                "Marlin Core",
                "SETS_BUDGET",
                "Tomas Kowalski set Marlin Core's monthly budget at $4,200",
            )],
        )
        .await;
        assert_eq!(edges.len(), 1, "{mode}");
        assert_eq!(edges[0].0, "Tomas Kowalski");
        assert_eq!(edges[0].2, "Marlin Core");
        assert!(!flag(&edges[0].4, "suspected_misattachment"), "{mode}");
        assert!(!flag(&edges[0].4, "literal_object"), "{mode}");
        assert_eq!(
            (r.edges_literal_object, r.edges_suspected_misattachment),
            (0, 0)
        );
    }
}

// (c): a pin that extraction still produces is stored, but marked and counted (FR-007) — in
// ontology mode and free-form alike (the mark is metadata only).
#[tokio::test]
async fn residual_pin_is_marked_suspected_misattachment() {
    for onto in [
        Some(ontology(OntologyMode::Strict)),
        Some(ontology(OntologyMode::Open)),
        None,
    ] {
        let label = format!("{:?}", onto.as_ref().map(|o| o.mode.clone()));
        let (r, edges, _) = run(
            onto,
            marlin_and_tomas(),
            vec![edge("Marlin Core", "Tomas Kowalski", "HAS_MEASURE", BUDGET)],
        )
        .await;
        assert_eq!(edges.len(), 1, "{label}: marked, never dropped");
        assert!(flag(&edges[0].4, "suspected_misattachment"), "{label}");
        assert_eq!(r.edges_suspected_misattachment, 1, "{label}");
        assert_eq!(r.edges_literal_object, 0, "{label}");
    }
}

// A value-free fact between two entities is never flagged, even if it names only one of them.
#[tokio::test]
async fn value_free_fact_is_not_flagged() {
    let (r, edges, _) = run(
        Some(ontology(OntologyMode::Open)),
        marlin_and_tomas(),
        vec![edge(
            "Tomas Kowalski",
            "Marlin Core",
            "SETS_BUDGET",
            "Tomas approves the spending",
        )],
    )
    .await;
    assert_eq!(edges.len(), 1);
    assert!(!flag(&edges[0].4, "suspected_misattachment"));
    assert_eq!(r.edges_suspected_misattachment, 0);
}

// (d): with no ontology, a name-level self-reference is still dropped, silently (FR-006).
#[tokio::test]
async fn no_ontology_self_reference_is_still_dropped() {
    let (r, edges, _) = run(
        None,
        marlin_and_tomas(),
        vec![edge("Marlin Core", "Marlin Core", "HAS_MEASURE", BUDGET)],
    )
    .await;
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(r.edges_extracted, 0);
    assert_eq!(r.edges_literal_object, 0);
    assert_eq!(
        r.edges_dropped_self_loop, 0,
        "name-level filter, never counted"
    );
}

// (d'): an ontology whose relation types are all `extract: false` counts as no ontology.
#[tokio::test]
async fn ontology_without_extractable_relations_still_drops_self_reference() {
    let mut onto = ontology(OntologyMode::Strict);
    for rt in &mut onto.relation_types {
        rt.extract = false;
    }
    let (r, edges, _) = run(
        Some(onto),
        marlin_and_tomas(),
        vec![edge("Marlin Core", "Marlin Core", "HAS_MEASURE", BUDGET)],
    )
    .await;
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(r.edges_literal_object, 0);
}

// (e): under an ontology, two *differently named* endpoints that resolve to one UUID are still
// the #666 error — dropped and counted, not admitted as a literal-object self-loop.
#[tokio::test]
async fn uuid_collapse_of_different_names_is_still_dropped() {
    let (db, _d) = make_db();
    let mut vectors = base_vectors();
    // 'Marlin Core Project' dedup-merges onto the stored 'Marlin Core'.
    vectors.insert("Marlin Core Project".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
    let state = make_state(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Open)),
        vec![
            ExtractionResult {
                entities: vec![ent("Marlin Core", "Project")],
                edges: vec![],
            },
            ExtractionResult {
                entities: vec![ent("Marlin Core Project", "Project")],
                edges: vec![edge(
                    "Marlin Core Project",
                    "Marlin Core",
                    "HAS_MEASURE",
                    BUDGET,
                )],
            },
        ],
        vectors,
    );
    ingest(&state, "ep-a").await;
    let r = ingest(&state, "ep-b").await;
    assert_eq!(r.edges_dropped_self_loop, 1, "{r:?}");
    assert_eq!(r.edges_literal_object, 0);
    assert!(stored_edges(&db).is_empty());
}

// (e'): under an ontology, endpoints made equal *by salvage* are still a collision and dropped.
#[tokio::test]
async fn salvage_created_self_reference_is_still_dropped() {
    let mut vectors = base_vectors();
    // Off-list 'Marlin-Core' salvages onto the batch entity 'Marlin Core'.
    vectors.insert("Marlin-Core".to_string(), vec![1.0, 0.0, 0.0, 0.0]);
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Open)),
        vec![ExtractionResult {
            entities: marlin_and_tomas(),
            edges: vec![
                edge("Marlin-Core", "Marlin Core", "HAS_MEASURE", BUDGET),
                // A sanctioned self-reference in the same batch survives the post-salvage filter.
                edge(
                    "Marlin Core",
                    "Marlin Core",
                    "SCHEDULED_FOR",
                    "Marlin Core launches in March 2027",
                ),
            ],
        }],
        vectors,
    );
    let r = ingest(&state, "ep-1").await;
    let edges = stored_edges(&db);
    assert_eq!(edges.len(), 1, "{edges:?}");
    assert_eq!(edges[0].1, "SCHEDULED_FOR");
    assert_eq!(r.edges_literal_object, 1);
    // Dropped by the post-salvage name-level filter, not as unresolvable or a UUID self-loop.
    assert_eq!(
        (r.edges_dropped_unresolvable, r.edges_dropped_self_loop),
        (0, 0),
        "{r:?}"
    );
}

// (f): a literal-object self-loop on a relation whose signature its entity does not fit
// (`SCHEDULED_FOR (any → Event)` on a Project) gets no spurious `signature_mismatch`.
#[tokio::test]
async fn self_loop_skips_the_direction_check() {
    for mode in [OntologyMode::Strict, OntologyMode::Open] {
        let (r, edges, _) = run(
            Some(ontology(mode.clone())),
            marlin_and_tomas(),
            vec![edge(
                "Marlin Core",
                "Marlin Core",
                "SCHEDULED_FOR",
                "Marlin Core has a scheduled launch date in March 2027",
            )],
        )
        .await;
        assert_eq!(edges.len(), 1, "{mode}");
        assert!(!flag(&edges[0].4, "signature_mismatch"), "{mode}");
        assert!(flag(&edges[0].4, "literal_object"), "{mode}");
        assert_eq!((r.edges_signature_mismatch, r.edges_swapped), (0, 0));
    }
}

// Edge case: several value facts about one entity in one chunk each stay on that entity.
#[tokio::test]
async fn several_value_facts_each_stay_on_their_entity() {
    let (r, edges, _) = run(
        Some(ontology(OntologyMode::Strict)),
        marlin_and_tomas(),
        vec![
            edge("Marlin Core", "Marlin Core", "HAS_MEASURE", BUDGET),
            edge(
                "Marlin Core",
                "Marlin Core",
                "HAS_MEASURE",
                "Marlin Core has a headcount of 12",
            ),
        ],
    )
    .await;
    assert_eq!(edges.len(), 2, "{edges:?}");
    assert!(edges
        .iter()
        .all(|e| e.0 == "Marlin Core" && e.2 == "Marlin Core"));
    assert_eq!(r.edges_literal_object, 2);
}
