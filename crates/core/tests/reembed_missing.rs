//! Integration tests for `knowledge_reembed_missing` (issue #809, ADR-0809): repair of rows stored
//! with a zero-vector embedding, driven off the durable record from #808.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::{EmbedRole, Embedder, HashEmbedder},
    extractor::MockExtractor,
    handlers,
    index_lifecycle::{DbIndexDdl, IndexDdl, IndexKind},
    ipc::IpcRequest,
    reembed_missing::{reembed_missing, reembed_missing_with_ddl, ReembedParams},
    replay_embed_retry::EmbedRetryPolicy,
    telemetry::{NoopSink, TelemetrySink},
    zero_vector_record::ZeroKind,
    EntityRow, EpisodicRow, Error, RelatesToEdge,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 8;
const G: &str = "g1";
const TS: &str = "2026-01-01 00:00:00";

// ── Test embedder ─────────────────────────────────────────────────────────────

/// Deterministic embedder with a shared event log and failure switches.
struct TestEmbedder {
    inner: HashEmbedder,
    events: Arc<Mutex<Vec<String>>>,
    calls: AtomicUsize,
    /// Texts starting with this fail permanently (HTTP 400) while `failing` is set.
    fail_prefix: &'static str,
    failing: AtomicBool,
    /// Set if any call observed `indices_built == false` (an index dropped before embedding ended).
    indices_flag: Mutex<Option<Arc<AtomicBool>>>,
    saw_flag_false: AtomicBool,
}

impl TestEmbedder {
    fn new(events: Arc<Mutex<Vec<String>>>) -> Arc<Self> {
        Arc::new(Self {
            inner: HashEmbedder::new(DIM),
            events,
            calls: AtomicUsize::new(0),
            fail_prefix: "bad",
            failing: AtomicBool::new(true),
            indices_flag: Mutex::new(None),
            saw_flag_false: AtomicBool::new(false),
        })
    }
}

impl Embedder for TestEmbedder {
    fn embed<'a>(
        &'a self,
        text: &'a str,
        role: EmbedRole,
    ) -> BoxFuture<'a, Result<Vec<f32>, Error>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.events.lock().unwrap().push("embed".to_string());
        if let Some(flag) = self.indices_flag.lock().unwrap().as_ref() {
            if !flag.load(Ordering::SeqCst) {
                self.saw_flag_false.store(true, Ordering::SeqCst);
            }
        }
        if text.starts_with(self.fail_prefix) && self.failing.load(Ordering::SeqCst) {
            return Box::pin(async {
                Err(Error::Ipc(
                    "UDS embedder returned status 400 Bad Request".to_string(),
                ))
            });
        }
        if text.starts_with("zero") {
            return Box::pin(async { Ok(vec![0.0; DIM]) });
        }
        if text.starts_with("short") {
            return Box::pin(async { Ok(vec![1.0; DIM - 1]) });
        }
        self.inner.embed(text, role)
    }

    fn dim(&self) -> usize {
        DIM
    }
}

/// Records index DDL into the shared event log, optionally failing/slowing a create.
struct RecordingDdl {
    real: DbIndexDdl,
    events: Arc<Mutex<Vec<String>>>,
    fail_create: Option<IndexKind>,
    slow_create: Option<std::time::Duration>,
}

impl IndexDdl for RecordingDdl {
    fn drop_index(&self, kind: IndexKind) -> Result<(), Error> {
        // Logged *after* the real drop: callers wait on this event to start a racing search, and a
        // query that overlaps the DDL itself can fail inside lbug (`bad_optional_access`) rather
        // than with the missing-index error the auto-heal path recognises.
        let r = self.real.drop_index(kind);
        self.events
            .lock()
            .unwrap()
            .push(format!("drop:{}", kind.as_str()));
        r
    }
    fn create_index(&self, kind: IndexKind) -> Result<(), Error> {
        self.events
            .lock()
            .unwrap()
            .push(format!("create:{}", kind.as_str()));
        if let Some(d) = self.slow_create {
            std::thread::sleep(d);
        }
        if self.fail_create == Some(kind) {
            return Err(Error::Ipc("injected rebuild failure".to_string()));
        }
        self.real.create_index(kind)
    }
}

