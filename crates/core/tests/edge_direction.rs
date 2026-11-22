//! Ingest-time edge direction check (#816, ADR-0816): endpoints are compared with the relation's
//! declared signature after the relation type is resolved — kept, swapped, or marked
//! `signature_mismatch`, and never dropped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    episode::{self, AddEpisodeResult},
    extractor::ConfigurableExtractor,
    ontology::{compute_ancestor_map, EntityTypeDef, Ontology, OntologyMode, RelationTypeDef},
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEdge, ExtractedEntity, ExtractionResult, SourceType},
    RelatesToEdge,
};
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "grp";

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("dir.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(DIM).unwrap();
    (db, dir)
}

fn etype(name: &str, parent: Option<&str>) -> EntityTypeDef {
    EntityTypeDef {
        name: name.to_string(),
        description: None,
        parent: parent.map(str::to_string),
        identity: false,
        extract: true,
    }
}

fn rtype(
    name: &str,
    src: &[&str],
    tgt: &[&str],
    inverse: &[&str],
    symmetric: bool,
) -> RelationTypeDef {
    RelationTypeDef {
        name: name.to_string(),
        description: None,
        source_types: src.iter().map(|s| s.to_string()).collect(),
        target_types: tgt.iter().map(|s| s.to_string()).collect(),
        inverse_aliases: inverse.iter().map(|s| s.to_string()).collect(),
        symmetric,
        aliases: vec![],
        keywords: vec![],
        extract: true,
    }
}

/// Person (parent of Employee), Policy, Project, Topic.
fn ontology(mode: OntologyMode) -> Ontology {
    let entity_types = vec![
        etype("Person", None),
        etype("Employee", Some("Person")),
        etype("Policy", None),
        etype("Project", None),
        etype("Topic", None),
    ];
    let ancestor_map = compute_ancestor_map(&entity_types);
    Ontology {
        mode,
        entity_types,
        relation_types: vec![
            rtype("APPROVES", &["Person"], &[], &["APPROVED_BY"], false),
            rtype("OWNS", &["Person"], &["Project", "Policy"], &[], false),
            rtype("KNOWS", &["Person"], &["Policy"], &[], true),
            rtype("WORKS_ON", &["Person"], &["Project"], &[], false),
            rtype("USES", &[], &[], &[], false),
            rtype("OWNED_BY_NOSIG", &[], &[], &[], false),
        ],
        ancestor_map,
    }
}

fn make_state(db: Arc<Db>, onto: Ontology, extractions: Vec<ExtractionResult>) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let root = TempDir::new().unwrap();
    let path = root.path().to_path_buf();
    std::mem::forget(root);
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor: Arc::new(ConfigurableExtractor::new(extractions)),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: None,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: Some(path),
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: Some(Arc::new(onto)),
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        chunk_inflight: Default::default(),
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

fn edge(src: &str, dst: &str, rt: &str) -> ExtractedEdge {
    ExtractedEdge {
        source_name: src.to_string(),
        target_name: dst.to_string(),
        fact: format!("{src} {rt} {dst}"),
        relation_type: Some(rt.to_string()),
        ..Default::default()
    }
}

async fn ingest(state: &Arc<AppState>) -> AddEpisodeResult {
    episode::add_episode(
        Arc::clone(state),
        "ep-1",
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

/// Runs one ingest of `entities` + `edges` and returns the result plus stored edges resolved to
/// `(source name, relation_type, target name, attributes)`.
async fn run(
    mode: OntologyMode,
    entities: Vec<ExtractedEntity>,
    edges: Vec<ExtractedEdge>,
) -> (
    AddEpisodeResult,
    Vec<(String, String, String, Value)>,
    Vec<RelatesToEdge>,
) {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        ontology(mode),
        vec![ExtractionResult { entities, edges }],
    );
    let r = ingest(&state).await;
    let conn = db.connect().unwrap();
    let stored = conn.get_edges_by_group_ids(Some(&[G])).unwrap();
    let names: HashMap<String, String> = conn
        .get_entities_by_group_ids(Some(&[G]))
        .unwrap()
        .into_iter()
        .map(|e| (e.uuid, e.name))
        .collect();
    let resolved = stored
        .iter()
        .map(|e| {
            (
                names[&e.source_node_uuid].clone(),
                e.relation_type.clone().unwrap_or_default(),
                names[&e.target_node_uuid].clone(),
                serde_json::from_str(&e.attributes).unwrap_or(Value::Null),
            )
        })
        .collect();
    (r, resolved, stored)
}

fn people_and_policy() -> Vec<ExtractedEntity> {
    vec![ent("Lena", "Person"), ent("Lumen Policy", "Policy")]
}

// (a) correct orientation is kept
#[tokio::test]
async fn a_correct_orientation_is_kept() {
    for mode in [OntologyMode::Strict, OntologyMode::Open] {
        let (r, edges, _) = run(
            mode.clone(),
            people_and_policy(),
            vec![edge("Lena", "Lumen Policy", "APPROVES")],
        )
        .await;
        assert_eq!(edges.len(), 1, "{mode}");
        assert_eq!(edges[0].0, "Lena", "{mode}");
        assert_eq!(edges[0].2, "Lumen Policy", "{mode}");
        assert_eq!((r.edges_swapped, r.edges_signature_mismatch), (0, 0));
        assert!(edges[0].3.get("signature_mismatch").is_none());
    }
}

// (b) reversed `Policy APPROVES Person` with `source_type: Person` is swapped
#[tokio::test]
async fn b_reversed_edge_is_swapped_and_counted() {
    for mode in [OntologyMode::Strict, OntologyMode::Open] {
        let (r, edges, stored) = run(
            mode.clone(),
            people_and_policy(),
            vec![edge("Lumen Policy", "Lena", "APPROVES")],
        )
        .await;
        assert_eq!(edges.len(), 1, "{mode}: edge count preserved");
        assert_eq!(edges[0].0, "Lena", "{mode}");
        assert_eq!(edges[0].1, "APPROVES");
        assert_eq!(edges[0].2, "Lumen Policy", "{mode}");
        assert_eq!(r.edges_swapped, 1, "{mode}");
        assert_eq!(r.edges_signature_mismatch, 0);
        assert_eq!(
            stored[0].name, "Lena → Lumen Policy",
            "name follows the swap"
        );
    }
}

// (c) an inverse alias (`APPROVED_BY`) is stored as `Person APPROVES Policy`
#[tokio::test]
async fn c_inverse_alias_is_retyped_and_swapped() {
    let (r, edges, _) = run(
        OntologyMode::Strict,
        people_and_policy(),
        vec![edge("Lumen Policy", "Lena", "APPROVED_BY")],
    )
    .await;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].0, "Lena");
    assert_eq!(edges[0].1, "APPROVES");
    assert_eq!(edges[0].2, "Lumen Policy");
    // The inverse swap already produced the right orientation, so no signature swap is counted.
    assert_eq!(r.edges_swapped, 0);
    assert_eq!(r.edges_signature_mismatch, 0);
}

