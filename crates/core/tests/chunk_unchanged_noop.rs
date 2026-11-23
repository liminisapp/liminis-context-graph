//! Issue #858 / ADR-0858: `knowledge_process_chunk` is a no-op returning `status: "unchanged"`
//! when a resubmitted chunk is identical to the chunk's single live episode. Counting stand-ins
//! for the extractor and embedder prove "zero calls"; the WAL directory's byte total proves "no
//! WAL record".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::ExtractOptions,
    handlers,
    ipc::IpcRequest,
    ontology::{compute_ancestor_map, EntityTypeDef, Ontology, OntologyMode},
    telemetry::{TelemetryEvent, TelemetrySink},
    types::ExtractionOutcome,
    ConfigurableExtractor, Db, EmbedRole, Embedder, ExtractedEdge, ExtractedEntity,
    ExtractionResult, Extractor, WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "liminis";

struct CountingExtractor {
    inner: ConfigurableExtractor,
    calls: Arc<AtomicUsize>,
}

impl Extractor for CountingExtractor {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, lcg_core::Error>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.extract(opts)
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        allowed_types: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, lcg_core::Error>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.classify_entities(entities, allowed_types)
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        allowed_types: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, lcg_core::Error>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.classify_relations(edges, allowed_types)
    }
}

struct CountingEmbedder {
    inner: MockEmbedder,
    calls: Arc<AtomicUsize>,
}

impl Embedder for CountingEmbedder {
    fn embed<'a>(
        &'a self,
        text: &'a str,
        role: EmbedRole,
    ) -> BoxFuture<'a, Result<Vec<f32>, lcg_core::Error>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.embed(text, role)
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }
}

/// Counts telemetry events of the oversize kind.
#[derive(Default)]
struct OversizeSink(AtomicUsize);

impl TelemetrySink for OversizeSink {
    fn emit(&self, event: TelemetryEvent) {
        if matches!(event, TelemetryEvent::ChunkTextOversized { .. }) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

struct Harness {
    state: Arc<AppState>,
    db: Arc<Db>,
    wal_dir: PathBuf,
    extract_calls: Arc<AtomicUsize>,
    embed_calls: Arc<AtomicUsize>,
    oversize: Arc<OversizeSink>,
    _dirs: (TempDir, TempDir),
}

fn extraction(n: usize) -> ExtractionResult {
    ExtractionResult {
        entities: vec![
            ExtractedEntity {
                name: format!("A{n}"),
                entity_type: "Thing".into(),
                summary: "s".into(),
                original_entity_type: None,
            },
            ExtractedEntity {
                name: format!("B{n}"),
                entity_type: "Thing".into(),
                summary: "s".into(),
                original_entity_type: None,
            },
        ],
        edges: vec![ExtractedEdge {
            source_name: format!("A{n}"),
            target_name: format!("B{n}"),
            fact: format!("fact {n}"),
            relation_type: Some("RELATED_TO".into()),
            valid_at: None,
            invalid_at: None,
            original_relation_type: None,
        }],
    }
}

fn harness() -> Harness {
    let db_dir = TempDir::new().unwrap();
    let wal_tmp = TempDir::new().unwrap();
    let wal_dir = wal_tmp.path().to_path_buf();
    let db = Arc::new(Db::open(db_dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
    }
    let extract_calls = Arc::new(AtomicUsize::new(0));
    let embed_calls = Arc::new(AtomicUsize::new(0));
    let oversize = Arc::new(OversizeSink::default());
    let writers: HashMap<String, WalWriter> = WalWriter::new(&wal_dir, 10_000, 5 * 1024 * 1024)
        .ok()
        .map(|w| (G.to_string(), w))
        .into_iter()
        .collect();
    let sink: Arc<dyn TelemetrySink> = Arc::clone(&oversize) as Arc<dyn TelemetrySink>;
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(Arc::clone(&db))),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(CountingEmbedder {
            inner: MockEmbedder::new(DIM),
            calls: Arc::clone(&embed_calls),
        }),
        extractor: Arc::new(CountingExtractor {
            inner: ConfigurableExtractor::new((0..40).map(extraction).collect()),
            calls: Arc::clone(&extract_calls),
        }),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(tokio::sync::RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: Some(wal_dir.clone()),
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(writers)),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: None,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        chunk_inflight: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    });
    Harness {
        state,
        db,
        wal_dir,
        extract_calls,
        embed_calls,
        oversize,
        _dirs: (db_dir, wal_tmp),
    }
}