// ── Fixture ───────────────────────────────────────────────────────────────────

struct Fx {
    state: Arc<AppState>,
    db: Arc<Db>,
    embedder: Arc<TestEmbedder>,
    events: Arc<Mutex<Vec<String>>>,
    _dir: TempDir,
}

fn fixture() -> Fx {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(DIM).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let embedder = TestEmbedder::new(Arc::clone(&events));
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let indices_built = Arc::new(AtomicBool::new(false));
    *embedder.indices_flag.lock().unwrap() = Some(Arc::clone(&indices_built));
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(Arc::clone(&db))),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: embedder.clone(),
        extractor: Arc::new(MockExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "t.db".to_string(),
        wal_root: None,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "test-model".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built,
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: None,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
        search_stages: Default::default(),
    });
    Fx {
        state,
        db,
        embedder,
        events,
        _dir: dir,
    }
}

fn policy() -> EmbedRetryPolicy {
    EmbedRetryPolicy {
        retries: 1,
        base_ms: 1,
        cap_ms: 2,
    }
}

fn params(group: &str, kinds: &[ZeroKind], dry_run: bool) -> ReembedParams {
    ReembedParams {
        group_id: group.to_string(),
        kinds: kinds.to_vec(),
        dry_run,
        retry_policy: Some(policy()),
    }
}

fn zero() -> Vec<f32> {
    vec![0.0; DIM]
}

fn entity(uuid: &str, group: &str, name: &str, summary: &str, real: bool) -> EntityRow {
    let v = |t: &str| {
        if real {
            futures::executor::block_on(HashEmbedder::new(DIM).embed(t, EmbedRole::Document))
                .unwrap()
        } else {
            zero()
        }
    };
    EntityRow {
        uuid: uuid.to_string(),
        name: name.to_string(),
        group_id: group.to_string(),
        labels: vec!["Entity".to_string()],
        created_at: TS.to_string(),
        name_embedding: v(name),
        summary: summary.to_string(),
        summary_embedding: v(summary),
        attributes: "{}".to_string(),
        ..Default::default()
    }
}

fn episode(uuid: &str, group: &str, content: &str) -> EpisodicRow {
    EpisodicRow {
        uuid: uuid.to_string(),
        name: format!("ep-{uuid}"),
        group_id: group.to_string(),
        created_at: TS.to_string(),
        source: "text".to_string(),
        source_description: "test".to_string(),
        content: content.to_string(),
        content_embedding: zero(),
        valid_at: TS.to_string(),
        entity_edges: vec![],
        ..Default::default()
    }
}

fn edge(uuid: &str, group: &str, src: &str, dst: &str, fact: &str) -> RelatesToEdge {
    RelatesToEdge {
        uuid: uuid.to_string(),
        name: "REL".to_string(),
        source_node_uuid: src.to_string(),
        target_node_uuid: dst.to_string(),
        group_id: group.to_string(),
        fact: fact.to_string(),
        fact_embedding: zero(),
        created_at: TS.to_string(),
        attributes: "{}".to_string(),
        ..Default::default()
    }
}

/// Records `(kind, uuid)` for `group` directly (the #808 record's own write path is crate-private).
fn record(db: &Db, group: &str, kind: ZeroKind, uuid: &str) {
    let k = kind.as_str();
    db.connect()
        .unwrap()
        .run_cypher(&format!(
            "MERGE (z:EmbeddingZeroRow {{id: '{group}|{k}|{uuid}'}}) \
             SET z.wal_group = '{group}', z.kind = '{k}', z.uuid = '{uuid}'"
        ))
        .unwrap();
}

fn count(db: &Db, group: &str) -> u64 {
    db.connect()
        .unwrap()
        .zero_vector_counts(group)
        .unwrap()
        .total()
}

async fn emb(text: &str) -> Vec<f32> {
    HashEmbedder::new(DIM)
        .embed(text, EmbedRole::Query)
        .await
        .unwrap()
}

