//! Issue #707 / ADR-0707: a new document can explicitly supersede an old one. Documents are
//! addressed by `doc_id`; `knowledge_supersede_document` records "A supersedes B, effective
//! T_eff" durably, B's facts stop being believed, and reads report whether a fact is *in force*
//! independently of whether it is *believed*. Supersession is never inferred and never writes
//! `valid_at` / `invalid_at`.

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
    schema,
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

/// Ingests one chunk as part of document `doc` (or none) in `group`.
async fn ingest(state: &Arc<AppState>, group: &str, chunk_id: &str, doc: Option<&str>) -> Value {
    let mut params = json!({
        "chunk_text": format!("text of {chunk_id}"), "chunk_id": chunk_id,
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// User Story 1
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// US1 AC1/AC2, SC-001, SC-002, SC-003: a retroactive supersession. B's facts leave the believed
/// set from the recording instant, but between T_eff and T_rec they are believed and not in force.
#[tokio::test]
async fn retroactive_supersession_separates_belief_from_force() {
    let (state, db, _d) = state_with(
        vec![one("b", "old decision"), one("a", "new decision")],
        None,
    );
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let before_validity = validity(&db);

    nap(5).await;
    let t_eff = ingested_us(&db, "new decision") + 2_000;
    nap(40).await; // T_rec lands well after T_eff
    let r = supersede(&state, G, "A", "B", &rfc3339(t_eff)).await;
    assert!(r.get("error").is_none(), "{r}");
    let r = &r["result"];
    assert_eq!(r["superseded_count"], 1);
    assert_eq!(r["already_recorded"], false);
    let t_rec = chrono::DateTime::parse_from_rfc3339(r["recorded_at"].as_str().unwrap())
        .unwrap()
        .timestamp_micros();
    assert!(
        t_rec > t_eff,
        "recording must land after the effective date"
    );

    // AC1 / SC-001: believed-now excludes B's fact; A's stays.
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["new decision"]);

    // AC2 / SC-002: in [T_eff, T_rec) the old fact is believed yet no longer in force.
    let mid = beliefs(&state, json!({"as_of": rfc3339(t_eff)})).await;
    assert_eq!(facts(&mid), vec!["new decision", "old decision"]);
    let old = only(&mid, "old decision");
    assert_eq!(old["state"], "believed");
    assert_eq!(old["in_force"], false);
    assert_eq!(only(&mid, "new decision")["in_force"], true);
    // Just before T_eff it is still in force.
    let pre = beliefs(&state, json!({"as_of": rfc3339(t_eff - 1)})).await;
    assert_eq!(only(&pre, "old decision")["in_force"], true);
    // At T_rec belief ends (half-open interval); force stays ended.
    let at_rec = beliefs(
        &state,
        json!({"as_of": rfc3339(t_rec), "believed_only": false}),
    )
    .await;
    let old = only(&at_rec, "old decision");
    assert_eq!(old["state"], "superseded");
    assert_eq!(old["in_force"], false);

    // The stored supersession is readable from any read: cause, superseding document, dates.
    let sup = &old["supersession"];
    assert_eq!(sup["cause"], "document_superseded");
    assert_eq!(sup["superseded_by_document"], "A");
    assert_eq!(sup["in_force_until"], rfc3339(t_eff));
    assert!(sup["recorded_at"].is_string());

    // SC-003: nothing wrote validity.
    assert_eq!(validity(&db), before_validity);
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 100, None, None).unwrap();
    let b_ep = eps
        .iter()
        .map(|(e, _)| e)
        .find(|e| e.doc_id.as_deref() == Some("B"))
        .unwrap();
    assert_eq!(b_ep.superseded_by_kind.as_deref(), Some("document"));
    assert_eq!(b_ep.superseded_by.as_deref(), Some("A"));
    assert_eq!(
        b_ep.supersession_reason.as_deref(),
        Some("document_superseded")
    );
}

/// US1 AC3, SC-005: a corrected fact and a superseded fact are told apart from the read alone.
#[tokio::test]
async fn corrected_and_superseded_facts_are_distinguishable() {
    let (state, _db, _d) = state_with(
        vec![
            one("c", "first draft"),
            one("c2", "second draft"),
            one("b", "old decision"),
            one("a", "new decision"),
        ],
        None,
    );
    ingest(&state, G, "c", Some("C")).await;
    ingest(&state, G, "c", Some("C")).await; // revision: "first draft" is corrected
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let eff = rfc3339(now_us() - 1_000);
    let r = supersede(&state, G, "A", "B", &eff).await;
    assert!(r.get("error").is_none(), "{r}");

    let all = beliefs(&state, json!({"believed_only": false})).await;
    let corrected = only(&all, "first draft");
    assert_eq!(corrected["supersession"]["cause"], "corrected");
    assert!(corrected["in_force"].is_null(), "{corrected}");
    assert!(corrected["supersession"].get("in_force_until").is_none());

    let superseded = only(&all, "old decision");
    assert_eq!(superseded["supersession"]["cause"], "document_superseded");
    assert_eq!(superseded["in_force"], false);

    // Live facts carry neither a supersession nor a false in-force claim.
    for fact in ["second draft", "new decision"] {
        let b = only(&all, fact);
        assert_eq!(b["in_force"], true, "{fact}");
        assert!(b.get("supersession").is_none(), "{fact}");
    }
}

/// US1 AC4: two documents on the same topic are unrelated until a caller says otherwise.
#[tokio::test]
async fn supersession_is_never_inferred() {
    let (state, _db, _d) = state_with(
        vec![one("t", "the budget is 10"), one("t", "the budget is 12")],
        None,
    );
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["the budget is 10", "the budget is 12"]);
    for v in now.values() {
        assert_eq!(v[0]["in_force"], true);
        assert!(v[0].get("supersession").is_none());
    }
}

