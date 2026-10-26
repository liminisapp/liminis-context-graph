//! `knowledge_reload_ontology` (issue #627): per-group ontology reload without a service restart.
//!
//! Covers the issue's seven acceptance scenarios (US1 adopt an edited file, US2 identity refusal
//! and its clearing, US3 idempotent no-op) plus first-resolution, unchecked-restore, validation
//! and the stale-sidecar guard for an episode that straddles a reload.

use std::collections::HashMap;
use std::path::Path;
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
    ontology::{content_hash, group_ontology_path, load_group_ontology},
    ontology_sidecar,
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEntity, ExtractionOutcome, ExtractionResult, SourceType},
    EntityRow,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "grp";
const H: &str = "other";

// Ontology A: flagless. B: adds a `Place` identity type and a relation vocabulary.
const A: &str = "mode: open\nentity_types:\n  - name: Person\n";
const B: &str = "mode: open\nentity_types:\n  - name: Person\n  - name: Place\n    identity: true\nrelation_types:\n  - name: AUTHORED\n    aliases: [WROTE]\n";
const PERSON_ID: &str = "mode: open\nentity_types:\n  - name: Person\n    identity: true\n";

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("reload.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(DIM).unwrap();
    (db, dir)
}

fn make_state_with(db: Arc<Db>, root: &Path, extractor: Arc<dyn Extractor>) -> Arc<AppState> {
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
        workspace_root: Some(root.to_path_buf()),
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: None,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn make_state(db: Arc<Db>, root: &Path, extractions: Vec<ExtractionResult>) -> Arc<AppState> {
    make_state_with(db, root, Arc::new(ConfigurableExtractor::new(extractions)))
}

fn ent(name: &str, ty: &str) -> ExtractedEntity {
    ExtractedEntity {
        name: name.to_string(),
        entity_type: ty.to_string(),
        summary: format!("{ty} {name}"),
        original_entity_type: None,
    }
}

fn one(entities: Vec<ExtractedEntity>) -> ExtractionResult {
    ExtractionResult {
        entities,
        edges: vec![],
    }
}

fn write_file(root: &Path, group: &str, yaml: &str) {
    let path = group_ontology_path(root, group).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, yaml).unwrap();
}

