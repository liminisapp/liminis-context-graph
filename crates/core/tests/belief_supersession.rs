//! Issue #705 / ADR-0705: belief time. Corrections and chunk removal *supersede* episodes instead
//! of deleting them; an edge's belief at time `t` is derived from its producing episode's
//! supersession state; `valid_at` / `invalid_at` are never written by any supersession path.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::UnconfiguredExtractor,
    handlers,
    ipc::IpcRequest,
    schema,
    telemetry::{NoopSink, TelemetrySink},
    ConfigurableExtractor, Db, EntityRow, ExtractedEdge, ExtractedEntity, ExtractionResult,
    Extractor, RelatesToEdge, WalReplayer, WalWriter,
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
        // Indexes exist before any supersession `SET`: lbug cannot `SET` an indexed column, so
        // this proves the new (unindexed) columns are settable on an FTS/HNSW-indexed node.
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

/// One extraction: `facts` are `(source, target, fact)` triples; entities are derived from them.
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

async fn chunk(state: &Arc<AppState>, group: &str, chunk_id: &str, extra: Value) -> Value {
    let mut params = json!({
        "chunk_text": format!("text of {chunk_id}"), "chunk_id": chunk_id,
        "source_file": "doc.md", "group_id": group,
        "reference_time": "2026-08-04T00:00:00Z",
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    let r = call("knowledge_process_chunk", params, state).await;
    assert_eq!(r["success"], true, "{r}");
    r
}

fn rfc3339(us: i64) -> String {
    chrono::DateTime::from_timestamp_micros(us)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// `fact → belief.state` for every edge visible under `params` (extra read params).
async fn belief_by_fact(state: &Arc<AppState>, extra: Value) -> BTreeMap<String, Vec<String>> {
    let mut params = json!({ "group_ids": [G] });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    let r = call("knowledge_get_edges_by_group", params, state).await;
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in r["edges"].as_array().unwrap() {
        out.entry(e["fact"].as_str().unwrap().to_string())
            .or_default()
            .push(
                e["belief"]["state"]
                    .as_str()
                    .unwrap_or("<unannotated>")
                    .to_string(),
            );
    }
    out
}

fn facts(m: &BTreeMap<String, Vec<String>>) -> Vec<&str> {
    m.keys().map(String::as_str).collect()
}

/// `(ingested_at_us of the F edge, superseded_at_us of the only superseded episode)`.
fn t1_t2(db: &Db, fact_text: &str) -> (i64, i64) {
    let conn = db.connect().unwrap();
    let edges = conn.get_edges_by_group_ids(None).unwrap();
    let e = edges.iter().find(|e| e.fact == fact_text).unwrap();
    let rows = conn
        .get_edge_belief_rows(std::slice::from_ref(&e.uuid))
        .unwrap();
    let row = &rows[&e.uuid];
    let eps = conn.get_episode_supersession(&row.episodes).unwrap();
    (
        row.ingested_at_us.unwrap(),
        eps[&row.episodes[0]].superseded_at_us.unwrap(),
    )
}

/// US1 AC1 / AC4, SC-001, SC-002, SC-006: a correction ends belief in a fact it dropped; history
/// stays readable; nothing is deleted; validity is untouched.
#[tokio::test]
async fn correction_ends_belief_but_keeps_history() {
    let (state, db, _d) = state_with(
        vec![
            extraction(&[("Alice", "Acme", "Alice works at Acme")]),
            extraction(&[("Alice", "Boston", "Alice lives in Boston")]),
        ],
        None,
    );
    let r1 = chunk(&state, G, "x", json!({})).await;
    assert_eq!(r1["superseded_episode_uuids"], json!([]));
    let r2 = chunk(&state, G, "x", json!({})).await;
    assert_eq!(
        r2["superseded_episode_uuids"],
        json!([r1["episode_uuid"].clone()])
    );

    // Default reads are unchanged: both edges, no `belief` key.
    let plain = belief_by_fact(&state, json!({})).await;
    assert_eq!(
        facts(&plain),
        vec!["Alice lives in Boston", "Alice works at Acme"]
    );
    assert_eq!(plain["Alice works at Acme"], vec!["<unannotated>"]);

    // Believed-only now: F is gone, the correction's fact stays.
    let now = belief_by_fact(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&now), vec!["Alice lives in Boston"]);

    // As-of: believed on [T1, T2), not at T2; the boundary is exact.
    let (t1, t2) = t1_t2(&db, "Alice works at Acme");
    assert!(
        t1 < t2,
        "correction must land strictly after the original ingest"
    );
    let before = belief_by_fact(&state, json!({"as_of": rfc3339(t1)})).await;
    assert_eq!(facts(&before), vec!["Alice works at Acme"]);
    let just_before = belief_by_fact(&state, json!({"as_of": rfc3339(t2 - 1)})).await;
    assert_eq!(facts(&just_before), vec!["Alice works at Acme"]);
    let at = belief_by_fact(&state, json!({"as_of": rfc3339(t2)})).await;
    assert_eq!(facts(&at), vec!["Alice lives in Boston"]);
    // Annotate without filtering.
    let all = belief_by_fact(
        &state,
        json!({"as_of": rfc3339(t2), "believed_only": false}),
    )
    .await;
    assert_eq!(all["Alice works at Acme"], vec!["superseded"]);
    assert_eq!(all["Alice lives in Boston"], vec!["believed"]);

    // SC-002: no Episodic deleted. SC-006 / AC4: no validity written.
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 100, None, None).unwrap();
    assert_eq!(eps.len(), 2);
    let old = eps
        .iter()
        .find(|(e, _)| e.uuid == r1["episode_uuid"].as_str().unwrap())
        .unwrap();
    assert_eq!(old.0.supersession_reason.as_deref(), Some("revision"));
    assert_eq!(old.0.superseded_by_kind.as_deref(), Some("episode"));
    assert_eq!(old.0.superseded_by.as_deref(), r2["episode_uuid"].as_str());
    assert!(old.0.superseded_at.is_some());
    let new = eps
        .iter()
        .find(|(e, _)| e.uuid == r2["episode_uuid"].as_str().unwrap())
        .unwrap();
    assert!(new.0.superseded_at.is_none());
    for e in conn.get_edges_by_group_ids(None).unwrap() {
        assert!(
            e.invalid_at.is_none(),
            "supersession must not write invalid_at"
        );
        assert_eq!(e.valid_at.as_deref(), Some("2026-08-04 00:00:00"));
    }
    // `get_episodes` exposes the supersession fields.
    let got = call("knowledge_get_episodes", json!({"group_ids": [G]}), &state).await;
    let ep = got["episodes"]
        .as_array()
        .or_else(|| got["result"].as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        ep.iter().any(|e| e["supersession_reason"] == "revision"),
        "{got}"
    );
}

/// FR-005 / FR-011: an extracted edge records its producing episode and the origin of `valid_at`.
#[tokio::test]
async fn extracted_edge_records_producing_episode_and_valid_at_source() {
    let mut with_text_date = extraction(&[("A", "B", "A met B")]);
    with_text_date.edges[0].valid_at = Some("2020-01-01T00:00:00Z".to_string());
    let (state, db, _d) = state_with(
        vec![with_text_date, extraction(&[("C", "D", "C met D")])],
        None,
    );
    let r1 = chunk(&state, G, "a", json!({})).await;
    let r2 = chunk(&state, G, "b", json!({})).await;
    let conn = db.connect().unwrap();
    let edges = conn.get_edges_by_group_ids(None).unwrap();
    let uuids: Vec<String> = edges.iter().map(|e| e.uuid.clone()).collect();
    let rows = conn.get_edge_belief_rows(&uuids).unwrap();
    for e in &edges {
        let row = &rows[&e.uuid];
        let (ep, src) = if e.fact == "A met B" {
            (r1["episode_uuid"].as_str().unwrap(), "text")
        } else {
            (r2["episode_uuid"].as_str().unwrap(), "reference_time")
        };
        assert_eq!(row.episodes, vec![ep.to_string()], "{}", e.fact);
        assert_eq!(row.valid_at_source.as_deref(), Some(src), "{}", e.fact);
        assert!(row.warrant.is_none());
    }
    // FR-012: the episode's `entity_edges` holds entity uuids, not edge uuids.
    let (ep, _) = &conn.retrieve_episodes_page(None, 10, None, None).unwrap()[0];
    for u in &ep.entity_edges {
        assert!(!uuids.contains(u), "entity_edges must not hold edge uuids");
    }
}

/// US1 AC2, FR-004: removal supersedes (reason `chunk_removed`), deletes nothing, is
/// group-scoped and idempotent; `knowledge_delete_chunk_episode` still deletes.
#[tokio::test]
async fn mark_chunk_removed_supersedes_without_deleting() {
    let (state, db, _d) = state_with(
        vec![
            extraction(&[("A", "B", "fact in g1")]),
            extraction(&[("A", "B", "fact in g2")]),
        ],
        None,
    );
    let r1 = chunk(&state, "g1", "x", json!({})).await;
    chunk(&state, "g2", "x", json!({})).await;

    let missing = try_call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "x"}),
        &state,
    )
    .await;
    assert!(missing.get("error").is_some(), "group_ids is required");

    let r = call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "x", "group_ids": ["g1"]}),
        &state,
    )
    .await;
    assert_eq!(r["superseded_count"], 1);
    assert_eq!(r["superseded_uuids"], json!([r1["episode_uuid"].clone()]));

    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 10, None, None).unwrap();
    assert_eq!(eps.len(), 2, "nothing deleted");
    for (e, _) in &eps {
        if e.group_id == "g1" {
            assert_eq!(e.supersession_reason.as_deref(), Some("chunk_removed"));
            assert!(e.superseded_by.is_none());
            assert!(e.superseded_at.is_some());
        } else {
            assert!(e.superseded_at.is_none(), "supersession crossed a group");
        }
    }
    let again = call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "x", "group_ids": ["g1"]}),
        &state,
    )
    .await;
    assert_eq!(again["superseded_count"], 0);

    // The explicit-deletion contract is unchanged.
    let del = call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "x", "group_ids": ["g2"]}),
        &state,
    )
    .await;
    assert_eq!(del["deleted_count"], 1);
    assert_eq!(
        conn.retrieve_episodes_page(None, 10, None, None)
            .unwrap()
            .len(),
        1
    );
}

