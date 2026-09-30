//! `extract: false` assert-only types (issue #637, ADR-0637): never offered to, nor accepted from,
//! the LLM extractor — but fully declared for asserts, identity and ancestry.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    episode::{self, AddEpisodeResult},
    error::Error,
    extractor::{ConfigurableExtractor, ExtractOptions, Extractor},
    handlers,
    ipc::IpcRequest,
    ontology::{compute_ancestor_map, EntityTypeDef, Ontology, OntologyMode, RelationTypeDef},
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEdge, ExtractedEntity, ExtractionOutcome, ExtractionResult, SourceType},
    EntityRow,
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

fn etype(name: &str, identity: bool, extract: bool, parent: Option<&str>) -> EntityTypeDef {
    EntityTypeDef {
        name: name.to_string(),
        description: None,
        parent: parent.map(str::to_string),
        identity,
        extract,
    }
}

fn rtype(name: &str, extract: bool) -> RelationTypeDef {
    RelationTypeDef {
        name: name.to_string(),
        description: None,
        source_type: None,
        target_type: None,
        aliases: vec![],
        keywords: vec![],
        extract,
    }
}

/// `Person`, `Source` (identity, assert-only), `WikiPage` (child of `Source`), and relations
/// `KNOWS` / `DERIVED_FROM` (assert-only).
fn ontology(mode: OntologyMode) -> Ontology {
    let mut entity_types = vec![
        etype("Person", false, true, None),
        etype("Source", true, false, None),
        etype("WikiPage", false, true, Some("Source")),
    ];
    lcg_core::ontology::validate_and_clean_parents(&mut entity_types);
    let ancestor_map = compute_ancestor_map(&entity_types);
    Ontology {
        mode,
        entity_types,
        relation_types: vec![rtype("KNOWS", true), rtype("DERIVED_FROM", false)],
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

async fn ingest(state: &Arc<AppState>, n: usize) -> AddEpisodeResult {
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

fn entities(db: &Db) -> Vec<EntityRow> {
    db.connect()
        .unwrap()
        .get_entities_by_group_ids(Some(&[G]))
        .unwrap()
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

// ── US2: stray extractor output is never given an assert-only type ───────────

#[tokio::test]
async fn stray_extract_false_entity_is_reclassified_in_both_modes() {
    for mode in [OntologyMode::Strict, OntologyMode::Open] {
        for raw in ["Source", "source", "SOURCE"] {
            let (db, _d) = make_db();
            let state = make_state(
                Arc::clone(&db),
                Some(ontology(mode.clone())),
                vec![ExtractionResult {
                    entities: vec![ent("wiki/Home", raw), ent("Ada", "Person")],
                    edges: vec![],
                }],
            );
            let r = ingest(&state, 1).await;
            assert_eq!(r.entities_reclassified_unclassified, 1, "{mode} {raw}");

            let rows = entities(&db);
            let home = rows.iter().find(|e| e.name == "wiki/Home").unwrap();
            assert_ne!(home.kind, "Source", "{mode} {raw}: {home:?}");
            assert!(!home.labels.contains(&"Source".to_string()), "{home:?}");
            assert!(
                home.labels.contains(&"Unclassified".to_string()),
                "{home:?}"
            );
            let attrs: Value = serde_json::from_str(&home.attributes).unwrap();
            assert_eq!(attrs["original_entity_type"], raw, "{home:?}");
            // An extractable sibling is untouched.
            let ada = rows.iter().find(|e| e.name == "Ada").unwrap();
            assert!(ada.labels.contains(&"Person".to_string()));
        }
    }
}

#[tokio::test]
async fn stray_extract_false_edge_is_not_stored_as_that_type() {
    for mode in [OntologyMode::Strict, OntologyMode::Open] {
        let (db, _d) = make_db();
        let state = make_state(
            Arc::clone(&db),
            Some(ontology(mode.clone())),
            vec![ExtractionResult {
                entities: vec![ent("Ada", "Person"), ent("Bob", "Person")],
                edges: vec![
                    edge("Ada", "Bob", "derived_from"),
                    edge("Ada", "Bob", "KNOWS"),
                ],
            }],
        );
        let r = ingest(&state, 1).await;
        assert_eq!(r.edges_reclassified_unclassified, 1, "{mode}");
        let edges = db
            .connect()
            .unwrap()
            .get_edges_by_group_ids(Some(&[G]))
            .unwrap();
        assert!(
            edges.iter().all(|e| !e
                .relation_type
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case("derived_from"))),
            "{mode}: {edges:?}"
        );
        assert!(
            edges
                .iter()
                .any(|e| e.relation_type.as_deref() == Some("UNCLASSIFIED")),
            "{mode}: {edges:?}"
        );
        assert!(
            edges
                .iter()
                .any(|e| e.relation_type.as_deref() == Some("KNOWS")),
            "{mode}: {edges:?}"
        );
    }
}

/// Strict mode where every type is `extract: false` has no extractable vocabulary: the prompt
/// falls back to the default one, so ingest must not reclassify ordinary labels — only stray
/// labels naming an assert-only type.
#[tokio::test]
async fn strict_all_extract_false_does_not_reclassify_ordinary_labels() {
    let entity_types = vec![etype("Source", true, false, None)];
    let ancestor_map = compute_ancestor_map(&entity_types);
    let onto = Ontology {
        mode: OntologyMode::Strict,
        entity_types,
        relation_types: vec![rtype("DERIVED_FROM", false)],
        ancestor_map,
    };
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        Some(onto),
        vec![ExtractionResult {
            entities: vec![
                ent("Ada", "Person"),
                ent("Bob", "Person"),
                ent("wiki/Home", "Source"),
            ],
            edges: vec![
                edge("Ada", "Bob", "KNOWS"),
                edge("Ada", "Bob", "DERIVED_FROM"),
            ],
        }],
    );
    let r = ingest(&state, 1).await;
    assert_eq!(r.entities_reclassified_unclassified, 1);
    assert_eq!(r.edges_reclassified_unclassified, 1);

    let rows = entities(&db);
    let ada = rows.iter().find(|e| e.name == "Ada").unwrap();
    assert!(ada.labels.contains(&"Person".to_string()), "{ada:?}");
    let home = rows.iter().find(|e| e.name == "wiki/Home").unwrap();
    assert_ne!(home.kind, "Source", "{home:?}");
    assert!(
        home.labels.contains(&"Unclassified".to_string()),
        "{home:?}"
    );

    let edges = db
        .connect()
        .unwrap()
        .get_edges_by_group_ids(Some(&[G]))
        .unwrap();
    assert!(
        edges
            .iter()
            .any(|e| e.relation_type.as_deref() == Some("KNOWS")),
        "{edges:?}"
    );
    assert!(
        edges
            .iter()
            .all(|e| e.relation_type.as_deref() != Some("DERIVED_FROM")),
        "{edges:?}"
    );
}

#[tokio::test]
async fn open_mode_other_undeclared_labels_are_unchanged() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Open)),
        vec![ExtractionResult {
            entities: vec![ent("Zed", "Gadget")],
            edges: vec![],
        }],
    );
    let r = ingest(&state, 1).await;
    assert_eq!(r.entities_reclassified_unclassified, 0);
    let rows = entities(&db);
    let zed = rows.iter().find(|e| e.name == "Zed").unwrap();
    assert!(zed.labels.contains(&"Gadget".to_string()), "{zed:?}");
}

