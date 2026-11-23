//! #775 / #776: a group rebuilt from only its WAL into a fresh workspace root (no identity-set
//! stamp) must stay writable. Covers the kind-aware D1 check (fix B), the stamp restore from the
//! WAL ontology sidecar (fix A), and the three orderings reported in #775.

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
    ontology::group_ontology_path,
    ontology_sidecar,
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEntity, ExtractionResult, SourceType},
    wal_group::group_wal_dir,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "grp";
const FLAGLESS: &str = "mode: open\nentity_types:\n  - name: Person\n";
const PERSON_ID: &str = "mode: open\nentity_types:\n  - name: Person\n    identity: true\n";

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("hydrate.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(DIM).unwrap();
    (db, dir)
}

fn make_state(
    db: Arc<Db>,
    root: &Path,
    wal_root: &Path,
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
        wal_root: Some(wal_root.to_path_buf()),
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
        search_stages: Default::default(),
        chunk_inflight: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn write_ontology(root: &Path, yaml: &str) {
    let path = group_ontology_path(root, G).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, yaml).unwrap();
}

async fn ingest(state: &Arc<AppState>, n: usize) -> Result<AddEpisodeResult, Error> {
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
}

async fn call(state: &Arc<AppState>, method: &str, params: Value, stream: bool) -> Value {
    // A progress channel selects the synchronous (streaming) rebuild path.
    // The receiver must outlive the call: a dropped receiver reads as a client disconnect and
    // cancels the replay.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let tx = stream.then_some(tx);
    serde_json::to_value(
        handlers::dispatch(
            IpcRequest {
                jsonrpc: "2.0".into(),
                id: json!(1),
                method: method.into(),
                params,
            },
            Arc::clone(state),
            tx,
        )
        .await,
    )
    .unwrap()
}

async fn rebuild(state: &Arc<AppState>) {
    let v = call(
        state,
        "knowledge_rebuild_from_wal",
        json!({"group_id": G}),
        true,
    )
    .await;
    assert!(v.get("error").is_none(), "rebuild failed: {v}");
    // The replay must actually have restored the publisher's entity (kind Person).
    let st = call(state, "knowledge_status", json!({}), false).await;
    assert_eq!(
        st["result"]["entity_count"], 1,
        "replay restored no entity: {st}"
    );
}

async fn reload(state: &Arc<AppState>) -> Value {
    let v = call(
        state,
        "knowledge_reload_ontology",
        json!({"group_id": G}),
        false,
    )
    .await;
    assert!(v.get("error").is_none(), "reload failed: {v}");
    v["result"].clone()
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), to).unwrap();
        }
    }
}

fn person(name: &str) -> ExtractionResult {
    ExtractionResult {
        entities: vec![ExtractedEntity {
            name: name.to_string(),
            entity_type: "Person".to_string(),
            summary: format!("Person {name}"),
            original_entity_type: None,
        }],
        edges: vec![],
    }
}

/// Publisher: ingest a Person under `identity: true`, leaving only a WAL behind. Returns the
/// publisher's WAL root (kept alive by the returned guards).
async fn publish() -> (TempDir, TempDir, TempDir) {
    let (db, db_dir) = make_db();
    let root = TempDir::new().unwrap();
    let wal = TempDir::new().unwrap();
    write_ontology(root.path(), PERSON_ID);
    let state = make_state(db, root.path(), wal.path(), vec![person("Ada")]);
    ingest(&state, 1).await.unwrap();
    assert!(identity_stamp::stamp_path(root.path(), G).unwrap().exists());
    (wal, root, db_dir)
}

/// Fresh consumer: new empty workspace root, new empty DB, only the group's WAL directory copied
/// over (optionally without the ontology sidecar).
fn consumer(
    pub_wal: &Path,
    keep_sidecar: bool,
) -> (Arc<AppState>, TempDir, TempDir, TempDir, TempDir) {
    let (db, db_dir) = make_db();
    let root = TempDir::new().unwrap();
    let wal = TempDir::new().unwrap();
    let src = group_wal_dir(pub_wal, G).unwrap();
    let dst = group_wal_dir(wal.path(), G).unwrap();
    copy_dir(&src, &dst);
    if !keep_sidecar {
        std::fs::remove_file(ontology_sidecar::wal_ontology_path(&dst)).unwrap();
    }
    assert!(!root.path().join(".lcg").join("identity-set").exists());
    let state = make_state(
        db,
        root.path(),
        wal.path(),
        vec![ExtractionResult::default()],
    );
    (state, root, wal, db_dir, TempDir::new().unwrap())
}

