//! Issue #752 / ADR-0752: episode origin. A closed-vocabulary, caller-declared label
//! (`primary` / `external` / `derived` / `answer`) so an agent's own pages never corroborate
//! themselves: derived episodes are cited but not counted, never override primary content, and
//! reads can be restricted by origin. Origin survives WAL rebuild and can be stamped onto an
//! existing graph without re-extraction.

use std::collections::{BTreeMap, HashMap};
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

fn summarised(facts: &[(&str, &str, &str)], summary: &str) -> ExtractionResult {
    let mut r = extraction(facts);
    for e in &mut r.entities {
        e.summary = format!("{} {summary}", e.name);
    }
    r
}

/// Ingests one chunk with an optional origin / doc_id / source file.
async fn put(
    state: &Arc<AppState>,
    chunk_id: &str,
    origin: Option<&str>,
    doc: Option<&str>,
) -> Value {
    let v = put_raw(
        state,
        chunk_id,
        origin,
        doc,
        "notes/source-note.md",
        json!({}),
    )
    .await;
    assert!(v.get("error").is_none(), "ingest errored: {v}");
    v["result"].clone()
}

/// Issue #858: an identical resubmission is a no-op, so each submission gets distinct text; a
/// repeat of a `chunk_id` here is then a genuine correction, as these tests intend.
fn next_submission() -> usize {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

async fn put_raw(
    state: &Arc<AppState>,
    chunk_id: &str,
    origin: Option<&str>,
    doc: Option<&str>,
    source_file: &str,
    attributes: Value,
) -> Value {
    let mut params = json!({
        "chunk_text": format!("text of {chunk_id} (submission {})", next_submission()), "chunk_id": chunk_id,
        "source_file": source_file, "group_id": G,
        "reference_time": "2026-08-04T00:00:00Z", "attributes": attributes,
    });
    if let Some(o) = origin {
        params["origin"] = json!(o);
    }
    if let Some(d) = doc {
        params["doc_id"] = json!(d);
    }
    try_call("knowledge_process_chunk", params, state).await
}

fn is_conflict(v: &Value) -> bool {
    v["error"]["message"]
        .as_str()
        .is_some_and(|m| m.contains("origin_conflict:"))
}

async fn relationships(state: &Arc<AppState>, extra: Value) -> Vec<Value> {
    let mut params = json!({"query": "uses", "group_ids": [G], "num_results": 20});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    call("knowledge_find_relationships", params, state).await["edges"]
        .as_array()
        .unwrap()
        .clone()
}

async fn entities(state: &Arc<AppState>, query: &str, extra: Value) -> Vec<Value> {
    let mut params = json!({"query": query, "group_ids": [G], "num_results": 20});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    call("knowledge_find_entities", params, state).await["nodes"]
        .as_array()
        .unwrap()
        .clone()
}

fn episode_origins(db: &Db) -> BTreeMap<String, String> {
    let conn = db.connect().unwrap();
    conn.retrieve_episodes_page(None, 1000, None, None)
        .unwrap()
        .into_iter()
        .map(|(e, _)| (e.name, e.origin.unwrap_or_default()))
        .collect()
}

fn episode_count(db: &Db) -> usize {
    let conn = db.connect().unwrap();
    conn.retrieve_episodes_page(None, 1000, None, None)
        .unwrap()
        .len()
}

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

fn fact(src: &str, dst: &str, f: &str) -> ExtractionResult {
    extraction(&[(src, dst, f)])
}

fn entity_summary(db: &Db, name: &str) -> String {
    let conn = db.connect().unwrap();
    conn.get_entity_by_name_ci(name, G, "Entity")
        .unwrap()
        .unwrap_or_else(|| panic!("no entity {name}"))
        .summary
}

// ── FR-001 / FR-002: vocabulary, default ────────────────────────────────────────────────────

#[tokio::test]
async fn missing_origin_is_primary_and_valid_values_are_recorded() {
    let (state, db, _d) = state_with(vec![fact("A", "B", "f1"), fact("C", "D", "f2")], None);
    put(&state, "plain", None, None).await;
    put(&state, "ext", Some("external"), None).await;
    let o = episode_origins(&db);
    assert_eq!(o["plain"], "primary");
    assert_eq!(o["ext"], "external");
}

#[tokio::test]
async fn invalid_origin_is_rejected_and_writes_nothing() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(vec![fact("A", "B", "f")], Some(wal.path().to_path_buf()));
    let before = wal_bytes(wal.path());
    let v = put_raw(&state, "c", Some("wiki"), None, "a.md", json!({})).await;
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("primary, external, derived, answer"), "{msg}");
    assert_eq!(episode_count(&db), 0);
    assert_eq!(wal_bytes(wal.path()), before);
    // knowledge_add_episode validates the same way.
    let v = try_call(
        "knowledge_add_episode",
        json!({"name": "n", "episode_body": "b", "group_id": G, "origin": 7}),
        &state,
    )
    .await;
    assert!(v.get("error").is_some(), "{v}");
    assert_eq!(episode_count(&db), 0);
}

#[tokio::test]
async fn add_episode_accepts_origin() {
    let (state, db, _d) = state_with(vec![fact("A", "B", "f")], None);
    call(
        "knowledge_add_episode",
        json!({"name": "n", "episode_body": "body", "group_id": G, "origin": "answer",
               "reference_time": "2026-08-04T00:00:00Z"}),
        &state,
    )
    .await;
    assert_eq!(episode_origins(&db)["n"], "answer");
}