#[tokio::test]
async fn child_of_extract_false_parent_still_stamps_ancestry() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Open)),
        vec![ExtractionResult {
            entities: vec![ent("Home", "WikiPage")],
            edges: vec![],
        }],
    );
    ingest(&state, 1).await;
    let rows = entities(&db);
    let home = rows.iter().find(|e| e.name == "Home").unwrap();
    assert!(home.labels.contains(&"WikiPage".to_string()), "{home:?}");
    assert!(home.labels.contains(&"Source".to_string()), "{home:?}");
    // Ancestors never confer kind (#616) — and Source is assert-only anyway.
    assert_eq!(home.kind, "Entity");
}

// ── US3: hosts can still assert the types ─────────────────────────────────────

#[tokio::test]
async fn assert_paths_treat_extract_false_types_as_declared() {
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Strict)),
        vec![],
    );

    let a = call(
        &state,
        "knowledge_assert_entity",
        json!({"name": "wiki/Home", "group_id": G, "kind": "Source"}),
    )
    .await;
    assert!(a.get("error").is_none_or(Value::is_null), "{a}");
    let b = call(
        &state,
        "knowledge_assert_entity",
        json!({"name": "Ada", "group_id": G}),
    )
    .await;
    assert!(b.get("error").is_none_or(Value::is_null), "{b}");

    let rows = entities(&db);
    let home = rows.iter().find(|e| e.name == "wiki/Home").unwrap();
    assert_eq!(home.kind, "Source", "{home:?}");

    let rel = call(
        &state,
        "knowledge_assert_relationship",
        json!({
            "source_name": "wiki/Home",
            "target_name": "Ada",
            "predicate": "derived from",
            "relation_type": "DERIVED_FROM",
            "group_id": G,
            "source_kind": "Source",
        }),
    )
    .await;
    assert!(rel.get("error").is_none_or(Value::is_null), "{rel}");
    let edges = db
        .connect()
        .unwrap()
        .get_edges_by_group_ids(Some(&[G]))
        .unwrap();
    assert!(
        edges
            .iter()
            .any(|e| e.relation_type.as_deref() == Some("DERIVED_FROM")),
        "{edges:?}"
    );
}

#[test]
fn extract_false_identity_type_counts_toward_identity_set() {
    let o = ontology(OntologyMode::Strict);
    assert!(o.identity_set().contains("Source"));
    assert!(o.entity_type_names().contains("Source"));
    assert!(!o.extractable_entity_type_names().contains("Source"));
}

// ── US4: reprocess never retypes into an assert-only type ────────────────────