/// Whether the top vector hit for `text`'s own embedding is `uuid` with ~0 distance.
async fn finds(db: &Db, kind: ZeroKind, text: &str, uuid: &str) -> bool {
    let q = emb(text).await;
    let conn = db.connect().unwrap();
    let gids = [G];
    let hits: Vec<(String, f64)> = match kind {
        ZeroKind::Name => conn.vector_search_entities(&q, Some(&gids), 5).unwrap(),
        ZeroKind::Summary => conn
            .vector_search_entities_by_summary(&q, Some(&gids), 5)
            .unwrap(),
        ZeroKind::Fact => conn.vector_search_edges(&q, Some(&gids), 5).unwrap(),
        ZeroKind::Content => conn
            .vector_search_episodic(&q, Some(&gids), 5)
            .unwrap()
            .into_iter()
            .map(|p| (p.uuid.clone(), p.score))
            .collect(),
    };
    hits.first()
        .is_some_and(|(u, d)| u == uuid && d.abs() < 1e-3)
}

/// Seeds one recorded zero-vector row of each kind (group G) plus builds indexes.
fn seed_all_kinds(fx: &Fx) {
    let conn = fx.db.connect().unwrap();
    conn.insert_entity(&entity("e1", G, "alpha name", "alpha summary", false))
        .unwrap();
    conn.insert_entity(&entity("e2", G, "beta name", "beta summary", false))
        .unwrap();
    conn.insert_episodic(&episode("ep1", G, "episode content one"))
        .unwrap();
    conn.insert_relates_to_edge(&edge("r1", G, "e1", "e2", "alpha relates to beta"))
        .unwrap();
    record(&fx.db, G, ZeroKind::Name, "e1");
    record(&fx.db, G, ZeroKind::Summary, "e1");
    record(&fx.db, G, ZeroKind::Content, "ep1");
    record(&fx.db, G, ZeroKind::Fact, "r1");
    conn.build_indices_and_constraints().unwrap();
    fx.state.indices_built.store(true, Ordering::SeqCst);
}

fn kind_counts(v: &Value, k: &str) -> (u64, u64, u64, u64) {
    let o = &v["kinds"][k];
    (
        o["candidates"].as_u64().unwrap(),
        o["reembedded"].as_u64().unwrap(),
        o["failed"].as_u64().unwrap(),
        o["skipped_no_source"].as_u64().unwrap(),
    )
}

fn events(fx: &Fx) -> Vec<String> {
    fx.events.lock().unwrap().clone()
}

fn req(method: &str, params: Value) -> IpcRequest {
    IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// AC1, AC2, AC6, SC-001, SC-003, SC-006: every kind is repaired and found by vector search,
/// unrecorded rows keep byte-identical embeddings, the count reaches 0, and no embed call happens
/// after an index was dropped.
#[tokio::test(flavor = "multi_thread")]
async fn repairs_all_four_kinds() {
    let fx = fixture();
    seed_all_kinds(&fx);
    // A control entity with real vectors that is NOT in the record.
    fx.db
        .connect()
        .unwrap()
        .insert_entity(&entity("ctl", G, "control name", "control summary", true))
        .unwrap();
    let ctl_before = fx
        .db
        .connect()
        .unwrap()
        .get_entity_by_uuid("ctl")
        .unwrap()
        .unwrap()
        .name_embedding;
    assert!(finds(&fx.db, ZeroKind::Summary, "control summary", "ctl").await);
    assert_eq!(count(&fx.db, G), 4);

    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &ZeroKind::ALL, false),
        None,
    )
    .await
    .unwrap();
    for k in ["name", "summary", "fact", "content"] {
        assert_eq!(kind_counts(&v, k), (1, 1, 0, 0), "{k}: {v}");
    }
    assert!(v.get("index_rebuild_errors").is_none(), "{v}");

    assert!(finds(&fx.db, ZeroKind::Name, "alpha name", "e1").await);
    assert!(finds(&fx.db, ZeroKind::Summary, "alpha summary", "e1").await);
    assert!(finds(&fx.db, ZeroKind::Content, "episode content one", "ep1").await);
    assert!(finds(&fx.db, ZeroKind::Fact, "alpha relates to beta", "r1").await);
    assert_eq!(
        count(&fx.db, G),
        0,
        "record must be empty after full repair"
    );
    assert!(fx.state.indices_built.load(Ordering::SeqCst));
    assert!(
        !fx.embedder.saw_flag_false.load(Ordering::SeqCst),
        "an embed call ran after the indexes were taken down"
    );

    let ctl_after = fx
        .db
        .connect()
        .unwrap()
        .get_entity_by_uuid("ctl")
        .unwrap()
        .unwrap()
        .name_embedding;
    assert_eq!(
        ctl_before, ctl_after,
        "unrecorded row must be byte-identical"
    );
    assert!(finds(&fx.db, ZeroKind::Summary, "control summary", "ctl").await);
    assert!(finds(&fx.db, ZeroKind::Name, "control name", "ctl").await);

    // Idempotent: a re-run has nothing to do and touches no index.
    fx.events.lock().unwrap().clear();
    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &ZeroKind::ALL, false),
        None,
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "name"), (0, 0, 0, 0));
    assert!(events(&fx).is_empty());
}

