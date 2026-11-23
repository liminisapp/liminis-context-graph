// Offline measurement for `lcg-eval retrieval --ingest-concurrency` (#900, FR-009 / SC-004).
//
// Question: what happens when a cassette recorded while several chunks were in flight is
// replayed one chunk at a time (the only way `lcg-eval` replays)?
//
// A content-deterministic stub extractor is wrapped in `RecordingExtractor`; a small corpus whose
// chunks all mention one shared entity is ingested through `episode::add_episode` on a shared
// `AppState`, once sequentially and once with every chunk in flight, in an order that makes the
// concurrent commit order differ from the manifest order. Each cassette is then replayed
// sequentially through a counting wrapper that tallies `CassetteMiss` per call type.
//
// What this pins down (the stable properties — the exact counts depend on scheduling):
//   * `extract` and `classify_*` keys never depend on graph state: zero misses at any order.
//   * `consolidate_summary` keys hash the *existing* summary, which depends on commit order: a
//     cassette recorded with chunks in flight misses them when replayed in manifest order. The
//     service swallows that miss (bounded fallback + an stderr line), which is why `lcg-eval`
//     marks such a cassette `order_dependent` and always replays at concurrency 1.
//   * The control: sequential record -> sequential replay has zero misses of every type.
//   * Concurrent Phase B runs without the write lock (ADR-0029), so chunks in flight together all
//     see "no match" for a new entity and each insert it: live N>1 graphs can hold duplicates.
//
// The miss counts observed are printed with `--nocapture` and recorded in ADR-0900.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    cassette::{CassetteWriter, RecordingExtractor, ReplayingExtractor},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    dedup_judge::{DedupVerdict, DuplicatePair},
    embedder::MockEmbedder,
    episode,
    error::Error,
    extractor::{ExtractOptions, Extractor},
    telemetry::{NoopSink, TelemetrySink},
    types::{ExtractedEntity, ExtractionOutcome, ExtractionResult, SourceType},
};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const EMB_DIM: usize = 4;
const REF_TIME: &str = "2026-01-01T00:00:00Z";
const GROUP: &str = "eval";
const CHUNKS: usize = 4;
/// Gap between extraction finishes, so the concurrent commit order is deterministic enough to
/// reverse the manifest order without making the test slow.
const STEP_MS: u64 = 60;

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(EMB_DIM).unwrap();
    (db, dir)
}

fn make_state(db: Arc<Db>, extractor: Arc<dyn Extractor>) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(EMB_DIM)),
        extractor,
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: None,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "test".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
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
    })
}

/// Content-deterministic stand-in for an LLM. Every chunk mentions the entity "Shared" with a
/// summary naming the chunk, plus one entity of its own. A chunk's extraction takes longer the
/// *earlier* it is in the manifest, so with everything in flight the last chunk commits first.
struct StubExtractor {
    delay: bool,
}

fn chunk_index(body: &str) -> u64 {
    body.trim_start_matches("chunk-").parse().unwrap()
}

impl Extractor for StubExtractor {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        let body = opts.episode_body.to_string();
        let delay = self.delay;
        Box::pin(async move {
            let i = chunk_index(&body);
            if delay {
                tokio::time::sleep(Duration::from_millis(
                    STEP_MS * (CHUNKS as u64 - i) + STEP_MS,
                ))
                .await;
            }
            let ent = |name: String, summary: String| ExtractedEntity {
                name,
                entity_type: "Thing".into(),
                summary,
                original_entity_type: None,
            };
            Ok(ExtractionOutcome {
                result: ExtractionResult {
                    entities: vec![
                        ent("Shared".into(), format!("Shared as described in {body}.")),
                        ent(format!("Own {i}"), format!("Only in {body}.")),
                    ],
                    edges: vec![],
                },
                entities_dropped_malformed: 0,
                edges_dropped_malformed: 0,
            })
        })
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        _allowed: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async move { Ok(vec![String::new(); entities.len()]) })
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        _allowed: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async move { Ok(vec![String::new(); edges.len()]) })
    }

    /// Order-sensitive on purpose: the result embeds the existing summary.
    fn consolidate_summary<'a>(
        &'a self,
        _name: &'a str,
        existing: &'a str,
        incoming: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async move { Ok(format!("{existing} {incoming}")) })
    }
}

/// Replays a cassette and counts the misses per call type; a miss is still returned as the error
/// it is, so the service-side degradation paths run exactly as in production.
struct CountingReplay {
    inner: ReplayingExtractor,
    misses: Mutex<BTreeMap<&'static str, usize>>,
}

impl CountingReplay {
    fn note<T>(&self, call: &'static str, r: &Result<T, Error>) {
        if matches!(r, Err(Error::CassetteMiss(_))) {
            *self.misses.lock().unwrap().entry(call).or_default() += 1;
        }
    }
    fn count(&self, call: &str) -> usize {
        self.misses.lock().unwrap().get(call).copied().unwrap_or(0)
    }
}

