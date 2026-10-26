//! Ontology-driven kind assignment (issue #616, ADR-0616): identity-bearing entity types become
//! the entity's `kind` during extraction, and a change to a group's identity-bearing set that
//! would reinterpret existing entities is refused (D1) rather than guessed at.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, GroupOntologyCacheState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    episode::{self, AddEpisodeResult},
    error::Error,
    extractor::{ConfigurableExtractor, ExtractOptions, Extractor},
    handlers, identity_stamp,
    ipc::IpcRequest,
    ontology::{compute_ancestor_map, EntityTypeDef, Ontology, OntologyMode},
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEdge, ExtractedEntity, ExtractionOutcome, ExtractionResult, SourceType},
    EntityRow,
};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::{Notify, RwLock};
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
            extract: true,
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
    make_state_with(
        db,
        root,
        ontology,
        Arc::new(ConfigurableExtractor::new(extractions)),
    )
}

fn make_state_with(
    db: Arc<Db>,
    root: Option<&Path>,
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

/// A NULL-keyed row of one kind must not be masked by an indexed row of another: the scan runs on
/// a partial probe miss, so the name stays `Ambiguous` instead of silently binding to the
/// indexed kind.
#[tokio::test]
async fn null_lookup_key_in_one_kind_does_not_mask_ambiguity() {
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
    let kinds = vec!["Entity".to_string(), "Person".to_string()];

    // Sanity: with both keys intact the name resolves to two kinds.
    {
        let conn = db.connect().unwrap();
        let rows = conn
            .resolve_entities_by_name_in_kinds("Aurora", G, &kinds)
            .unwrap();
        assert_eq!(rows.len(), 2);
    }

    // Clear the default-kind row's key (failed backfill / raw-Cypher write); Person keeps its.
    {
        let conn = db.connect().unwrap();
        conn.run_cypher(&format!(
            "MATCH (e:Entity {{name: 'Aurora', group_id: '{G}', kind: 'Entity'}}) \
             SET e.lookup_key = NULL"
        ))
        .unwrap();
    }
    {
        let conn = db.connect().unwrap();
        let rows = conn
            .resolve_entities_by_name_in_kinds("Aurora", G, &kinds)
            .unwrap();
        let mut got: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        got.sort();
        assert_eq!(got, vec!["Entity", "Person"], "must stay ambiguous");
    }

    // End to end: the cross-batch edge to the ambiguous name is dropped, not bound to Person.
    {
        let conn = db.connect().unwrap();
        conn.run_cypher(&format!(
            "MATCH (e:Entity {{name: 'Aurora', group_id: '{G}', kind: 'Entity'}}) \
             SET e.lookup_key = NULL"
        ))
        .unwrap();
    }
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
async fn delete_by_group_removes_only_the_purged_groups_stamp() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let state = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", true, None)])),
        vec![extraction(vec![ent("Ada", "Person")], vec![])],
    );
    ingest(&state, G, 1).await.unwrap();
    ingest(&state, "other-group", 1).await.unwrap();
    assert!(!identity_stamp::read_stamp(root.path(), G)
        .unwrap()
        .is_empty());
    assert!(!identity_stamp::read_stamp(root.path(), "other-group")
        .unwrap()
        .is_empty());

    let purge = |dry_run: bool| {
        let state = Arc::clone(&state);
        async move {
            let v = serde_json::to_value(
                handlers::dispatch(
                    IpcRequest {
                        jsonrpc: "2.0".into(),
                        id: json!(1),
                        method: "knowledge_delete_by_group".into(),
                        params: json!({"group_ids": [G], "confirm": true, "dry_run": dry_run}),
                    },
                    state,
                    None,
                )
                .await,
            )
            .unwrap();
            assert!(v.get("error").is_none(), "{v}");
        }
    };

    // A dry run purges nothing, so it must not touch the stamp.
    purge(true).await;
    assert!(!identity_stamp::read_stamp(root.path(), G)
        .unwrap()
        .is_empty());

    purge(false).await;
    assert!(identity_stamp::read_stamp(root.path(), G)
        .unwrap()
        .is_empty());
    assert!(!identity_stamp::stamp_path(root.path(), G).unwrap().exists());
    assert!(
        !identity_stamp::read_stamp(root.path(), "other-group")
            .unwrap()
            .is_empty(),
        "another group's stamp must survive"
    );
}