/// Issue #850: replay records a summary unit's zero `content_embedding` under the `content` kind,
/// keyed by the summary's uuid. It must be repaired like an episode's, not taken for a stale entry
/// (no `Episodic` row has that uuid) and dropped from the record with the zero vector left behind.
#[tokio::test(flavor = "multi_thread")]
async fn repairs_a_summary_units_content_vector() {
    let fx = fixture();
    let conn = fx.db.connect().unwrap();
    conn.insert_episodic(&episode("ep1", G, "episode content one"))
        .unwrap();
    conn.insert_summary_unit(&lcg_core::summary_units::SummaryUnitRow {
        uuid: "su1".to_string(),
        group_id: G.to_string(),
        episode_uuid: "ep1".to_string(),
        content: "a summary of the first episode".to_string(),
        content_embedding: zero(),
        created_at: TS.to_string(),
    })
    .unwrap();
    record(&fx.db, G, ZeroKind::Content, "ep1");
    record(&fx.db, G, ZeroKind::Content, "su1");
    conn.build_indices_and_constraints().unwrap();
    fx.state.indices_built.store(true, Ordering::SeqCst);
    drop(conn);

    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &[ZeroKind::Content], false),
        None,
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "content"), (2, 2, 0, 0), "{v}");
    assert!(v.get("index_rebuild_errors").is_none(), "{v}");
    assert_eq!(count(&fx.db, G), 0, "both entries repaired, none dropped");

    assert!(finds(&fx.db, ZeroKind::Content, "episode content one", "ep1").await);
    let q = emb("a summary of the first episode").await;
    let hits = fx
        .db
        .connect()
        .unwrap()
        .vector_search_summary_units(&q, Some(&[G]), 5, 5)
        .unwrap();
    let top = hits
        .first()
        .expect("the summary index is rebuilt and searchable");
    assert_eq!(top.uuid, "su1");
    assert!(
        top.distance.abs() < 1e-3,
        "repaired vector: {}",
        top.distance
    );
}

/// AC3, SC-004: `dry_run` counts, makes no embedder call, writes nothing; the real run's
/// `candidates` match.
#[tokio::test(flavor = "multi_thread")]
async fn dry_run_has_no_side_effects_and_matches_real_candidates() {
    let fx = fixture();
    seed_all_kinds(&fx);
    let ddl: Arc<dyn IndexDdl> = Arc::new(RecordingDdl {
        real: DbIndexDdl::new(Arc::clone(&fx.db)),
        events: Arc::clone(&fx.events),
        fail_create: None,
        slow_create: None,
    });
    let dry = reembed_missing_with_ddl(
        Arc::clone(&fx.state),
        params(G, &ZeroKind::ALL, true),
        None,
        Some(Arc::clone(&ddl)),
    )
    .await
    .unwrap();
    assert_eq!(dry["dry_run"], true);
    assert_eq!(fx.embedder.calls.load(Ordering::SeqCst), 0);
    assert!(
        events(&fx).is_empty(),
        "dry run touched an index or embedder"
    );
    assert_eq!(count(&fx.db, G), 4);
    assert!(!finds(&fx.db, ZeroKind::Name, "alpha name", "e1").await);

    let real = reembed_missing_with_ddl(
        Arc::clone(&fx.state),
        params(G, &ZeroKind::ALL, false),
        None,
        Some(ddl),
    )
    .await
    .unwrap();
    for k in ["name", "summary", "fact", "content"] {
        assert_eq!(kind_counts(&dry, k).0, kind_counts(&real, k).0, "{k}");
    }
}