// ── US1: derived cites but never corroborates ────────────────────────────────────────────────

#[tokio::test]
async fn derived_restatement_adds_citation_not_corroboration() {
    let (state, _db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres (summary page)"),
        ],
        None,
    );
    put(&state, "note-a", None, None).await;
    put(&state, "wiki-1", Some("derived"), None).await;
    let edges = relationships(&state, json!({})).await;
    assert_eq!(edges.len(), 2, "two edges, one per episode: {edges:?}");
    for e in &edges {
        assert_eq!(e["support"]["primary"], 1, "{e}");
        assert_eq!(e["support"]["derived"], 1, "{e}");
        assert_eq!(e["support"]["external"], 0);
        assert_eq!(e["support"]["answer"], 0);
        assert_eq!(e["corroboration"], 1, "corroboration is 1, not 2");
    }
}

#[tokio::test]
async fn external_and_answer_are_counted_under_their_own_keys() {
    let (state, _db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres (clip)"),
            fact("X", "Postgres", "X uses Postgres (qa)"),
        ],
        None,
    );
    put(&state, "n", None, None).await;
    put(&state, "clip", Some("external"), None).await;
    put(&state, "qa", Some("answer"), None).await;
    let e = &relationships(&state, json!({})).await[0];
    assert_eq!(
        (
            e["support"]["primary"].as_u64(),
            e["support"]["external"].as_u64(),
            e["support"]["answer"].as_u64()
        ),
        (Some(1), Some(1), Some(1))
    );
    assert_eq!(e["corroboration"], 2, "primary + external corroborate");
}

/// SC-008: one long note split into many chunks counts once when a doc_id is supplied.
#[tokio::test]
async fn chunks_of_one_document_count_once() {
    let (state, _db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres, again"),
            fact("X", "Postgres", "X uses Postgres, thrice"),
        ],
        None,
    );
    put(&state, "c1", None, Some("note-a")).await;
    put(&state, "c2", None, Some("note-a")).await;
    put(&state, "c3", None, Some("note-a")).await;
    for e in relationships(&state, json!({})).await {
        assert_eq!(e["support"]["primary"], 1, "{e}");
    }
}

#[tokio::test]
async fn episodes_without_doc_id_each_count_as_a_document() {
    let (state, _db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres, again"),
        ],
        None,
    );
    put(&state, "c1", None, None).await;
    put(&state, "c2", None, None).await;
    assert_eq!(
        relationships(&state, json!({})).await[0]["support"]["primary"],
        2
    );
}