/// FR-003: supersession never crosses a group, and `knowledge_add_episode` (free-form `name`)
/// never supersedes.
#[tokio::test]
async fn supersession_is_group_scoped_and_add_episode_never_supersedes() {
    let (state, db, _d) = state_with(
        vec![
            extraction(&[("A", "B", "g1 fact")]),
            extraction(&[("A", "B", "g2 fact")]),
            extraction(&[("A", "B", "g2 fact v2")]),
            extraction(&[("A", "B", "free 1")]),
            extraction(&[("A", "B", "free 2")]),
        ],
        None,
    );
    chunk(&state, "g1", "x", json!({})).await;
    chunk(&state, "g2", "x", json!({})).await;
    let r = chunk(&state, "g2", "x", json!({})).await;
    assert_eq!(r["superseded_episode_uuids"].as_array().unwrap().len(), 1);
    for _ in 0..2 {
        call(
            "knowledge_add_episode",
            json!({"name": "x", "episode_body": "free text", "group_id": "g2",
                   "reference_time": "2026-08-04T00:00:00Z"}),
            &state,
        )
        .await;
    }
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 20, None, None).unwrap();
    let live = |g: &str| {
        eps.iter()
            .filter(|(e, _)| e.group_id == g && e.superseded_at.is_none())
            .count()
    };
    assert_eq!(live("g1"), 1, "g1 untouched by a g2 correction");
    // g2: v1 superseded; v2 + two add_episode episodes live.
    assert_eq!(live("g2"), 3);
}

