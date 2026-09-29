//! Ontology-driven kind assignment (issue #616, ADR-0616): identity-bearing entity types become
//! the entity's `kind` during extraction, and a change to a group's identity-bearing set that
//! would reinterpret existing entities is refused (D1) rather than guessed at.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    episode::{self, AddEpisodeResult},
    error::Error,
    extractor::ConfigurableExtractor,
    handlers, identity_stamp,
    ipc::IpcRequest,
    ontology::{compute_ancestor_map, EntityTypeDef, Ontology, OntologyMode},
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEdge, ExtractedEntity, ExtractionResult, SourceType},
    EntityRow,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "grp";

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("identity.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
    }
    (db, dir)
}

/// `(name, identity, parent)`
fn ontology(mode: OntologyMode, types: &[(&str, bool, Option<&str>)]) -> Ontology {
    let mut entity_types: Vec<EntityTypeDef> = types
        .iter()
        .map(|(n, id, p)| EntityTypeDef {
            name: n.to_string(),
            description: None,
            parent: p.map(|s| s.to_string()),
            identity: *id,
        })
        .collect();
    lcg_core::ontology::validate_and_clean_parents(&mut entity_types);
    let ancestor_map = compute_ancestor_map(&entity_types);
    Ontology {
        mode,
        entity_types,
        relation_types: vec![],
        ancestor_map,
    }
}

fn make_state(
    db: Arc<Db>,
    root: Option<&Path>,
    ontology: Option<Ontology>,
    extractions: Vec<ExtractionResult>,
) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
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
        workspace_root: root.map(|r| r.to_path_buf()),
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

async fn ingest(state: &Arc<AppState>, group: &str, n: usize) -> Result<AddEpisodeResult, Error> {
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
}

fn entities(db: &Db, group: &str) -> Vec<EntityRow> {
    let conn = db.connect().unwrap();
    let mut rows = conn.get_entities_by_group_ids(Some(&[group])).unwrap();
    rows.sort_by(|a, b| (&a.name, &a.kind).cmp(&(&b.name, &b.kind)));
    rows
}

fn aurora_rows(db: &Db) -> Vec<EntityRow> {
    entities(db, G)
        .into_iter()
        .filter(|e| e.name == "Aurora")
        .collect()
}

fn edge_count(db: &Db) -> usize {
    let conn = db.connect().unwrap();
    conn.get_edges_by_group_ids(Some(&[G])).unwrap().len()
}

// ── US1: SC-001 / SC-002 ──────────────────────────────────────────────────────

#[tokio::test]
async fn person_and_technology_aurora_stay_distinct() {
    let (db, _d) = make_db();
    let onto = ontology(
        OntologyMode::Open,
        &[("Person", true, None), ("Technology", false, None)],
    );
    let state = make_state(
        Arc::clone(&db),
        None,
        Some(onto),
        vec![
            extraction(vec![ent("Aurora", "Person")], vec![]),
            extraction(vec![ent("Aurora", "Technology")], vec![]),
        ],
    );
    ingest(&state, G, 1).await.unwrap();
    ingest(&state, G, 2).await.unwrap();

    let rows = aurora_rows(&db);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let person = rows.iter().find(|r| r.kind == "Person").unwrap();
    assert!(person.labels.contains(&"Person".to_string()));
    let tech = rows.iter().find(|r| r.kind == "Entity").unwrap();
    assert!(tech.labels.contains(&"Technology".to_string()));
    assert!(!tech.labels.contains(&"Person".to_string()));
}

#[tokio::test]
async fn no_identity_flags_merges_by_name_as_before() {
    let (db, _d) = make_db();
    let onto = ontology(
        OntologyMode::Open,
        &[("Person", false, None), ("Technology", false, None)],
    );
    let state = make_state(
        Arc::clone(&db),
        None,
        Some(onto),
        vec![
            extraction(vec![ent("Aurora", "Person")], vec![]),
            extraction(vec![ent("Aurora", "Technology")], vec![]),
        ],
    );
    ingest(&state, G, 1).await.unwrap();
    ingest(&state, G, 2).await.unwrap();
    let rows = aurora_rows(&db);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].kind, "Entity");
}