#[tokio::test]
async fn no_origin_client_sees_unchanged_membership_and_order() {
    let (state, _db, _d) = state_with(
        vec![fact("A", "B", "A uses B"), fact("C", "D", "C uses D")],
        None,
    );
    put(&state, "x", None, None).await;
    put(&state, "y", None, None).await;
    let plain = relationships(&state, json!({})).await;
    let all = relationships(
        &state,
        json!({"origins": ["primary", "external", "derived", "answer"]}),
    )
    .await;
    let uuids = |v: &Vec<Value>| -> Vec<String> {
        v.iter()
            .map(|e| e["uuid"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(uuids(&plain), uuids(&all));
}

// ── US3: origin-aware reads ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn relationships_origins_filter_excludes_derived_only_facts() {
    let (state, _db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "MySQL", "X uses MySQL"),
        ],
        None,
    );
    put(&state, "note", None, None).await;
    put(&state, "page", Some("derived"), None).await;
    let facts: Vec<String> = relationships(&state, json!({"origins": ["primary", "external"]}))
        .await
        .iter()
        .map(|e| e["fact"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(facts, vec!["X uses Postgres"]);
    let derived: Vec<Value> = relationships(&state, json!({"origins": ["derived"]})).await;
    assert_eq!(derived.len(), 1);
    assert_eq!(derived[0]["fact"], "X uses MySQL");
    assert_eq!(derived[0]["corroboration"], 0);
}

#[tokio::test]
async fn entities_origins_filter_excludes_derived_only_entities() {
    let (state, _db, _d) = state_with(
        vec![
            fact("Alpha", "Beta", "Alpha uses Beta"),
            fact("Gamma", "Delta", "Gamma uses Delta"),
        ],
        None,
    );
    put(&state, "note", None, None).await;
    put(&state, "page", Some("derived"), None).await;
    let names = |v: Vec<Value>| -> Vec<String> {
        let mut n: Vec<String> = v
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect();
        n.sort();
        n
    };
    let primary = entities(&state, "a", json!({"origins": ["primary", "external"]})).await;
    assert_eq!(names(primary), vec!["Alpha", "Beta"]);
    let all = entities(&state, "a", json!({})).await;
    assert_eq!(all.len(), 4);
    for e in &all {
        assert!(e["support"].is_object(), "support is always present: {e}");
    }
}

#[tokio::test]
async fn invalid_origins_filters_are_rejected_on_all_three_reads() {
    let (state, _db, _d) = state_with(vec![], None);
    for method in [
        "knowledge_find_relationships",
        "knowledge_find_entities",
        "knowledge_search_passages",
    ] {
        for bad in [json!([]), json!(["nope"]), json!("primary")] {
            let v = try_call(
                method,
                json!({"query": "q", "group_ids": [G], "origins": bad}),
                &state,
            )
            .await;
            assert!(v.get("error").is_some(), "{method} {bad}: {v}");
        }
    }
}

#[tokio::test]
async fn search_passages_origins_filter_and_origin_field() {
    let (state, _db, _d) = state_with(vec![fact("A", "B", "f1"), fact("C", "D", "f2")], None);
    put(&state, "note", None, None).await;
    put(&state, "page", Some("derived"), None).await;
    let call_search = |extra: Value| {
        let state = Arc::clone(&state);
        async move {
            let mut params =
                json!({"query": "text", "group_ids": [G], "min_score": 0.0, "num_results": 10});
            for (k, v) in extra.as_object().cloned().unwrap_or_default() {
                params[k] = v;
            }
            call("knowledge_search_passages", params, &state).await["passages"]
                .as_array()
                .unwrap()
                .clone()
        }
    };
    let all = call_search(json!({})).await;
    assert_eq!(all.len(), 2, "{all:?}");
    let origins: BTreeMap<String, String> = all
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().into(),
                p["origin"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(origins["note"], "primary");
    assert_eq!(origins["page"], "derived");
    let only = call_search(json!({"origins": ["primary", "external"]})).await;
    assert_eq!(only.len(), 1);
    assert_eq!(only[0]["name"], "note");
}

// ── US2: derived never overrides primary ─────────────────────────────────────────────────────

/// SC-003: a conflicting derived claim is stored as its own derived-only fact; the primary fact
/// is untouched.
#[tokio::test]
async fn derived_contradiction_leaves_primary_fact_untouched() {
    let (state, db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "MySQL", "X uses MySQL"),
        ],
        None,
    );
    put(&state, "note", None, None).await;
    put(&state, "page", Some("derived"), None).await;
    let conn = db.connect().unwrap();
    let edges = conn.get_edges_by_group_ids(None).unwrap();
    let pg = edges.iter().find(|e| e.fact == "X uses Postgres").unwrap();
    assert!(pg.invalid_at.is_none());
    let my = edges.iter().find(|e| e.fact == "X uses MySQL").unwrap();
    assert!(my.invalid_at.is_none());
    let beliefs = relationships(&state, json!({"believed_only": true})).await;
    assert_eq!(beliefs.len(), 2);
    let mysql = beliefs
        .iter()
        .find(|e| e["fact"] == "X uses MySQL")
        .unwrap();
    assert_eq!(mysql["corroboration"], 0, "derived-only");
    let pgv = beliefs
        .iter()
        .find(|e| e["fact"] == "X uses Postgres")
        .unwrap();
    assert_eq!(pgv["corroboration"], 1);
}

#[tokio::test]
async fn derived_episode_does_not_rewrite_a_primary_entity() {
    let (state, db, _d) = state_with(
        vec![
            summarised(
                &[("Postgres", "Redis", "Postgres beside Redis")],
                "is a database",
            ),
            summarised(
                &[("Postgres", "Kafka", "Postgres beside Kafka")],
                "is totally different",
            ),
        ],
        None,
    );
    put(&state, "note", None, None).await;
    let before = entity_summary(&db, "Postgres");
    put(&state, "page", Some("derived"), None).await;
    assert_eq!(entity_summary(&db, "Postgres"), before);
    // Still one Postgres: the identity-level match links the derived episode to it (FR-006).
    let conn = db.connect().unwrap();
    let all = conn.list_entities(None, 100).unwrap();
    assert_eq!(all.iter().filter(|e| e.name == "Postgres").count(), 1);
    // ...and the entity now shows the derived mention as support.
    let pg = entities(&state, "Postgres", json!({}))
        .await
        .into_iter()
        .find(|e| e["name"] == "Postgres")
        .unwrap();
    assert_eq!(pg["support"]["primary"], 1);
    assert_eq!(pg["support"]["derived"], 1);
}

#[tokio::test]
async fn primary_episode_still_updates_an_entity_normally() {
    let (state, db, _d) = state_with(
        vec![
            summarised(&[("Postgres", "Redis", "p beside r")], "is a database"),
            summarised(
                &[("Postgres", "Kafka", "p beside k")],
                "also stores relational data",
            ),
        ],
        None,
    );
    put(&state, "note1", None, None).await;
    let before = entity_summary(&db, "Postgres");
    put(&state, "note2", None, None).await;
    assert_ne!(
        entity_summary(&db, "Postgres"),
        before,
        "primary merge unchanged"
    );
}

#[tokio::test]
async fn derived_revision_of_a_primary_chunk_is_refused_and_writes_nothing() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(
        vec![fact("A", "B", "f1"), fact("C", "D", "f2")],
        Some(wal.path().to_path_buf()),
    );
    put(&state, "chunk", None, None).await;
    let wal_before = wal_bytes(wal.path());
    let eps_before = episode_count(&db);
    for supersede in [true, false] {
        let mut params = json!({
            "chunk_text": "t", "chunk_id": "chunk", "source_file": "w.md", "group_id": G,
            "origin": "derived", "supersede": supersede,
        });
        params["reference_time"] = json!("2026-08-04T00:00:00Z");
        let v = try_call("knowledge_process_chunk", params, &state).await;
        assert!(is_conflict(&v), "supersede={supersede}: {v}");
    }
    assert_eq!(episode_count(&db), eps_before);
    assert_eq!(wal_bytes(wal.path()), wal_before, "no WAL entry (SC-006)");
    let conn = db.connect().unwrap();
    let eps = conn.retrieve_episodes_page(None, 10, None, None).unwrap();
    assert!(
        eps[0].0.superseded_at.is_none(),
        "belief in the primary episode is unchanged"
    );
}