// (d) a neither-way edge is stored unchanged and marked `signature_mismatch`
#[tokio::test]
async fn d_neither_way_edge_is_stored_unchanged_and_marked() {
    for mode in [OntologyMode::Strict, OntologyMode::Open] {
        let (r, edges, _) = run(
            mode.clone(),
            vec![ent("Quill", "Project"), ent("Pricing", "Topic")],
            vec![edge("Quill", "Pricing", "WORKS_ON")],
        )
        .await;
        assert_eq!(edges.len(), 1, "{mode}: never dropped");
        assert_eq!(edges[0].0, "Quill", "{mode}: unchanged");
        assert_eq!(edges[0].2, "Pricing", "{mode}");
        assert_eq!(edges[0].3["signature_mismatch"], true, "{mode}");
        assert_eq!(
            (r.edges_swapped, r.edges_signature_mismatch),
            (0, 1),
            "{mode}"
        );
    }
}

// (e) a symmetric type is untouched
#[tokio::test]
async fn e_symmetric_type_is_untouched() {
    let (r, edges, _) = run(
        OntologyMode::Strict,
        people_and_policy(),
        vec![edge("Lumen Policy", "Lena", "KNOWS")],
    )
    .await;
    assert_eq!(edges[0].0, "Lumen Policy");
    assert_eq!(edges[0].2, "Lena");
    assert!(edges[0].3.get("signature_mismatch").is_none());
    assert_eq!((r.edges_swapped, r.edges_signature_mismatch), (0, 0));
}

// (f) parent types match through ancestry
#[tokio::test]
async fn f_parent_types_match_through_ancestry() {
    for mode in [OntologyMode::Strict, OntologyMode::Open] {
        let (r, edges, _) = run(
            mode.clone(),
            vec![ent("Ada", "Employee"), ent("Lumen Policy", "Policy")],
            vec![edge("Lumen Policy", "Ada", "APPROVES")],
        )
        .await;
        assert_eq!(
            edges[0].0, "Ada",
            "{mode}: Employee matches Person via ancestry"
        );
        assert_eq!(edges[0].2, "Lumen Policy", "{mode}");
        assert_eq!(r.edges_swapped, 1, "{mode}");
    }
}

#[tokio::test]
async fn relation_without_signature_and_untyped_endpoint_are_skipped() {
    let (r, edges, _) = run(
        OntologyMode::Strict,
        vec![
            ent("Lena", "Person"),
            ent("Thing", ""),
            ent("Lumen Policy", "Policy"),
        ],
        vec![
            edge("Lumen Policy", "Lena", "USES"),
            edge("Thing", "Lena", "APPROVES"),
        ],
    )
    .await;
    assert_eq!(edges.len(), 2);
    let uses = edges.iter().find(|e| e.1 == "USES").unwrap();
    assert_eq!((uses.0.as_str(), uses.2.as_str()), ("Lumen Policy", "Lena"));
    let appr = edges.iter().find(|e| e.1 == "APPROVES").unwrap();
    assert_eq!((appr.0.as_str(), appr.2.as_str()), ("Thing", "Lena"));
    assert_eq!((r.edges_swapped, r.edges_signature_mismatch), (0, 0));
}

#[tokio::test]
async fn mixed_batch_never_drops_an_edge() {
    let (r, edges, _) = run(
        OntologyMode::Strict,
        vec![
            ent("Lena", "Person"),
            ent("Lumen Policy", "Policy"),
            ent("Quill", "Project"),
            ent("Pricing", "Topic"),
        ],
        vec![
            edge("Lena", "Lumen Policy", "APPROVES"),
            edge("Lumen Policy", "Lena", "APPROVES"),
            edge("Quill", "Pricing", "WORKS_ON"),
            edge("Lumen Policy", "Lena", "APPROVED_BY"),
        ],
    )
    .await;
    assert_eq!(edges.len(), 4);
    assert_eq!(r.edges_extracted, 4);
    assert_eq!(r.edges_swapped, 1);
    assert_eq!(r.edges_signature_mismatch, 1);
    assert!(edges
        .iter()
        .filter(|e| e.1 == "APPROVES")
        .all(|e| e.0 == "Lena"));
}