/// FR-003: a failed re-ingest leaves the old episode live (belief ends late, never wrongly).
#[tokio::test]
async fn failed_reingest_leaves_old_episode_live() {
    let (db, _dir) = make_db();
    let ok = make_state(
        Arc::clone(&db),
        None,
        Arc::new(ConfigurableExtractor::new(vec![extraction(&[(
            "A", "B", "orig",
        )])])),
    );
    chunk(&ok, G, "x", json!({})).await;
    let failing = make_state(Arc::clone(&db), None, Arc::new(UnconfiguredExtractor));
    let v = try_call(
        "knowledge_process_chunk",
        json!({"chunk_text": "t", "chunk_id": "x", "source_file": "d.md"}),
        &failing,
    )
    .await;
    assert!(v.get("error").is_some(), "{v}");
    let m = belief_by_fact(&ok, json!({"believed_only": true})).await;
    assert_eq!(facts(&m), vec!["orig"]);
}

/// Split units: `supersede:false` on non-final units, `same_revision_episode_uuids` on the last.
#[tokio::test]
async fn multi_unit_revision_keeps_earlier_units_live() {
    let (state, _db, _d) = state_with(
        vec![
            extraction(&[("A", "B", "old")]),
            extraction(&[("A", "B", "unit 1")]),
            extraction(&[("A", "B", "unit 2")]),
        ],
        None,
    );
    chunk(&state, G, "x", json!({})).await;
    let u1 = chunk(&state, G, "x", json!({"supersede": false})).await;
    assert_eq!(u1["superseded_episode_uuids"], json!([]));
    let u2 = chunk(
        &state,
        G,
        "x",
        json!({"same_revision_episode_uuids": [u1["episode_uuid"].clone()]}),
    )
    .await;
    assert_eq!(u2["superseded_episode_uuids"].as_array().unwrap().len(), 1);
    let m = belief_by_fact(&state, json!({"believed_only": true})).await;
    assert_eq!(facts(&m), vec!["unit 1", "unit 2"]);
    let bad = try_call(
        "knowledge_process_chunk",
        json!({"chunk_text": "t", "chunk_id": "x", "source_file": "d.md", "supersede": "no"}),
        &state,
    )
    .await;
    assert!(bad.get("error").is_some());
}