async fn try_call(method: &str, params: Value, state: &Arc<AppState>) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, Arc::clone(state), None).await).unwrap()
}

async fn call(method: &str, params: Value, state: &Arc<AppState>) -> Value {
    let v = try_call(method, params, state).await;
    assert!(v.get("error").is_none(), "{method} errored: {v}");
    v["result"].clone()
}

fn base_params(group: &str, chunk_id: &str) -> Value {
    json!({
        "chunk_text": format!("text of {chunk_id}"), "chunk_id": chunk_id,
        "source_file": "doc.md", "group_id": group,
        "reference_time": "2026-08-04T00:00:00Z",
    })
}

fn with(mut p: Value, extra: Value) -> Value {
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        p[k] = v;
    }
    p
}

async fn chunk(h: &Harness, params: Value) -> Value {
    let r = call("knowledge_process_chunk", params, &h.state).await;
    assert_eq!(r["success"], true, "{r}");
    r
}

/// Total bytes of every file under `dir`: any appended WAL record changes it.
fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                total += dir_bytes(&p);
            } else {
                total += std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// `(episodes, live episodes)` in the whole DB.
fn episode_counts(db: &Db) -> (usize, usize) {
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 100, None, None).unwrap();
    let live = eps
        .iter()
        .filter(|(e, _)| e.superseded_at.is_none())
        .count();
    (eps.len(), live)
}

struct Snapshot {
    extract: usize,
    embed: usize,
    wal: u64,
    episodes: (usize, usize),
}

fn snap(h: &Harness) -> Snapshot {
    Snapshot {
        extract: h.extract_calls.load(Ordering::SeqCst),
        embed: h.embed_calls.load(Ordering::SeqCst),
        wal: dir_bytes(&h.wal_dir),
        episodes: episode_counts(&h.db),
    }
}

fn assert_untouched(before: &Snapshot, after: &Snapshot) {
    assert_eq!(before.extract, after.extract, "extractor was called");
    assert_eq!(before.embed, after.embed, "embedder was called");
    assert_eq!(before.wal, after.wal, "a WAL record was written");
    assert_eq!(before.episodes, after.episodes, "episodes changed");
}