fn hash_of(root: &Path, group: &str) -> String {
    content_hash(load_group_ontology(root, group).as_ref())
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

async fn call(state: &Arc<AppState>, method: &str, params: Value) -> Value {
    serde_json::to_value(
        handlers::dispatch(
            IpcRequest {
                jsonrpc: "2.0".into(),
                id: json!(1),
                method: method.into(),
                params,
            },
            Arc::clone(state),
            None,
        )
        .await,
    )
    .unwrap()
}

async fn reload(state: &Arc<AppState>, group: &str) -> Value {
    let v = call(
        state,
        "knowledge_reload_ontology",
        json!({"group_id": group}),
    )
    .await;
    assert!(v.get("error").is_none(), "reload failed: {v}");
    v["result"].clone()
}

async fn status(state: &Arc<AppState>) -> Value {
    call(state, "knowledge_status", json!({})).await["result"].clone()
}

fn drift_for(status: &Value, group: &str) -> Option<Value> {
    status["group_ontology_drift"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["group_id"] == group)
        .cloned()
}

fn refused_groups(status: &Value) -> Vec<String> {
    status["group_identity_refusals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["group_id"].as_str().unwrap().to_string())
        .collect()
}

fn entities(db: &Db, group: &str) -> Vec<EntityRow> {
    db.connect()
        .unwrap()
        .get_entities_by_group_ids(Some(&[group]))
        .unwrap()
}

fn is_refusal(r: &Result<AddEpisodeResult, Error>) -> bool {
    matches!(r, Err(Error::IdentitySetChangeRefused { .. }))
}

// ── US1: adopt an edited per-group ontology ───────────────────────────────────

#[tokio::test]
async fn reload_adopts_edited_file_and_leaves_other_groups_untouched() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    write_file(root.path(), G, A);
    let state = make_state(
        Arc::clone(&db),
        root.path(),
        vec![
            one(vec![ent("Ada", "Person")]),
            one(vec![ent("Hal", "Person")]),
            one(vec![ent("Rome", "Place")]),
            one(vec![ent("Ivy", "Person")]),
        ],
    );
    ingest(&state, G, 1).await.unwrap(); // G resolved under A
    ingest(&state, H, 2).await.unwrap(); // H resolved (no ontology at all)
    let h_hash_before = state.cached_ontology_hash(H);
    let h_drift_before = drift_for(&status(&state).await, H);
    assert_eq!(h_hash_before.as_deref(), Some("none"));

    // Under A the group has no relation vocabulary: canonicalize requires one.
    let before = call(
        &state,
        "knowledge_canonicalize_relations",
        json!({"group_id": G, "dry_run": true}),
    )
    .await;
    assert_eq!(before["error"]["code"], -32000, "{before}");

    // Editing the file alone changes nothing (cached for the life of the process)…
    write_file(root.path(), G, B);
    assert_eq!(state.cached_ontology_hash(G), Some(hash_of_a(root.path())));

    // …until the reload.
    let r = reload(&state, G).await;
    assert_eq!(r["group_id"], G);
    assert_eq!(r["changed"], true, "{r}");
    assert_ne!(r["previous_hash"], r["new_hash"], "{r}");
    assert_eq!(r["new_hash"], json!(hash_of(root.path(), G)));
    assert!(r["identity_refusal"].is_null(), "{r}");

    // US1.5: drift in status reflects B (ingested under A, now resolving B) and matches the
    // response's `drift`.
    let st = status(&state).await;
    let drift = drift_for(&st, G).expect("G must be in group_ontology_drift");
    assert_eq!(drift["drifted"], true, "{st}");
    assert_eq!(r["drift"], drift);

    // US1.2: extraction is now typed by B (`Place` is an identity type there).
    ingest(&state, G, 3).await.unwrap();
    let rome = entities(&db, G)
        .into_iter()
        .find(|e| e.name == "Rome")
        .unwrap();
    assert_eq!(rome.kind, "Place");

    // US1.3: canonicalize no longer fails with -32000.
    let after = call(
        &state,
        "knowledge_canonicalize_relations",
        json!({"group_id": G, "dry_run": true}),
    )
    .await;
    assert!(after.get("error").is_none(), "{after}");

    // US1.6: H is untouched and still serves writes.
    assert_eq!(state.cached_ontology_hash(H), h_hash_before);
    assert_eq!(drift_for(&status(&state).await, H), h_drift_before);
    ingest(&state, H, 4).await.unwrap();
}

fn hash_of_a(root: &Path) -> String {
    // Hash of ontology A independent of what the file currently holds.
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), G, A);
    let _ = root;
    hash_of(tmp.path(), G)
}

/// US1.3/US1.4: a group first resolved under *no* ontology adopts a file written afterwards.
#[tokio::test]
async fn reload_gives_an_unresolved_vocabulary_group_its_ontology() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let state = make_state(Arc::clone(&db), root.path(), vec![]);
    assert!(state.resolve_ontology(G).is_none());

    let off = |s: &Arc<AppState>| {
        let s = Arc::clone(s);
        async move {
            call(
                &s,
                "knowledge_reprocess_entity_types",
                json!({"group_id": G, "scope": "off_ontology"}),
            )
            .await
        }
    };
    assert_eq!(off(&state).await["result"]["success"], false);

    write_file(root.path(), G, B);
    // Still the cached resolution until reloaded.
    assert_eq!(off(&state).await["result"]["success"], false);

    let r = reload(&state, G).await;
    assert_eq!(r["previous_hash"], "none");
    assert_eq!(r["changed"], true);
    assert_eq!(off(&state).await["result"]["success"], true);
    let c = call(
        &state,
        "knowledge_canonicalize_relations",
        json!({"group_id": G, "dry_run": true}),
    )
    .await;
    assert!(c.get("error").is_none(), "{c}");
}