fn stamp(root: &Path) -> Vec<String> {
    identity_stamp::read_stamp(root, G)
        .unwrap()
        .into_iter()
        .collect()
}

// ── #775 reproduction: all three orderings ────────────────────────────────────

async fn run_ordering(order: u8, keep_sidecar: bool) {
    let (pub_wal, _pub_root, _pub_db) = publish().await;
    let (state, root, _w, _d, _x) = consumer(pub_wal.path(), keep_sidecar);
    match order {
        // hydrate → reload: the ontology is installed after the rebuild.
        1 => {
            write_ontology(root.path(), FLAGLESS);
            rebuild(&state).await;
            write_ontology(root.path(), PERSON_ID);
            let r = reload(&state).await;
            assert!(r["identity_refusal"].is_null(), "{r}");
        }
        // install → hydrate → add_episode
        2 => {
            write_ontology(root.path(), PERSON_ID);
            rebuild(&state).await;
        }
        // install → reload → hydrate → add_episode
        3 => {
            write_ontology(root.path(), PERSON_ID);
            let r = reload(&state).await;
            assert!(r["identity_refusal"].is_null(), "{r}");
            rebuild(&state).await;
        }
        _ => unreachable!(),
    }
    ingest(&state, 2)
        .await
        .unwrap_or_else(|e| panic!("ordering {order} (sidecar={keep_sidecar}): {e}"));
    assert_eq!(
        stamp(root.path()),
        vec!["Person".to_string()],
        "ordering {order} (sidecar={keep_sidecar})"
    );
}

#[tokio::test]
async fn hydrate_then_reload_accepts_with_sidecar() {
    run_ordering(1, true).await;
}

#[tokio::test]
async fn hydrate_then_reload_accepts_without_sidecar() {
    run_ordering(1, false).await;
}

#[tokio::test]
async fn install_hydrate_add_episode_accepts_with_sidecar() {
    run_ordering(2, true).await;
}

#[tokio::test]
async fn install_hydrate_add_episode_accepts_without_sidecar() {
    run_ordering(2, false).await;
}

#[tokio::test]
async fn install_reload_hydrate_add_episode_accepts_with_sidecar() {
    run_ordering(3, true).await;
}

#[tokio::test]
async fn install_reload_hydrate_add_episode_accepts_without_sidecar() {
    run_ordering(3, false).await;
}

// ── Fix A: stamp restore ──────────────────────────────────────────────────────

#[tokio::test]
async fn rebuild_restores_stamp_equal_to_sidecar_set() {
    let (pub_wal, _r, _d) = publish().await;
    let (state, root, wal, _d2, _x) = consumer(pub_wal.path(), true);
    let sc = ontology_sidecar::read_wal_ontology_sidecar(&group_wal_dir(wal.path(), G).unwrap())
        .unwrap();
    assert_eq!(sc.identity_types, Some(vec!["Person".to_string()]));
    // No ontology installed on the consumer: the stamp can only come from the restore.
    rebuild(&state).await;
    assert_eq!(stamp(root.path()), vec!["Person".to_string()]);
}