#[tokio::test]
async fn same_identity_kind_resolves_to_one_entity() {
    let (db, _d) = make_db();
    let onto = ontology(OntologyMode::Open, &[("Person", true, None)]);
    let state = make_state(
        Arc::clone(&db),
        None,
        Some(onto),
        vec![
            extraction(vec![ent("Aurora", "Person")], vec![]),
            extraction(vec![ent("aurora", "Person")], vec![]),
        ],
    );
    ingest(&state, G, 1).await.unwrap();
    ingest(&state, G, 2).await.unwrap();
    let rows: Vec<_> = entities(&db, G)
        .into_iter()
        .filter(|e| e.name.eq_ignore_ascii_case("aurora"))
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].kind, "Person");
}

#[tokio::test]
async fn identity_kind_never_merges_into_existing_default_kind_entity() {
    let (db, _d) = make_db();
    let onto = ontology(
        OntologyMode::Open,
        &[("Person", true, None), ("Technology", false, None)],
    );
    let state = make_state(
        Arc::clone(&db),
        None,
        Some(onto),
        vec![
            extraction(vec![ent("Aurora", "Technology")], vec![]),
            extraction(vec![ent("Aurora", "Person")], vec![]),
            // and a later default-kind Aurora merges into the Technology one, not the Person
            extraction(vec![ent("Aurora", "Technology")], vec![]),
        ],
    );
    ingest(&state, G, 1).await.unwrap();
    ingest(&state, G, 2).await.unwrap();
    ingest(&state, G, 3).await.unwrap();
    let rows = aurora_rows(&db);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let person = rows.iter().find(|r| r.kind == "Person").unwrap();
    assert!(
        !person.summary.contains("Technology"),
        "Person must not absorb Technology text: {person:?}"
    );
}

#[tokio::test]
async fn open_mode_raw_type_spelling_normalizes_for_kind_and_label() {
    let (db, _d) = make_db();
    let onto = ontology(OntologyMode::Open, &[("Person", true, None)]);
    let state = make_state(
        Arc::clone(&db),
        None,
        Some(onto),
        vec![extraction(vec![ent("Ada", "person")], vec![])],
    );
    ingest(&state, G, 1).await.unwrap();
    let rows = entities(&db, "grp");
    let ada = rows.iter().find(|r| r.name == "Ada").unwrap();
    assert_eq!(ada.kind, "Person");
    assert!(ada.labels.contains(&"Person".to_string()), "{ada:?}");
    assert_eq!(ada.labels.first().map(String::as_str), Some("Entity"));
}

// ── FR-003 / SC-007: primary type only, ancestors never confer kind ───────────

#[tokio::test]
async fn ancestor_identity_does_not_confer_kind() {
    for reversed in [false, true] {
        let (db, _d) = make_db();
        let mut types = vec![("Document", true, None), ("Rfc", false, Some("Document"))];
        if reversed {
            types.reverse();
        }
        let state = make_state(
            Arc::clone(&db),
            None,
            Some(ontology(OntologyMode::Open, &types)),
            vec![extraction(
                vec![ent("RFC 9110", "Rfc"), ent("Spec", "Document")],
                vec![],
            )],
        );
        ingest(&state, G, 1).await.unwrap();
        let rows = entities(&db, G);
        let rfc = rows.iter().find(|r| r.name == "RFC 9110").unwrap();
        assert_eq!(rfc.kind, "Entity", "reversed={reversed}: {rfc:?}");
        assert!(rfc.labels.contains(&"Document".to_string()));
        assert!(rfc.labels.contains(&"Rfc".to_string()));
        let spec = rows.iter().find(|r| r.name == "Spec").unwrap();
        assert_eq!(spec.kind, "Document", "reversed={reversed}");
    }
}