/// AC4, AC5, SC-005, SC-006: zero candidates drop nothing; a fact-only repair drops/rebuilds only
/// the fact index, and every embed call precedes the first drop.
#[tokio::test(flavor = "multi_thread")]
async fn only_affected_indexes_are_dropped_after_all_embedding() {
    let fx = fixture();
    seed_all_kinds(&fx);
    let ddl: Arc<dyn IndexDdl> = Arc::new(RecordingDdl {
        real: DbIndexDdl::new(Arc::clone(&fx.db)),
        events: Arc::clone(&fx.events),
        fail_create: None,
        slow_create: None,
    });

    // Zero candidates: kind filter matches none of the recorded kinds once cleared.
    let v = reembed_missing_with_ddl(
        Arc::clone(&fx.state),
        params("no-such-group", &ZeroKind::ALL, false),
        None,
        Some(Arc::clone(&ddl)),
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "name"), (0, 0, 0, 0));
    assert!(events(&fx).is_empty());

    let v = reembed_missing_with_ddl(
        Arc::clone(&fx.state),
        params(G, &[ZeroKind::Fact], false),
        None,
        Some(ddl),
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "fact"), (1, 1, 0, 0), "{v}");
    assert_eq!(kind_counts(&v, "name"), (0, 0, 0, 0), "{v}");
    let ev = events(&fx);
    assert_eq!(
        ev.iter()
            .filter(|e| e.starts_with("drop"))
            .collect::<Vec<_>>(),
        ["drop:fact"]
    );
    assert_eq!(
        ev.iter()
            .filter(|e| e.starts_with("create"))
            .collect::<Vec<_>>(),
        ["create:fact"]
    );
    let first_drop = ev.iter().position(|e| e.starts_with("drop")).unwrap();
    let last_embed = ev.iter().rposition(|e| e == "embed").unwrap();
    assert!(
        last_embed < first_drop,
        "embedding must finish before any drop: {ev:?}"
    );
    // Other kinds remain recorded and unrepaired.
    assert_eq!(count(&fx.db, G), 3);
}

/// FR-003 / User Story 1 scenario 3: `kinds` limits the repair and the other records are untouched.
#[tokio::test(flavor = "multi_thread")]
async fn kinds_filter_leaves_other_records() {
    let fx = fixture();
    seed_all_kinds(&fx);
    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &[ZeroKind::Name], false),
        None,
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "name"), (1, 1, 0, 0));
    assert_eq!(count(&fx.db, G), 3);
    assert!(finds(&fx.db, ZeroKind::Name, "alpha name", "e1").await);
    assert!(!finds(&fx.db, ZeroKind::Summary, "alpha summary", "e1").await);
}

/// AC6, User Story 3: a permanently failing text fails only its own row and stays recorded; a
/// re-run after the embedder recovers repairs it.
#[tokio::test(flavor = "multi_thread")]
async fn partial_failure_stays_recorded_and_retries() {
    let fx = fixture();
    let conn = fx.db.connect().unwrap();
    conn.insert_entity(&entity("g1", G, "good one", "s", false))
        .unwrap();
    conn.insert_entity(&entity("g2", G, "good two", "s", false))
        .unwrap();
    conn.insert_entity(&entity("b1", G, "bad one", "s", false))
        .unwrap();
    for u in ["g1", "g2", "b1"] {
        record(&fx.db, G, ZeroKind::Name, u);
    }
    conn.build_indices_and_constraints().unwrap();
    fx.state.indices_built.store(true, Ordering::SeqCst);

    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &[ZeroKind::Name], false),
        None,
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "name"), (3, 2, 1, 0), "{v}");
    assert_eq!(count(&fx.db, G), 1, "status count equals failed count");
    assert!(finds(&fx.db, ZeroKind::Name, "good one", "g1").await);

    fx.embedder.failing.store(false, Ordering::SeqCst);
    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &[ZeroKind::Name], false),
        None,
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "name"), (1, 1, 0, 0), "{v}");
    assert_eq!(count(&fx.db, G), 0);
}

