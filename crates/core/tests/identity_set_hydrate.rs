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