// ── US2: identity-set refusal ─────────────────────────────────────────────────

#[tokio::test]
async fn reload_that_flips_a_carried_identity_flag_is_refused_and_clearable() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    write_file(root.path(), G, A);
    let state = make_state(
        Arc::clone(&db),
        root.path(),
        vec![
            one(vec![ent("Ada", "Person")]),
            one(vec![ent("Bob", "Person")]),
            one(vec![ent("Cy", "Person")]),
            one(vec![ent("Di", "Person")]),
        ],
    );
    ingest(&state, G, 1).await.unwrap(); // Person entities exist, flagless
    ingest(&state, H, 2).await.unwrap();

    // US2.1: B flags Person, which G already holds.
    write_file(root.path(), G, PERSON_ID);
    let r = reload(&state, G).await;
    assert_eq!(r["changed"], true);
    let refusal = &r["identity_refusal"];
    assert_eq!(refusal["label"], "Person", "{r}");
    assert_eq!(refusal["adding"], true);
    assert!(refusal["count"].as_u64().unwrap() >= 1);
    assert!(refusal["message"].as_str().unwrap().contains("Person"));

    // US2.2 / US2.3: writes fail with -32003, status lists G only, H unaffected.
    assert!(is_refusal(&ingest(&state, G, 3).await));
    let v = call(
        &state,
        "knowledge_add_episode",
        json!({"name": "n", "episode_body": "b", "group_id": G}),
    )
    .await;
    assert_eq!(v["error"]["code"], -32003, "{v}");
    assert_eq!(refused_groups(&status(&state).await), vec![G.to_string()]);
    ingest(&state, H, 4).await.unwrap();

    // Reloading the same refused file keeps the refusal, and reports it.
    let again = reload(&state, G).await;
    assert_eq!(again["changed"], false);
    assert_eq!(again["identity_refusal"]["label"], "Person");
    assert!(is_refusal(&ingest(&state, G, 5).await));

    // US2.4: restoring the previous identity-bearing set and reloading clears it.
    write_file(root.path(), G, A);
    let cleared = reload(&state, G).await;
    assert_eq!(cleared["changed"], true);
    assert!(cleared["identity_refusal"].is_null(), "{cleared}");
    assert!(refused_groups(&status(&state).await).is_empty());
    ingest(&state, G, 6).await.unwrap();
}

/// US2.5 / FR-012: a flag on a type nobody carries — or on a group with no entities at all —
/// reloads normally, and the new set takes effect.
#[tokio::test]
async fn reload_without_carriers_is_accepted() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    write_file(root.path(), G, A);
    let state = make_state(
        Arc::clone(&db),
        root.path(),
        vec![
            one(vec![ent("Ada", "Person")]),
            one(vec![ent("Rome", "Place")]),
        ],
    );
    ingest(&state, G, 1).await.unwrap();

    write_file(root.path(), G, B); // flags Place; no Place entity exists
    let r = reload(&state, G).await;
    assert!(r["identity_refusal"].is_null(), "{r}");
    assert!(refused_groups(&status(&state).await).is_empty());
    ingest(&state, G, 2).await.unwrap();

    // A group with no entities at all.
    write_file(root.path(), "empty", PERSON_ID);
    let r = reload(&state, "empty").await;
    assert!(r["identity_refusal"].is_null(), "{r}");
}

// ── US3: unchanged reload is a no-op ──────────────────────────────────────────