/// Edge case: a future effective date keeps B believed and in force until then, and a correction
/// of B before that date still ends belief at the correction (the episode is not stuck "superseded").
#[tokio::test]
async fn future_effective_date_keeps_old_document_until_then() {
    let (state, _db, _d) = state_with(
        vec![
            one("b", "old decision"),
            one("a", "new decision"),
            one("b2", "corrected decision"),
        ],
        None,
    );
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let t_eff = now_us() + 3_600_000_000;
    let r = supersede(&state, G, "A", "B", &rfc3339(t_eff)).await;
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["superseded_count"], 1);

    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["new decision", "old decision"]);
    assert_eq!(only(&now, "old decision")["in_force"], true);
    // It already carries the stored supersession, so the end date is readable ahead of time.
    assert_eq!(
        only(&now, "old decision")["supersession"]["in_force_until"],
        rfc3339(t_eff)
    );

    // From T_eff on: not believed, not in force.
    let after = beliefs(
        &state,
        json!({"as_of": rfc3339(t_eff), "believed_only": false}),
    )
    .await;
    let old = only(&after, "old decision");
    assert_eq!(old["state"], "superseded");
    assert_eq!(old["in_force"], false);
    let before = beliefs(&state, json!({"as_of": rfc3339(t_eff - 1)})).await;
    assert_eq!(only(&before, "old decision")["state"], "believed");

    // A correction of B before T_eff ends belief at the correction, and is then "corrected".
    let r2 = ingest(&state, G, "b1", Some("B")).await;
    assert_eq!(
        r2["superseded_episode_uuids"].as_array().unwrap().len(),
        1,
        "the scheduled-superseded episode is still live for a revision: {r2}"
    );
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["corrected decision", "new decision"]);
    let all = beliefs(&state, json!({"believed_only": false})).await;
    let old = only(&all, "old decision");
    assert_eq!(old["state"], "superseded");
    assert_eq!(old["supersession"]["cause"], "corrected");
    assert!(old["in_force"].is_null());
}

