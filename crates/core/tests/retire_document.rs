//! Issue #859 / ADR-0859: `knowledge_retire_document` supersedes every live episode of a document
//! with reason `document_removed` — nothing is deleted, the episodes stay readable as history,
//! the call is idempotent and WAL-replayable.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// One single-fact extraction per `(doc-ish label, fact)`: distinct entities per fact so edges
/// never collide.
fn one(label: &str, fact: &str) -> ExtractionResult {
    extraction(&[(&format!("{label}-src"), &format!("{label}-dst"), fact)])
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

/// Issue #858: an identical resubmission is a no-op, so each submission gets distinct text; a
/// repeat of a `chunk_id` here is then a genuine correction, as these tests intend.
fn next_submission() -> usize {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// Ingests one chunk as part of document `doc` (or none) in `group`.
async fn ingest(state: &Arc<AppState>, group: &str, chunk_id: &str, doc: Option<&str>) -> Value {
    let mut params = json!({
        "chunk_text": format!("text of {chunk_id} (submission {})", next_submission()), "chunk_id": chunk_id,
        "source_file": "doc.md", "group_id": group,
        "reference_time": "2026-08-04T00:00:00Z",
    });
    if let Some(d) = doc {
        params["doc_id"] = json!(d);
    }
    let r = call("knowledge_process_chunk", params, state).await;
    assert_eq!(r["success"], true, "{r}");
    r
}

async fn supersede(
    state: &Arc<AppState>,
    group: &str,
    new_doc: &str,
    old_doc: &str,
    effective_at: &str,
) -> Value {
    try_call(
        "knowledge_supersede_document",
        json!({
            "group_id": group, "superseding_doc_id": new_doc,
            "superseded_doc_id": old_doc, "effective_at": effective_at,
        }),
        state,
    )
    .await
}

fn rfc3339(us: i64) -> String {
    chrono::DateTime::from_timestamp_micros(us)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn now_us() -> i64 {
    chrono::Utc::now().timestamp_micros()
}

async fn nap(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// `fact → [belief object]` for every edge visible in `group` under the extra read params.
async fn beliefs_in(
    state: &Arc<AppState>,
    group: &str,
    extra: Value,
) -> BTreeMap<String, Vec<Value>> {
    let mut params = json!({ "group_ids": [group] });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    let r = call("knowledge_get_edges_by_group", params, state).await;
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for e in r["edges"].as_array().unwrap() {
        out.entry(e["fact"].as_str().unwrap().to_string())
            .or_default()
            .push(e["belief"].clone());
    }
    out
}

async fn beliefs(state: &Arc<AppState>, extra: Value) -> BTreeMap<String, Vec<Value>> {
    beliefs_in(state, G, extra).await
}

fn only(m: &BTreeMap<String, Vec<Value>>, fact: &str) -> Value {
    let v = m
        .get(fact)
        .unwrap_or_else(|| panic!("fact {fact:?} not visible in {m:?}"));
    assert_eq!(v.len(), 1, "{fact}: {v:?}");
    v[0].clone()
}

fn facts(m: &BTreeMap<String, Vec<Value>>) -> Vec<&str> {
    m.keys().map(String::as_str).collect()
}

/// Ingest time (µs) of the edge carrying `fact`.
fn ingested_us(db: &Db, fact: &str) -> i64 {
    let conn = db.connect().unwrap();
    let edges = conn.get_edges_by_group_ids(None).unwrap();
    let e = edges.iter().find(|e| e.fact == fact).unwrap();
    let rows = conn
        .get_edge_belief_rows(std::slice::from_ref(&e.uuid))
        .unwrap();
    rows[&e.uuid].ingested_at_us.unwrap()
}

/// `(fact, valid_at, invalid_at, expired_at)` of every edge: what "validity untouched" compares.
fn validity(db: &Db) -> Vec<(String, String, String)> {
    let conn = db.connect().unwrap();
    let mut v: Vec<_> = conn
        .get_edges_by_group_ids(None)
        .unwrap()
        .into_iter()
        .map(|e| {
            (
                e.fact,
                format!("{:?}", e.valid_at),
                format!("{:?}", e.invalid_at),
            )
        })
        .collect();
    v.sort();
    v
}

/// Every byte under a WAL directory, in path order.
fn wal_bytes(dir: &Path) -> Vec<u8> {
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        if p.is_dir() {
            for e in std::fs::read_dir(p).unwrap() {
                walk(&e.unwrap().path(), out);
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    files
        .iter()
        .flat_map(|f| std::fs::read(f).unwrap())
        .collect()
}

/// Episodes' supersession columns, the records and edge rows — everything document supersession
/// depends on, at microsecond precision.
fn doc_snapshot(db: &Db) -> String {
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 1000, None, None).unwrap();
    let uuids: Vec<String> = eps.iter().map(|(e, _)| e.uuid.clone()).collect();
    let sup = conn.get_episode_supersession(&uuids).unwrap();
    let mut rows: Vec<String> = eps
        .iter()
        .map(|(e, _)| {
            format!(
                "{} {:?} {:?} {:?} {:?} {:?}",
                e.uuid,
                sup[&e.uuid].superseded_at_us,
                e.superseded_by,
                e.superseded_by_kind,
                e.supersession_reason,
                e.doc_id
            )
        })
        .collect();
    rows.sort();
    let mut keys = Vec::new();
    for (e, _) in &eps {
        if let Some(d) = &e.doc_id {
            keys.push((e.group_id.clone(), d.clone()));
        }
    }
    keys.sort();
    keys.dedup();
    let recs = conn.get_doc_supersessions(&keys).unwrap();
    let mut rec_rows: Vec<String> = recs.values().map(|r| format!("{r:?}")).collect();
    rec_rows.sort();
    format!("{}\n--\n{}", rows.join("\n"), rec_rows.join("\n"))
}

async fn replayed(source: &Path, times: usize) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("r.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        for _ in 0..times {
            WalReplayer::new(source)
                .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
                .unwrap();
        }
    }
    let state = make_state(
        Arc::clone(&db),
        None,
        Arc::new(ConfigurableExtractor::new(vec![])),
    );
    (state, db, dir)
}

/// Every episode of `group` from `knowledge_get_episodes`: `(chunk_id-ish name, reason)`.
async fn episode_reasons(state: &Arc<AppState>, group: &str) -> Vec<(String, Option<String>)> {
    let r = call(
        "knowledge_get_episodes",
        json!({"group_ids": [group], "last_n": 100}),
        state,
    )
    .await;
    let arr = r["episodes"]
        .as_array()
        .or_else(|| r.as_array())
        .cloned()
        .unwrap_or_else(|| panic!("no episodes in {r}"));
    let mut out: Vec<(String, Option<String>)> = arr
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap_or_default().to_string(),
                e["supersession_reason"].as_str().map(str::to_string),
            )
        })
        .collect();
    out.sort();
    out
}