impl Extractor for CountingReplay {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        Box::pin(async move {
            let r = self.inner.extract(opts).await;
            self.note("extract", &r);
            r
        })
    }
    fn classify_entities<'a>(
        &'a self,
        e: &'a [(&'a str, &'a str)],
        a: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async move {
            let r = self.inner.classify_entities(e, a).await;
            self.note("classify_entities", &r);
            r
        })
    }
    fn classify_relations<'a>(
        &'a self,
        e: &'a [(&'a str, &'a str)],
        a: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async move {
            let r = self.inner.classify_relations(e, a).await;
            self.note("classify_relations", &r);
            r
        })
    }
    fn consolidate_summary<'a>(
        &'a self,
        n: &'a str,
        e: &'a str,
        i: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async move {
            let r = self.inner.consolidate_summary(n, e, i).await;
            self.note("consolidate_summary", &r);
            r
        })
    }
    fn judge_duplicates<'a>(
        &'a self,
        p: &'a [DuplicatePair],
    ) -> BoxFuture<'a, Result<Vec<DedupVerdict>, Error>> {
        Box::pin(async move {
            let r = self.inner.judge_duplicates(p).await;
            self.note("judge_duplicates", &r);
            r
        })
    }
}

async fn add(state: Arc<AppState>, i: usize) {
    episode::add_episode(
        state,
        &format!("chunk#{i}"),
        &format!("chunk-{i}"),
        "test",
        "test source",
        REF_TIME,
        GROUP,
        SourceType::Text,
        None,
        "",
    )
    .await
    .unwrap();
}

/// Ingests every chunk, `concurrently` or one after another, recording to `cassette`. Returns how
/// many entities named "Shared" the graph ended with.
async fn record(cassette: &std::path::Path, concurrently: bool) -> usize {
    let (db, _dir) = make_db();
    let writer = Arc::new(CassetteWriter::open(cassette).unwrap());
    let rec = RecordingExtractor::new(
        Arc::new(StubExtractor {
            delay: concurrently,
        }),
        "test",
        "stub",
        writer,
    );
    let state = make_state(Arc::clone(&db), Arc::new(rec));
    if concurrently {
        futures::future::join_all((0..CHUNKS).map(|i| add(Arc::clone(&state), i))).await;
    } else {
        for i in 0..CHUNKS {
            add(Arc::clone(&state), i).await;
        }
    }
    let conn = db.connect().unwrap();
    let n = conn.count_entities_by_name_ci("Shared", GROUP).unwrap();
    drop(conn);
    n
}

/// Replays `cassette` one chunk at a time, in manifest order, and returns the miss tally.
async fn replay_sequentially(cassette: &std::path::Path) -> Arc<CountingReplay> {
    let (db, _dir) = make_db();
    let replay = Arc::new(CountingReplay {
        inner: ReplayingExtractor::load(cassette).unwrap(),
        misses: Mutex::new(BTreeMap::new()),
    });
    let state = make_state(db, replay.clone());
    for i in 0..CHUNKS {
        add(Arc::clone(&state), i).await;
    }
    replay
}

#[tokio::test]
async fn sequential_recording_replays_sequentially_with_zero_misses() {
    let dir = TempDir::new().unwrap();
    let cassette = dir.path().join("seq.jsonl");
    let shared = record(&cassette, false).await;
    assert_eq!(
        shared, 1,
        "sequential ingest resolves the shared entity to one node"
    );

    let replay = replay_sequentially(&cassette).await;
    assert!(
        replay.misses.lock().unwrap().is_empty(),
        "control: a sequential cassette replayed sequentially must be exact, got {:?}",
        replay.misses.lock().unwrap()
    );
}

#[tokio::test]
async fn concurrent_recording_replayed_sequentially_misses_only_state_keyed_calls() {
    let dir = TempDir::new().unwrap();
    let cassette = dir.path().join("conc.jsonl");
    let shared = record(&cassette, true).await;

    let replay = replay_sequentially(&cassette).await;
    let misses = replay.misses.lock().unwrap().clone();
    println!(
        "N={CHUNKS} record -> N=1 replay: \"Shared\" entities recorded live = {shared} \
         (sequential: 1); replay misses = {misses:?}"
    );

    // State-independent keys: exact regardless of order.
    assert_eq!(replay.count("extract"), 0, "{misses:?}");
    assert_eq!(replay.count("classify_entities"), 0, "{misses:?}");
    assert_eq!(replay.count("classify_relations"), 0, "{misses:?}");

    // State-keyed: every chunk in flight together commits in reverse manifest order, so the
    // existing-summary hashes recorded differ from those a manifest-order replay computes. The
    // run still completes (the service degrades a consolidation miss to its bounded fallback),
    // which is exactly why this has to be marked rather than detected at replay time.
    assert!(
        replay.count("consolidate_summary") >= 1,
        "a concurrently recorded cassette should miss a state-keyed call: {misses:?}"
    );
    // TOCTOU (ADR-0029): chunks in flight together do not see each other's uncommitted entities.
    assert!(
        shared >= 1,
        "the shared entity exists at least once (more than once is the documented race)"
    );
}