#[tokio::test]
async fn primary_may_revise_a_derived_chunk() {
    let (state, db, _d) = state_with(vec![fact("A", "B", "f1"), fact("C", "D", "f2")], None);
    put(&state, "chunk", Some("derived"), None).await;
    let r = put(&state, "chunk", None, None).await;
    assert_eq!(r["superseded_episode_uuids"].as_array().unwrap().len(), 1);
    assert_eq!(episode_count(&db), 2);
}

#[tokio::test]
async fn derived_revision_of_a_derived_chunk_is_allowed() {
    let (state, _db, _d) = state_with(vec![fact("A", "B", "f1"), fact("C", "D", "f2")], None);
    put(&state, "chunk", Some("derived"), None).await;
    let r = put(&state, "chunk", Some("derived"), None).await;
    assert_eq!(r["superseded_episode_uuids"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn one_doc_id_has_one_origin() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(
        vec![fact("A", "B", "f1"), fact("C", "D", "f2")],
        Some(wal.path().to_path_buf()),
    );
    put(&state, "c1", None, Some("doc")).await;
    let wal_before = wal_bytes(wal.path());
    let v = put_raw(
        &state,
        "c2",
        Some("derived"),
        Some("doc"),
        "a.md",
        json!({}),
    )
    .await;
    assert!(is_conflict(&v), "{v}");
    assert_eq!(episode_count(&db), 1);
    assert_eq!(wal_bytes(wal.path()), wal_before);
    // The same origin is fine.
    put(&state, "c3", None, Some("doc")).await;
    assert_eq!(episode_count(&db), 2);
}

#[tokio::test]
async fn derived_document_cannot_supersede_a_primary_one() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(
        vec![fact("A", "B", "old"), fact("C", "D", "new")],
        Some(wal.path().to_path_buf()),
    );
    put(&state, "p1", None, Some("P")).await;
    put(&state, "d1", Some("derived"), Some("D")).await;
    let wal_before = wal_bytes(wal.path());
    let v = try_call(
        "knowledge_supersede_document",
        json!({"group_id": G, "superseding_doc_id": "D", "superseded_doc_id": "P",
               "effective_at": "2026-08-05T00:00:00Z"}),
        &state,
    )
    .await;
    assert!(is_conflict(&v), "{v}");
    assert_eq!(wal_bytes(wal.path()), wal_before);
    let conn = db.connect().unwrap();
    assert!(conn
        .retrieve_episodes_page(None, 10, None, None)
        .unwrap()
        .iter()
        .all(|(e, _)| e.superseded_at.is_none()));
    // The other direction is allowed: primary content may supersede derived content.
    let v = try_call(
        "knowledge_supersede_document",
        json!({"group_id": G, "superseding_doc_id": "P", "superseded_doc_id": "D",
               "effective_at": "2026-08-05T00:00:00Z"}),
        &state,
    )
    .await;
    assert!(v.get("error").is_none(), "{v}");
}

#[tokio::test]
async fn derived_removal_cannot_end_belief_in_a_primary_chunk() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(vec![fact("A", "B", "f")], Some(wal.path().to_path_buf()));
    put(&state, "chunk", None, None).await;
    let wal_before = wal_bytes(wal.path());
    let v = try_call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "chunk", "group_ids": [G], "origin": "derived"}),
        &state,
    )
    .await;
    assert!(is_conflict(&v), "{v}");
    assert_eq!(wal_bytes(wal.path()), wal_before);
    let conn = db.connect().unwrap();
    assert!(
        conn.retrieve_episodes_page(None, 10, None, None).unwrap()[0]
            .0
            .superseded_at
            .is_none()
    );
    // Without an origin the call behaves exactly as before.
    let r = call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "chunk", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(r["superseded_count"], 1);
}

/// `as_of` + `origins` / `support` are evaluated at the `as_of` instant (not "now"), so they agree
/// with the `belief` block: an episode live at T0 and removed afterwards still supports the edge
/// when read as of T0.
#[tokio::test]
async fn as_of_read_evaluates_origins_and_support_at_that_instant() {
    let (state, _db, _d) = state_with(vec![fact("A", "B", "f")], None);
    put(&state, "chunk", None, None).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let t0 = chrono::Utc::now().to_rfc3339();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let r = call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "chunk", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(r["superseded_count"], 1);

    // As of T0 the primary episode was live: the edge is returned and its support counts it.
    let past = relationships(&state, json!({"as_of": t0, "origins": ["primary"]})).await;
    assert_eq!(past.len(), 1, "{past:?}");
    assert_eq!(past[0]["support"]["primary"], 1, "{:?}", past[0]);
    assert_eq!(past[0]["corroboration"], 1);

    // Read now, the episode is superseded, so nothing primary supports the edge any more.
    let now = relationships(&state, json!({"origins": ["primary"]})).await;
    assert!(now.is_empty(), "{now:?}");
}

// ── US4: WAL rebuild ─────────────────────────────────────────────────────────────────────────

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

/// `(fact, support, corroboration)` of every edge, sorted.
async fn support_table(state: &Arc<AppState>) -> Vec<String> {
    let mut v: Vec<String> = relationships(state, json!({}))
        .await
        .iter()
        .map(|e| format!("{} {} {}", e["fact"], e["support"], e["corroboration"]))
        .collect();
    v.sort();
    v
}