/// Resubmits `params` and asserts it was an ordinary supersede (extracted, new uuid).
async fn assert_supersedes(h: &Harness, first_uuid: &str, params: Value) -> String {
    let before = snap(h);
    let r = chunk(h, params).await;
    assert_ne!(r["status"], "unchanged", "{r}");
    assert_ne!(r["episode_uuid"], first_uuid, "{r}");
    assert_eq!(r["superseded_episode_uuids"], json!([first_uuid]), "{r}");
    let after = snap(h);
    assert!(after.extract > before.extract, "extractor not called");
    assert!(after.embed > before.embed, "embedder not called");
    r["episode_uuid"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn identical_resubmission_is_a_noop() {
    let h = harness();
    let p = base_params(G, "c1");
    let first = chunk(&h, p.clone()).await;
    assert!(first.get("status").is_none(), "{first}");
    let before = snap(&h);
    assert!(before.extract > 0 && before.embed > 0 && before.wal > 0);

    let second = chunk(&h, p).await;
    assert_eq!(second["status"], "unchanged", "{second}");
    assert_eq!(second["episode_uuid"], first["episode_uuid"], "{second}");
    assert!(second.get("nodes_extracted").is_none(), "{second}");
    assert!(second.get("superseded_episode_uuids").is_none(), "{second}");
    assert_untouched(&before, &snap(&h));
    assert_eq!(episode_counts(&h.db), (1, 1), "episode still live");
}

#[tokio::test]
async fn attributes_key_order_only_is_unchanged() {
    let h = harness();
    let first = chunk(
        &h,
        with(
            base_params(G, "c1"),
            json!({"attributes": {"a": 1, "b": 2}}),
        ),
    )
    .await;
    let before = snap(&h);
    let r = chunk(
        &h,
        with(
            base_params(G, "c1"),
            json!({"attributes": {"b": 2, "a": 1}}),
        ),
    )
    .await;
    assert_eq!(r["status"], "unchanged", "{r}");
    assert_eq!(r["episode_uuid"], first["episode_uuid"]);
    assert_untouched(&before, &snap(&h));
}

#[tokio::test]
async fn omitted_reference_time_is_not_a_difference() {
    let h = harness();
    let mut p = base_params(G, "c1");
    p.as_object_mut().unwrap().remove("reference_time");
    let first = chunk(&h, p.clone()).await;
    let before = snap(&h);
    let r = chunk(&h, p).await;
    assert_eq!(r["status"], "unchanged", "{r}");
    assert_eq!(r["episode_uuid"], first["episode_uuid"]);
    assert_untouched(&before, &snap(&h));
}

#[tokio::test]
async fn equivalent_explicit_reference_time_is_unchanged() {
    let h = harness();
    chunk(&h, base_params(G, "c1")).await;
    let before = snap(&h);
    // Same instant, different offset notation.
    let r = chunk(
        &h,
        with(
            base_params(G, "c1"),
            json!({"reference_time": "2026-08-04T02:00:00+02:00"}),
        ),
    )
    .await;
    assert_eq!(r["status"], "unchanged", "{r}");
    assert_untouched(&before, &snap(&h));
}

#[tokio::test]
async fn each_single_field_change_supersedes() {
    let cases: Vec<(&str, Value)> = vec![
        ("text", json!({"chunk_text": "text of c1!"})),
        ("attributes", json!({"attributes": {"k": "v"}})),
        ("origin", json!({"origin": "external"})),
        ("source_file", json!({"source_file": "other.md"})),
        ("doc_id", json!({"doc_id": "d1"})),
        (
            "reference_time",
            json!({"reference_time": "2026-08-05T00:00:00Z"}),
        ),
    ];
    for (label, change) in cases {
        let h = harness();
        let first = chunk(&h, base_params(G, "c1")).await;
        let uuid = first["episode_uuid"].as_str().unwrap();
        let new = assert_supersedes(&h, uuid, with(base_params(G, "c1"), change)).await;
        assert_ne!(new, uuid, "{label}");
        assert_eq!(episode_counts(&h.db), (2, 1), "{label}");
    }
}

#[tokio::test]
async fn superseded_revision_text_is_compared_against_live_episode() {
    let h = harness();
    let first = chunk(&h, base_params(G, "c1")).await;
    let second = chunk(
        &h,
        with(base_params(G, "c1"), json!({"chunk_text": "revised text"})),
    )
    .await;
    // The original text now differs from the live (revised) episode.
    let r = chunk(&h, base_params(G, "c1")).await;
    assert_ne!(r["status"], "unchanged", "{r}");
    assert_eq!(
        r["superseded_episode_uuids"],
        json!([second["episode_uuid"]]),
        "{r}"
    );
    assert_ne!(r["episode_uuid"], first["episode_uuid"]);
}

#[tokio::test]
async fn removed_chunk_is_ingested_normally() {
    let h = harness();
    chunk(&h, base_params(G, "c1")).await;
    let rm = call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "c1", "group_ids": [G]}),
        &h.state,
    )
    .await;
    assert_eq!(rm["superseded_count"], 1);
    let before = snap(&h);
    let r = chunk(&h, base_params(G, "c1")).await;
    assert_ne!(r["status"], "unchanged", "{r}");
    let after = snap(&h);
    assert!(after.extract > before.extract);
    assert_eq!(after.episodes, (2, 1));
}