/// Purging a group deletes its stamp; the cached ontology entry must be dropped with it so the
/// re-ingest re-stamps. Otherwise the stamp stays absent while `Person` entities accumulate and
/// the next restart falsely refuses the group though its ontology never changed.
#[tokio::test]
async fn purge_then_reingest_then_restart_does_not_falsely_refuse() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let onto = || ontology(OntologyMode::Open, &[("Person", true, None)]);
    let extractions = || {
        vec![
            extraction(vec![ent("Ada", "Person")], vec![]),
            extraction(vec![ent("Bea", "Person")], vec![]),
        ]
    };
    let state = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(onto()),
        extractions(),
    );
    ingest(&state, G, 1).await.unwrap();

    let v = serde_json::to_value(
        handlers::dispatch(
            IpcRequest {
                jsonrpc: "2.0".into(),
                id: json!(1),
                method: "knowledge_delete_by_group".into(),
                params: json!({"group_ids": [G], "confirm": true}),
            },
            Arc::clone(&state),
            None,
        )
        .await,
    )
    .unwrap();
    assert!(v.get("error").is_none(), "{v}");
    assert!(!identity_stamp::stamp_path(root.path(), G).unwrap().exists());

    // Re-ingest in the same process: creates a kind-`Person` entity and must re-stamp.
    ingest(&state, G, 2).await.unwrap();
    assert_eq!(
        identity_stamp::read_stamp(root.path(), G)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec!["Person"],
        "re-ingest after a purge must re-record the stamp"
    );

    // Restart: a fresh process with the unchanged ontology still resolves the group.
    let restarted = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(onto()),
        extractions(),
    );
    restarted
        .check_identity(G)
        .expect("unchanged ontology must not be refused after a restart");
    assert!(restarted.group_identity_refusal(G).is_none());
}

/// Extractor whose `extract` signals it has started, then waits for `release` before returning
/// its queued result, so a test can land a purge in the middle of an ingest's extraction.
struct GatedExtractor {
    inner: ConfigurableExtractor,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl Extractor for GatedExtractor {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            self.inner.extract(opts).await
        })
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        allowed_types: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        self.inner.classify_entities(entities, allowed_types)
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        relation_types: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        self.inner.classify_relations(edges, relation_types)
    }
}

/// A purge that completes while an ingest into the same group is mid-extraction removes the
/// stamp and drops the cached ontology after that ingest's lock-free D1 check has already passed.
/// The ingest must re-stamp once it holds the write lock; otherwise it writes `Person` entities
/// with no stamp and the group is falsely refused from then on.
#[tokio::test]
async fn purge_during_in_flight_ingest_does_not_falsely_refuse() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let onto = || ontology(OntologyMode::Open, &[("Person", true, None)]);
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let state = make_state_with(
        Arc::clone(&db),
        Some(root.path()),
        Some(onto()),
        Arc::new(GatedExtractor {
            inner: ConfigurableExtractor::new(vec![extraction(vec![ent("Ada", "Person")], vec![])]),
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        }),
    );

    // Establish the stamp so the purge has something to remove.
    state.check_identity(G).unwrap();
    assert!(identity_stamp::stamp_path(root.path(), G).unwrap().exists());

    let in_flight = tokio::spawn({
        let state = Arc::clone(&state);
        async move { ingest(&state, G, 1).await }
    });
    started.notified().await;

    let v = serde_json::to_value(
        handlers::dispatch(
            IpcRequest {
                jsonrpc: "2.0".into(),
                id: json!(1),
                method: "knowledge_delete_by_group".into(),
                params: json!({"group_ids": [G], "confirm": true}),
            },
            Arc::clone(&state),
            None,
        )
        .await,
    )
    .unwrap();
    assert!(v.get("error").is_none(), "{v}");
    assert!(!identity_stamp::stamp_path(root.path(), G).unwrap().exists());

    release.notify_one();
    in_flight.await.unwrap().unwrap();
    assert!(entities(&db, G).iter().any(|e| e.kind == "Person"));
    assert_eq!(
        identity_stamp::read_stamp(root.path(), G)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec!["Person"],
        "the in-flight ingest must re-record the stamp the purge removed"
    );

    // Restart: a fresh process with the unchanged ontology still resolves the group.
    let restarted = make_state(Arc::clone(&db), Some(root.path()), Some(onto()), vec![]);
    restarted
        .check_identity(G)
        .expect("unchanged ontology must not be refused after a purge raced an ingest");
}