async fn mixed_history() -> (Arc<AppState>, Arc<Db>, TempDir, TempDir) {
    let wal = TempDir::new().unwrap();
    let (state, db, dir) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres (clip)"),
            fact("X", "Postgres", "X uses Postgres (page)"),
            fact("X", "Postgres", "X uses Postgres (qa)"),
        ],
        Some(wal.path().to_path_buf()),
    );
    put(&state, "n", None, Some("note")).await;
    put(&state, "e", Some("external"), Some("clip")).await;
    put(&state, "d", Some("derived"), Some("page")).await;
    put(&state, "a", Some("answer"), None).await;
    (state, db, dir, wal)
}

#[tokio::test]
async fn wal_rebuild_preserves_origin_and_support() {
    let (state, db, _d, wal) = mixed_history().await;
    let (state2, db2, _d2) = replayed(wal.path()).await;
    assert_eq!(episode_origins(&db2), episode_origins(&db));
    assert_eq!(support_table(&state2).await, support_table(&state).await);
    assert!(support_table(&state2).await[0].contains(r#""derived":1"#));
}

#[tokio::test]
async fn dump_then_replay_preserves_origin() {
    let (state, db, _d, _wal) = mixed_history().await;
    let out = TempDir::new().unwrap();
    let target = out.path().join("dump");
    let r = call(
        "knowledge_dump_wal",
        json!({"target_dir": target.to_str().unwrap()}),
        &state,
    )
    .await;
    assert_eq!(r["success"], true, "{r}");
    let (state2, db2, _d2) = replayed(&target).await;
    assert_eq!(episode_origins(&db2), episode_origins(&db));
    assert_eq!(support_table(&state2).await, support_table(&state).await);
}

/// US4 AC3: the guard decision is baked into the logged statements, so replay does not re-open a
/// protected entity.
#[tokio::test]
async fn replay_reproduces_guard_outcomes() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = state_with(
        vec![
            summarised(&[("Postgres", "Redis", "p beside r")], "is a database"),
            summarised(
                &[("Postgres", "Kafka", "p beside k")],
                "is totally different",
            ),
        ],
        Some(wal.path().to_path_buf()),
    );
    put(&state, "note", None, None).await;
    put(&state, "page", Some("derived"), None).await;
    let (_s2, db2, _d2) = replayed(wal.path()).await;
    assert_eq!(
        entity_summary(&db2, "Postgres"),
        entity_summary(&db, "Postgres")
    );
}

#[tokio::test]
async fn pre_feature_wal_replays_as_primary() {
    // A WAL written before this feature never names `origin`; replay must leave NULL → primary.
    let wal = TempDir::new().unwrap();
    let (state, _db, _d) = state_with(vec![fact("A", "B", "f")], Some(wal.path().to_path_buf()));
    put(&state, "n", None, None).await;
    for entry in std::fs::read_dir(wal.path()).unwrap().flatten() {
        let mut files = vec![entry.path()];
        while let Some(p) = files.pop() {
            if p.is_dir() {
                files.extend(std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
            } else {
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                let stripped = text
                    .replace(", origin: $origin", "")
                    .replace(r#","origin":"primary""#, "");
                std::fs::write(&p, stripped).unwrap();
            }
        }
    }
    let (state2, db2, _d2) = replayed(wal.path()).await;
    assert_eq!(episode_count(&db2), 1);
    assert_eq!(episode_origins(&db2)["n"], "");
    let e = &relationships(&state2, json!({})).await[0];
    assert_eq!(e["support"]["primary"], 1, "reads as primary");
    let all = relationships(&state2, json!({"origins": ["primary"]})).await;
    assert_eq!(all.len(), 1);
}

// ── US5: stamping ────────────────────────────────────────────────────────────────────────────

async fn stamp(state: &Arc<AppState>, rules: Value, dry_run: bool) -> Value {
    try_call(
        "knowledge_stamp_origin",
        json!({"group_id": G, "rules": rules, "dry_run": dry_run}),
        state,
    )
    .await
}

async fn legacy_corpus() -> (Arc<AppState>, Arc<Db>, TempDir, TempDir) {
    let wal = TempDir::new().unwrap();
    let (state, db, dir) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres (page)"),
            fact("X", "Postgres", "X uses Postgres (consolidation)"),
            fact("Y", "Z", "Y uses Z"),
        ],
        Some(wal.path().to_path_buf()),
    );
    put_raw(
        &state,
        "note",
        None,
        None,
        "notes/source-note.md",
        json!({}),
    )
    .await;
    put_raw(
        &state,
        "page1",
        None,
        Some("wiki-doc"),
        "wiki/page.md",
        json!({}),
    )
    .await;
    put_raw(
        &state,
        "chat",
        None,
        None,
        "chat/1.md",
        json!({"kind": "knowledge"}),
    )
    .await;
    put_raw(
        &state,
        "other",
        None,
        None,
        "notes/b.md",
        json!({"kind": "note"}),
    )
    .await;
    (state, db, dir, wal)
}

#[tokio::test]
async fn stamp_by_path_prefix_and_by_attributes_kind() {
    let (state, db, _d, _w) = legacy_corpus().await;
    let r = stamp(
        &state,
        json!([
            {"origin": "derived", "source_path_prefix": "wiki/"},
            {"origin": "derived", "attributes_kind": "knowledge"},
        ]),
        false,
    )
    .await;
    assert_eq!(r["result"]["changed"], 2, "{r}");
    assert_eq!(r["result"]["dry_run"], false);
    let o = episode_origins(&db);
    assert_eq!(o["page1"], "derived");
    assert_eq!(o["chat"], "derived");
    assert_eq!(o["note"], "primary");
    assert_eq!(o["other"], "primary");
    // Support now reflects the stamp.
    let facts = relationships(&state, json!({"origins": ["primary"]})).await;
    assert!(facts.iter().all(|e| e["fact"] != "X uses Postgres (page)"));
}

#[tokio::test]
async fn stamp_selectors_are_anded_within_a_rule() {
    let (state, db, _d, _w) = legacy_corpus().await;
    stamp(
        &state,
        json!([{"origin": "external", "source_path_prefix": "notes/", "attributes_kind": "note"}]),
        false,
    )
    .await;
    let o = episode_origins(&db);
    assert_eq!(o["other"], "external");
    assert_eq!(o["note"], "primary");
}

#[tokio::test]
async fn stamp_dry_run_writes_nothing() {
    let (state, db, _d, wal) = legacy_corpus().await;
    let wal_before = wal_bytes(wal.path());
    let before = episode_origins(&db);
    let r = stamp(
        &state,
        json!([{"origin": "derived", "source_path_prefix": "wiki/"}]),
        true,
    )
    .await;
    assert_eq!(r["result"]["dry_run"], true);
    assert_eq!(r["result"]["changed"], 1);
    assert_eq!(r["result"]["changes"][0]["from"], "primary");
    assert_eq!(r["result"]["changes"][0]["to"], "derived");
    assert_eq!(episode_origins(&db), before);
    assert_eq!(wal_bytes(wal.path()), wal_before);
}

#[tokio::test]
async fn stamp_is_idempotent() {
    let (state, _db, _d, wal) = legacy_corpus().await;
    let rules = json!([{"origin": "derived", "source_path_prefix": "wiki/"}]);
    assert_eq!(
        stamp(&state, rules.clone(), false).await["result"]["changed"],
        1
    );
    let wal_before = wal_bytes(wal.path());
    let r = stamp(&state, rules, false).await;
    assert_eq!(r["result"]["changed"], 0);
    assert_eq!(r["result"]["unchanged"], 1);
    assert_eq!(
        wal_bytes(wal.path()),
        wal_before,
        "a no-op stamp writes no WAL entry"
    );
}

#[tokio::test]
async fn stamp_expands_to_the_whole_document_and_refuses_split_origins() {
    let (state, db, _d) = state_with(
        vec![
            fact("A", "B", "f1"),
            fact("C", "D", "f2"),
            fact("E", "F", "f3"),
        ],
        None,
    );
    put_raw(&state, "c1", None, Some("doc"), "wiki/p.md", json!({})).await;
    put_raw(&state, "c2", None, Some("doc"), "elsewhere/p.md", json!({})).await;
    put_raw(&state, "solo", None, None, "elsewhere/q.md", json!({})).await;
    let r = stamp(
        &state,
        json!([{"origin": "derived", "source_path_prefix": "wiki/"}]),
        false,
    )
    .await;
    assert_eq!(
        r["result"]["changed"], 2,
        "c2 comes along with its document: {r}"
    );
    let o = episode_origins(&db);
    assert_eq!((o["c1"].as_str(), o["c2"].as_str()), ("derived", "derived"));
    assert_eq!(o["solo"], "primary");

    // Two rules that would give one document two origins are refused, and nothing changes.
    let (state, db, _d) = state_with(vec![fact("A", "B", "f1"), fact("C", "D", "f2")], None);
    put_raw(&state, "c1", None, Some("doc"), "wiki/p.md", json!({})).await;
    put_raw(&state, "c2", None, Some("doc"), "clips/p.md", json!({})).await;
    let r = stamp(
        &state,
        json!([
            {"origin": "derived", "source_path_prefix": "wiki/"},
            {"origin": "external", "source_path_prefix": "clips/"},
        ]),
        false,
    )
    .await;
    assert!(is_conflict(&r), "{r}");
    assert!(episode_origins(&db).values().all(|v| v == "primary"));
}

#[tokio::test]
async fn stamp_validates_its_rules() {
    let (state, _db, _d) = state_with(vec![], None);
    for bad in [
        json!([]),
        json!([{"origin": "derived"}]),
        json!([{"origin": "nope", "source_path_prefix": "a"}]),
        json!([{"source_path_prefix": "a"}]),
    ] {
        let r = stamp(&state, bad.clone(), false).await;
        assert!(r.get("error").is_some(), "{bad}: {r}");
    }
}

#[tokio::test]
async fn stamp_survives_wal_rebuild_and_needs_no_extraction() {
    let (state, db, _d, wal) = legacy_corpus().await;
    // The extractor queue is empty now; a stamp that tried to extract would get nothing and the
    // episode count would change.
    let eps = episode_count(&db);
    stamp(
        &state,
        json!([{"origin": "derived", "source_path_prefix": "wiki/"}]),
        false,
    )
    .await;
    assert_eq!(episode_count(&db), eps);
    let (state2, db2, _d2) = replayed(wal.path()).await;
    assert_eq!(episode_origins(&db2), episode_origins(&db));
    assert_eq!(support_table(&state2).await, support_table(&state).await);
    // Replaying the stamp over an overlapping range changes nothing.
    let (_s3, db3, _d3) = replayed(wal.path()).await;
    assert_eq!(episode_origins(&db3), episode_origins(&db));
}

// ── Issue #857: find_entities `include_episodes` provenance ──────────────────────────────────

fn str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

fn hit<'a>(nodes: &'a [Value], name: &str) -> &'a Value {
    nodes
        .iter()
        .find(|n| n["name"] == name)
        .unwrap_or_else(|| panic!("no hit {name} in {nodes:?}"))
}

