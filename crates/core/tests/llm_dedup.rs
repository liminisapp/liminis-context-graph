// Integration tests for the LLM-verified extraction dedup check (issue #652, ADR-0652).
//
// A stub extractor both supplies the extraction results (via ConfigurableExtractor) and answers
// the dedup judge call, so each test controls exactly what the "LLM" says and counts how many
// times it was asked. NameMapEmbedder forces cosine >= DEDUP_THRESHOLD between chosen names.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::{DedupAdapter, ExtractorDedupAdapter, PassthroughDedupAdapter},
    dedup_judge::{DedupVerdict, DuplicatePair},
    embedder::NameMapEmbedder,
    episode,
    error::Error,
    extractor::{ConfigurableExtractor, ExtractOptions, Extractor},
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEntity, ExtractionOutcome, ExtractionResult, SourceType},
};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const EMB_DIM: usize = 4;
const REF_TIME: &str = "2026-01-01T00:00:00Z";
const GROUP: &str = "llm-dedup-grp";
const CONSOLIDATED: &str = "Alice Smith is a person (consolidated).";

type Answer = Box<dyn Fn(&[DuplicatePair]) -> Result<Vec<DedupVerdict>, Error> + Send + Sync>;

/// Extraction results come from `inner`; judge calls are counted and answered by `answer`.
struct StubExtractor {
    inner: ConfigurableExtractor,
    judge_calls: AtomicUsize,
    group_sizes: Mutex<Vec<usize>>,
    answer: Answer,
    delay: Duration,
    /// Cancelled from inside the judge call, to exercise mid-call cancellation.
    cancel_on_judge: Option<CancellationToken>,
}

impl StubExtractor {
    fn new(results: Vec<ExtractionResult>, answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            inner: ConfigurableExtractor::new(results),
            judge_calls: AtomicUsize::new(0),
            group_sizes: Mutex::new(vec![]),
            answer,
            delay: Duration::ZERO,
            cancel_on_judge: None,
        })
    }

    fn calls(&self) -> usize {
        self.judge_calls.load(Ordering::SeqCst)
    }
}

impl Extractor for StubExtractor {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        self.inner.extract(opts)
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
        allowed_types: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        self.inner.classify_relations(edges, allowed_types)
    }

    /// Fixed answer, so a judge-confirmed merge can be shown to go through #651's consolidation.
    fn consolidate_summary<'a>(
        &'a self,
        _entity_name: &'a str,
        _existing: &'a str,
        _incoming: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async { Ok(CONSOLIDATED.to_string()) })
    }

    fn judge_duplicates<'a>(
        &'a self,
        pairs: &'a [DuplicatePair],
    ) -> BoxFuture<'a, Result<Vec<DedupVerdict>, Error>> {
        self.judge_calls.fetch_add(1, Ordering::SeqCst);
        self.group_sizes.lock().unwrap().push(pairs.len());
        if let Some(t) = &self.cancel_on_judge {
            t.cancel();
        }
        let r = (self.answer)(pairs);
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            r
        })
    }
}

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(EMB_DIM).unwrap();
    (db, dir)
}

fn make_state(
    db: Arc<Db>,
    extractor: Arc<dyn Extractor>,
    dedup: Arc<dyn DedupAdapter>,
    emb: HashMap<String, Vec<f32>>,
    cancel: CancellationToken,
) -> Arc<AppState> {
    make_state_wal(db, extractor, dedup, emb, cancel, None)
}

fn make_state_wal(
    db: Arc<Db>,
    extractor: Arc<dyn Extractor>,
    dedup: Arc<dyn DedupAdapter>,
    emb: HashMap<String, Vec<f32>>,
    cancel: CancellationToken,
    wal_root: Option<std::path::PathBuf>,
) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(NameMapEmbedder::new(EMB_DIM, emb)),
        extractor,
        dedup,
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "test".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: cancel,
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: None,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn entities(names: &[&str]) -> ExtractionResult {
    ExtractionResult {
        entities: names
            .iter()
            .map(|n| ExtractedEntity {
                name: n.to_string(),
                entity_type: "Person".to_string(),
                summary: format!("{n} is a person"),
                original_entity_type: None,
            })
            .collect(),
        edges: vec![],
    }
}