/// A remediation's `clear_group_drift` landing on a `Resolving` slot (issue #495) upserts the
/// entry the racing first resolution then backs off from. That entry must carry the D1 check, or
/// the resolution's refusal is dropped and ingest proceeds under a changed identity set.
#[tokio::test]
async fn clear_group_drift_racing_first_resolution_keeps_the_refusal() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let s1 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(ontology(OntologyMode::Open, &[("Person", false, None)])),
        vec![extraction(vec![ent("Ada", "Person")], vec![])],
    );
    ingest(&s1, G, 1).await.unwrap();

    // Restart with `Person` newly flagged: the change must be refused (a carrier exists).
    let onto = ontology(OntologyMode::Open, &[("Person", true, None)]);
    let s2 = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(onto.clone()),
        vec![extraction(vec![ent("Cy", "Person")], vec![])],
    );
    // Simulate the race: a first resolution has marked the group `Resolving`, and a
    // remediation's `clear_group_drift` lands before that resolution inserts its own entry.
    s2.group_ontologies
        .lock()
        .unwrap()
        .insert(G.to_string(), GroupOntologyCacheState::Resolving);
    s2.clear_group_drift(G, Some(Arc::new(onto)));

    let (group, label, count) = refused(ingest(&s2, G, 2).await);
    assert_eq!((group.as_str(), label.as_str(), count), (G, "Person", 1));
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

// ── US4 / SC-005: reprocess never changes kind ────────────────────────────────

/// Extractor whose `classify_entities` assigns every entity the same type.
struct ClassifyAs(&'static str);

impl lcg_core::extractor::Extractor for ClassifyAs {
    fn extract<'a>(
        &'a self,
        _opts: lcg_core::extractor::ExtractOptions<'a>,
    ) -> futures::future::BoxFuture<'a, Result<lcg_core::types::ExtractionOutcome, Error>> {
        Box::pin(async { Ok(ExtractionResult::default().into()) })
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        _allowed_types: Option<&'a [String]>,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, Error>> {
        let n = entities.len();
        let t = self.0.to_string();
        Box::pin(async move { Ok(vec![t; n]) })
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        _allowed_types: &'a [(String, Option<String>)],
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, Error>> {
        let n = edges.len();
        Box::pin(async move { Ok(vec![String::new(); n]) })
    }
}

async fn reprocess(state: &Arc<AppState>, dry_run: bool) -> serde_json::Value {
    let v = serde_json::to_value(
        handlers::dispatch(
            IpcRequest {
                jsonrpc: "2.0".into(),
                id: json!(1),
                method: "knowledge_reprocess_entity_types".into(),
                params: json!({"group_id": G, "scope": "all", "dry_run": dry_run}),
            },
            Arc::clone(state),
            None,
        )
        .await,
    )
    .unwrap();
    assert!(v.get("error").is_none(), "{v}");
    v["result"].clone()
}

#[tokio::test]
async fn reprocess_never_changes_kind_and_reports_disagreement() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let onto = || {
        ontology(
            OntologyMode::Open,
            &[("Person", true, None), ("Organization", true, None)],
        )
    };
    let seed = make_state(
        Arc::clone(&db),
        Some(root.path()),
        Some(onto()),
        vec![extraction(vec![ent("Aurora", "Person")], vec![])],
    );
    ingest(&seed, G, 1).await.unwrap();

    // Reclassification lands on the *other* identity-bearing type.
    let state = {
        let s = make_state(Arc::clone(&db), Some(root.path()), Some(onto()), vec![]);
        let mut inner = Arc::try_unwrap(s).ok().unwrap();
        inner.extractor = Arc::new(ClassifyAs("Organization"));
        Arc::new(inner)
    };

    let dry = reprocess(&state, true).await;
    assert_eq!(
        dry["kind_disagreements"].as_array().unwrap().len(),
        1,
        "{dry}"
    );
    assert_eq!(dry["kind_disagreements"][0]["kind"], "Person");
    assert_eq!(
        dry["kind_disagreements"][0]["classified_type"],
        "Organization"
    );

    let first = reprocess(&state, false).await;
    assert_eq!(first["reclassified_count"], 1, "{first}");
    assert_eq!(first["kind_disagreements"].as_array().unwrap().len(), 1);
    let row = aurora_rows(&db).remove(0);
    assert_eq!(row.kind, "Person", "kind must never change: {row:?}");
    assert!(row.labels.contains(&"Person".to_string()), "{row:?}");
    assert!(row.labels.contains(&"Organization".to_string()), "{row:?}");

    // Idempotent: the second run has nothing to reclassify, but still reports the disagreement.
    let second = reprocess(&state, false).await;
    assert_eq!(second["reclassified_count"], 0, "{second}");
    assert_eq!(second["unchanged_count"], 1, "{second}");
    assert_eq!(second["kind_disagreements"].as_array().unwrap().len(), 1);
    assert_eq!(aurora_rows(&db)[0].kind, "Person");
}