/// Edge case, chains: A supersedes B, which superseded C — each link keeps its own effective date.
#[tokio::test]
async fn chain_keeps_a_date_per_link() {
    let (state, _db, _d) = state_with(
        vec![one("c", "fact c"), one("b", "fact b"), one("a", "fact a")],
        None,
    );
    ingest(&state, G, "c1", Some("C")).await;
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let e1 = now_us() + 1_000_000_000;
    let e2 = e1 + 1_000_000_000;
    assert!(supersede(&state, G, "B", "C", &rfc3339(e1)).await["result"]["success"] == true);
    assert!(supersede(&state, G, "A", "B", &rfc3339(e2)).await["result"]["success"] == true);

    let all = beliefs(&state, json!({"believed_only": false})).await;
    let c = &only(&all, "fact c")["supersession"];
    assert_eq!(c["superseded_by_document"], "B");
    assert_eq!(c["in_force_until"], rfc3339(e1));
    let b = &only(&all, "fact b")["supersession"];
    assert_eq!(b["superseded_by_document"], "A");
    assert_eq!(b["in_force_until"], rfc3339(e2));
    assert!(only(&all, "fact a").get("supersession").is_none());

    // Between the two dates C is out of force, B still in force.
    let mid = beliefs(
        &state,
        json!({"as_of": rfc3339(e1 + 1), "believed_only": false}),
    )
    .await;
    assert_eq!(only(&mid, "fact c")["in_force"], false);
    assert_eq!(only(&mid, "fact b")["in_force"], true);
}

/// Edge case: a recorded supersession is not undone by later changes to the superseding document.
#[tokio::test]
async fn later_correction_of_superseding_document_does_not_undo_it() {
    let (state, _db, _d) = state_with(
        vec![
            one("b", "old decision"),
            one("a", "new decision"),
            one("a2", "newer"),
        ],
        None,
    );
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let eff = rfc3339(now_us() - 1_000);
    assert!(supersede(&state, G, "A", "B", &eff)
        .await
        .get("error")
        .is_none());
    ingest(&state, G, "a1", Some("A")).await; // A's own chunk is corrected
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["newer"]);
    let all = beliefs(&state, json!({"believed_only": false})).await;
    assert_eq!(
        only(&all, "old decision")["supersession"]["cause"],
        "document_superseded"
    );
}

/// Edge case: a fact both documents assert stays believed through A's own episode.
#[tokio::test]
async fn fact_asserted_by_both_documents_stays_believed() {
    let (state, _db, _d) = state_with(
        vec![
            extraction(&[("x", "y", "shared fact"), ("x", "z", "only in old")]),
            extraction(&[("x", "y", "shared fact")]),
        ],
        None,
    );
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let eff = rfc3339(now_us() - 1_000);
    assert!(supersede(&state, G, "A", "B", &eff)
        .await
        .get("error")
        .is_none());
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["shared fact"]);
    assert_eq!(
        now["shared fact"].len(),
        1,
        "only A's copy remains believed"
    );
}

/// Edge cases: B ingested after the recording is untouched; another group's same `doc_id` is
/// untouched; a B with no live episodes can still be recorded.
#[tokio::test]
async fn scope_of_a_supersession() {
    let (state, db, _d) = state_with(
        vec![
            one("b", "old decision"),
            one("a", "new decision"),
            one("o", "other group fact"),
            one("oa", "other group A"),
            one("late", "late b fact"),
        ],
        None,
    );
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    ingest(&state, "other", "b1", Some("B")).await;
    ingest(&state, "other", "a1", Some("A")).await;
    let eff = rfc3339(now_us() - 1_000);
    let r = supersede(&state, G, "A", "B", &eff).await;
    assert_eq!(r["result"]["superseded_count"], 1, "{r}");

    // Same doc_id in another group: untouched.
    let other = beliefs_in(&state, "other", json!({"believed_only": true})).await;
    assert_eq!(facts(&other), vec!["other group A", "other group fact"]);
    let conn = db.connect().unwrap();
    assert!(conn
        .get_doc_supersessions(&[("other".into(), "B".into())])
        .unwrap()
        .is_empty());

    // An episode of B ingested after the recording is not marked.
    ingest(&state, G, "b2", Some("B")).await;
    let now = beliefs(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["late b fact", "new decision"]);
}