/// `a` on axis `i`; `b` at ~22.5 degrees from `a` (cosine ~0.924, above the 0.85 threshold) and
/// orthogonal to every other axis-pair's vectors.
fn near_pair(map: &mut HashMap<String, Vec<f32>>, i: usize, a: &str, b: &str) {
    let mut va = vec![0.0f32; EMB_DIM];
    va[i] = 1.0;
    let mut vb = vec![0.0f32; EMB_DIM];
    vb[i] = 0.9239;
    vb[(i + 1) % EMB_DIM] = 0.3827;
    map.insert(a.to_string(), va);
    map.insert(b.to_string(), vb);
}

async fn ingest(state: &Arc<AppState>, name: &str) -> Result<episode::AddEpisodeResult, Error> {
    episode::add_episode(
        Arc::clone(state),
        name,
        "body",
        "test",
        "test source",
        REF_TIME,
        GROUP,
        SourceType::Text,
        None,
        "",
    )
    .await
}

fn entity_count(db: &Db) -> usize {
    db.connect().unwrap().entity_count_in_group(GROUP).unwrap()
}

/// Sorted entity names in `GROUP`.
fn names_in(conn: &lcg_core::db::Conn<'_>) -> Vec<String> {
    let mut v: Vec<String> = conn
        .get_entities_by_group_ids(Some(&[GROUP]))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    v.sort();
    v
}

fn confirm_all() -> Answer {
    Box::new(|pairs| Ok(vec![DedupVerdict::Duplicate; pairs.len()]))
}

fn reject_all() -> Answer {
    Box::new(|pairs| Ok(vec![DedupVerdict::Distinct; pairs.len()]))
}

/// Builds a two-episode scenario: episode 1 inserts `first`, episode 2 extracts `second`.
fn scenario(
    first: &[&str],
    second: &[&str],
    emb: HashMap<String, Vec<f32>>,
    answer: Answer,
) -> (Arc<StubExtractor>, Arc<AppState>, Arc<Db>, TempDir) {
    let (db, dir) = make_db();
    let stub = StubExtractor::new(vec![entities(first), entities(second)], answer);
    let dedup: Arc<dyn DedupAdapter> = Arc::new(ExtractorDedupAdapter::new(stub.clone()));
    let state = make_state(
        Arc::clone(&db),
        stub.clone(),
        dedup,
        emb,
        CancellationToken::new(),
    );
    (stub, state, db, dir)
}

// ── User Story 1: the LLM verdict decides Merge vs Insert ─────────────────────

#[tokio::test]
async fn llm_rejects_look_alike_candidate_and_inserts() {
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "Alice Smith", "Alice Smyth");
    let (stub, state, db, _d) = scenario(&["Alice Smith"], &["Alice Smyth"], emb, reject_all());
    ingest(&state, "ep1").await.unwrap();
    let r = ingest(&state, "ep2").await.unwrap();
    assert_eq!(stub.calls(), 1);
    assert_eq!(entity_count(&db), 2, "LLM rejected: both entities kept");
    assert_eq!(r.dedup_paths.llm_rejected, 1);
    assert_eq!(r.dedup_paths.llm_confirmed, 0);
    assert_eq!(r.dedup_paths.embedding_merge, 0);
}

#[tokio::test]
async fn llm_confirms_candidate_and_merges() {
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "Alice Smith", "Alice Smyth");
    let (stub, state, db, _d) = scenario(&["Alice Smith"], &["Alice Smyth"], emb, confirm_all());
    ingest(&state, "ep1").await.unwrap();
    let r = ingest(&state, "ep2").await.unwrap();
    assert_eq!(stub.calls(), 1);
    assert_eq!(entity_count(&db), 1);
    assert_eq!(r.dedup_paths.llm_confirmed, 1);
    assert_eq!(r.dedup_paths.llm_rejected, 0);
}