#[tokio::test]
async fn first_time_chunk_is_ingested_normally() {
    let h = harness();
    let r = chunk(&h, base_params(G, "never-seen")).await;
    assert!(r.get("status").is_none(), "{r}");
    assert!(h.extract_calls.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn supersede_false_never_shortcuts() {
    let h = harness();
    chunk(&h, base_params(G, "c1")).await;

    // supersede:false is a non-final unit of a multi-call revision.
    let before = snap(&h);
    let r = chunk(&h, with(base_params(G, "c1"), json!({"supersede": false}))).await;
    assert_ne!(r["status"], "unchanged", "{r}");
    assert!(snap(&h).extract > before.extract);
    assert_eq!(r["superseded_episode_uuids"], json!([]), "{r}");
    // Two live episodes for the chunk now.
    assert_eq!(episode_counts(&h.db), (2, 2));
}

#[tokio::test]
async fn more_than_one_live_episode_never_shortcuts() {
    let h = harness();
    chunk(&h, base_params(G, "c1")).await;
    chunk(&h, with(base_params(G, "c1"), json!({"supersede": false}))).await;
    assert_eq!(episode_counts(&h.db), (2, 2));

    // More than one live episode: never `unchanged`, even for identical text.
    let before = snap(&h);
    let r = chunk(&h, base_params(G, "c1")).await;
    assert_ne!(r["status"], "unchanged", "{r}");
    assert!(snap(&h).extract > before.extract);
}

#[tokio::test]
async fn same_revision_episode_uuids_never_shortcuts() {
    let h = harness();
    let first = chunk(&h, base_params(G, "c1")).await;
    let before = snap(&h);
    let r = chunk(
        &h,
        with(
            base_params(G, "c1"),
            json!({"same_revision_episode_uuids": [first["episode_uuid"]]}),
        ),
    )
    .await;
    assert_ne!(r["status"], "unchanged", "{r}");
    assert!(snap(&h).extract > before.extract);
}

#[tokio::test]
async fn foreign_live_episode_never_yields_unchanged() {
    let h = harness();
    let foreign = call(
        "knowledge_add_episode",
        json!({"name": "c1", "episode_body": "text of c1", "group_id": G,
               "source_description": "someone else",
               "reference_time": "2026-08-04T00:00:00Z"}),
        &h.state,
    )
    .await["episode_uuid"]
        .as_str()
        .unwrap()
        .to_string();
    let before = snap(&h);
    let r = chunk(&h, base_params(G, "c1")).await;
    assert_ne!(r["status"], "unchanged", "{r}");
    assert!(snap(&h).extract > before.extract);
    assert_eq!(r["left_alone_episode_uuids"], json!([foreign]), "{r}");
}

#[tokio::test]
async fn same_chunk_id_in_another_group_is_not_consulted() {
    let h = harness();
    chunk(&h, base_params("g1", "c1")).await;
    let before = snap(&h);
    let r = chunk(&h, base_params("g2", "c1")).await;
    assert_ne!(r["status"], "unchanged", "{r}");
    assert!(snap(&h).extract > before.extract);
}

#[tokio::test]
async fn invalid_requests_are_rejected_not_unchanged() {
    let h = harness();
    chunk(&h, base_params(G, "c1")).await;
    for bad in [
        json!({"reference_time": "not-a-time"}),
        json!({"origin": "bogus"}),
        json!({"supersede": "yes"}),
        json!({"chunk_text": ""}),
        json!({"attributes": "x", "doc_id": 5}),
    ] {
        let v = try_call(
            "knowledge_process_chunk",
            with(base_params(G, "c1"), bad.clone()),
            &h.state,
        )
        .await;
        assert!(v.get("error").is_some(), "{bad} was not rejected: {v}");
    }
}

#[tokio::test]
async fn unchanged_oversized_chunk_emits_no_warning_or_telemetry() {
    let h = harness();
    let big = "x".repeat(20_000);
    let p = with(base_params(G, "c1"), json!({"chunk_text": big}));
    let first = chunk(&h, p.clone()).await;
    assert!(first.get("warning").is_some(), "{first}");
    let events = h.oversize.0.load(Ordering::SeqCst);
    assert_eq!(events, 1);
    let r = chunk(&h, p).await;
    assert_eq!(r["status"], "unchanged", "{r}");
    assert!(r.get("warning").is_none(), "{r}");
    assert_eq!(h.oversize.0.load(Ordering::SeqCst), events);
}

/// Review item 1: a scheduled (future-effective) document supersession is pending on the live
/// episode, so an identical resubmission must take the ordinary path, which overrides it.
#[tokio::test]
async fn pending_document_supersession_is_not_unchanged() {
    let h = harness();
    let p1 = with(base_params(G, "c1"), json!({"doc_id": "d1"}));
    let first = chunk(&h, p1.clone()).await;
    chunk(&h, with(base_params(G, "c2"), json!({"doc_id": "d2"}))).await;
    let future = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
    let s = call(
        "knowledge_supersede_document",
        json!({
            "group_id": G, "superseding_doc_id": "d2",
            "superseded_doc_id": "d1", "effective_at": future,
        }),
        &h.state,
    )
    .await;
    assert_eq!(s["success"], true, "{s}");
    let before = snap(&h);
    let r = chunk(&h, p1).await;
    assert_ne!(r["status"], "unchanged", "{r}");
    assert_ne!(r["episode_uuid"], first["episode_uuid"], "{r}");
    assert!(
        snap(&h).extract > before.extract,
        "ordinary path must extract"
    );
}

/// Review item 3: while another call for the same chunk is in flight, the no-op is not taken
/// (commit-instant ordering decides); once it finishes the no-op applies again.
#[tokio::test]
async fn in_flight_call_for_the_chunk_disables_the_noop() {
    let h = harness();
    let first = chunk(&h, base_params(G, "c1")).await;
    let (guard, alone) = h.state.chunk_inflight_enter(G, "c1");
    assert!(alone);
    let before = snap(&h);
    let r = chunk(&h, base_params(G, "c1")).await;
    assert_ne!(r["status"], "unchanged", "{r}");
    assert_ne!(r["episode_uuid"], first["episode_uuid"], "{r}");
    assert!(snap(&h).extract > before.extract);
    // A different chunk, or the same chunk id in another group, is unaffected.
    let other = chunk(&h, base_params(G, "c9")).await;
    let before = snap(&h);
    let r = chunk(&h, base_params(G, "c9")).await;
    assert_eq!(r["status"], "unchanged", "{r}");
    assert_eq!(r["episode_uuid"], other["episode_uuid"]);
    assert_untouched(&before, &snap(&h));
    drop(guard);
    let live = chunk(&h, base_params(G, "c1")).await;
    assert_eq!(live["status"], "unchanged", "{live}");
    // Every call released its registration.
    assert!(h.state.chunk_inflight.lock().unwrap().is_empty());
}

/// Review item 3: a concurrent `knowledge_mark_chunk_removed` counts as in flight too, and
/// releases its registration when done.
#[tokio::test]
async fn mark_chunk_removed_counts_as_in_flight_and_releases() {
    let h = harness();
    chunk(&h, base_params(G, "c1")).await;
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "c1", "group_ids": [G]}),
        &h.state,
    )
    .await;
    assert!(h.state.chunk_inflight.lock().unwrap().is_empty());
    // Two registrations for one key: only the first is alone.
    let (g1, a1) = h.state.chunk_inflight_enter(G, "c1");
    let (g2, a2) = h.state.chunk_inflight_enter(G, "c1");
    assert!(a1 && !a2);
    drop(g1);
    drop(g2);
    assert!(h.state.chunk_inflight.lock().unwrap().is_empty());
}