#[tokio::test]
async fn document_with_no_live_episodes_can_still_be_superseded() {
    let (state, _db, _d) = state_with(vec![one("b", "old"), one("a", "new")], None);
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "b1", "group_ids": [G]}),
        &state,
    )
    .await;
    let r = supersede(&state, G, "A", "B", &rfc3339(now_us() - 1_000)).await;
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["superseded_count"], 0);
    assert_eq!(r["result"]["already_recorded"], false);
    // The corrected episode keeps its original cause; nothing was re-superseded.
    let all = beliefs(&state, json!({"believed_only": false})).await;
    assert_eq!(only(&all, "old")["supersession"]["cause"], "corrected");
}

/// FR-001 / FR-010: `doc_id` is optional, never defaulted, round-trips, and an episode without one
/// cannot take part.
#[tokio::test]
async fn doc_id_is_optional_and_round_trips() {
    let (state, _db, _d) = state_with(vec![one("n", "no doc"), one("d", "with doc")], None);
    ingest(&state, G, "plain", None).await;
    ingest(&state, G, "docd", Some("D")).await;
    let got = call("knowledge_get_episodes", json!({"group_ids": [G]}), &state).await;
    let eps = got["episodes"].as_array().unwrap();
    let plain = eps.iter().find(|e| e["name"] == "plain").unwrap();
    assert!(plain.get("doc_id").is_none(), "{plain}");
    let docd = eps.iter().find(|e| e["name"] == "docd").unwrap();
    assert_eq!(docd["doc_id"], "D");

    // knowledge_add_episode takes it too.
    let a = call(
        "knowledge_add_episode",
        json!({"name": "e", "episode_body": "b", "group_id": G, "doc_id": "E",
               "reference_time": "2026-08-04T00:00:00Z"}),
        &state,
    )
    .await;
    assert!(a["episode_uuid"].is_string());
    let r = supersede(&state, G, "D", "plain", &rfc3339(now_us())).await;
    assert!(
        r.get("error").is_some(),
        "an episode with no doc_id cannot take part: {r}"
    );

    for bad in [json!(""), json!(5)] {
        let v = try_call(
            "knowledge_process_chunk",
            json!({"chunk_text": "t", "chunk_id": "c", "source_file": "f", "doc_id": bad}),
            &state,
        )
        .await;
        assert!(v.get("error").is_some(), "{v}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Rejections and re-recording (FR-006, SC-006)
// ─────────────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn rejected_requests_change_nothing() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(
        vec![one("b", "old"), one("a", "new"), one("z", "other")],
        Some(wal.path().to_path_buf()),
    );
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    ingest(&state, G, "z1", Some("Z")).await;
    let eff = rfc3339(now_us() - 1_000);
    assert!(supersede(&state, G, "A", "B", &eff)
        .await
        .get("error")
        .is_none());

    let wal_before = wal_bytes(wal.path());
    let snap_before = doc_snapshot(&db);

    let later = rfc3339(now_us() + 5_000_000);
    let cases: Vec<(&str, Value)> = vec![
        // same document
        ("same", supersede(&state, G, "A", "A", &eff).await),
        // unknown on either side
        ("unknown old", supersede(&state, G, "A", "nope", &eff).await),
        ("unknown new", supersede(&state, G, "nope", "Z", &eff).await),
        // wrong group
        (
            "other group",
            supersede(&state, "other", "A", "Z", &eff).await,
        ),
        // cycle: B already superseded by A, so A superseded by B closes a loop
        ("cycle", supersede(&state, G, "B", "A", &eff).await),
        // transitive cycle: Z supersedes A would be fine, then A supersedes Z loops through A->B
        // is covered separately; conflicting superseding document:
        ("conflict doc", supersede(&state, G, "Z", "B", &eff).await),
        // conflicting effective date
        (
            "conflict date",
            supersede(&state, G, "A", "B", &later).await,
        ),
        // malformed
        (
            "naive timestamp",
            supersede(&state, G, "Z", "A", "2026-01-01 00:00:00").await,
        ),
    ];
    for (label, v) in &cases {
        assert!(v.get("error").is_some(), "{label} must be rejected: {v}");
    }
    assert_eq!(
        wal_bytes(wal.path()),
        wal_before,
        "rejections must not touch the WAL"
    );
    assert_eq!(doc_snapshot(&db), snap_before);

    // A longer cycle is also caught: Z supersedes A (ok), then B supersedes Z would loop A->B->...
    assert!(supersede(&state, G, "Z", "A", &eff)
        .await
        .get("error")
        .is_none());
    let v = supersede(&state, G, "B", "Z", &eff).await;
    assert!(v.get("error").is_some(), "B -> A -> Z -> B is a cycle: {v}");
}

#[tokio::test]
async fn identical_rerecord_is_a_noop_that_keeps_first_timestamps() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(
        vec![one("b", "old"), one("a", "new")],
        Some(wal.path().to_path_buf()),
    );
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let eff = rfc3339(now_us() - 1_000);
    let first = supersede(&state, G, "A", "B", &eff).await;
    assert_eq!(first["result"]["already_recorded"], false);
    let wal_before = wal_bytes(wal.path());
    let snap_before = doc_snapshot(&db);
    nap(5).await;
    let again = supersede(&state, G, "A", "B", &eff).await;
    assert!(again.get("error").is_none(), "{again}");
    assert_eq!(again["result"]["already_recorded"], true);
    assert_eq!(again["result"]["superseded_count"], 0);
    assert_eq!(
        again["result"]["recorded_at"], first["result"]["recorded_at"],
        "the first recording instant is kept"
    );
    assert_eq!(wal_bytes(wal.path()), wal_before);
    assert_eq!(doc_snapshot(&db), snap_before);
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// User Story 2: durability
// ─────────────────────────────────────────────────────────────────────────────────────────────

async fn history() -> (Arc<AppState>, Arc<Db>, TempDir, TempDir, Vec<i64>) {
    let wal = TempDir::new().unwrap();
    let (state, db, dir) = state_with(
        vec![
            one("c", "fact c"),
            one("b", "fact b"),
            one("a", "fact a"),
            one("cc", "corrected fact"),
            one("cc2", "corrected fact v2"),
        ],
        Some(wal.path().to_path_buf()),
    );
    ingest(&state, G, "c1", Some("C")).await;
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    ingest(&state, G, "k", Some("K")).await;
    ingest(&state, G, "k", Some("K")).await;
    nap(5).await;
    let past = ingested_us(&db, "fact a") + 1_000;
    let future = now_us() + 3_600_000_000;
    assert!(supersede(&state, G, "B", "C", &rfc3339(past))
        .await
        .get("error")
        .is_none());
    nap(20).await;
    assert!(supersede(&state, G, "A", "B", &rfc3339(future))
        .await
        .get("error")
        .is_none());
    let probes = vec![past - 1, past, future - 1, future, now_us()];
    (state, db, dir, wal, probes)
}

async fn answers(state: &Arc<AppState>, probes: &[i64]) -> Vec<BTreeMap<String, Vec<Value>>> {
    let mut out = vec![beliefs(state, json!({"believed_only": false})).await];
    for p in probes {
        out.push(beliefs(state, json!({"as_of": rfc3339(*p)})).await);
    }
    out
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

/// US2 AC1, SC-004: rebuilding from the raw WAL reproduces every answer.
#[tokio::test]
async fn wal_replay_reproduces_supersession_and_answers() {
    let (state, db, _d, wal, probes) = history().await;
    let original = answers(&state, &probes[..4]).await;
    let snap = doc_snapshot(&db);
    let (state2, db2, _d2) = replayed(wal.path(), 1).await;
    assert_eq!(doc_snapshot(&db2), snap);
    assert_eq!(answers(&state2, &probes[..4]).await, original);
}

/// US2 AC2: the supersession statements are fixed-valued and guarded, so replaying them again
/// (overlapping tail recovery, ADR-0026) — even with later values — changes nothing and keeps the
/// first timestamps.
#[tokio::test]
async fn repeated_supersession_statements_keep_first_timestamps() {
    let (state, _db, _d, wal, probes) = history().await;
    let original = answers(&state, &probes[..4]).await;
    let (state2, db2, _d2) = replayed(wal.path(), 1).await;
    let snap = doc_snapshot(&db2);
    {
        let conn = db2.connect().unwrap();
        let keys = [(G.to_string(), "B".to_string())];
        let rec = conn.get_doc_supersessions(&keys).unwrap()[&keys[0]].clone();
        let later = "2099-01-01T00:00:00.000000Z";
        conn.create_doc_supersession(&rec, later, later).unwrap();
        let uuids: Vec<String> = conn
            .retrieve_episodes_page(None, 1000, None, None)
            .unwrap()
            .into_iter()
            .filter(|(e, _)| e.superseded_at.is_some())
            .map(|(e, _)| e.uuid)
            .collect();
        assert_eq!(uuids.len(), 3, "C, B and the corrected K are superseded");
        conn.supersede_episodes_for_document(&uuids, later, "X")
            .unwrap();
        conn.supersede_episodes_by_uuids(&uuids, later, None, "chunk_removed")
            .unwrap();
    }
    assert_eq!(doc_snapshot(&db2), snap);
    assert_eq!(answers(&state2, &probes[..4]).await, original);
}

/// SC-004 / ADR-0028: a compacted (dumped) WAL carries the records and `doc_id` through.
#[tokio::test]
async fn dump_then_replay_reproduces_supersession() {
    let (state, db, _d, _wal, probes) = history().await;
    let original = answers(&state, &probes[..4]).await;
    let snap = doc_snapshot(&db);
    let out = TempDir::new().unwrap();
    let target = out.path().join("dump");
    let r = call(
        "knowledge_dump_wal",
        json!({"target_dir": target.to_str().unwrap()}),
        &state,
    )
    .await;
    assert_eq!(r["success"], true, "{r}");
    let (state2, db2, _d2) = replayed(&target, 1).await;
    assert_eq!(doc_snapshot(&db2), snap);
    assert_eq!(answers(&state2, &probes[..4]).await, original);
}

/// A purged group takes its supersession records with it, so a re-ingested document is not met by
/// a stale conflict.
#[tokio::test]
async fn group_purge_removes_records() {
    let (state, db, _d) = state_with(
        vec![
            one("b", "old"),
            one("a", "new"),
            one("b", "old again"),
            one("a", "new again"),
        ],
        None,
    );
    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    let eff = rfc3339(now_us() - 1_000);
    assert!(supersede(&state, G, "A", "B", &eff)
        .await
        .get("error")
        .is_none());
    call(
        "knowledge_delete_by_group",
        json!({"group_ids": [G], "confirm": true}),
        &state,
    )
    .await;
    let conn = db.connect().unwrap();
    assert!(conn
        .get_doc_supersessions(&[(G.into(), "B".into())])
        .unwrap()
        .is_empty());

    ingest(&state, G, "b1", Some("B")).await;
    ingest(&state, G, "a1", Some("A")).await;
    // A different relation than before is now fine: nothing stale to conflict with.
    let r = supersede(&state, G, "B", "A", &eff).await;
    assert!(r.get("error").is_none(), "{r}");
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Storage-level behaviour
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `MERGE ... ON CREATE SET`: a second create never overwrites the first record's values.
#[test]
fn record_create_keeps_first_values() {
    let (db, _d) = make_db();
    let conn = db.connect().unwrap();
    let rec = |by: &str| lcg_core::belief::DocSupersessionRecord {
        group_id: G.into(),
        superseded_doc_id: "B".into(),
        superseding_doc_id: by.into(),
        ..Default::default()
    };
    conn.create_doc_supersession(
        &rec("A"),
        "2026-01-01T00:00:00.000000Z",
        "2026-02-01T00:00:00.000000Z",
    )
    .unwrap();
    conn.create_doc_supersession(
        &rec("X"),
        "2027-01-01T00:00:00.000000Z",
        "2027-02-01T00:00:00.000000Z",
    )
    .unwrap();
    let got = conn
        .get_doc_supersessions(&[(G.into(), "B".into())])
        .unwrap();
    let r = &got[&(G.to_string(), "B".to_string())];
    assert_eq!(r.superseding_doc_id, "A");
    assert_eq!(r.effective_at_us, 1_767_225_600_000_000);
    assert_eq!(r.recorded_at_us, 1_769_904_000_000_000);
}

/// The new `Episodic.doc_id` column is added to a pre-#707 database idempotently.
#[test]
fn migrate_adds_doc_id_column() {
    let dir = TempDir::new().unwrap();
    let db = Db::open(dir.path().join("old.db").to_str().unwrap()).unwrap();
    let conn = db.connect().unwrap();
    conn.run_cypher("CREATE NODE TABLE Entity (uuid STRING PRIMARY KEY, name STRING)")
        .unwrap();
    conn.run_cypher("CREATE NODE TABLE Episodic (uuid STRING PRIMARY KEY, name STRING)")
        .unwrap();
    conn.run_cypher(
        "CREATE NODE TABLE RelatesToNode_ (uuid STRING PRIMARY KEY, relation_type STRING, \
         episodes STRING[], expired_at TIMESTAMP)",
    )
    .unwrap();
    conn.run_cypher("CREATE REL TABLE MENTIONS (FROM Episodic TO Entity, group_id STRING)")
        .unwrap();
    schema::migrate(&conn, DIM);
    schema::migrate(&conn, DIM);
    conn.run_cypher("MATCH (n:Episodic) RETURN n.doc_id LIMIT 0")
        .expect("Episodic.doc_id must bind after migrate");
}

/// A database created before #707 has no `DocSupersession` table; `init_schema` adds it
/// (`CREATE ... IF NOT EXISTS`) and a second pass leaves its rows alone.
#[test]
fn init_schema_adds_record_table_to_a_pre_707_database() {
    let (db, _d) = make_db();
    let conn = db.connect().unwrap();
    conn.run_cypher("DROP TABLE DocSupersession").unwrap();
    conn.run_cypher("MATCH (d:DocSupersession) RETURN d.key LIMIT 0")
        .expect_err("the table is gone, as in a pre-#707 database");

    conn.init_schema(DIM).unwrap();
    conn.run_cypher("MATCH (d:DocSupersession) RETURN d.key LIMIT 0")
        .expect("init_schema must create DocSupersession");

    let rec = lcg_core::belief::DocSupersessionRecord {
        group_id: G.into(),
        superseded_doc_id: "B".into(),
        superseding_doc_id: "A".into(),
        ..Default::default()
    };
    conn.create_doc_supersession(
        &rec,
        "2026-01-01T00:00:00.000000Z",
        "2026-02-01T00:00:00.000000Z",
    )
    .unwrap();
    conn.init_schema(DIM).unwrap();
    assert_eq!(
        conn.get_doc_supersessions(&[(G.into(), "B".into())])
            .unwrap()
            .len(),
        1,
        "a repeated init_schema must not drop the records"
    );
}