/// US1 AC3, SC-003: a heading rename re-addresses unchanged content (new chunk id ingested, old
/// reported removed). Because the new ingest precedes the removal, the fact is believed at every
/// sampled instant. SC-005: believed-only shows exactly one copy of the re-extracted fact.
#[tokio::test]
async fn heading_rename_never_unbelieves_a_surviving_fact() {
    let same = extraction(&[("A", "B", "A funds B")]);
    let (state, db, _d) = state_with(vec![same.clone(), same], None);
    chunk(&state, G, "doc#old-heading", json!({})).await;
    chunk(&state, G, "doc#new-heading", json!({})).await;
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "doc#old-heading", "group_ids": [G]}),
        &state,
    )
    .await;

    let conn = db.connect().unwrap();
    let edges = conn.get_edges_by_group_ids(None).unwrap();
    let uuids: Vec<String> = edges.iter().map(|e| e.uuid.clone()).collect();
    let rows = conn.get_edge_belief_rows(&uuids).unwrap();
    let t_first = rows
        .values()
        .filter_map(|r| r.ingested_at_us)
        .min()
        .unwrap();
    let t_second = rows
        .values()
        .filter_map(|r| r.ingested_at_us)
        .max()
        .unwrap();
    let (_, removed_at) = {
        let eps: Vec<String> = rows.values().flat_map(|r| r.episodes.clone()).collect();
        let sup = conn.get_episode_supersession(&eps).unwrap();
        (
            0,
            sup.values()
                .filter_map(|s| s.superseded_at_us)
                .max()
                .unwrap(),
        )
    };
    for t in [
        t_first,
        t_second - 1,
        t_second,
        removed_at - 1,
        removed_at,
        removed_at + 1_000_000,
    ] {
        let m = belief_by_fact(&state, json!({"as_of": rfc3339(t)})).await;
        assert!(
            m.get("A funds B")
                .is_some_and(|v| v.iter().any(|s| s == "believed")),
            "fact unbelieved at t={t}: {m:?}"
        );
    }
    // SC-005: one believed copy now; the superseded duplicate is hidden.
    let now = belief_by_fact(&state, json!({"believed_only": true})).await;
    assert_eq!(now["A funds B"].len(), 1);
}

/// Edge case: the same fact from two live sources stays believed until both are superseded.
#[tokio::test]
async fn same_fact_in_two_sources_survives_one_correction() {
    let f = extraction(&[("A", "B", "shared fact")]);
    let (state, _db, _d) = state_with(vec![f.clone(), f, extraction(&[("C", "D", "other")])], None);
    chunk(&state, G, "p", json!({})).await;
    chunk(&state, G, "q", json!({})).await;
    chunk(&state, G, "p", json!({})).await; // corrects p: drops the shared fact
    let m = belief_by_fact(&state, json!({"believed_only": true})).await;
    assert_eq!(m["shared fact"].len(), 1, "q's copy is still believed");
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "q", "group_ids": [G]}),
        &state,
    )
    .await;
    let m = belief_by_fact(&state, json!({"believed_only": true})).await;
    assert!(!m.contains_key("shared fact"), "{m:?}");
}

