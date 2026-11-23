//! Issue #880 / ADR-0880: corroboration that does not over-count one author restating a fact.
//!
//! A research fixture, not an implementation (FR-014): production counting is unchanged. It
//!
//! 1. ingests a dated corpus (`fixtures/restatement/corpus.json`) through the real ingest path,
//!    with each note's caller-declared `source_key` carried in the existing `attributes` JSON;
//! 2. pins **today's** `corroboration` per fact against the live service (the inflated baseline);
//! 3. applies ADR-0880's counting rule, as a pure reference function over the episode rows read
//!    back from the DB, and asserts it reproduces the documented expectations;
//! 4. asserts all of it is identical after a WAL replay (SC-004).
//!
//! The WAL/replay helpers mirror `episode_origin.rs` (ADR-0752).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    ConfigurableExtractor, Db, ExtractedEdge, ExtractedEntity, ExtractionResult, Extractor,
    WalReplayer, WalWriter,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "liminis";

fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
    }
    (db, dir)
}

fn make_state(
    db: Arc<Db>,
    wal_dir: Option<PathBuf>,
    extractor: Arc<dyn Extractor>,
) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let writers: HashMap<String, WalWriter> = wal_dir
        .as_ref()
        .and_then(|d| WalWriter::new(d, 10_000, 5 * 1024 * 1024).ok())
        .map(|w| (G.to_string(), w))
        .into_iter()
        .collect();
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor,
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(tokio::sync::RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: wal_dir,
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
    })
}

fn extraction(facts: &[(&str, &str, &str)]) -> ExtractionResult {
    let mut names: Vec<&str> = facts.iter().flat_map(|(s, t, _)| [*s, *t]).collect();
    names.sort();
    names.dedup();
    ExtractionResult {
        entities: names
            .into_iter()
            .map(|n| ExtractedEntity {
                name: n.to_string(),
                entity_type: "Thing".to_string(),
                summary: format!("{n} summary"),
                original_entity_type: None,
            })
            .collect(),
        edges: facts
            .iter()
            .map(|(s, t, f)| ExtractedEdge {
                source_name: s.to_string(),
                target_name: t.to_string(),
                fact: f.to_string(),
                relation_type: Some("RELATED_TO".to_string()),
                valid_at: None,
                invalid_at: None,
                original_relation_type: None,
            })
            .collect(),
    }
}

fn state_with(
    queue: Vec<ExtractionResult>,
    wal_dir: Option<PathBuf>,
) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let (db, dir) = make_db();
    let state = make_state(
        Arc::clone(&db),
        wal_dir,
        Arc::new(ConfigurableExtractor::new(queue)),
    );
    (state, db, dir)
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

async fn replayed(source: &Path) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("r.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        WalReplayer::new(source)
            .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
            .unwrap();
    }
    let state = make_state(
        Arc::clone(&db),
        None,
        Arc::new(ConfigurableExtractor::new(vec![])),
    );
    (state, db, dir)
}

// ── Corpus ──────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct Series {
    id: String,
    fact: String,
    count: usize,
    start: String,
    origin: String,
    source_key: Option<String>,
    doc: String,
    #[serde(default)]
    chunks_per_note: Option<usize>,
    #[serde(default)]
    fact_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct Expected {
    baseline_corroboration: usize,
    independent_corroboration: usize,
    restatements: usize,
    first_asserted: String,
}

#[derive(Debug, Deserialize)]
struct Corpus {
    series: Vec<Series>,
    removed_chunks: Vec<String>,
    expected: BTreeMap<String, Expected>,
}

/// One ingestable chunk expanded from a series.
#[derive(Debug, Clone)]
struct Note {
    chunk_id: String,
    fact: String,
    fact_text: String,
    reference_time: String,
    origin: String,
    doc_id: Option<String>,
    source_key: Option<String>,
}