async fn put_file(state: &Arc<AppState>, chunk_id: &str, source_file: &str) {
    let v = put_raw(state, chunk_id, None, None, source_file, json!({})).await;
    assert!(v.get("error").is_none(), "ingest errored: {v}");
}

/// Ingests three chunks that each mention A and B, then removes the third.
async fn three_chunks_one_superseded() -> (Arc<AppState>, Arc<Db>, TempDir) {
    let (state, db, d) = state_with(
        vec![
            fact("Aaa", "Bbb", "f1"),
            fact("Aaa", "Bbb", "f2"),
            fact("Aaa", "Bbb", "f3"),
        ],
        None,
    );
    put_file(&state, "c1", "notes/one.md").await;
    put_file(&state, "c2", "notes/two.md").await;
    put_file(&state, "c3", "notes/three.md").await;
    let r = call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "c3", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(r["superseded_count"], 1);
    (state, db, d)
}

#[tokio::test]
async fn find_entities_include_episodes_excludes_superseded_and_matches_neighbors() {
    let (state, db, _d) = three_chunks_one_superseded().await;

    let nodes = entities(&state, "Bbb", json!({"include_episodes": true})).await;
    let b = hit(&nodes, "Bbb");
    let got = str_list(&b["episode_uuids"]);
    assert_eq!(got.len(), 2, "{b}");
    let descs = str_list(&b["source_descriptions"]);
    assert_eq!(descs.len(), 2, "index-aligned with episode_uuids: {b}");
    let mut files = str_list(&b["source_files"]);
    files.sort();
    assert_eq!(files, vec!["notes/one.md", "notes/two.md"]);
    assert!(b.get("episodes_truncated").is_none());

    // Live subset of what get_entity_neighbors reports for the same entity (neighbors does not
    // filter superseded episodes — documented divergence, ADR-0857).
    let a = hit(&nodes, "Aaa").clone();
    let nb = call(
        "knowledge_get_entity_neighbors",
        json!({"entity_uuid": a["uuid"], "group_ids": [G]}),
        &state,
    )
    .await;
    let nb_b = nb["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "Bbb")
        .unwrap()
        .clone();
    let conn = db.connect().unwrap();
    let live_in_neighbors: std::collections::BTreeSet<String> = str_list(&nb_b["episode_uuids"])
        .into_iter()
        .filter(|u| {
            conn.get_episode_origin_info(std::slice::from_ref(u))
                .unwrap()
                .get(u)
                .is_some_and(|i| i.superseded_at_us.is_none())
        })
        .collect();
    assert_eq!(live_in_neighbors.len(), 2);
    assert_eq!(
        got.iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        live_in_neighbors
    );
    assert_eq!(
        str_list(&nb_b["episode_uuids"]).len(),
        3,
        "neighbors keeps superseded"
    );

    // support is unchanged by the flag.
    let off = entities(&state, "Bbb", json!({})).await;
    assert_eq!(hit(&off, "Bbb")["support"], b["support"]);
}

#[tokio::test]
async fn find_entities_flag_off_and_false_are_identical_and_add_no_keys() {
    let (state, _db, _d) = three_chunks_one_superseded().await;
    let absent = entities(&state, "Bbb", json!({})).await;
    let off = entities(&state, "Bbb", json!({"include_episodes": false})).await;
    assert_eq!(absent, off);
    let b = hit(&absent, "Bbb");
    assert_eq!(b["episode_uuids"], json!([]));
    assert_eq!(b["source_descriptions"], json!([]));
    assert!(b.get("source_files").is_none());
    assert!(b.get("episodes_truncated").is_none());
    // Serialised bytes are identical too.
    assert_eq!(
        serde_json::to_string(&absent).unwrap(),
        serde_json::to_string(&off).unwrap()
    );
}

#[tokio::test]
async fn find_entities_include_episodes_rejects_non_boolean() {
    let (state, _db, _d) = state_with(vec![], None);
    for bad in [json!("true"), json!(1), json!([true])] {
        let v = try_call(
            "knowledge_find_entities",
            json!({"query": "q", "group_ids": [G], "include_episodes": bad}),
            &state,
        )
        .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("include_episodes must be a boolean")),
            "{bad}: {v}"
        );
    }
}