#[tokio::test]
async fn unchanged_reload_is_a_noop() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    write_file(root.path(), G, A);
    let state = make_state(
        Arc::clone(&db),
        root.path(),
        vec![one(vec![ent("Ada", "Person")])],
    );
    ingest(&state, G, 1).await.unwrap();
    let before = status(&state).await;

    let r1 = reload(&state, G).await;
    let r2 = reload(&state, G).await;
    for r in [&r1, &r2] {
        assert_eq!(r["changed"], false, "{r}");
        assert_eq!(r["previous_hash"], r["new_hash"]);
        assert!(r["identity_refusal"].is_null());
    }
    let after = status(&state).await;
    assert_eq!(
        before["group_ontology_drift"],
        after["group_ontology_drift"]
    );
    assert_eq!(
        before["group_identity_refusals"],
        after["group_identity_refusals"]
    );
    assert_eq!(state.cached_ontology_hash(G), Some(hash_of(root.path(), G)));
}

/// A reload leaves stored data alone (FR-016).
#[tokio::test]
async fn reload_does_not_modify_stored_data() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    write_file(root.path(), G, A);
    let state = make_state(
        Arc::clone(&db),
        root.path(),
        vec![one(vec![ent("Ada", "Person")])],
    );
    ingest(&state, G, 1).await.unwrap();
    let count = |db: &Db| entities(db, G).len();
    let before = count(&db);
    write_file(root.path(), G, B);
    reload(&state, G).await;
    assert_eq!(count(&db), before);
}

// ── first resolution, removal, validation, degraded ───────────────────────────

#[tokio::test]
async fn first_reload_of_a_never_resolved_group() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let state = make_state(Arc::clone(&db), root.path(), vec![]);

    // No file, no workspace ontology → resolves to none: previous null, unchanged.
    let none = reload(&state, "nofile").await;
    assert!(none["previous_hash"].is_null(), "{none}");
    assert_eq!(none["new_hash"], "none");
    assert_eq!(none["changed"], false);

    // A file → first resolution to an ontology reports changed.
    write_file(root.path(), G, A);
    let first = reload(&state, G).await;
    assert!(first["previous_hash"].is_null());
    assert_eq!(first["changed"], true);
    assert_eq!(first["new_hash"], json!(hash_of(root.path(), G)));
}

/// Removing the per-group file re-resolves through the normal precedence (here: to none).
#[tokio::test]
async fn reload_after_file_removal_falls_back() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    write_file(root.path(), G, A);
    let state = make_state(Arc::clone(&db), root.path(), vec![]);
    state.resolve_ontology(G);
    std::fs::remove_file(group_ontology_path(root.path(), G).unwrap()).unwrap();
    let r = reload(&state, G).await;
    assert_eq!(r["new_hash"], "none");
    assert_eq!(r["changed"], true);
}

#[tokio::test]
async fn missing_or_empty_group_id_is_rejected() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    let state = make_state(Arc::clone(&db), root.path(), vec![]);
    for params in [json!({}), json!({"group_id": ""}), json!({"group_id": 3})] {
        let v = call(&state, "knowledge_reload_ontology", params).await;
        assert!(v.get("error").is_some(), "{v}");
    }
    assert!(state.cached_ontology_hash(G).is_none());
}

#[tokio::test]
async fn degraded_mode_fails_without_touching_the_cache() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    write_file(root.path(), G, A);
    let state = make_state(Arc::clone(&db), root.path(), vec![]);
    state.resolve_ontology(G);
    let cached = state.cached_ontology_hash(G);
    state.db.store(None);

    write_file(root.path(), G, B);
    let v = call(&state, "knowledge_reload_ontology", json!({"group_id": G})).await;
    assert!(v.get("error").is_some(), "{v}");
    assert_eq!(state.cached_ontology_hash(G), cached);
}