/// #651 and #652 together: the judge gates the candidate, and a confirmed merge still
/// consolidates the summary (and refreshes its embedding) on the merge path.
#[tokio::test]
async fn judge_confirmed_merge_consolidates_the_summary() {
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "Alice Smith", "Alice Smyth");
    let (stub, state, db, _d) = scenario(&["Alice Smith"], &["Alice Smyth"], emb, confirm_all());
    ingest(&state, "ep1").await.unwrap();
    ingest(&state, "ep2").await.unwrap();
    assert_eq!(stub.calls(), 1, "the judge still gated the candidate");
    let conn = db.connect().unwrap();
    let rows = conn.get_entities_by_group_ids(Some(&[GROUP])).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].summary, CONSOLIDATED);
}

// ── User Story 2: failures never merge and never abort ────────────────────────

async fn failure_inserts(answer: Answer, timeout: Option<Duration>, delay: Duration) {
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "Alice Smith", "Alice Smyth");
    let (db, _dir) = make_db();
    let stub = Arc::new(StubExtractor {
        inner: ConfigurableExtractor::new(vec![
            entities(&["Alice Smith"]),
            entities(&["Alice Smyth"]),
        ]),
        judge_calls: AtomicUsize::new(0),
        group_sizes: Mutex::new(vec![]),
        answer,
        delay,
        cancel_on_judge: None,
    });
    let adapter = match timeout {
        Some(t) => ExtractorDedupAdapter::with_timeout(stub.clone(), t),
        None => ExtractorDedupAdapter::new(stub.clone()),
    };
    let state = make_state(
        Arc::clone(&db),
        stub.clone(),
        Arc::new(adapter),
        emb,
        CancellationToken::new(),
    );
    ingest(&state, "ep1").await.unwrap();
    let r = ingest(&state, "ep2")
        .await
        .expect("a failed dedup judgement must not abort the chunk");
    assert_eq!(
        entity_count(&db),
        2,
        "unverifiable candidate must not merge"
    );
    assert_eq!(r.dedup_paths.llm_unavailable, 1);
    assert_eq!(r.dedup_paths.llm_confirmed, 0);
}

#[tokio::test]
async fn extractor_error_inserts_and_extraction_completes() {
    failure_inserts(
        Box::new(|_| Err(Error::Ipc("boom".into()))),
        None,
        Duration::ZERO,
    )
    .await;
}

#[tokio::test]
async fn malformed_answer_inserts() {
    // A reply that parsed to no attributable verdicts reaches the adapter as all-Unknown.
    failure_inserts(
        Box::new(|pairs| Ok(vec![DedupVerdict::Unknown; pairs.len()])),
        None,
        Duration::ZERO,
    )
    .await;
}

#[tokio::test]
async fn short_answer_inserts() {
    failure_inserts(Box::new(|_| Ok(vec![])), None, Duration::ZERO).await;
}

#[tokio::test]
async fn timeout_inserts() {
    failure_inserts(
        confirm_all(),
        Some(Duration::from_millis(20)),
        Duration::from_millis(500),
    )
    .await;
}

#[tokio::test]
async fn cancellation_during_judge_call_propagates_as_cancelled() {
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "Alice Smith", "Alice Smyth");
    let (db, _dir) = make_db();
    let cancel = CancellationToken::new();
    let stub = Arc::new(StubExtractor {
        inner: ConfigurableExtractor::new(vec![
            entities(&["Alice Smith"]),
            entities(&["Alice Smyth"]),
        ]),
        judge_calls: AtomicUsize::new(0),
        group_sizes: Mutex::new(vec![]),
        answer: confirm_all(),
        delay: Duration::from_secs(30),
        cancel_on_judge: Some(cancel.clone()),
    });
    let dedup: Arc<dyn DedupAdapter> = Arc::new(ExtractorDedupAdapter::new(stub.clone()));
    let state = make_state(Arc::clone(&db), stub.clone(), dedup, emb, cancel);
    // Episode 1 inserts (no candidate, so no judge call and the token is untouched).
    ingest(&state, "ep1").await.unwrap();
    assert_eq!(stub.calls(), 0);
    let r = ingest(&state, "ep2").await;
    assert!(matches!(r, Err(Error::Cancelled)), "got {r:?}");
    assert_eq!(state.cancelled_chunks.load(Ordering::Relaxed), 1);
}