#[tokio::test]
async fn find_entities_include_episodes_caps_at_fifty_newest_first() {
    let total = lcg_core::origin_reads::ENTITY_EPISODE_CAP + 3;
    let queue: Vec<ExtractionResult> = (0..total).map(|_| fact("Aaa", "Bbb", "f")).collect();
    let (state, _db, _d) = state_with(queue, None);
    for i in 0..total {
        put_file(&state, &format!("c{i:03}"), "notes/same.md").await;
    }
    let nodes = entities(&state, "Bbb", json!({"include_episodes": true})).await;
    let b = hit(&nodes, "Bbb");
    assert_eq!(
        str_list(&b["episode_uuids"]).len(),
        lcg_core::origin_reads::ENTITY_EPISODE_CAP
    );
    assert_eq!(b["episodes_truncated"], json!(true));
    assert_eq!(b["support"]["primary"], json!(total));
    // Deterministic: a repeat call returns the same subset in the same order.
    let again = entities(&state, "Bbb", json!({"include_episodes": true})).await;
    assert_eq!(hit(&again, "Bbb")["episode_uuids"], b["episode_uuids"]);
    // De-duplicated files: one shared source file.
    assert_eq!(b["source_files"], json!(["notes/same.md"]));
}

// ── Issue #857: get_episodes `uuids` filter ──────────────────────────────────────────────────