/// Review item 4: the doc_id "one document has one origin" refusal applies to a no-op exactly as
/// to the ingest it replaces.
#[tokio::test]
async fn unchanged_resubmission_still_hits_the_doc_origin_guard() {
    let h = harness();
    let p1 = with(base_params(G, "c1"), json!({"doc_id": "d1"}));
    chunk(&h, p1.clone()).await;
    chunk(&h, with(base_params(G, "c2"), json!({"doc_id": "d1"}))).await;
    // Give the document's other chunk a different origin behind the guard's back.
    {
        let conn = h.db.connect().unwrap();
        conn.cypher_query("MATCH (ep:Episodic) WHERE ep.name = 'c2' SET ep.origin = 'external'")
            .unwrap();
    }
    let before = snap(&h);
    let v = try_call("knowledge_process_chunk", p1, &h.state).await;
    let err = v["error"].to_string();
    assert!(err.contains("origin_conflict"), "expected a refusal: {v}");
    assert_untouched(&before, &snap(&h));
}

/// An ontology with a single `Thing` entity type, optionally identity-bearing.
fn thing_ontology(identity: bool) -> Ontology {
    let entity_types = vec![EntityTypeDef {
        name: "Thing".to_string(),
        description: None,
        parent: None,
        identity,
        extract: true,
    }];
    let ancestor_map = compute_ancestor_map(&entity_types);
    Ontology {
        mode: OntologyMode::Open,
        entity_types,
        relation_types: vec![],
        ancestor_map,
    }
}