#[tokio::test]
async fn reprocess_relation_types_all_extract_false_errors_clearly() {
    let (db, _d) = make_db();
    let mut o = ontology(OntologyMode::Strict);
    for r in o.relation_types.iter_mut() {
        r.extract = false;
    }
    let state = make_state(Arc::clone(&db), Some(o), vec![]);
    let v = call(
        &state,
        "knowledge_reprocess_relation_types",
        json!({"group_id": G, "scope": "all"}),
    )
    .await;
    assert!(v.get("error").is_none_or(Value::is_null), "{v}");
    assert_eq!(v["result"]["success"], false, "{v}");
    assert!(
        v["result"]["error"]
            .as_str()
            .unwrap()
            .contains("extract: false"),
        "{v}"
    );
}

/// A misbehaving classifier: records the menu and the names it was asked about, and always
/// answers with the assert-only type.
#[derive(Default)]
struct SourceHappyClassifier {
    menus: Mutex<Vec<Vec<String>>>,
    asked: Mutex<Vec<String>>,
}

impl Extractor for SourceHappyClassifier {
    fn extract<'a>(
        &'a self,
        _opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        Box::pin(async { Ok(ExtractionResult::default().into()) })
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        allowed_types: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        self.menus
            .lock()
            .unwrap()
            .push(allowed_types.map(<[String]>::to_vec).unwrap_or_default());
        self.asked
            .lock()
            .unwrap()
            .extend(entities.iter().map(|(n, _)| n.to_string()));
        let count = entities.len();
        Box::pin(async move { Ok(vec!["Source".to_string(); count]) })
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        _allowed_types: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let count = edges.len();
        Box::pin(async move { Ok(vec!["DERIVED_FROM".to_string(); count]) })
    }
}

fn seed_entity(db: &Db, name: &str, labels: &[&str], kind: &str) {
    db.connect()
        .unwrap()
        .insert_entity(&EntityRow {
            uuid: format!("uuid-{name}"),
            name: name.into(),
            group_id: G.into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            kind: kind.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            name_embedding: vec![0.0; DIM],
            summary: String::new(),
            attributes: "{}".into(),
            ..Default::default()
        })
        .unwrap();
}

#[tokio::test]
async fn reprocess_entity_types_never_retypes_into_extract_false_type() {
    let (db, _d) = make_db();
    seed_entity(&db, "Gizmo", &["Entity", "Gadget"], "Entity"); // off-ontology candidate
    seed_entity(&db, "wiki/Home", &["Entity", "Source"], "Source"); // asserted, declared

    let classifier = Arc::new(SourceHappyClassifier::default());
    let state = make_state_with(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Strict)),
        Arc::clone(&classifier) as Arc<dyn Extractor>,
    );
    let v = call(
        &state,
        "knowledge_reprocess_entity_types",
        json!({"group_id": G, "scope": "off_ontology"}),
    )
    .await;
    assert!(v.get("error").is_none_or(Value::is_null), "{v}");

    // The menu never offers Source; the asserted Source entity is not an off-ontology candidate.
    for menu in classifier.menus.lock().unwrap().iter() {
        assert!(!menu.contains(&"Source".to_string()), "{menu:?}");
        assert!(menu.contains(&"Person".to_string()), "{menu:?}");
    }
    assert_eq!(*classifier.asked.lock().unwrap(), vec!["Gizmo".to_string()]);

    // Even though the classifier answered "Source", nothing was retyped into it.
    let rows = entities(&db);
    let gizmo = rows.iter().find(|e| e.name == "Gizmo").unwrap();
    assert!(!gizmo.labels.contains(&"Source".to_string()), "{gizmo:?}");
    assert_eq!(gizmo.kind, "Entity");
}

#[tokio::test]
async fn reprocess_relation_types_never_retypes_into_extract_false_type() {
    let (db, _d) = make_db();
    {
        let conn = db.connect().unwrap();
        for n in ["Ada", "Bob"] {
            seed_entity(&db, n, &["Entity"], "Entity");
        }
        conn.insert_relates_to_edge(&lcg_core::types::RelatesToEdge {
            uuid: "e1".into(),
            name: "rel".into(),
            source_node_uuid: "uuid-Ada".into(),
            target_node_uuid: "uuid-Bob".into(),
            group_id: G.into(),
            fact: "Ada relates to Bob".into(),
            fact_embedding: vec![1.0, 0.0, 0.0, 0.0],
            created_at: "2026-01-01 00:00:00".into(),
            attributes: "{}".into(),
            relation_type: Some("OTHER".into()),
            ..Default::default()
        })
        .unwrap();
    }
    let state = make_state_with(
        Arc::clone(&db),
        Some(ontology(OntologyMode::Strict)),
        Arc::new(SourceHappyClassifier::default()),
    );
    let v = call(
        &state,
        "knowledge_reprocess_relation_types",
        json!({"group_id": G, "scope": "off_ontology"}),
    )
    .await;
    assert!(v.get("error").is_none_or(Value::is_null), "{v}");
    let edges = db
        .connect()
        .unwrap()
        .get_edges_by_group_ids(Some(&[G]))
        .unwrap();
    assert!(
        edges
            .iter()
            .all(|e| e.relation_type.as_deref() != Some("DERIVED_FROM")),
        "{edges:?}"
    );
}