/// FR-006 / FR-007 / FR-010: assertions carry a warrant and never overwrite an extracted edge;
/// legacy rows are believed and reported; a hard-deleted producing episode is `support_deleted`.
#[tokio::test]
async fn warrant_legacy_and_support_deleted() {
    let (state, db, _d) = state_with(
        vec![extraction(&[("Alice", "Acme", "Alice works at Acme")])],
        None,
    );
    let r = chunk(&state, G, "x", json!({})).await;
    let extracted_name = {
        let conn = db.connect().unwrap();
        conn.get_edges_by_group_ids(None).unwrap()[0].name.clone()
    };

    // FR-010: predicate equal to the extracted edge's name would match it by name.
    let a = call(
        "knowledge_assert_relationship",
        json!({"source_name": "Alice", "target_name": "Acme", "predicate": extracted_name,
               "group_id": G, "fact": "asserted fact"}),
        &state,
    )
    .await;
    assert_eq!(a["created"], true, "{a}");

    // Legacy edge: no episodes, no warrant.
    {
        let conn = db.connect().unwrap();
        let ents = conn.get_entities_by_group_ids(None).unwrap();
        let uuid_of = |n: &str| ents.iter().find(|e| e.name == n).unwrap().uuid.clone();
        conn.insert_relates_to_edge(&RelatesToEdge {
            uuid: "legacy-edge".into(),
            name: "OLD".into(),
            source_node_uuid: uuid_of("Alice"),
            target_node_uuid: uuid_of("Acme"),
            group_id: G.into(),
            fact: "legacy fact".into(),
            fact_embedding: vec![1.0, 0.0, 0.0, 0.0],
            created_at: "2026-01-01 00:00:00".into(),
            attributes: "{}".into(),
            ..Default::default()
        })
        .unwrap();
    }

    let all = belief_by_fact(&state, json!({"believed_only": true})).await;
    assert_eq!(
        all["Alice works at Acme"],
        vec!["believed"],
        "not overwritten"
    );
    assert_eq!(all["asserted fact"], vec!["believed"]);
    assert_eq!(all["legacy fact"], vec!["legacy"]);
    let status = call("knowledge_status", json!({}), &state).await;
    assert_eq!(status["legacy_edge_count"], 1);

    {
        let conn = db.connect().unwrap();
        let edges = conn.get_edges_by_group_ids(None).unwrap();
        let asserted = edges.iter().find(|e| e.fact == "asserted fact").unwrap();
        let rows = conn
            .get_edge_belief_rows(std::slice::from_ref(&asserted.uuid))
            .unwrap();
        assert_eq!(rows[&asserted.uuid].warrant.as_deref(), Some("assertion"));
        assert!(rows[&asserted.uuid].episodes.is_empty());
    }

    // Hard-deleting the producing episode leaves the extracted fact unsupported.
    call(
        "knowledge_delete_episode",
        json!({"episode_uuid": r["episode_uuid"]}),
        &state,
    )
    .await;
    let m = belief_by_fact(&state, json!({"believed_only": true})).await;
    assert!(!m.contains_key("Alice works at Acme"), "{m:?}");
    let m = belief_by_fact(
        &state,
        json!({"believed_only": false, "as_of": "2100-01-01T00:00:00Z"}),
    )
    .await;
    assert_eq!(m["Alice works at Acme"], vec!["support_deleted"]);
}