async fn episode_uuids_by_chunk(state: &Arc<AppState>) -> Vec<(String, String)> {
    let r = call("knowledge_get_episodes", json!({"group_ids": [G]}), state).await;
    r["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["chunk_id"].as_str().unwrap_or("").to_string(),
                e["uuid"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

async fn get_eps(state: &Arc<AppState>, extra: Value) -> Vec<String> {
    let mut params = json!({"group_ids": [G]});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    let r = call("knowledge_get_episodes", params, state).await;
    assert_eq!(
        r["count"].as_u64().unwrap() as usize,
        r["episodes"].as_array().unwrap().len()
    );
    let mut u = str_list(&json!(r["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["uuid"].clone())
        .collect::<Vec<_>>()));
    u.sort();
    u
}

#[tokio::test]
async fn get_episodes_uuids_filter() {
    let (state, _db, _d) = three_chunks_one_superseded().await;
    let all = episode_uuids_by_chunk(&state).await;
    assert_eq!(all.len(), 3);
    let id = |c: &str| all.iter().find(|(ch, _)| ch == c).unwrap().1.clone();
    let (a, b, c) = (id("c1"), id("c2"), id("c3"));
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };

    // Exactly the listed episodes (superseded ones included — no liveness filter on get_episodes).
    assert_eq!(
        get_eps(&state, json!({"uuids": [a, b]})).await,
        sorted(vec![a.clone(), b.clone()])
    );
    // Unknown uuid is absent, not an error.
    assert_eq!(
        get_eps(&state, json!({"uuids": [a, "nope"]})).await,
        vec![a.clone()]
    );
    // Duplicates return each episode once.
    assert_eq!(
        get_eps(&state, json!({"uuids": [c, c, c]})).await,
        vec![c.clone()]
    );
    // Empty array returns nothing.
    assert!(get_eps(&state, json!({"uuids": []})).await.is_empty());
    // No key: unchanged (all three).
    assert_eq!(get_eps(&state, json!({})).await.len(), 3);
    // Explicit last_n still caps a uuids request.
    assert_eq!(
        get_eps(&state, json!({"uuids": [a, b, c], "last_n": 2}))
            .await
            .len(),
        2
    );
    // Group scope holds: another group sees none of them.
    let r = call(
        "knowledge_get_episodes",
        json!({"group_ids": ["other-group"], "uuids": [a, b, c]}),
        &state,
    )
    .await;
    assert_eq!(r["count"], 0);
    // A uuids-only call keeps the unpaged shape.
    assert!(r.get("next_cursor").is_none());
    // Malformed input is a validation error.
    for bad in [json!([1]), json!("x"), json!({"a": 1})] {
        let v = try_call(
            "knowledge_get_episodes",
            json!({"group_ids": [G], "uuids": bad}),
            &state,
        )
        .await;
        assert!(
            v["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("uuids must be an array of strings")),
            "{bad}: {v}"
        );
    }
}

#[tokio::test]
async fn get_episodes_uuids_without_last_n_is_not_capped_at_fifty() {
    let total = 55;
    let queue: Vec<ExtractionResult> = (0..total).map(|_| fact("Aaa", "Bbb", "f")).collect();
    let (state, _db, _d) = state_with(queue, None);
    for i in 0..total {
        put_file(&state, &format!("c{i:03}"), "n.md").await;
    }
    let everything = call(
        "knowledge_get_episodes",
        json!({"group_ids": [G], "last_n": 1000}),
        &state,
    )
    .await;
    let uuids: Vec<Value> = everything["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["uuid"].clone())
        .collect();
    assert_eq!(uuids.len(), total);
    assert_eq!(get_eps(&state, json!({"uuids": uuids})).await.len(), total);
}

#[tokio::test]
async fn get_episodes_cursor_is_bound_to_the_uuids_set() {
    let (state, _db, _d) = three_chunks_one_superseded().await;
    let all = episode_uuids_by_chunk(&state).await;
    let ids: Vec<String> = all.iter().map(|(_, u)| u.clone()).collect();
    let page = call(
        "knowledge_get_episodes",
        json!({"group_ids": [G], "uuids": ids, "last_n": 1, "cursor": null, "name_prefix": ""}),
        &state,
    )
    .await;
    let cursor = page["next_cursor"].as_str().expect("next page").to_string();
    // Replayed under a different uuids set: rejected.
    let v = try_call(
        "knowledge_get_episodes",
        json!({"group_ids": [G], "uuids": [ids[0]], "last_n": 1, "cursor": cursor}),
        &state,
    )
    .await;
    assert!(v.get("error").is_some(), "{v}");
    // Replayed under the same set: accepted.
    let v = try_call(
        "knowledge_get_episodes",
        json!({"group_ids": [G], "uuids": ids, "last_n": 1, "cursor": cursor}),
        &state,
    )
    .await;
    assert!(v.get("error").is_none(), "{v}");
}