async fn retire(state: &Arc<AppState>, group: &str, doc: &str, extra: Value) -> Value {
    let mut params = json!({"group_id": group, "doc_id": doc});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    try_call("knowledge_retire_document", params, state).await
}

async fn two_chunk_history() -> (Arc<AppState>, Arc<Db>, TempDir, TempDir) {
    let wal = TempDir::new().unwrap();
    let (state, db, dir) = state_with(
        vec![
            one("d1", "fact one"),
            one("d2", "fact two"),
            one("e1", "other doc fact"),
        ],
        Some(wal.path().to_path_buf()),
    );
    ingest(&state, G, "d1", Some("D")).await;
    ingest(&state, G, "d2", Some("D")).await;
    ingest(&state, G, "e1", Some("E")).await;
    (state, db, dir, wal)
}

/// US1 AC1/AC2, SC-001, SC-002: retire a two-chunk document; beliefs drop its facts, episodes stay
/// listed as superseded with reason `document_removed`, a WAL rebuild reproduces it.
#[tokio::test]
async fn retire_two_chunk_document_and_rebuild() {
    let (state, db, _d, wal) = two_chunk_history().await;
    let before_validity = validity(&db);
    let r = retire(&state, G, "D", json!({})).await;
    assert!(r.get("error").is_none(), "{r}");
    let r = &r["result"];
    assert_eq!(r["success"], true);
    assert_eq!(r["doc_id"], "D");
    assert_eq!(r["group_id"], G);
    assert_eq!(r["superseded_count"], 2);
    assert_eq!(r["superseded_uuids"].as_array().unwrap().len(), 2);
    assert!(r["effective_at"].is_null());

    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["other doc fact"]);
    let all = beliefs(&state, json!({"believed_only": false})).await;
    assert_eq!(all.len(), 3, "history stays readable: {all:?}");
    let gone = only(&all, "fact one");
    assert_eq!(gone["state"], "superseded");
    assert_eq!(gone["supersession"]["cause"], "document_removed");
    assert_eq!(gone["in_force"], false);

    let reasons = episode_reasons(&state, G).await;
    assert_eq!(
        reasons
            .iter()
            .filter(|(_, r)| r.as_deref() == Some("document_removed"))
            .count(),
        2,
        "{reasons:?}"
    );
    assert_eq!(reasons.len(), 3, "nothing deleted: {reasons:?}");
    assert_eq!(validity(&db), before_validity, "validity untouched");

    // SC-002: rebuild from the WAL gives the same state and reads.
    let snap = doc_snapshot(&db);
    let (state2, db2, _d2) = replayed(wal.path(), 1).await;
    assert_eq!(doc_snapshot(&db2), snap);
    assert_eq!(beliefs(&state2, json!({"believed_only": false})).await, all);
    assert_eq!(episode_reasons(&state2, G).await, reasons);
    // Replaying twice (overlapping tail) is idempotent too.
    let (_s3, db3, _d3) = replayed(wal.path(), 2).await;
    assert_eq!(doc_snapshot(&db3), snap);
}