/// A `same_as` merge re-creates an edge under a new uuid; it must keep its producing episode or
/// it would silently become a believed-forever legacy row.
#[tokio::test]
async fn same_as_copy_keeps_producing_episode() {
    let (state, db, _d) = state_with(vec![], None);
    let workspace = TempDir::new().unwrap();
    let liminis = workspace.path().join(".liminis");
    std::fs::create_dir_all(&liminis).unwrap();
    std::fs::write(
        liminis.join("knowledge-corrections.yaml"),
        "corrections:\n  - id: c-1\n    type: same_as\n    canonical: \"Canon\"\n    aliases:\n      - \"Alias\"\n",
    )
    .unwrap();
    {
        let conn = db.connect().unwrap();
        for (u, n) in [("canon", "Canon"), ("alias", "Alias"), ("other", "Other")] {
            conn.insert_entity(&EntityRow {
                uuid: u.into(),
                name: n.into(),
                group_id: "g".into(),
                labels: vec!["Entity".into()],
                created_at: "2026-01-01 00:00:00".into(),
                name_embedding: vec![1.0, 0.0, 0.0, 0.0],
                summary: "s".into(),
                attributes: "{}".into(),
                ..Default::default()
            })
            .unwrap();
        }
        conn.insert_relates_to_edge(&RelatesToEdge {
            uuid: "old-edge".into(),
            name: "KNOWS".into(),
            source_node_uuid: "alias".into(),
            target_node_uuid: "other".into(),
            group_id: "g".into(),
            fact: "alias knows other".into(),
            fact_embedding: vec![1.0, 0.0, 0.0, 0.0],
            created_at: "2026-01-01 00:00:00".into(),
            attributes: "{}".into(),
            episodes: vec!["ep-1".into()],
            valid_at_source: Some("text".into()),
            ..Default::default()
        })
        .unwrap();
    }
    let mut state = state;
    Arc::get_mut(&mut state).unwrap().workspace_root = Some(workspace.path().to_path_buf());
    let r = call("knowledge_apply_corrections", json!({}), &state).await;
    assert_eq!(r["success"], true, "{r}");

    let conn = db.connect().unwrap();
    let copy = conn
        .get_edges_by_group_ids(None)
        .unwrap()
        .into_iter()
        .find(|e| e.uuid != "old-edge" && e.source_node_uuid == "canon")
        .expect("re-pointed copy");
    let rows = conn
        .get_edge_belief_rows(std::slice::from_ref(&copy.uuid))
        .unwrap();
    assert_eq!(rows[&copy.uuid].episodes, vec!["ep-1".to_string()]);
    assert_eq!(rows[&copy.uuid].valid_at_source.as_deref(), Some("text"));
}

/// `knowledge_add_cross_group_edge` marks its edge with the `cross_group` warrant (FR-006).
#[tokio::test]
async fn cross_group_edge_has_warrant() {
    let (state, db, _d) = state_with(vec![], None);
    {
        let conn = db.connect().unwrap();
        conn.insert_entity(&EntityRow {
            uuid: "hub".into(),
            name: "hub".into(),
            group_id: "layer".into(),
            labels: vec!["Entity".into()],
            created_at: "2026-01-01 00:00:00".into(),
            name_embedding: vec![1.0, 0.0, 0.0, 0.0],
            summary: "s".into(),
            attributes: "{}".into(),
            ..Default::default()
        })
        .unwrap();
    }
    let r = call(
        "knowledge_add_cross_group_edge",
        json!({
            "name": "REFERENCES", "group_id": "layer", "source": {"uuid": "hub"},
            "target": {"source_group_id": "other", "endpoint_name": "widget"},
            "fact": "hub references widget",
        }),
        &state,
    )
    .await;
    let uuid = r["uuid"].as_str().unwrap().to_string();
    let conn = db.connect().unwrap();
    let rows = conn
        .get_edge_belief_rows(std::slice::from_ref(&uuid))
        .unwrap();
    assert_eq!(rows[&uuid].warrant.as_deref(), Some("cross_group"));
}

type Snap = (
    BTreeMap<String, (Option<i64>, String, String, String)>,
    BTreeMap<String, (Vec<String>, Option<String>, Option<String>, Option<i64>)>,
);

/// Everything belief depends on, at microsecond precision.
fn belief_snapshot(db: &Db) -> Snap {
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 1000, None, None).unwrap();
    let ep_uuids: Vec<String> = eps.iter().map(|(e, _)| e.uuid.clone()).collect();
    let sup = conn.get_episode_supersession(&ep_uuids).unwrap();
    let episodes = eps
        .iter()
        .map(|(e, _)| {
            (
                e.uuid.clone(),
                (
                    sup[&e.uuid].superseded_at_us,
                    e.superseded_by.clone().unwrap_or_default(),
                    e.superseded_by_kind.clone().unwrap_or_default(),
                    e.supersession_reason.clone().unwrap_or_default(),
                ),
            )
        })
        .collect();
    let edge_uuids: Vec<String> = conn
        .get_edges_by_group_ids(None)
        .unwrap()
        .into_iter()
        .map(|e| e.uuid)
        .collect();
    let edges = conn
        .get_edge_belief_rows(&edge_uuids)
        .unwrap()
        .into_values()
        .map(|r| {
            (
                r.uuid,
                (r.episodes, r.warrant, r.valid_at_source, r.ingested_at_us),
            )
        })
        .collect();
    (episodes, edges)
}