/// The identity check cannot run (DB gone): the reload changes nothing and reports why, rather
/// than leaving the group invalidated-but-unresolved.
#[tokio::test]
async fn unchecked_reload_restores_prior_state() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    write_file(root.path(), G, A);
    let state = make_state(Arc::clone(&db), root.path(), vec![]);
    state.resolve_ontology(G);
    let cached = state.cached_ontology_hash(G);
    assert!(cached.is_some());

    state.db.store(None);
    write_file(root.path(), G, PERSON_ID); // identity set differs → needs the DB to check
    let out = state.reload_group_ontology(G);
    assert!(out.identity_unchecked.is_some(), "{out:?}");
    assert_eq!(state.cached_ontology_hash(G), cached);

    // Never-resolved group: no marker leaks either.
    let out = state.reload_group_ontology("fresh");
    assert!(out.identity_unchecked.is_none()); // no file for "fresh": nothing differs
    write_file(root.path(), "fresh2", PERSON_ID);
    let out = state.reload_group_ontology("fresh2");
    assert!(out.identity_unchecked.is_some());
    assert!(state.cached_ontology_hash("fresh2").is_none());
}

// ── stale-commit guard ────────────────────────────────────────────────────────

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

/// An episode extracted under A that commits after a reload to B must not overwrite the group's
/// sidecar with A's hash or clear the drift the reload reported.
#[tokio::test]
async fn episode_straddling_a_reload_does_not_clobber_sidecar_or_drift() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    write_file(root.path(), G, A);
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let state = make_state_with(
        Arc::clone(&db),
        root.path(),
        Arc::new(GatedExtractor {
            inner: ConfigurableExtractor::new(vec![
                one(vec![ent("Ada", "Person")]),
                one(vec![ent("Bob", "Person")]),
            ]),
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        }),
    );

    // First ingest under A records A's hash in the group sidecar.
    release.notify_one();
    ingest(&state, G, 1).await.unwrap();
    started.notified().await; // drain the stored permit
    let hash_a = hash_of(root.path(), G);
    assert_eq!(
        ontology_sidecar::read_group_sidecar(root.path(), G)
            .map(|s| s.hash)
            .as_deref(),
        Some(hash_a.as_str())
    );

    // Second ingest extracts under A (Phase A is lock-free) and parks in the extractor.
    let in_flight = tokio::spawn({
        let state = Arc::clone(&state);
        async move { ingest(&state, G, 2).await }
    });
    started.notified().await;

    write_file(root.path(), G, B);
    let r = reload(&state, G).await;
    assert_eq!(r["drift"]["drifted"], true, "{r}");

    release.notify_one();
    in_flight.await.unwrap().unwrap();

    assert_eq!(
        ontology_sidecar::read_group_sidecar(root.path(), G)
            .map(|s| s.hash)
            .as_deref(),
        Some(hash_a.as_str()),
        "the stale episode must not record its (old) ontology hash over the reloaded one"
    );
    let st = status(&state).await;
    assert_eq!(drift_for(&st, G).unwrap()["drifted"], true, "{st}");
}

// ── #637: flipping `extract` is drift, never an identity refusal ──────────────

#[tokio::test]
async fn flipping_extract_is_drift_and_accepted() {
    let (db, _d) = make_db();
    let root = TempDir::new().unwrap();
    // A carried identity type: flipping its identity flag would be refused, flipping `extract`
    // must not be.
    write_file(root.path(), G, PERSON_ID);
    let state = make_state(
        Arc::clone(&db),
        root.path(),
        vec![one(vec![ent("Ada", "Person")])],
    );
    ingest(&state, G, 1).await.unwrap();

    write_file(
        root.path(),
        G,
        "mode: open\nentity_types:\n  - name: Person\n    identity: true\n    extract: false\n",
    );
    let r = reload(&state, G).await;
    assert_eq!(r["changed"], true, "{r}");
    assert_ne!(r["previous_hash"], r["new_hash"], "{r}");
    assert!(r["identity_refusal"].is_null(), "{r}");
    assert!(refused_groups(&status(&state).await).is_empty());
    let st = status(&state).await;
    let drift = drift_for(&st, G).expect("G must be in group_ontology_drift");
    assert_eq!(drift["drifted"], true, "{st}");
}
