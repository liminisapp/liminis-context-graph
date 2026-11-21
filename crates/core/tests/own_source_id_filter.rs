//! Own-source-ID entity filter (issue #824, ADR-0824): an extracted entity named after the
//! episode's own note ID is dropped, with the edges touching it; everything else is kept.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    episode,
    extractor::{ConfigurableExtractor, Extractor},
    handlers,
    ipc::IpcRequest,
    ontology::{compute_ancestor_map, EntityTypeDef, Ontology, OntologyMode, RelationTypeDef},
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEdge, ExtractedEntity, ExtractionResult, SourceType},
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "grp";

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("xf.db").to_str().unwrap()).unwrap());
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

fn rtype(name: &str) -> RelationTypeDef {
    RelationTypeDef {
        name: name.to_string(),
        description: None,
        source_types: vec![],
        target_types: vec![],
        inverse_aliases: vec![],
        symmetric: false,
        aliases: vec![],
        keywords: vec![],
        extract: true,
    }
}

/// An ontology with an extractable `Document` type — the invitation to extract the note itself.
fn ontology(mode: OntologyMode) -> Ontology {
    let entity_types = vec![etype("Person"), etype("Document")];
    let ancestor_map = compute_ancestor_map(&entity_types);
    Ontology {
        mode,
        entity_types,
        relation_types: vec![rtype("KNOWS"), rtype("CITES")],
        ancestor_map,
    }
}

/// A private workspace root (corrections handlers require one); leaked for the test's lifetime.
fn workspace_root() -> std::path::PathBuf {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    path
}

fn make_state(
    db: Arc<Db>,
    ontology: Option<Ontology>,
    extractions: Vec<ExtractionResult>,
) -> Arc<AppState> {
    make_state_with(
        db,
        ontology,
        Arc::new(ConfigurableExtractor::new(extractions)),
    )
}

fn make_state_with(
    db: Arc<Db>,
    ontology: Option<Ontology>,
    extractor: Arc<dyn Extractor>,
) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor,
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
        workspace_root: Some(workspace_root()),
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: ontology.map(Arc::new),
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

fn edge(src: &str, dst: &str, rt: &str) -> ExtractedEdge {
    ExtractedEdge {
        source_name: src.to_string(),
        target_name: dst.to_string(),
        fact: format!("{src} {rt} {dst}"),
        relation_type: Some(rt.to_string()),
        ..Default::default()
    }
}

fn entity_names(db: &Db) -> Vec<String> {
    let mut v: Vec<String> = db
        .connect()
        .unwrap()
        .get_entities_by_group_ids(Some(&[G]))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    v.sort();
    v
}

fn edge_facts(db: &Db) -> Vec<String> {
    let mut v: Vec<String> = db
        .connect()
        .unwrap()
        .get_edges_by_group_ids(Some(&[G]))
        .unwrap()
        .into_iter()
        .map(|e| e.fact)
        .collect();
    v.sort();
    v
}

async fn call(state: &Arc<AppState>, method: &str, params: Value) -> Value {
    let resp = handlers::dispatch(
        IpcRequest {
            jsonrpc: "2.0".into(),
            id: json!(1),
            method: method.into(),
            params,
        },
        Arc::clone(state),
        None,
    )
    .await;
    serde_json::to_value(resp).unwrap()
}

/// Mirrors the eval harness: `chunk_id = "{note_id}#0"`, `source_file = note_id`.
async fn process_chunk(state: &Arc<AppState>, note_id: &str) -> Value {
    call(
        state,
        "knowledge_process_chunk",
        json!({
            "chunk_text": "Ada works on Quill Core.",
            "chunk_id": format!("{note_id}#0"),
            "source_file": note_id,
            "group_id": G,
        }),
    )
    .await["result"]
        .clone()
}

fn extraction_with_note_entity(note_entity_name: &str) -> ExtractionResult {
    ExtractionResult {
        entities: vec![
            ent(note_entity_name, "Document"),
            ent("Ada", "Person"),
            ent("Quill Core", "Document"),
        ],
        edges: vec![
            edge("Ada", "Quill Core", "KNOWS"),
            edge(note_entity_name, "Ada", "CITES"),
            edge("Quill Core", note_entity_name, "CITES"),
        ],
    }
}