async fn build_history() -> (Arc<AppState>, Arc<Db>, TempDir, TempDir) {
    let wal = TempDir::new().unwrap();
    let (state, db, dir) = state_with(
        vec![
            extraction(&[("Alice", "Acme", "Alice works at Acme")]),
            extraction(&[("Alice", "Boston", "Alice lives in Boston")]),
            extraction(&[("C", "D", "gone soon")]),
        ],
        Some(wal.path().to_path_buf()),
    );
    chunk(&state, G, "x", json!({})).await;
    chunk(&state, G, "x", json!({})).await; // revision
    chunk(&state, G, "y", json!({})).await;
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "y", "group_ids": [G]}),
        &state,
    )
    .await;
    call(
        "knowledge_assert_relationship",
        json!({"source_name": "Alice", "target_name": "Boston", "predicate": "LIKES",
               "group_id": G}),
        &state,
    )
    .await;
    (state, db, dir, wal)
}

/// SC-004: WAL replay reproduces identical belief state, every supersession timestamp included.
#[tokio::test]
async fn wal_replay_reproduces_belief_state() {
    let (_state, db, _dir, wal) = build_history().await;
    let original = belief_snapshot(&db);
    assert_eq!(original.0.len(), 3);
    assert_eq!(
        original.0.values().filter(|v| v.0.is_some()).count(),
        2,
        "two superseded episodes"
    );

    let dir2 = TempDir::new().unwrap();
    let db2 = Db::open(dir2.path().join("r.db").to_str().unwrap()).unwrap();
    {
        let conn = db2.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        WalReplayer::new(wal.path())
            .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
            .unwrap();
    }
    assert_eq!(belief_snapshot(&db2), original);
}

/// SC-004 / ADR-0028: dump compaction carries the belief columns through.
#[tokio::test]
async fn dump_then_replay_reproduces_belief_state() {
    let (state, db, _dir, _wal) = build_history().await;
    let original = belief_snapshot(&db);
    let out = TempDir::new().unwrap();
    let target = out.path().join("dump");
    let r = call(
        "knowledge_dump_wal",
        json!({"target_dir": target.to_str().unwrap()}),
        &state,
    )
    .await;
    assert_eq!(r["success"], true, "{r}");

    let dir2 = TempDir::new().unwrap();
    let db2 = Db::open(dir2.path().join("r.db").to_str().unwrap()).unwrap();
    {
        let conn = db2.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        WalReplayer::new(&target)
            .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
            .unwrap();
    }
    assert_eq!(belief_snapshot(&db2), original);
}

/// The belief columns are added to a pre-#705 database by probe-then-`ALTER`, idempotently.
#[test]
fn migrate_adds_belief_columns_idempotently() {
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
    for (t, c) in [
        ("Episodic", "superseded_at"),
        ("Episodic", "superseded_by"),
        ("Episodic", "superseded_by_kind"),
        ("Episodic", "supersession_reason"),
        ("RelatesToNode_", "warrant"),
        ("RelatesToNode_", "valid_at_source"),
    ] {
        conn.run_cypher(&format!("MATCH (n:{t}) RETURN n.{c} LIMIT 0"))
            .unwrap_or_else(|e| panic!("{t}.{c} must bind after migrate: {e}"));
    }
}

/// The new method is dispatchable and `believed_only`/`as_of` validate their types.
#[tokio::test]
async fn belief_params_are_validated() {
    let (state, _db, _d) = state_with(vec![], None);
    for bad in [
        json!({"believed_only": "yes"}),
        json!({"as_of": "yesterday"}),
        json!({"as_of": 5}),
    ] {
        let v = try_call("knowledge_get_edges_by_group", bad, &state).await;
        assert!(v.get("error").is_some(), "{v}");
    }
}