// ── User Story 3: cheap pairs never reach the LLM ────────────────────────────

#[tokio::test]
async fn exact_name_match_makes_no_llm_call() {
    let (stub, state, db, _d) = scenario(
        &["Alice Smith"],
        &["alice smith"],
        HashMap::new(),
        confirm_all(),
    );
    ingest(&state, "ep1").await.unwrap();
    let r = ingest(&state, "ep2").await.unwrap();
    assert_eq!(stub.calls(), 0);
    assert_eq!(entity_count(&db), 1);
    assert_eq!(r.dedup_paths.exact_name, 1);
}

#[tokio::test]
async fn veto_rejected_pair_makes_no_llm_call() {
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "ADR 2018", "ADR 2019");
    let (stub, state, db, _d) = scenario(&["ADR 2018"], &["ADR 2019"], emb, confirm_all());
    ingest(&state, "ep1").await.unwrap();
    let r = ingest(&state, "ep2").await.unwrap();
    assert_eq!(stub.calls(), 0, "the identifier veto is authoritative");
    assert_eq!(entity_count(&db), 2);
    assert_eq!(r.dedup_paths.vetoed, 1);
}

#[tokio::test]
async fn identical_normalized_name_makes_no_llm_call() {
    // A trailing tab is stripped by `normalize_name` but is not an exact-name match.
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "Alice Smith", "Alice Smith\t");
    let (stub, state, db, _d) = scenario(&["Alice Smith"], &["Alice Smith\t"], emb, reject_all());
    ingest(&state, "ep1").await.unwrap();
    ingest(&state, "ep2").await.unwrap();
    assert_eq!(stub.calls(), 0);
    assert_eq!(entity_count(&db), 1);
}

#[tokio::test]
async fn several_candidates_are_judged_in_one_call_with_correct_attribution() {
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "Alpha Group", "Alpha Grp");
    near_pair(&mut emb, 1, "Beta Group", "Beta Grp");
    near_pair(&mut emb, 2, "Gamma Group", "Gamma Grp");
    // Confirm only the Alpha and Gamma pairs, judged by content so a positional mix-up fails.
    let answer: Answer = Box::new(|pairs| {
        Ok(pairs
            .iter()
            .map(|p| {
                if p.incoming.name.starts_with("Beta") {
                    DedupVerdict::Distinct
                } else {
                    DedupVerdict::Duplicate
                }
            })
            .collect())
    });
    let (stub, state, db, _d) = scenario(
        &["Alpha Group", "Beta Group", "Gamma Group"],
        &["Alpha Grp", "Beta Grp", "Gamma Grp"],
        emb,
        answer,
    );
    ingest(&state, "ep1").await.unwrap();
    let r = ingest(&state, "ep2").await.unwrap();
    assert_eq!(stub.calls(), 1, "one batched call per chunk");
    assert_eq!(*stub.group_sizes.lock().unwrap(), vec![3]);
    assert_eq!(
        entity_count(&db),
        4,
        "Alpha and Gamma merged, Beta kept apart"
    );
    assert_eq!(r.dedup_paths.llm_confirmed, 2);
    assert_eq!(r.dedup_paths.llm_rejected, 1);
    let conn = db.connect().unwrap();
    assert_eq!(
        conn.count_entities_by_name_ci("Beta Grp", GROUP).unwrap(),
        1
    );
    assert_eq!(
        conn.count_entities_by_name_ci("Alpha Grp", GROUP).unwrap(),
        0
    );
    assert_eq!(
        conn.count_entities_by_name_ci("Gamma Grp", GROUP).unwrap(),
        0
    );
}