/// FR-011, FR-013: a stale entry is removed, an empty-text row stays recorded, a row belonging to
/// another group is neither written nor unrecorded.
#[tokio::test(flavor = "multi_thread")]
async fn stale_empty_and_foreign_group_rows() {
    let fx = fixture();
    let conn = fx.db.connect().unwrap();
    conn.insert_entity(&entity("live", G, "live name", "s", false))
        .unwrap();
    conn.insert_entity(&entity("blank", G, "   ", "s", false))
        .unwrap();
    conn.insert_entity(&entity("foreign", "other", "foreign name", "s", false))
        .unwrap();
    record(&fx.db, G, ZeroKind::Name, "live");
    record(&fx.db, G, ZeroKind::Name, "gone");
    record(&fx.db, G, ZeroKind::Name, "blank");
    record(&fx.db, G, ZeroKind::Name, "foreign");
    conn.build_indices_and_constraints().unwrap();
    fx.state.indices_built.store(true, Ordering::SeqCst);

    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &[ZeroKind::Name], false),
        None,
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "name"), (4, 1, 0, 3), "{v}");
    // Left recorded: blank + foreign. Removed: live (repaired) and gone (stale).
    let left = conn.list_zero_vector_rows(G, None).unwrap();
    let uuids: Vec<&str> = left.iter().map(|(_, u)| u.as_str()).collect();
    assert_eq!(uuids, ["blank", "foreign"]);
}

/// FR-012: a zero vector or a wrong-dimension vector from the embedder is `failed`, never written.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_vectors_are_failed_not_written() {
    let fx = fixture();
    let conn = fx.db.connect().unwrap();
    conn.insert_entity(&entity("z", G, "zero text", "s", false))
        .unwrap();
    conn.insert_entity(&entity("s", G, "short text", "s", false))
        .unwrap();
    conn.insert_entity(&entity("ok", G, "fine text", "s", false))
        .unwrap();
    for u in ["z", "s", "ok"] {
        record(&fx.db, G, ZeroKind::Name, u);
    }
    conn.build_indices_and_constraints().unwrap();
    fx.state.indices_built.store(true, Ordering::SeqCst);

    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &[ZeroKind::Name], false),
        None,
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "name"), (3, 1, 2, 0), "{v}");
    assert_eq!(count(&fx.db, G), 2);
    assert!(finds(&fx.db, ZeroKind::Name, "fine text", "ok").await);
}

/// All embeds fail → nothing writable → no lock/index activity.
#[tokio::test(flavor = "multi_thread")]
async fn all_failed_touches_no_index() {
    let fx = fixture();
    let conn = fx.db.connect().unwrap();
    conn.insert_entity(&entity("b", G, "bad only", "s", false))
        .unwrap();
    record(&fx.db, G, ZeroKind::Name, "b");
    conn.build_indices_and_constraints().unwrap();
    fx.state.indices_built.store(true, Ordering::SeqCst);
    let ddl: Arc<dyn IndexDdl> = Arc::new(RecordingDdl {
        real: DbIndexDdl::new(Arc::clone(&fx.db)),
        events: Arc::clone(&fx.events),
        fail_create: None,
        slow_create: None,
    });
    let v = reembed_missing_with_ddl(
        Arc::clone(&fx.state),
        params(G, &[ZeroKind::Name], false),
        None,
        Some(ddl),
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "name"), (1, 0, 1, 0));
    assert!(
        events(&fx).iter().all(|e| e == "embed"),
        "{:?}",
        events(&fx)
    );
    assert!(fx.state.indices_built.load(Ordering::SeqCst));
}