// FR-008 / US1 scenario 1-3: a chunk whose header carries a note ID yields no entity of that name,
// edges touching it are discarded, and the rest of the extraction survives.
#[tokio::test]
async fn note_id_entity_is_dropped_and_rest_survives() {
    for mode in [OntologyMode::Open, OntologyMode::Strict] {
        let (db, _d) = make_db();
        let state = make_state(
            Arc::clone(&db),
            Some(ontology(mode.clone())),
            vec![extraction_with_note_entity("project-quill-core-1")],
        );
        let r = process_chunk(&state, "project-quill-core-1").await;
        assert_eq!(r["success"], json!(true), "{mode}: {r}");
        assert_eq!(r["entities_dropped_source_id"], json!(1), "{mode}: {r}");
        assert_eq!(r["nodes_extracted"], json!(2), "{mode}: {r}");
        assert_eq!(r["edges_dropped_unresolvable"], json!(0), "{mode}: {r}");
        assert_eq!(entity_names(&db), vec!["Ada", "Quill Core"], "{mode}");
        assert_eq!(
            edge_facts(&db),
            vec!["Ada KNOWS Quill Core"],
            "{mode}: edges touching the dropped entity must not survive or be salvaged"
        );
    }
}

// FR-004: case, extension, path prefix, wikilink wrapping.
#[tokio::test]
async fn note_id_variants_are_dropped() {
    for variant in [
        "Project-Quill-Core-1",
        "project-quill-core-1.md",
        "notes/derived/project-quill-core-1.md",
        "[[project-quill-core-1]]",
        "  `project-quill-core-1` ",
    ] {
        let (db, _d) = make_db();
        let state = make_state(
            Arc::clone(&db),
            Some(ontology(OntologyMode::Open)),
            vec![extraction_with_note_entity(variant)],
        );
        let r = process_chunk(&state, "project-quill-core-1").await;
        assert_eq!(r["entities_dropped_source_id"], json!(1), "{variant}: {r}");
        assert_eq!(entity_names(&db), vec!["Ada", "Quill Core"], "{variant}");
    }
}

// FR-007: another note's ID is a legitimate entity and is kept.
#[tokio::test]
async fn other_notes_id_is_kept() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Open)),
        vec![extraction_with_note_entity("project-quill-core-2")],
    );
    let r = process_chunk(&state, "project-quill-core-1").await;
    assert_eq!(r["entities_dropped_source_id"], json!(0), "{r}");
    assert_eq!(
        entity_names(&db),
        vec!["Ada", "Quill Core", "project-quill-core-2"]
    );
}

// A note filed under its subject's name (`people/Alice.md`) has no slug-shaped ID: the real `Alice`
// entity the note is about must survive, with its edges.
#[tokio::test]
async fn subject_named_note_does_not_drop_the_subject_entity() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Open)),
        vec![ExtractionResult {
            entities: vec![ent("Alice", "Person"), ent("Bob", "Person")],
            edges: vec![edge("Alice", "Bob", "KNOWS")],
        }],
    );
    let r = process_chunk(&state, "people/Alice.md").await;
    assert_eq!(r["entities_dropped_source_id"], json!(0), "{r}");
    assert_eq!(entity_names(&db), vec!["Alice", "Bob"]);
    assert_eq!(edge_facts(&db), vec!["Alice KNOWS Bob"]);
}

// FR-009: a free-form `knowledge_add_episode` name is not a source ID — an entity that happens to
// share it is kept — but an explicit `doc_id` still applies on every path.
#[tokio::test]
async fn free_form_episode_name_is_not_a_source_id() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Open)),
        vec![ExtractionResult {
            entities: vec![ent("Alice", "Person"), ent("Bob", "Person")],
            edges: vec![edge("Alice", "Bob", "KNOWS")],
        }],
    );
    let r = episode::add_episode(
        Arc::clone(&state),
        "Alice",
        "body",
        "text",
        "Alice:Alice",
        "2026-01-01T00:00:00Z",
        G,
        SourceType::Text,
        None,
        "",
    )
    .await
    .unwrap();
    assert_eq!(r.entities_dropped_source_id, 0);
    assert_eq!(entity_names(&db), vec!["Alice", "Bob"]);
    assert_eq!(edge_facts(&db), vec!["Alice KNOWS Bob"]);
}

#[tokio::test]
async fn doc_id_applies_on_the_free_form_path() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        None,
        vec![ExtractionResult {
            entities: vec![ent("note-7", "Document"), ent("Bob", "Person")],
            edges: vec![edge("note-7", "Bob", "KNOWS")],
        }],
    );
    let r = episode::add_episode_with_opts(
        Arc::clone(&state),
        "free form name",
        "body",
        "text",
        "free form",
        "2026-01-01T00:00:00Z",
        G,
        SourceType::Text,
        None,
        "",
        episode::EpisodeOpts {
            doc_id: Some("note-7.md".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.entities_dropped_source_id, 1);
    assert_eq!(entity_names(&db), vec!["Bob"]);
    assert!(edge_facts(&db).is_empty());
}