#[tokio::test]
async fn unclassified_and_undeclared_types_are_default_kind() {
    // strict mode: undeclared → Unclassified
    let (db, _d) = make_db();
    let state = make_state(
        Arc::clone(&db),
        None,
        Some(ontology(OntologyMode::Strict, &[("Person", true, None)])),
        vec![extraction(
            vec![ent("Widget", "Gadget"), ent("Bob", "Person")],
            vec![],
        )],
    );
    ingest(&state, G, 1).await.unwrap();
    let rows = entities(&db, G);
    assert_eq!(
        rows.iter().find(|r| r.name == "Widget").unwrap().kind,
        "Entity"
    );
    assert_eq!(
        rows.iter().find(|r| r.name == "Bob").unwrap().kind,
        "Person"
    );

    // open mode: undeclared stays default too
    let (db2, _d2) = make_db();
    let state2 = make_state(
        Arc::clone(&db2),
        None,
        Some(ontology(OntologyMode::Open, &[("Person", true, None)])),
        vec![extraction(
            vec![ent("Widget", "Gadget"), ent("Nobody", "")],
            vec![],
        )],
    );
    ingest(&state2, G, 1).await.unwrap();
    for r in entities(&db2, G) {
        assert_eq!(r.kind, "Entity", "{r:?}");
    }
}

// ── Edge endpoints ────────────────────────────────────────────────────────────

#[tokio::test]
async fn edges_to_identity_kind_entities_resolve_in_batch_and_cross_batch() {
    let (db, _d) = make_db();
    let onto = ontology(OntologyMode::Open, &[("Person", true, None)]);
    let state = make_state(
        Arc::clone(&db),
        None,
        Some(onto),
        vec![
            // in-batch
            extraction(
                vec![ent("Aurora", "Person"), ent("Widget", "Gadget")],
                vec![edge("Aurora", "Widget")],
            ),
            // cross-batch: Aurora (Person) is not in this batch
            extraction(vec![ent("Gizmo", "Gadget")], vec![edge("Gizmo", "Aurora")]),
        ],
    );
    let r1 = ingest(&state, G, 1).await.unwrap();
    assert_eq!(r1.edges_dropped_unresolvable, 0);
    assert_eq!(edge_count(&db), 1);
    let r2 = ingest(&state, G, 2).await.unwrap();
    assert_eq!(r2.edges_dropped_unresolvable, 0, "{:?}", r2.dropped_edges);
    assert_eq!(edge_count(&db), 2);
}

#[tokio::test]
async fn ambiguous_same_name_endpoint_drops_the_edge() {
    let (db, _d) = make_db();
    let onto = ontology(OntologyMode::Open, &[("Person", true, None)]);
    let state = make_state(
        Arc::clone(&db),
        None,
        Some(onto),
        vec![extraction(
            vec![
                ent("Aurora", "Person"),
                ent("Aurora", "Technology"),
                ent("Widget", "Gadget"),
            ],
            vec![edge("Widget", "Aurora")],
        )],
    );
    let r = ingest(&state, G, 1).await.unwrap();
    assert_eq!(r.edges_dropped_unresolvable, 1);
    assert_eq!(edge_count(&db), 0);
    assert_eq!(aurora_rows(&db).len(), 2);
}

#[tokio::test]
async fn ambiguous_cross_batch_endpoint_drops_the_edge() {
    let (db, _d) = make_db();
    let onto = ontology(OntologyMode::Open, &[("Person", true, None)]);
    let state = make_state(
        Arc::clone(&db),
        None,
        Some(onto),
        vec![
            extraction(
                vec![ent("Aurora", "Person"), ent("Aurora", "Technology")],
                vec![],
            ),
            extraction(
                vec![ent("Widget", "Gadget")],
                vec![edge("Widget", "Aurora")],
            ),
        ],
    );
    ingest(&state, G, 1).await.unwrap();
    let r = ingest(&state, G, 2).await.unwrap();
    assert_eq!(r.edges_dropped_unresolvable, 1);
    assert_eq!(edge_count(&db), 0);
}