/// FR-017, SC-007: a failed rebuild is reported, `indices_built` is left false, and the next
/// search rebuilds the indexes without manual intervention.
#[tokio::test(flavor = "multi_thread")]
async fn rebuild_failure_is_reported_and_next_search_heals() {
    let fx = fixture();
    seed_all_kinds(&fx);
    let ddl: Arc<dyn IndexDdl> = Arc::new(RecordingDdl {
        real: DbIndexDdl::new(Arc::clone(&fx.db)),
        events: Arc::clone(&fx.events),
        fail_create: Some(IndexKind::Fact),
        slow_create: None,
    });
    let v = reembed_missing_with_ddl(
        Arc::clone(&fx.state),
        params(G, &[ZeroKind::Fact], false),
        None,
        Some(ddl),
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "fact"), (1, 1, 0, 0), "{v}");
    assert_eq!(v["index_rebuild_errors"][0]["kind"], "fact", "{v}");
    assert!(!fx.state.indices_built.load(Ordering::SeqCst));

    let resp = handlers::dispatch(
        req(
            "knowledge_find_relationships",
            json!({"query": "alpha relates to beta", "group_ids": [G], "num_results": 5}),
        ),
        Arc::clone(&fx.state),
        None,
    )
    .await;
    let resp = serde_json::to_value(resp).unwrap();
    assert!(
        resp.get("error").is_none(),
        "search after failed rebuild: {resp}"
    );
    assert!(
        fx.state.indices_built.load(Ordering::SeqCst),
        "auto-heal must rebuild"
    );
    assert!(finds(&fx.db, ZeroKind::Fact, "alpha relates to beta", "r1").await);
}

/// FR-019: cancelling during the embed phase (client gone) touches nothing.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_during_embedding_changes_nothing() {
    let fx = fixture();
    seed_all_kinds(&fx);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx); // client disconnected
    let ddl: Arc<dyn IndexDdl> = Arc::new(RecordingDdl {
        real: DbIndexDdl::new(Arc::clone(&fx.db)),
        events: Arc::clone(&fx.events),
        fail_create: None,
        slow_create: None,
    });
    let v = reembed_missing_with_ddl(
        Arc::clone(&fx.state),
        params(G, &ZeroKind::ALL, false),
        Some(tx),
        Some(ddl),
    )
    .await
    .unwrap();
    assert_eq!(v["cancelled"], true, "{v}");
    assert_eq!(count(&fx.db, G), 4);
    assert!(events(&fx).iter().all(|e| e == "embed"));
    assert!(fx.state.indices_built.load(Ordering::SeqCst));
}

/// A cancelled run changes nothing — including stale record entries, which are only removed once
/// the embed phase (the last cancellation point) has completed.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_leaves_stale_entries_recorded() {
    let fx = fixture();
    seed_all_kinds(&fx);
    record(&fx.db, G, ZeroKind::Name, "gone");
    assert_eq!(count(&fx.db, G), 5);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx); // client disconnected
    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &ZeroKind::ALL, false),
        Some(tx),
    )
    .await
    .unwrap();
    assert_eq!(v["cancelled"], true, "{v}");
    assert_eq!(
        count(&fx.db, G),
        5,
        "a cancelled run must not remove stale entries"
    );
}

/// A present-but-non-boolean `dry_run` is rejected rather than silently running the real repair.
#[tokio::test(flavor = "multi_thread")]
async fn non_boolean_dry_run_is_rejected() {
    let fx = fixture();
    seed_all_kinds(&fx);
    for bad in [json!("true"), json!(1), json!("yes")] {
        let resp = handlers::dispatch(
            req(
                "knowledge_reembed_missing",
                json!({"group_id": G, "dry_run": bad}),
            ),
            Arc::clone(&fx.state),
            None,
        )
        .await;
        let v = serde_json::to_value(resp).unwrap();
        assert!(v.get("error").is_some(), "expected rejection: {v}");
    }
    assert_eq!(count(&fx.db, G), 4);
    assert!(events(&fx).is_empty(), "no embedder call or index change");
}

/// Progress events precede the terminal result and cover both phases.
#[tokio::test(flavor = "multi_thread")]
async fn streams_progress_events() {
    let fx = fixture();
    seed_all_kinds(&fx);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let v = reembed_missing(
        Arc::clone(&fx.state),
        params(G, &ZeroKind::ALL, false),
        Some(tx),
    )
    .await
    .unwrap();
    assert_eq!(kind_counts(&v, "name").1, 1);
    let mut phases = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        assert_eq!(ev["type"], "progress");
        phases.push(ev["phase"].as_str().unwrap().to_string());
    }
    assert!(phases.contains(&"embedding".to_string()), "{phases:?}");
    assert!(phases.contains(&"writing".to_string()), "{phases:?}");
}