/// US1 AC3, SC-003: retiring again is a no-op reporting zero and adding no durable record.
#[tokio::test]
async fn retire_twice_is_a_noop() {
    let (state, db, _d, wal) = two_chunk_history().await;
    assert_eq!(
        retire(&state, G, "D", json!({})).await["result"]["superseded_count"],
        2
    );
    let wal_before = wal_bytes(wal.path());
    let snap = doc_snapshot(&db);
    let r = retire(&state, G, "D", json!({})).await;
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["superseded_count"], 0);
    assert_eq!(r["result"]["superseded_uuids"], json!([]));
    assert_eq!(wal_bytes(wal.path()), wal_before, "no durable record");
    assert_eq!(doc_snapshot(&db), snap);
}

/// US1 AC4: a fact also asserted by a live document stays believed.
#[tokio::test]
async fn fact_shared_with_live_document_stays_believed() {
    let (state, _db, _d) = state_with(
        vec![
            extraction(&[("x", "y", "shared fact"), ("x", "z", "only in retired")]),
            extraction(&[("x", "y", "shared fact")]),
        ],
        None,
    );
    ingest(&state, G, "d1", Some("D")).await;
    ingest(&state, G, "e1", Some("E")).await;
    assert_eq!(
        retire(&state, G, "D", json!({})).await["result"]["superseded_count"],
        1
    );
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["shared fact"]);
}

/// Rejections: unknown doc, bad / naive `effective_at`, missing group or doc — no WAL write.
#[tokio::test]
async fn rejected_requests_write_nothing() {
    let (state, db, _d, wal) = two_chunk_history().await;
    let wal_before = wal_bytes(wal.path());
    let snap = doc_snapshot(&db);
    let cases = vec![
        ("unknown doc", retire(&state, G, "nope", json!({})).await),
        ("other group", retire(&state, "other", "D", json!({})).await),
        (
            "naive timestamp",
            retire(
                &state,
                G,
                "D",
                json!({"effective_at": "2026-01-01 00:00:00"}),
            )
            .await,
        ),
        (
            "no offset",
            retire(
                &state,
                G,
                "D",
                json!({"effective_at": "2026-01-01T00:00:00"}),
            )
            .await,
        ),
        (
            "non-string effective_at",
            retire(&state, G, "D", json!({"effective_at": 5})).await,
        ),
        (
            "missing group",
            try_call("knowledge_retire_document", json!({"doc_id": "D"}), &state).await,
        ),
        (
            "missing doc",
            try_call("knowledge_retire_document", json!({"group_id": G}), &state).await,
        ),
    ];
    for (label, v) in &cases {
        assert!(v.get("error").is_some(), "{label} must be rejected: {v}");
    }
    assert_eq!(wal_bytes(wal.path()), wal_before);
    assert_eq!(doc_snapshot(&db), snap);
}