/// A second `AppState` over the harness's DB, WAL and counting stand-ins, with its own
/// workspace root and ontology (so the identity-set check can run).
fn state_with_ontology(h: &Harness, root: &Path, ontology: Ontology) -> Arc<AppState> {
    let s = &h.state;
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(Arc::clone(&h.db))),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::clone(&s.embedder),
        extractor: Arc::clone(&s.extractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(tokio::sync::RwLock::new(())),
        sink: Arc::clone(&s.sink),
        db_path: "test.db".to_string(),
        wal_root: s.wal_root.clone(),
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::clone(&s.wal_writers),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: Some(root.to_path_buf()),
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: Some(Arc::new(ontology)),
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        chunk_inflight: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

/// Review (PR 878): `check_identity` runs before the no-op, so a group whose identity-bearing set
/// drifted (#616) refuses an identical resubmission exactly as it would refuse the ingest, rather
/// than answering `unchanged`. Pins the ordering of the refusal and the shortcut.
#[tokio::test]
async fn identity_drift_refuses_an_identical_resubmission() {
    let h = harness();
    let root = TempDir::new().unwrap();

    // Ingest under a flagless ontology: the group now holds `Thing` entities.
    let s1 = state_with_ontology(&h, root.path(), thing_ontology(false));
    let p = base_params(G, "c1");
    let r = call("knowledge_process_chunk", p.clone(), &s1).await;
    assert_eq!(r["success"], true, "{r}");
    // Sanity: with no drift the identical resubmission is a no-op.
    let r = call("knowledge_process_chunk", p.clone(), &s1).await;
    assert_eq!(r["status"], "unchanged", "{r}");

    // `Thing` becomes identity-bearing while entities already carry that label: refused.
    let s2 = state_with_ontology(&h, root.path(), thing_ontology(true));
    let before = snap(&h);
    let v = try_call("knowledge_process_chunk", p, &s2).await;
    assert!(
        v.get("error").is_some(),
        "expected an identity refusal: {v}"
    );
    assert!(
        v["result"]["status"] != "unchanged",
        "identity drift must not be reported as unchanged: {v}"
    );
    assert_untouched(&before, &snap(&h));
}