// ── Veto-only mode is unchanged ───────────────────────────────────────────────

#[tokio::test]
async fn veto_only_mode_still_merges_without_any_judge_call() {
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "Alice Smith", "Alice Smyth");
    let (db, _dir) = make_db();
    let stub = StubExtractor::new(
        vec![entities(&["Alice Smith"]), entities(&["Alice Smyth"])],
        reject_all(),
    );
    let state = make_state(
        Arc::clone(&db),
        stub.clone(),
        Arc::new(PassthroughDedupAdapter),
        emb,
        CancellationToken::new(),
    );
    ingest(&state, "ep1").await.unwrap();
    let r = ingest(&state, "ep2").await.unwrap();
    assert_eq!(entity_count(&db), 1);
    assert_eq!(r.dedup_paths.embedding_merge, 1);
    assert_eq!(r.dedup_paths.llm_confirmed, 0);
    assert_eq!(stub.calls(), 0);
}

// ── User Story 6: WAL replay never calls the LLM and reproduces the graph ─────

#[test]
fn wal_replay_reproduces_llm_decisions_without_calling_the_judge() {
    let mut emb = HashMap::new();
    near_pair(&mut emb, 0, "Alpha Group", "Alpha Grp");
    near_pair(&mut emb, 1, "Beta Group", "Beta Grp");
    let answer: Answer = Box::new(|pairs| {
        Ok(pairs
            .iter()
            .map(|p| {
                if p.incoming.name.starts_with("Beta") {
                    DedupVerdict::Distinct
                } else {
                    DedupVerdict::Duplicate
                }
            })
            .collect())
    });
    let wal_dir = TempDir::new().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (live_names, calls_after_ingest, stub) = rt.block_on(async {
        let (db, _dir) = make_db();
        let stub = StubExtractor::new(
            vec![
                entities(&["Alpha Group", "Beta Group"]),
                entities(&["Alpha Grp", "Beta Grp"]),
            ],
            answer,
        );
        let dedup: Arc<dyn DedupAdapter> = Arc::new(ExtractorDedupAdapter::new(stub.clone()));
        let state = make_state_wal(
            Arc::clone(&db),
            stub.clone(),
            dedup,
            emb,
            CancellationToken::new(),
            Some(wal_dir.path().to_path_buf()),
        );
        ingest(&state, "ep1").await.unwrap();
        ingest(&state, "ep2").await.unwrap();
        let names = names_in(&db.connect().unwrap());
        (names, stub.calls(), stub)
    });
    assert_eq!(calls_after_ingest, 1);
    assert_eq!(
        live_names.len(),
        3,
        "Alpha merged, Beta kept apart: {live_names:?}"
    );

    // Replay every group's WAL stream into a fresh db. `WalReplayer` takes only a connection and
    // an embed function: it has no path to the extractor or the dedup adapter by construction.
    let replay_dir = TempDir::new().unwrap();
    let replay_db = Db::open(replay_dir.path().join("replay.db").to_str().unwrap()).unwrap();
    let conn = replay_db.connect().unwrap();
    conn.init_schema(EMB_DIM).unwrap();
    let mut replayed_any = false;
    for entry in std::fs::read_dir(wal_dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            lcg_core::WalReplayer::new(&path)
                .replay(&conn, lcg_core::zero_vector_embed_fn(EMB_DIM), EMB_DIM)
                .unwrap();
            replayed_any = true;
        }
    }
    assert!(replayed_any, "expected a per-group WAL directory");
    let mut replayed_names = names_in(&conn);
    replayed_names.sort();
    assert_eq!(
        replayed_names, live_names,
        "replay must reproduce the live graph"
    );
    assert_eq!(
        stub.calls(),
        calls_after_ingest,
        "replay must not call the judge"
    );
}