/// Edge case: an already `document_superseded` document is a no-op that keeps its reason.
#[tokio::test]
async fn superseded_document_is_not_overwritten() {
    let (state, db, _d) = state_with(vec![one("b", "old"), one("a", "new")], None);
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let eff = rfc3339(now_us() - 1_000);
    assert!(supersede(&state, G, "A", "B", &eff)
        .await
        .get("error")
        .is_none());
    let snap = doc_snapshot(&db);
    let r = retire(&state, G, "B", json!({})).await;
    assert_eq!(r["result"]["superseded_count"], 0, "{r}");
    assert_eq!(doc_snapshot(&db), snap);
    let reasons = episode_reasons(&state, G).await;
    assert!(reasons
        .iter()
        .any(|(_, r)| r.as_deref() == Some("document_superseded")));
    assert!(!reasons
        .iter()
        .any(|(_, r)| r.as_deref() == Some("document_removed")));
}

/// Edge case: a chunk removed earlier keeps its reason; only the remaining live episode retires.
#[tokio::test]
async fn earlier_chunk_removal_is_untouched() {
    let (state, _db, _d, _wal) = two_chunk_history().await;
    let r = call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "d1", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(r["superseded_count"], 1);
    let r = retire(&state, G, "D", json!({})).await;
    assert_eq!(r["result"]["superseded_count"], 1, "{r}");
    let reasons = episode_reasons(&state, G).await;
    let by = |reason: &str| {
        reasons
            .iter()
            .filter(|(_, r)| r.as_deref() == Some(reason))
            .count()
    };
    assert_eq!(by("chunk_removed"), 1, "{reasons:?}");
    assert_eq!(by("document_removed"), 1, "{reasons:?}");
}

/// FR-007: a future `effective_at` keeps the fact believed and in force until then.
#[tokio::test]
async fn future_effective_date_keeps_fact_until_then() {
    let (state, _db, _d, _wal) = two_chunk_history().await;
    let t_eff = now_us() + 3_600_000_000;
    let r = retire(&state, G, "D", json!({"effective_at": rfc3339(t_eff)})).await;
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["superseded_count"], 2);
    assert_eq!(r["result"]["superseded_at"], rfc3339(t_eff));
    assert_eq!(r["result"]["effective_at"], rfc3339(t_eff));

    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["fact one", "fact two", "other doc fact"]);
    let one_now = only(&now, "fact one");
    assert_eq!(one_now["in_force"], true);
    assert_eq!(one_now["supersession"]["in_force_until"], rfc3339(t_eff));

    let after = beliefs(
        &state,
        json!({"as_of": rfc3339(t_eff), "believed_only": false}),
    )
    .await;
    let gone = only(&after, "fact one");
    assert_eq!(gone["state"], "superseded");
    assert_eq!(gone["in_force"], false);
    // Retiring again for the same or a later instant is a no-op that says why: the scheduled
    // instant is reported, and nothing is moved later.
    for extra in [
        json!({"effective_at": rfc3339(t_eff)}),
        json!({"effective_at": rfc3339(t_eff + 3_600_000_000)}),
    ] {
        let again = retire(&state, G, "D", extra).await;
        assert_eq!(again["result"]["superseded_count"], 0, "{again}");
        assert_eq!(
            again["result"]["existing_retirement_at"],
            rfc3339(t_eff),
            "{again}"
        );
    }
}

/// A pending retirement can be moved earlier, never later: a retire with no `effective_at` ends
/// belief now.
#[tokio::test]
async fn pending_retirement_can_be_moved_earlier() {
    let (state, _db, _d, wal) = two_chunk_history().await;
    let t_eff = now_us() + 3_600_000_000;
    retire(&state, G, "D", json!({"effective_at": rfc3339(t_eff)})).await;
    let r = retire(&state, G, "D", json!({})).await;
    assert_eq!(r["result"]["superseded_count"], 2, "{r}");
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["other doc fact"]);
    // The earlier instant replays: the later statement is guarded out when applied after it.
    let (state2, _db2, _d2) = replayed(wal.path(), 2).await;
    assert_eq!(
        facts(&beliefs(&state2, json!({"believed_only": true})).await),
        vec!["other doc fact"]
    );
}