fn corpus() -> Corpus {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/restatement/corpus.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn expand(c: &Corpus) -> Vec<Note> {
    let mut out = Vec::new();
    for s in &c.series {
        let start = chrono::NaiveDate::parse_from_str(&s.start, "%Y-%m-%d").unwrap();
        for n in 1..=s.count {
            let day = start + chrono::Duration::days(n as i64 - 1);
            let chunks = s.chunks_per_note.unwrap_or(1);
            for k in 1..=chunks {
                let note_id = format!("{}-{}-{n:02}", s.fact, s.id);
                let chunk_id = if chunks > 1 {
                    format!("{note_id}-c{k}")
                } else {
                    note_id.clone()
                };
                out.push(Note {
                    chunk_id,
                    fact: s.fact.clone(),
                    fact_text: s.fact_text.clone().unwrap_or_else(|| {
                        format!("{0} uses {0}-target: {1} note {n}", s.fact, s.id)
                    }),
                    reference_time: format!("{day}T09:00:00Z"),
                    origin: s.origin.clone(),
                    doc_id: (s.doc == "per_note").then(|| format!("doc-{note_id}")),
                    source_key: s.source_key.clone(),
                });
            }
        }
    }
    out
}

fn fact_of_chunk(chunk_id: &str) -> &str {
    chunk_id.split('-').next().unwrap()
}

async fn ingest(state: &Arc<AppState>, n: &Note) {
    let attributes = match &n.source_key {
        Some(k) => json!({ "source_key": k }),
        None => json!({}),
    };
    let mut params = json!({
        "chunk_text": format!("text of {}", n.chunk_id), "chunk_id": n.chunk_id,
        "source_file": format!("notes/{}.md", n.chunk_id), "group_id": G,
        "reference_time": n.reference_time, "attributes": attributes, "origin": n.origin,
    });
    if let Some(d) = &n.doc_id {
        params["doc_id"] = json!(d);
    }
    let v = try_call("knowledge_process_chunk", params, state).await;
    assert!(
        v.get("error").is_none(),
        "ingest {} errored: {v}",
        n.chunk_id
    );
}

/// Builds the corpus in a fresh DB with a WAL, then applies the removals.
async fn build(c: &Corpus) -> (Arc<AppState>, Arc<Db>, TempDir, TempDir) {
    let notes = expand(c);
    let wal = TempDir::new().unwrap();
    let queue: Vec<ExtractionResult> = notes
        .iter()
        .map(|n| {
            extraction(&[(
                n.fact.as_str(),
                format!("{}-target", n.fact).as_str(),
                n.fact_text.as_str(),
            )])
        })
        .collect();
    let (state, db, dir) = state_with(queue, Some(wal.path().to_path_buf()));
    for n in &notes {
        ingest(&state, n).await;
    }
    for chunk in &c.removed_chunks {
        let r = call(
            "knowledge_mark_chunk_removed",
            json!({"chunk_id": chunk, "group_ids": [G]}),
            &state,
        )
        .await;
        assert_eq!(r["superseded_count"], 1, "{chunk}: {r}");
    }
    (state, db, dir, wal)
}

// ── Live reads ──────────────────────────────────────────────────────────────────────────────────

async fn all_edges(state: &Arc<AppState>) -> Vec<Value> {
    let params = json!({"query": "uses", "group_ids": [G], "num_results": 500});
    call("knowledge_find_relationships", params, state).await["edges"]
        .as_array()
        .unwrap()
        .clone()
}

/// Today's `corroboration` for each fact, from the live service. Every sibling edge of one fact
/// must report the same figure, so a disagreement fails here rather than being averaged away.
async fn live_baseline(state: &Arc<AppState>) -> BTreeMap<String, usize> {
    let mut seen: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for e in all_edges(state).await {
        let fact = e["fact"].as_str().unwrap().split(' ').next().unwrap();
        seen.entry(fact.to_string())
            .or_default()
            .insert(e["corroboration"].as_u64().unwrap() as usize);
    }
    seen.into_iter()
        .map(|(f, v)| {
            assert_eq!(
                v.len(),
                1,
                "fact {f}: sibling edges disagree on corroboration: {v:?}"
            );
            (f, v.into_iter().next().unwrap())
        })
        .collect()
}

/// An episode as the reference rule sees it, read back from the DB.
#[derive(Debug, Clone)]
struct EpRec {
    uuid: String,
    name: String,
    valid_at: String,
    ingested_at: String,
    origin: String,
    doc_id: Option<String>,
    source_key: Option<String>,
    live: bool,
}

fn episode_records(db: &Db) -> Vec<EpRec> {
    let conn = db.connect().unwrap();
    conn.retrieve_episodes_page(None, 5000, None, None)
        .unwrap()
        .into_iter()
        .map(|(e, _)| {
            let attrs: Value = serde_json::from_str(&e.attributes).unwrap_or(Value::Null);
            EpRec {
                uuid: e.uuid,
                name: e.name,
                valid_at: e.valid_at,
                ingested_at: e.ingested_at,
                origin: e.origin.unwrap_or_else(|| "primary".to_string()),
                doc_id: e.doc_id,
                source_key: attrs["source_key"].as_str().map(str::to_string),
                live: e.superseded_at.is_none(),
            }
        })
        .collect()
}

// ── Reference rule (ADR-0880 Decision) ─────────────────────────────────────────────────────────
//
// Pure, no I/O: the specification of the counting the follow-up implements, applied to the
// episode records of ONE same-fact class. It lives in test code on purpose (FR-014): production
// counting is unchanged, and the expected numbers are computed mechanically instead of typed.

#[derive(Debug, PartialEq, Eq)]
struct Reference {
    /// Today's rule: distinct documents per corroborating origin, summed.
    corroboration: usize,
    /// Distinct source units across primary and external.
    independent_corroboration: usize,
    restatements: usize,
    /// `(valid_at, uuid)` of the first assertion among live corroborating episodes.
    first_asserted: Option<(String, String)>,
}

fn doc_key(e: &EpRec) -> String {
    match &e.doc_id {
        Some(d) => format!("doc:{d}"),
        None => format!("ep:{}", e.uuid),
    }
}

fn reference_rule(eps: &[EpRec]) -> Reference {
    let corroborating: Vec<&EpRec> = eps
        .iter()
        .filter(|e| e.live && (e.origin == "primary" || e.origin == "external"))
        .collect();
    let documents: BTreeSet<(String, String)> = corroborating
        .iter()
        .map(|e| (e.origin.clone(), doc_key(e)))
        .collect();
    // A keyed episode is a unit `(origin, source_key)`; a keyless one is its own unit, keyed by
    // its document key, which reproduces today's counting exactly (no backfill needed).
    let units: BTreeSet<String> = corroborating
        .iter()
        .map(|e| match &e.source_key {
            Some(k) => format!("key:{}:{k}", e.origin),
            None => format!("doc:{}:{}", e.origin, doc_key(e)),
        })
        .collect();
    let first = corroborating
        .iter()
        .min_by(|a, b| {
            (&a.valid_at, &a.ingested_at, &a.uuid).cmp(&(&b.valid_at, &b.ingested_at, &b.uuid))
        })
        .map(|e| (e.valid_at.clone(), e.uuid.clone()));
    Reference {
        corroboration: documents.len(),
        independent_corroboration: units.len(),
        restatements: documents.len() - units.len(),
        first_asserted: first,
    }
}

/// The note a chunk belongs to. Chunks of one note share `valid_at` and (to the second)
/// `ingested_at`, so which chunk wins the uuid tie-break is arbitrary per ingest, though stable
/// across WAL replay. `first_asserted` is therefore asserted at note granularity (ADR-0880).
fn note_of(chunk_id: &str) -> String {
    match chunk_id.rsplit_once("-c") {
        Some((note, k)) if k.chars().all(|c| c.is_ascii_digit()) => note.to_string(),
        _ => chunk_id.to_string(),
    }
}

fn by_fact(eps: &[EpRec]) -> BTreeMap<String, Vec<EpRec>> {
    let mut m: BTreeMap<String, Vec<EpRec>> = BTreeMap::new();
    for e in eps {
        m.entry(fact_of_chunk(&e.name).to_string())
            .or_default()
            .push(e.clone());
    }
    m
}

/// Reference output per fact, with `first_asserted` rendered as the first episode's chunk id.
fn reference_table(db: &Db) -> BTreeMap<String, Expected> {
    let eps = episode_records(db);
    let names: HashMap<String, String> = eps
        .iter()
        .map(|e| (e.uuid.clone(), e.name.clone()))
        .collect();
    by_fact(&eps)
        .into_iter()
        .map(|(fact, group)| {
            let r = reference_rule(&group);
            let first = r
                .first_asserted
                .map(|(_, uuid)| note_of(&names[&uuid]))
                .unwrap_or_default();
            (
                fact,
                Expected {
                    baseline_corroboration: r.corroboration,
                    independent_corroboration: r.independent_corroboration,
                    restatements: r.restatements,
                    first_asserted: first,
                },
            )
        })
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────────────────────────────────

/// FR-009 / FR-011 / SC-002 baseline: today's `corroboration` for the corpus, from the live
/// service, equals the documented baseline. A fact restated N times reports N.
#[tokio::test]
async fn baseline_is_inflated_by_restatement() {
    let c = corpus();
    let (state, _db, _d, _wal) = build(&c).await;
    let live = live_baseline(&state).await;
    for (fact, exp) in &c.expected {
        assert_eq!(
            live.get(fact).copied(),
            Some(exp.baseline_corroboration),
            "fact {fact}: live corroboration"
        );
    }
    assert_eq!(
        live.len(),
        c.expected.len(),
        "unexpected extra facts: {live:?}"
    );
    assert_eq!(live["A"], 12, "twelve daily notes by one author report 12");
}

/// The reference rule, applied to episode rows read back from the live DB, reproduces the
/// documented expectations, and its `corroboration` equals today's live value (so the rule only
/// ever adds a reading, never changes the existing one — FR-008).
#[tokio::test]
async fn reference_rule_matches_documented_expectations() {
    let c = corpus();
    let (state, db, _d, _wal) = build(&c).await;
    let table = reference_table(&db);
    assert_eq!(table, c.expected);
    let live = live_baseline(&state).await;
    for (fact, row) in &table {
        assert_eq!(live[fact], row.baseline_corroboration, "fact {fact}");
    }
}

/// SC-002: A's independent corroboration does not depend on N; SC-003: one independent source
/// beside the restating author yields 2, not N + 1.
#[test]
fn independent_corroboration_is_independent_of_restatement_count() {
    let mk = |n: usize, key: Option<&str>, origin: &str, tag: &str| -> Vec<EpRec> {
        (0..n)
            .map(|i| EpRec {
                uuid: format!("{tag}-{i:03}"),
                name: format!("X-{tag}-{i:03}"),
                valid_at: format!("2026-03-{:02}T09:00:00Z", (i % 28) + 1),
                ingested_at: "2026-11-22 00:00:00".into(),
                origin: origin.into(),
                doc_id: Some(format!("doc-{tag}-{i}")),
                source_key: key.map(str::to_string),
                live: true,
            })
            .collect()
    };
    for n in [1, 2, 10, 12, 50] {
        let alice = mk(n, Some("alice"), "primary", "a");
        let r = reference_rule(&alice);
        assert_eq!(r.corroboration, n);
        assert_eq!(r.independent_corroboration, 1, "N={n}");
        assert_eq!(r.restatements, n - 1);

        let mut both = alice.clone();
        both.extend(mk(1, Some("bob"), "primary", "b"));
        let r = reference_rule(&both);
        assert_eq!(r.independent_corroboration, 2, "N={n}");
        assert_eq!(r.corroboration, n + 1);
    }
}

/// SC-006 / FR-005: with no `source_key` anywhere the rule returns today's number for every fact,
/// whatever the corpus says. Stripping the keys from the whole corpus must change nothing about
/// `independent_corroboration == corroboration`.
#[tokio::test]
async fn keyless_corpus_reproduces_todays_counts() {
    let c = corpus();
    let (_state, db, _d, _wal) = build(&c).await;
    for (fact, group) in by_fact(&episode_records(&db)) {
        let stripped: Vec<EpRec> = group
            .into_iter()
            .map(|mut e| {
                e.source_key = None;
                e
            })
            .collect();
        let r = reference_rule(&stripped);
        assert_eq!(r.independent_corroboration, r.corroboration, "fact {fact}");
        assert_eq!(r.restatements, 0, "fact {fact}");
    }
}

/// FR-006: first assertion is the earliest `valid_at`; ties break on `ingested_at`, then on the
/// lowest uuid, so the choice is a pure function of replayed data.
#[test]
fn first_assertion_ties_break_deterministically() {
    let ep = |uuid: &str, valid: &str, ingested: &str| EpRec {
        uuid: uuid.into(),
        name: uuid.into(),
        valid_at: valid.into(),
        ingested_at: ingested.into(),
        origin: "primary".into(),
        doc_id: Some(format!("d-{uuid}")),
        source_key: Some("alice".into()),
        live: true,
    };
    // A note backfilled late carries an earlier valid_at and still wins over the earlier ingest.
    let r = reference_rule(&[
        ep("u1", "2026-03-02T09:00:00Z", "2026-03-02 09:00:00"),
        ep("u2", "2026-03-01T09:00:00Z", "2026-11-22 00:00:00"),
    ]);
    assert_eq!(r.first_asserted.unwrap().1, "u2");
    // Same valid_at: earlier ingested_at wins.
    let r = reference_rule(&[
        ep("u1", "2026-03-01T09:00:00Z", "2026-03-02 09:00:00"),
        ep("u2", "2026-03-01T09:00:00Z", "2026-03-01 09:00:00"),
    ]);
    assert_eq!(r.first_asserted.unwrap().1, "u2");
    // Same valid_at and ingested_at: lowest uuid wins, in either input order.
    let a = ep("u9", "2026-03-01T09:00:00Z", "2026-03-01 09:00:00");
    let b = ep("u3", "2026-03-01T09:00:00Z", "2026-03-01 09:00:00");
    assert_eq!(
        reference_rule(&[a.clone(), b.clone()])
            .first_asserted
            .unwrap()
            .1,
        "u3"
    );
    assert_eq!(reference_rule(&[b, a]).first_asserted.unwrap().1, "u3");
}

/// SC-004: after a full WAL replay, today's live counts, the reference-rule output and every
/// declared `source_key` are identical to the original ingest.
#[tokio::test]
async fn wal_replay_preserves_support_and_reference_output() {
    let c = corpus();
    let (state, db, _d, wal) = build(&c).await;
    let (state2, db2, _d2) = replayed(wal.path()).await;
    assert_eq!(live_baseline(&state2).await, live_baseline(&state).await);
    assert_eq!(reference_table(&db2), reference_table(&db));
    assert_eq!(reference_table(&db2), c.expected);
    let keys = |db: &Db| -> BTreeMap<String, Option<String>> {
        episode_records(db)
            .into_iter()
            .map(|e| (e.name, e.source_key))
            .collect()
    };
    assert_eq!(keys(&db2), keys(&db));
    assert_eq!(
        keys(&db)["A-alice-07"].as_deref(),
        Some("alice"),
        "source_key must survive in attributes"
    );
}