/// AC5, FR-018: a search issued while the index is down returns no error, and once the repair
/// finishes the repaired row is found.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_search_is_safe_and_correct_afterwards() {
    let fx = fixture();
    seed_all_kinds(&fx);
    let ddl: Arc<dyn IndexDdl> = Arc::new(RecordingDdl {
        real: DbIndexDdl::new(Arc::clone(&fx.db)),
        events: Arc::clone(&fx.events),
        fail_create: None,
        slow_create: Some(std::time::Duration::from_millis(400)),
    });
    let repair_state = Arc::clone(&fx.state);
    let repair = tokio::spawn(async move {
        reembed_missing_with_ddl(
            repair_state,
            params(G, &[ZeroKind::Name], false),
            None,
            Some(ddl),
        )
        .await
    });
    // Wait until the name index has been dropped, then race a search in.
    for _ in 0..200 {
        if events(&fx).iter().any(|e| e == "drop:name") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let resp = handlers::dispatch(
        req(
            "knowledge_find_entities",
            json!({"query": "alpha name", "group_ids": [G], "num_results": 5}),
        ),
        Arc::clone(&fx.state),
        None,
    )
    .await;
    let resp = serde_json::to_value(resp).unwrap();
    assert!(
        resp.get("error").is_none(),
        "concurrent search failed: {resp}"
    );
    let v = repair.await.unwrap().unwrap();
    assert_eq!(kind_counts(&v, "name"), (1, 1, 0, 0), "{v}");
    assert!(finds(&fx.db, ZeroKind::Name, "alpha name", "e1").await);
}

/// Another group's rows and records are untouched.
#[tokio::test(flavor = "multi_thread")]
async fn other_groups_are_untouched() {
    let fx = fixture();
    seed_all_kinds(&fx);
    let conn = fx.db.connect().unwrap();
    conn.insert_entity(&entity("o1", "other", "other name", "s", false))
        .unwrap();
    record(&fx.db, "other", ZeroKind::Name, "o1");
    reembed_missing(
        Arc::clone(&fx.state),
        params(G, &ZeroKind::ALL, false),
        None,
    )
    .await
    .unwrap();
    assert_eq!(count(&fx.db, "other"), 1);
    assert_eq!(count(&fx.db, G), 0);
}

/// An invalid `kinds` value is rejected before any work (through the IPC dispatch).
#[tokio::test(flavor = "multi_thread")]
async fn invalid_kinds_rejected_before_work() {
    let fx = fixture();
    seed_all_kinds(&fx);
    for kinds in [json!(["bogus"]), json!([]), json!("name")] {
        let resp = handlers::dispatch(
            req(
                "knowledge_reembed_missing",
                json!({"group_id": G, "kinds": kinds}),
            ),
            Arc::clone(&fx.state),
            None,
        )
        .await;
        let v = serde_json::to_value(resp).unwrap();
        assert!(v.get("error").is_some(), "expected rejection: {v}");
    }
    let resp = handlers::dispatch(
        req("knowledge_reembed_missing", json!({})),
        Arc::clone(&fx.state),
        None,
    )
    .await;
    assert!(serde_json::to_value(resp).unwrap().get("error").is_some());
    assert_eq!(fx.embedder.calls.load(Ordering::SeqCst), 0);
    assert_eq!(count(&fx.db, G), 4);
}

/// The IPC method end to end (default retry policy from env) returns the documented envelope.
#[tokio::test(flavor = "multi_thread")]
async fn ipc_envelope_and_dry_run() {
    let fx = fixture();
    seed_all_kinds(&fx);
    let resp = handlers::dispatch(
        req(
            "knowledge_reembed_missing",
            json!({"group_id": G, "dry_run": true}),
        ),
        Arc::clone(&fx.state),
        None,
    )
    .await;
    let v = serde_json::to_value(resp).unwrap();
    let r = &v["result"];
    assert_eq!(r["group_id"], G, "{v}");
    assert_eq!(r["dry_run"], true);
    for k in ["name", "fact", "summary", "content"] {
        assert_eq!(r["kinds"][k]["candidates"], 1, "{k}: {v}");
    }
}