// ── US2 / SC-006: flagless workspaces are undisturbed ─────────────────────────

#[tokio::test]
async fn flagless_workspace_writes_no_identity_stamp() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let state = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", false, None)])),
        vec![extraction(vec![ent("Ada", "Person")], vec![])],
    );
    ingest(&state, G, 1).await.unwrap();
    assert!(!root.path().join(".lcg").join("identity-set").exists());
    assert_eq!(entities(&db, G)[0].kind, "Entity");
}

// ── US3: D1 guard ─────────────────────────────────────────────────────────────

fn refused(r: Result<AddEpisodeResult, Error>) -> (String, String, usize) {
    match r {
        Err(Error::IdentitySetChangeRefused {
            group_id,
            label,
            count,
            message,
        }) => {
            assert!(message.contains(&label), "{message}");
            assert!(message.contains("re-ingest"), "{message}");
            (group_id, label, count)
        }
        Err(e) => panic!("expected identity refusal, got {e}"),
        Ok(_) => panic!("expected identity refusal, ingest succeeded"),
    }
}

#[tokio::test]
async fn flag_on_unused_type_is_accepted_and_takes_effect() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    // Seed: the group only ever extracted Technology entities, flagless.
    let s1 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(
            OntologyMode::Open,
            &[("Technology", false, None), ("Organization", false, None)],
        )),
        vec![extraction(vec![ent("Aurora", "Technology")], vec![])],
    );
    ingest(&s1, G, 1).await.unwrap();

    // Later `Organization` gains the flag: no existing entity carries that label.
    let s2 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(
            OntologyMode::Open,
            &[("Technology", false, None), ("Organization", true, None)],
        )),
        vec![extraction(vec![ent("Acme", "Organization")], vec![])],
    );
    ingest(&s2, G, 2).await.unwrap();
    let stamp = identity_stamp::read_stamp(root.path(), G).unwrap();
    assert_eq!(stamp.into_iter().collect::<Vec<_>>(), vec!["Organization"]);
    let acme = entities(&db, G)
        .into_iter()
        .find(|e| e.name == "Acme")
        .unwrap();
    assert_eq!(acme.kind, "Organization");
}

#[tokio::test]
async fn adding_flag_on_carried_label_is_refused_without_modifying_data() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let s1 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", false, None)])),
        vec![extraction(
            vec![ent("Ada", "Person"), ent("Bob", "Person")],
            vec![],
        )],
    );
    ingest(&s1, G, 1).await.unwrap();
    let before = entities(&db, G);

    let s2 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", true, None)])),
        vec![extraction(vec![ent("Cy", "Person")], vec![])],
    );
    let (group, label, count) = refused(ingest(&s2, G, 2).await);
    assert_eq!((group.as_str(), label.as_str(), count), (G, "Person", 2));

    // Nothing was modified and no stamp was recorded for the refused set.
    let after = entities(&db, G);
    assert_eq!(before.len(), after.len());
    assert!(after.iter().all(|e| e.kind == "Entity"));
    assert!(identity_stamp::read_stamp(root.path(), G)
        .unwrap()
        .is_empty());

    // Refusal is per group: another group with no Person entities is unaffected.
    ingest(&s2, "other", 3).await.unwrap();

    // knowledge_status reports it.
    let v = serde_json::to_value(
        handlers::dispatch(
            IpcRequest {
                jsonrpc: "2.0".into(),
                id: json!(1),
                method: "knowledge_status".into(),
                params: json!({}),
            },
            Arc::clone(&s2),
            None,
        )
        .await,
    )
    .unwrap();
    let refusals = v["result"]["group_identity_refusals"].as_array().unwrap();
    assert_eq!(refusals.len(), 1, "{v}");
    assert_eq!(refusals[0]["group_id"], G);

    // Restoring the recorded (empty) set loads normally.
    let s3 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", false, None)])),
        vec![extraction(vec![ent("Di", "Person")], vec![])],
    );
    ingest(&s3, G, 4).await.unwrap();
}