/// A pending future-dated retirement must not block a correction: re-ingesting a chunk of the
/// document with new content supersedes the old episode instead of leaving both believed, and the
/// document baseline still sees the pending episodes as live.
#[tokio::test]
async fn pending_retirement_does_not_block_a_correction() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(
        vec![
            one("d1", "fact one"),
            one("d2", "fact two"),
            one("d1b", "fact one corrected"),
        ],
        Some(wal.path().to_path_buf()),
    );
    ingest(&state, G, "d1", Some("D")).await;
    ingest(&state, G, "d2", Some("D")).await;
    let t_eff = now_us() + 30 * 86_400_000_000;
    retire(&state, G, "D", json!({"effective_at": rfc3339(t_eff)})).await;
    let baseline = db
        .connect()
        .unwrap()
        .document_baseline_episodes(G, "D")
        .unwrap();
    assert_eq!(baseline.len(), 2, "pending episodes are still live");

    ingest(&state, G, "d1", Some("D")).await;
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(
        facts(&now),
        vec!["fact one corrected", "fact two"],
        "the old fact must not stay believed beside its correction: {now:?}"
    );
    let (state2, _db2, _d2) = replayed(wal.path(), 1).await;
    assert_eq!(
        facts(&beliefs(&state2, json!({"believed_only": true})).await),
        vec!["fact one corrected", "fact two"]
    );
}

/// Once a retirement has taken effect the baseline is empty again (re-ingest is all "added").
#[tokio::test]
async fn effective_retirement_leaves_an_empty_baseline() {
    let (state, db, _d, _wal) = two_chunk_history().await;
    retire(&state, G, "D", json!({})).await;
    let baseline = db
        .connect()
        .unwrap()
        .document_baseline_episodes(G, "D")
        .unwrap();
    assert!(baseline.is_empty());
}

/// ADR-0858: while a retire waits for the write lock, every chunk of the document is registered as
/// in flight, so a concurrent identical `process_chunk` cannot take the `unchanged` no-op and then
/// have its episode retired underneath it. The registration is gone once the retire finishes.
#[tokio::test]
async fn retire_registers_the_documents_chunks_as_in_flight() {
    let (state, _db, _d, _wal) = two_chunk_history().await;
    let held = state.write_lock.write().await;
    let st = Arc::clone(&state);
    let task = tokio::spawn(async move { retire(&st, G, "D", json!({})).await });
    let mut seen = false;
    for _ in 0..200 {
        nap(10).await;
        let keys: Vec<(String, String)> = state
            .chunk_inflight
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        if keys.contains(&(G.to_string(), "d1".to_string()))
            && keys.contains(&(G.to_string(), "d2".to_string()))
        {
            assert!(
                !keys.contains(&(G.to_string(), "e1".to_string())),
                "another document's chunk must not be registered: {keys:?}"
            );
            seen = true;
            break;
        }
    }
    assert!(seen, "retire never registered d1 and d2 while queued");
    drop(held);
    let r = task.await.unwrap();
    assert_eq!(r["result"]["superseded_count"], 2, "{r}");
    assert!(state.chunk_inflight.lock().unwrap().is_empty());
}

/// A past `effective_at` behaves as "removed now": belief ends at the recording instant.
#[tokio::test]
async fn past_effective_date_ends_belief_at_recording() {
    let (state, _db, _d, _wal) = two_chunk_history().await;
    let r = retire(
        &state,
        G,
        "D",
        json!({"effective_at": rfc3339(now_us() - 3_600_000_000)}),
    )
    .await;
    assert_eq!(r["result"]["superseded_count"], 2, "{r}");
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["other doc fact"]);
}

/// Re-ingesting under the same `doc_id` after retirement creates new live episodes.
#[tokio::test]
async fn reingest_after_retirement_is_live() {
    let (state, _db, _d) = state_with(vec![one("d1", "fact one"), one("d1b", "fact back")], None);
    ingest(&state, G, "d1", Some("D")).await;
    assert_eq!(
        retire(&state, G, "D", json!({})).await["result"]["superseded_count"],
        1
    );
    ingest(&state, G, "d1", Some("D")).await;
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["fact back"]);
}

/// ADR-0752: a derived actor may not retire a primary document; nothing is written.
#[tokio::test]
async fn derived_origin_cannot_retire_primary_document() {
    let (state, db, _d, wal) = two_chunk_history().await;
    let wal_before = wal_bytes(wal.path());
    let snap = doc_snapshot(&db);
    let v = retire(&state, G, "D", json!({"origin": "derived"})).await;
    assert!(v.get("error").is_some(), "{v}");
    assert!(v.to_string().contains("origin_conflict"), "{v}");
    assert_eq!(wal_bytes(wal.path()), wal_before);
    assert_eq!(doc_snapshot(&db), snap);
}