#[tokio::test]
async fn restored_baseline_still_refuses_a_flip_made_while_the_stamp_was_lost() {
    let (pub_wal, _r, _d) = publish().await;
    let (state, root, _w, _d2, _x) = consumer(pub_wal.path(), true);
    // The consumer's ontology drops the flag; the restored {Person} stamp makes that a removal
    // against entities whose kind is Person.
    write_ontology(root.path(), FLAGLESS);
    rebuild(&state).await;
    match ingest(&state, 2).await {
        Err(Error::IdentitySetChangeRefused { label, count, .. }) => {
            assert_eq!((label.as_str(), count), ("Person", 1));
        }
        other => panic!("expected refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn rebuild_never_overwrites_an_existing_stamp() {
    let (pub_wal, _r, _d) = publish().await;
    let (state, root, _w, _d2, _x) = consumer(pub_wal.path(), true);
    write_ontology(root.path(), PERSON_ID);
    // A pre-existing stamp, with bytes `write_stamp` would never produce.
    let p = identity_stamp::stamp_path(root.path(), G).unwrap();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let raw = r#"{"identity_types":["Person"]}"#;
    std::fs::write(&p, raw).unwrap();
    rebuild(&state).await;
    assert_eq!(std::fs::read_to_string(&p).unwrap(), raw);
}

#[tokio::test]
async fn older_sidecar_without_identity_field_rebuilds_without_error() {
    let (pub_wal, _r, _d) = publish().await;
    let (state, root, wal, _d2, _x) = consumer(pub_wal.path(), true);
    std::fs::write(
        ontology_sidecar::wal_ontology_path(&group_wal_dir(wal.path(), G).unwrap()),
        r#"{"hash":"h","mode":"open","entity_types":["Person"],"relation_types":[]}"#,
    )
    .unwrap();
    write_ontology(root.path(), PERSON_ID);
    rebuild(&state).await;
    // Nothing to restore; the kind-aware check covers the group.
    ingest(&state, 2).await.unwrap();
    assert_eq!(stamp(root.path()), vec!["Person".to_string()]);
}

// ── #778: a missing stamp must not hide an identity removal ───────────────────

const PERSON_ORG_ID: &str = "mode: open\nentity_types:\n  - name: Person\n    identity: true\n  - name: Organization\n    identity: true\n";
const ORG_ID: &str = "mode: open\nentity_types:\n  - name: Organization\n    identity: true\n";
const PERSON_ONLY_ID: &str = PERSON_ID;
const ORG_ONLY_FLAGLESS_PERSON_ID: &str =
    "mode: open\nentity_types:\n  - name: Person\n    identity: true\n  - name: Organization\n";
const NO_PERSON: &str = "mode: open\nentity_types:\n  - name: Organization\n";

fn org(name: &str) -> ExtractedEntity {
    ExtractedEntity {
        name: name.to_string(),
        entity_type: "Organization".to_string(),
        summary: format!("Org {name}"),
        original_entity_type: None,
    }
}

/// A single workspace that "restarts": same DB, workspace root and WAL root, fresh `AppState`
/// (empty ontology cache).
struct Ws {
    db: Arc<Db>,
    root: TempDir,
    wal: TempDir,
    _db_dir: TempDir,
}

impl Ws {
    /// Ingests one episode with `extraction` under `yaml`.
    async fn new(yaml: &str, extraction: ExtractionResult) -> Ws {
        let (db, _db_dir) = make_db();
        let root = TempDir::new().unwrap();
        let wal = TempDir::new().unwrap();
        write_ontology(root.path(), yaml);
        let state = make_state(Arc::clone(&db), root.path(), wal.path(), vec![extraction]);
        ingest(&state, 1).await.unwrap();
        Ws {
            db,
            root,
            wal,
            _db_dir,
        }
    }

    fn restart(&self) -> Arc<AppState> {
        make_state(
            Arc::clone(&self.db),
            self.root.path(),
            self.wal.path(),
            vec![ExtractionResult::default()],
        )
    }

    fn stamp_path(&self) -> std::path::PathBuf {
        identity_stamp::stamp_path(self.root.path(), G).unwrap()
    }

    fn sidecar_path(&self) -> std::path::PathBuf {
        ontology_sidecar::wal_ontology_path(&group_wal_dir(self.wal.path(), G).unwrap())
    }

    fn delete_stamp(&self) {
        std::fs::remove_file(self.stamp_path()).unwrap();
    }
}

fn with_orgs() -> ExtractionResult {
    ExtractionResult {
        entities: vec![person("Ada").entities.remove(0), org("Acme")],
        edges: vec![],
    }
}

/// Asserts the group is refused (`-32003`) on both `knowledge_reload_ontology` and
/// `knowledge_add_episode`, after a restart.
async fn assert_refused(ws: &Ws, label: &str) {
    let state = ws.restart();
    let r = call(
        &state,
        "knowledge_reload_ontology",
        json!({"group_id": G}),
        false,
    )
    .await;
    assert!(r.get("error").is_none(), "{r}");
    assert!(
        !r["result"]["identity_refusal"].is_null(),
        "reload must report a refusal: {r}"
    );
    match ingest(&state, 2).await {
        Err(e @ Error::IdentitySetChangeRefused { .. }) => {
            let msg = e.to_string();
            assert!(msg.contains(label), "{msg}");
        }
        other => panic!("expected refusal, got {other:?}"),
    }
    // Also via dispatch: the wire error code is -32003.
    let v = call(
        &state,
        "knowledge_add_episode",
        json!({"group_id": G, "name": "x", "episode_body": "b", "source_description": "d"}),
        false,
    )
    .await;
    assert_eq!(v["error"]["code"], -32003, "{v}");
}

async fn assert_accepted(ws: &Ws) {
    let state = ws.restart();
    let r = call(
        &state,
        "knowledge_reload_ontology",
        json!({"group_id": G}),
        false,
    )
    .await;
    assert!(r.get("error").is_none(), "{r}");
    assert!(r["result"]["identity_refusal"].is_null(), "{r}");
    ingest(&state, 2).await.unwrap();
}

#[tokio::test]
async fn removal_refused_when_stamp_missing_sidecar_present() {
    let ws = Ws::new(PERSON_ID, person("Ada")).await;
    ws.delete_stamp();
    write_ontology(ws.root.path(), FLAGLESS);
    assert_refused(&ws, "Person").await;
    // The stamp was restored from the sidecar's set.
    assert_eq!(stamp(ws.root.path()), vec!["Person".to_string()]);
}

#[tokio::test]
async fn removal_refused_when_stamp_and_sidecar_missing() {
    let ws = Ws::new(PERSON_ID, person("Ada")).await;
    ws.delete_stamp();
    std::fs::remove_file(ws.sidecar_path()).unwrap();
    write_ontology(ws.root.path(), FLAGLESS);
    assert_refused(&ws, "Person").await;
}

#[tokio::test]
async fn removal_refused_when_sidecar_has_no_identity_field() {
    let ws = Ws::new(PERSON_ID, person("Ada")).await;
    ws.delete_stamp();
    std::fs::write(
        ws.sidecar_path(),
        r#"{"hash":"h","mode":"open","entity_types":["Person"],"relation_types":[]}"#,
    )
    .unwrap();
    write_ontology(ws.root.path(), FLAGLESS);
    assert_refused(&ws, "Person").await;
}

#[tokio::test]
async fn removal_refused_when_sidecar_records_empty_set_but_data_has_person_kind() {
    let ws = Ws::new(PERSON_ID, person("Ada")).await;
    ws.delete_stamp();
    let text = std::fs::read_to_string(ws.sidecar_path()).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["identity_types"] = json!([]);
    std::fs::write(ws.sidecar_path(), v.to_string()).unwrap();
    write_ontology(ws.root.path(), FLAGLESS);
    assert_refused(&ws, "Person").await;
}

#[tokio::test]
async fn stored_data_wins_over_a_disagreeing_sidecar() {
    // Sidecar says {Organization}, current is {Organization}, but stored kinds are Person.
    let ws = Ws::new(PERSON_ID, person("Ada")).await;
    ws.delete_stamp();
    let text = std::fs::read_to_string(ws.sidecar_path()).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["identity_types"] = json!(["Organization"]);
    std::fs::write(ws.sidecar_path(), v.to_string()).unwrap();
    write_ontology(ws.root.path(), ORG_ID);
    assert_refused(&ws, "Person").await;
}

#[tokio::test]
async fn mixed_removal_is_refused_when_one_identity_type_stays() {
    let ws = Ws::new(PERSON_ORG_ID, with_orgs()).await;
    ws.delete_stamp();
    std::fs::remove_file(ws.sidecar_path()).unwrap();
    // Person stays identity-bearing; Organization loses its flag.
    write_ontology(ws.root.path(), ORG_ONLY_FLAGLESS_PERSON_ID);
    assert_refused(&ws, "Organization").await;
}

#[tokio::test]
async fn type_removed_from_ontology_entirely_is_refused() {
    let ws = Ws::new(PERSON_ID, person("Ada")).await;
    ws.delete_stamp();
    std::fs::remove_file(ws.sidecar_path()).unwrap();
    write_ontology(ws.root.path(), NO_PERSON);
    assert_refused(&ws, "Person").await;
}

#[tokio::test]
async fn unchanged_ontology_with_missing_stamp_restores_and_accepts() {
    let ws = Ws::new(PERSON_ID, person("Ada")).await;
    ws.delete_stamp();
    assert_accepted(&ws).await;
    assert_eq!(stamp(ws.root.path()), vec!["Person".to_string()]);
}

#[tokio::test]
async fn unchanged_ontology_with_missing_stamp_and_sidecar_accepts() {
    // Stored kinds are all in the current identity set: not a removal.
    let ws = Ws::new(PERSON_ONLY_ID, person("Ada")).await;
    ws.delete_stamp();
    std::fs::remove_file(ws.sidecar_path()).unwrap();
    assert_accepted(&ws).await;
}

#[tokio::test]
async fn empty_group_with_missing_stamp_is_accepted_and_writes_no_stamp() {
    let (db, db_dir) = make_db();
    let ws = Ws {
        db,
        root: TempDir::new().unwrap(),
        wal: TempDir::new().unwrap(),
        _db_dir: db_dir,
    };
    write_ontology(ws.root.path(), FLAGLESS);
    assert_accepted(&ws).await;
    assert!(!ws.stamp_path().exists());
}

#[tokio::test]
async fn entity_kind_only_group_with_missing_stamp_is_accepted() {
    // Ingested under a flagless ontology: kind is the default `Entity`.
    let ws = Ws::new(FLAGLESS, person("Ada")).await;
    assert!(!ws.stamp_path().exists());
    assert_accepted(&ws).await;
    assert!(!ws.stamp_path().exists());
}

#[tokio::test]
async fn stamp_present_removal_is_still_refused() {
    let ws = Ws::new(PERSON_ID, person("Ada")).await;
    assert!(ws.stamp_path().exists());
    write_ontology(ws.root.path(), FLAGLESS);
    assert_refused(&ws, "Person").await;
}

#[tokio::test]
async fn rebuild_then_plain_restart_do_not_conflict() {
    let (pub_wal, _r, _d) = publish().await;
    let (state, root, wal, db_dir, _x) = consumer(pub_wal.path(), true);
    write_ontology(root.path(), PERSON_ID);
    rebuild(&state).await;
    assert_eq!(stamp(root.path()), vec!["Person".to_string()]);
    // Plain restart over the same DB, root and WAL: stamp present, ontology unchanged.
    let restarted = make_state(
        state.db.load_full().unwrap(),
        root.path(),
        wal.path(),
        vec![ExtractionResult::default()],
    );
    ingest(&restarted, 2).await.unwrap();
    assert_eq!(stamp(root.path()), vec!["Person".to_string()]);
    drop(db_dir);
}

#[tokio::test]
async fn distinct_non_default_kinds_is_group_scoped_and_skips_default() {
    let ws = Ws::new(PERSON_ORG_ID, with_orgs()).await;
    let conn = ws.db.connect().unwrap();
    assert_eq!(
        conn.distinct_non_default_kinds(G).unwrap(),
        vec!["Organization".to_string(), "Person".to_string()]
    );
    assert!(conn.distinct_non_default_kinds("other").unwrap().is_empty());
    let flagless = Ws::new(FLAGLESS, person("Ada")).await;
    assert!(flagless
        .db
        .connect()
        .unwrap()
        .distinct_non_default_kinds(G)
        .unwrap()
        .is_empty());
}