#[tokio::test]
async fn removing_flag_on_carried_label_is_refused() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let s1 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", true, None)])),
        vec![extraction(vec![ent("Ada", "Person")], vec![])],
    );
    ingest(&s1, G, 1).await.unwrap();
    assert_eq!(
        identity_stamp::read_stamp(root.path(), G)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec!["Person"]
    );

    let s2 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", false, None)])),
        vec![extraction(vec![ent("Bob", "Person")], vec![])],
    );
    let (_, label, count) = refused(ingest(&s2, G, 2).await);
    assert_eq!((label.as_str(), count), ("Person", 1));
    // Stamp untouched by the refused change.
    assert_eq!(identity_stamp::read_stamp(root.path(), G).unwrap().len(), 1);
}

#[tokio::test]
async fn asserted_entity_and_ancestor_label_count_as_carried() {
    // Asserted Person entity: kind Person carries the Person label.
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    {
        let conn = db.connect().unwrap();
        conn.insert_entity(&EntityRow {
            uuid: "u1".into(),
            name: "Ada".into(),
            group_id: G.into(),
            labels: vec!["Entity".into(), "Person".into()],
            kind: "Person".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            name_embedding: vec![0.0; DIM],
            summary: String::new(),
            attributes: "{}".into(),
            ..Default::default()
        })
        .unwrap();
    }
    let s = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", true, None)])),
        vec![],
    );
    let (_, label, _) = refused(ingest(&s, G, 1).await);
    assert_eq!(label, "Person");

    // Ancestor label: an Rfc entity carries Document.
    let (db2, _d2) = make_db();
    let root2 = TempDir::new().unwrap();
    let hier = |doc_identity| {
        ontology(
            OntologyMode::Open,
            &[
                ("Document", doc_identity, None),
                ("Rfc", false, Some("Document")),
            ],
        )
    };
    let s1 = make_state(
        Arc::clone(&db2),
        Some(root2.path()),
        Some(hier(false)),
        vec![extraction(vec![ent("RFC 1", "Rfc")], vec![])],
    );
    ingest(&s1, G, 1).await.unwrap();
    let s2 = make_state(
        Arc::clone(&db2),
        Some(root2.path()),
        Some(hier(true)),
        vec![],
    );
    let (_, label, count) = refused(ingest(&s2, G, 2).await);
    assert_eq!((label.as_str(), count), ("Document", 1));
}

#[tokio::test]
async fn per_group_ontology_file_is_checked_like_the_workspace_fallback() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let s1 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", false, None)])),
        vec![extraction(vec![ent("Ada", "Person")], vec![])],
    );
    ingest(&s1, G, 1).await.unwrap();

    // The group now resolves through its own file, which flags Person.
    let path = lcg_core::ontology::group_ontology_path(root.path(), G).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "entity_types:\n  - name: Person\n    identity: true\n",
    )
    .unwrap();
    let s2 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", false, None)])),
        vec![extraction(vec![ent("Bob", "Person")], vec![])],
    );
    let (_, label, _) = refused(ingest(&s2, G, 2).await);
    assert_eq!(label, "Person");
    // A group without its own file still resolves through the (flagless) fallback.
    ingest(&s2, "elsewhere", 3).await.unwrap();
}

#[tokio::test]
async fn stamp_survives_when_sets_are_unchanged_across_restarts() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    for n in 0..2 {
        let s = make_state(
            Arc::clone(&db),
            Some(root.path()),
            Some(ontology(OntologyMode::Open, &[("Person", true, None)])),
            vec![extraction(vec![ent("Ada", "Person")], vec![])],
        );
        ingest(&s, G, n).await.unwrap();
    }
    let rows: Vec<_> = entities(&db, G)
        .into_iter()
        .filter(|e| e.name == "Ada")
        .collect();
    assert_eq!(rows.len(), 1);
}
