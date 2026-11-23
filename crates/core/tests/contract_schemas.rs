//! Issue #852 / ADR-0852: real handler responses must validate against the published JSON
//! Schemas in `contract/`. A response that drifts from its schema (renamed, dropped or retyped
//! field) fails here, naming the method and the JSON path, before a release can ship.
//!
//! Every response below comes from `handlers::dispatch` against a real `Db`; the negative tests
//! then break a copy of a real response and assert the schema rejects it. Variant coverage:
//! health (healthy, degraded, busy-writing, busy-rebuilding), status (empty, with WAL groups,
//! degraded; not-queryable is derived from a real response, see below), find_entities (with and
//! without `include_episodes`), find_relationships (with and without `belief`), search_passages,
//! process_chunk (normal, re-ingest, oversized warning, dropped edges) and every lint category.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::contract;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    handlers,
    ipc::IpcRequest,
    rebuild_job::{JobStatus, RebuildJob},
    telemetry::{NoopSink, TelemetrySink},
    ConfigurableExtractor, Db, ExtractedEdge, ExtractedEntity, ExtractionResult, Extractor,
    WalWriter,
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
    db: Option<Arc<Db>>,
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
        degraded_reason: Arc::new(Mutex::new(db.is_none().then(|| "corrupt wal".to_string()))),
        db: ArcSwapOption::from(db),
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

fn simple_state(db: Option<Arc<Db>>) -> Arc<AppState> {
    make_state(db, None, Arc::new(ConfigurableExtractor::new(vec![])))
}

fn state_with(
    queue: Vec<ExtractionResult>,
    wal_dir: Option<PathBuf>,
) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let (db, dir) = make_db();
    let state = make_state(
        Some(Arc::clone(&db)),
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

fn now_us() -> i64 {
    chrono::Utc::now().timestamp_micros()
}

fn rfc3339(us: i64) -> String {
    chrono::DateTime::from_timestamp_micros(us)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

async fn put(
    state: &Arc<AppState>,
    chunk_id: &str,
    origin: Option<&str>,
    doc: Option<&str>,
    attributes: Value,
) {
    let mut params = json!({
        "chunk_text": format!("text of {chunk_id}"), "chunk_id": chunk_id,
        "source_file": "notes/source-note.md", "group_id": G,
        "reference_time": "2026-08-04T00:00:00Z", "attributes": attributes,
    });
    if let Some(o) = origin {
        params["origin"] = json!(o);
    }
    if let Some(d) = doc {
        params["doc_id"] = json!(d);
    }
    let v = try_call("knowledge_process_chunk", params, state).await;
    assert!(v.get("error").is_none(), "ingest errored: {v}");
}

fn fact(src: &str, dst: &str, f: &str) -> ExtractionResult {
    extraction(&[(src, dst, f)])
}

async fn lint(state: &Arc<AppState>, extra: Value) -> Value {
    let mut params = json!({"group_ids": [G]});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    call("knowledge_lint", params, state).await
}

fn findings_of<'a>(r: &'a Value, category: &str) -> Vec<&'a Value> {
    r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["category"] == category)
        .collect()
}

// ── schema loading and validation ───────────────────────────────────────────────────────────

fn schema(method: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contract")
        .join(contract::file_name(method));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// All validation errors for `instance` against `method`'s committed schema, as
/// `"<instance path>: <message>"`.
fn errors(method: &str, instance: &Value) -> Vec<String> {
    let s = schema(method);
    let v = jsonschema::draft7::new(&s).expect("schema compiles");
    v.iter_errors(instance)
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect()
}

/// Asserts the real `instance` validates, naming the method and the offending paths if not.
fn assert_valid(method: &str, instance: &Value) {
    let errs = errors(method, instance);
    assert!(
        errs.is_empty(),
        "{method} response no longer validates against contract/{method}.schema.json:\n  {}\nresponse: {instance}",
        errs.join("\n  ")
    );
}

/// Asserts a deliberately broken copy of a real response is rejected.
fn assert_invalid(method: &str, what: &str, instance: &Value) {
    assert!(
        !errors(method, instance).is_empty(),
        "{method}: schema accepted a response with {what}: {instance}"
    );
}

/// Renames key `from` to `to` in the object at `ptr` (a JSON pointer).
fn rename_key(v: &mut Value, ptr: &str, from: &str, to: &str) {
    let obj = v
        .pointer_mut(ptr)
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("no object at {ptr}"));
    let val = obj
        .remove(from)
        .unwrap_or_else(|| panic!("no key {from} at {ptr}"));
    obj.insert(to.to_string(), val);
}

fn remove_key(v: &mut Value, ptr: &str, key: &str) {
    v.pointer_mut(ptr)
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("no object at {ptr}"))
        .remove(key)
        .unwrap_or_else(|| panic!("no key {key} at {ptr}"));
}

fn set(v: &mut Value, ptr: &str, new: Value) {
    *v.pointer_mut(ptr)
        .unwrap_or_else(|| panic!("no value at {ptr}")) = new;
}

async fn health(state: &Arc<AppState>) -> Value {
    call("health_check", json!({}), state).await
}

async fn seeded() -> (Arc<AppState>, Arc<Db>, TempDir, TempDir) {
    let wal = TempDir::new().unwrap();
    let (state, db, dir) = state_with(
        vec![
            fact("Postgres", "Database", "Postgres is a Database"),
            fact("Postgres", "Svc1", "Svc1 uses Postgres"),
        ],
        Some(wal.path().to_path_buf()),
    );
    put(&state, "pg", None, Some("pg"), json!({})).await;
    put(&state, "svc1", None, Some("svc1"), json!({})).await;
    call("knowledge_build_indices", json!({}), &state).await;
    (state, db, dir, wal)
}

// ── health_check ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_check_healthy_degraded_and_busy_validate() {
    let (db, _d) = make_db();
    let state = simple_state(Some(Arc::clone(&db)));
    let healthy = health(&state).await;
    assert_eq!(healthy["state"], "healthy");
    assert_valid("health_check", &healthy);

    let degraded_state = simple_state(None);
    let degraded = health(&degraded_state).await;
    assert_eq!(degraded["state"], "degraded");
    assert_valid("health_check", &degraded);

    {
        let _w = state.write_lock.write().await;
        let writing = health(&state).await;
        assert_eq!(writing["activity"], "writing");
        assert_valid("health_check", &writing);

        let mut job = RebuildJob::new("job-1".to_string());
        job.status = JobStatus::Running;
        job.mutations_replayed = 10;
        job.wal_files_total = 3;
        state
            .rebuild_jobs
            .lock()
            .unwrap()
            .insert("job-1".to_string(), job);
        let rebuilding = health(&state).await;
        assert_eq!(rebuilding["activity"], "rebuilding");
        assert!(rebuilding.get("progress").is_some());
        assert_valid("health_check", &rebuilding);
    }

    // Drift: renamed / retyped / dropped fields are rejected.
    let mut m = healthy.clone();
    rename_key(&mut m, "", "ready_for_reads", "ready");
    assert_invalid("health_check", "ready_for_reads renamed", &m);
    let mut m = healthy.clone();
    set(&mut m, "/healthy", json!("yes"));
    assert_invalid("health_check", "healthy retyped", &m);
    let mut m = healthy.clone();
    remove_key(&mut m, "", "providers");
    assert_invalid("health_check", "providers dropped", &m);
    let mut m = degraded.clone();
    remove_key(&mut m, "", "reason");
    assert_invalid("health_check", "degraded without reason", &m);
}

// ── knowledge_status ────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn status_variants_validate() {
    // Queryable, empty graph (no index_created_at, empty wal_groups).
    let (db, _d) = make_db();
    let empty_state = simple_state(Some(db));
    let empty = call("knowledge_status", json!({}), &empty_state).await;
    assert!(empty.get("index_created_at").is_none());
    assert_eq!(empty["wal_groups"], json!({}));
    assert_valid("knowledge_status", &empty);

    // Queryable with data and a WAL root: wal_groups and wal.scope present.
    let (state, _db, _d, _wal) = seeded().await;
    let full = call("knowledge_status", json!({}), &state).await;
    assert!(full["wal_groups"]
        .as_object()
        .is_some_and(|m| !m.is_empty()));
    assert_eq!(full["wal"]["scope"], "default_group");
    assert!(full.get("index_created_at").is_some());
    assert_valid("knowledge_status", &full);

    // Degraded: database not open.
    let degraded = call("knowledge_status", json!({}), &simple_state(None)).await;
    assert_eq!(degraded["degraded"], true);
    assert_valid("knowledge_status", &degraded);

    // Not queryable (ADR-0325). Needs a missing core table, which no public path produces, so
    // this shape is derived from a real queryable response by applying exactly the differences
    // handle_knowledge_status_inner's NotQueryable arm makes. Documented gap in ADR-0852.
    let mut nq = full.clone();
    for k in [
        "entity_count",
        "relationship_count",
        "episode_count",
        "last_index_time",
        "index_created_at",
    ] {
        nq[k] = Value::Null;
    }
    remove_key(&mut nq, "", "legacy_edge_count");
    nq["queryable"] = json!(false);
    nq["reason"] = json!("table Entity does not exist");
    nq["indices_built"] = json!(false);
    nq["cross_group_pointers"] = json!({"bound": null, "unbound": null, "ambiguous": null});
    assert_valid("knowledge_status", &nq);

    // Opt-in episode breakdown (#884): absent by default, a block when asked for, and `null`
    // plus a reason when unavailable (degraded / not queryable).
    assert!(full.get("episode_breakdown").is_none());
    let with_bd = call(
        "knowledge_status",
        json!({"include_episode_breakdown": true}),
        &state,
    )
    .await;
    assert!(
        with_bd["episode_breakdown"]["groups"].is_array(),
        "{with_bd}"
    );
    assert_valid("knowledge_status", &with_bd);
    let degraded_bd = call(
        "knowledge_status",
        json!({"include_episode_breakdown": true}),
        &simple_state(None),
    )
    .await;
    assert_eq!(degraded_bd["episode_breakdown"], Value::Null);
    assert!(degraded_bd["episode_breakdown_unavailable"].is_string());
    assert_valid("knowledge_status", &degraded_bd);
    let mut nq_bd = nq.clone();
    nq_bd["episode_breakdown"] = Value::Null;
    nq_bd["episode_breakdown_unavailable"] = json!("database not queryable");
    assert_valid("knowledge_status", &nq_bd);
    let mut m = with_bd.clone();
    rename_key(&mut m, "/episode_breakdown/groups/0", "live", "alive");
    assert_invalid(
        "knowledge_status",
        "episode_breakdown group.live renamed",
        &m,
    );
    let mut m = with_bd.clone();
    set(&mut m, "/episode_breakdown", json!("none"));
    assert_invalid("knowledge_status", "episode_breakdown as a string", &m);
    let mut m = degraded_bd.clone();
    set(&mut m, "/episode_breakdown_unavailable", Value::Null);
    assert_invalid(
        "knowledge_status",
        "episode_breakdown_unavailable: null",
        &m,
    );

    // Drift.
    let mut m = full.clone();
    rename_key(&mut m, "/wal_groups/liminis", "applied_seq", "applied");
    assert_invalid("knowledge_status", "wal_groups.<g>.applied_seq renamed", &m);
    let mut m = full.clone();
    set(&mut m, "/wal_groups", json!([]));
    assert_invalid("knowledge_status", "wal_groups as an array", &m);
    let mut m = full.clone();
    set(&mut m, "/entity_count", json!("many"));
    assert_invalid("knowledge_status", "entity_count retyped", &m);
    let mut m = full.clone();
    remove_key(&mut m, "/wal", "applied_seq");
    assert_invalid("knowledge_status", "wal.applied_seq dropped", &m);
    let mut m = degraded.clone();
    remove_key(&mut m, "", "recovery_available");
    assert_invalid(
        "knowledge_status",
        "degraded without recovery_available",
        &m,
    );
}

// ── knowledge_process_chunk ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn process_chunk_validates_including_warning_and_dropped_edges() {
    let (state, _db, _d) = state_with(
        vec![
            fact("Alpha", "Beta", "Alpha uses Beta"),
            fact("Alpha", "Beta", "Alpha uses Beta"),
        ],
        None,
    );
    let params = json!({
        "chunk_text": "Alpha uses Beta", "chunk_id": "c1", "source_file": "notes/a.md",
        "group_id": G, "reference_time": "2026-08-04T00:00:00Z",
    });
    let normal = call("knowledge_process_chunk", params.clone(), &state).await;
    assert!(normal.get("warning").is_none());
    assert_valid("knowledge_process_chunk", &normal);

    // Omitting group_id falls back to the default group and says so in `warnings` (#856).
    let (state_ig, _dbi, _di) = state_with(vec![fact("Alpha", "Beta", "Alpha uses Beta")], None);
    let implicit = call(
        "knowledge_process_chunk",
        json!({
            "chunk_text": "Alpha uses Beta", "chunk_id": "c0", "source_file": "notes/a.md",
            "reference_time": "2026-08-04T00:00:00Z",
        }),
        &state_ig,
    )
    .await;
    assert!(implicit["warnings"].is_array(), "{implicit}");
    assert_valid("knowledge_process_chunk", &implicit);
    let mut m = implicit.clone();
    m["warnings"] = json!("group_id omitted");
    assert_invalid("knowledge_process_chunk", "warnings as a string", &m);
    let mut m = implicit.clone();
    m["warnings"] = Value::Null;
    assert_invalid("knowledge_process_chunk", "warnings: null", &m);

    // Re-ingest under the same chunk id: superseded_episode_uuids is non-empty.
    let again = call("knowledge_process_chunk", params, &state).await;
    assert_valid("knowledge_process_chunk", &again);

    // Oversized chunk_text carries the advisory warning.
    let (state2, _db2, _d2) = state_with(vec![fact("Alpha", "Beta", "Alpha uses Beta")], None);
    let big = call(
        "knowledge_process_chunk",
        json!({
            "chunk_text": "x ".repeat(6000), "chunk_id": "c2", "source_file": "notes/b.md",
            "group_id": G, "reference_time": "2026-08-04T00:00:00Z",
        }),
        &state2,
    )
    .await;
    assert_eq!(big["warning"]["type"], "chunk_text_oversized", "{big}");
    assert_valid("knowledge_process_chunk", &big);

    // dropped_edges entries: an extracted edge whose endpoint is not among the extracted entities.
    let dropped_extraction = ExtractionResult {
        entities: vec![ExtractedEntity {
            name: "Solo".to_string(),
            entity_type: "Thing".to_string(),
            summary: "Solo summary".to_string(),
            original_entity_type: None,
        }],
        edges: vec![ExtractedEdge {
            source_name: "Solo".to_string(),
            target_name: "Ghost".to_string(),
            fact: "Solo meets Ghost".to_string(),
            relation_type: Some("RELATED_TO".to_string()),
            valid_at: None,
            invalid_at: None,
            original_relation_type: None,
        }],
    };
    let (state3, _db3, _d3) = state_with(vec![dropped_extraction], None);
    let dropped = call(
        "knowledge_process_chunk",
        json!({
            "chunk_text": "Solo meets Ghost", "chunk_id": "c3", "source_file": "notes/c.md",
            "group_id": G, "reference_time": "2026-08-04T00:00:00Z",
        }),
        &state3,
    )
    .await;
    assert!(
        !dropped["dropped_edges"].as_array().unwrap().is_empty(),
        "scenario produced no dropped edge: {dropped}"
    );
    assert_valid("knowledge_process_chunk", &dropped);

    // Drift.
    let mut m = normal.clone();
    rename_key(&mut m, "", "nodes_extracted", "entities_extracted");
    assert_invalid("knowledge_process_chunk", "nodes_extracted renamed", &m);
    let mut m = normal.clone();
    set(&mut m, "/edges_extracted", json!("2"));
    assert_invalid("knowledge_process_chunk", "edges_extracted retyped", &m);
    let mut m = normal.clone();
    remove_key(&mut m, "/dedup_paths", "exact_name");
    assert_invalid(
        "knowledge_process_chunk",
        "dedup_paths.exact_name dropped",
        &m,
    );
    let mut m = big.clone();
    set(&mut m, "/warning/chunk_text_chars", json!("lots"));
    assert_invalid(
        "knowledge_process_chunk",
        "warning.chunk_text_chars retyped",
        &m,
    );
}

// ── find_entities / find_relationships / search_passages ────────────────────────────────────

#[tokio::test]
async fn find_entities_validates_with_and_without_provenance() {
    let (state, _db, _d, _wal) = seeded().await;
    let plain = call(
        "knowledge_find_entities",
        json!({"query": "Postgres", "group_ids": [G]}),
        &state,
    )
    .await;
    assert!(plain["count"].as_u64().unwrap() > 0, "{plain}");
    assert!(plain["nodes"][0].get("source_files").is_none());
    assert_valid("knowledge_find_entities", &plain);

    let with = call(
        "knowledge_find_entities",
        json!({"query": "Postgres", "group_ids": [G], "include_episodes": true}),
        &state,
    )
    .await;
    assert!(with["nodes"][0].get("source_files").is_some(), "{with}");
    assert_valid("knowledge_find_entities", &with);

    let none = call(
        "knowledge_find_entities",
        json!({"query": "Postgres", "group_ids": ["no-such-group"]}),
        &state,
    )
    .await;
    assert_eq!(none["count"], 0);
    assert_valid("knowledge_find_entities", &none);

    // A hit that reports episodes_truncated (#857) is the same node shape plus one optional key.
    let mut capped = with.clone();
    capped["nodes"][0]["episodes_truncated"] = json!(true);
    assert_valid("knowledge_find_entities", &capped);

    // Drift.
    let mut m = plain.clone();
    rename_key(&mut m, "/nodes/0", "support", "origins");
    assert_invalid("knowledge_find_entities", "support renamed", &m);
    let mut m = plain.clone();
    set(&mut m, "/nodes/0/corroboration", json!("2"));
    assert_invalid("knowledge_find_entities", "corroboration retyped", &m);
    let mut m = plain.clone();
    remove_key(&mut m, "/nodes/0/search", "rrf_score");
    assert_invalid("knowledge_find_entities", "search.rrf_score dropped", &m);
    let mut m = with.clone();
    set(&mut m, "/nodes/0/source_files", json!("a.md"));
    assert_invalid("knowledge_find_entities", "source_files as a string", &m);
    let mut m = plain.clone();
    rename_key(&mut m, "", "nodes", "entities");
    assert_invalid("knowledge_find_entities", "nodes renamed", &m);
}

#[tokio::test]
async fn find_relationships_validates_with_and_without_belief() {
    let (state, _db, _d, _wal) = seeded().await;
    let plain = call(
        "knowledge_find_relationships",
        json!({"query": "Postgres", "group_ids": [G]}),
        &state,
    )
    .await;
    assert!(plain["count"].as_u64().unwrap() > 0, "{plain}");
    assert!(plain["edges"][0].get("belief").is_none());
    assert_valid("knowledge_find_relationships", &plain);

    let believed = call(
        "knowledge_find_relationships",
        json!({"query": "Postgres", "group_ids": [G], "believed_only": true}),
        &state,
    )
    .await;
    assert!(believed["edges"][0].get("belief").is_some(), "{believed}");
    assert_valid("knowledge_find_relationships", &believed);

    let none = call(
        "knowledge_find_relationships",
        json!({"query": "Postgres", "group_ids": ["no-such-group"]}),
        &state,
    )
    .await;
    assert_eq!(none["count"], 0);
    assert_valid("knowledge_find_relationships", &none);

    // Drift.
    let mut m = plain.clone();
    rename_key(&mut m, "/edges/0", "valid_at", "valid_from");
    assert_invalid("knowledge_find_relationships", "valid_at renamed", &m);
    let mut m = plain.clone();
    set(&mut m, "/edges/0/fact", json!(7));
    assert_invalid("knowledge_find_relationships", "fact retyped", &m);
    let mut m = believed.clone();
    set(&mut m, "/edges/0/belief/believed", json!("yes"));
    assert_invalid(
        "knowledge_find_relationships",
        "belief.believed retyped",
        &m,
    );
    let mut m = believed.clone();
    remove_key(&mut m, "/edges/0/belief", "state");
    assert_invalid("knowledge_find_relationships", "belief.state dropped", &m);
    let mut m = plain.clone();
    remove_key(&mut m, "/edges/0/search", "fact_similarity");
    assert_invalid(
        "knowledge_find_relationships",
        "search.fact_similarity dropped",
        &m,
    );
}

#[tokio::test]
async fn search_passages_validates() {
    let (state, _db, _d, _wal) = seeded().await;
    let r = call(
        "knowledge_search_passages",
        json!({"query": "text of pg", "group_ids": [G], "min_score": 0.0}),
        &state,
    )
    .await;
    assert!(r["count"].as_u64().unwrap() > 0, "{r}");
    assert_valid("knowledge_search_passages", &r);

    // Passages carry source_file / chunk_id (#853) for episodes ingested through process_chunk.
    assert!(r["passages"][0].get("source_file").is_some(), "{r}");

    let none = call(
        "knowledge_search_passages",
        json!({"query": "text of pg", "group_ids": ["no-such-group"], "min_score": 0.0}),
        &state,
    )
    .await;
    assert_eq!(none["count"], 0);
    assert_valid("knowledge_search_passages", &none);

    // Hybrid mode adds per-leg evidence (#871); drive it for real and check the shape.
    let hybrid = call(
        "knowledge_search_passages",
        json!({"query": "text of pg", "group_ids": [G], "min_score": 0.0, "mode": "hybrid"}),
        &state,
    )
    .await;
    assert_valid("knowledge_search_passages", &hybrid);
    let legs: Vec<&str> = hybrid["passages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["legs"].as_str())
        .collect();
    assert!(!legs.is_empty(), "hybrid passages carry `legs`: {hybrid}");
    let mut m = hybrid.clone();
    set(&mut m, "/passages/0/legs", json!("sideways"));
    assert_invalid("knowledge_search_passages", "unknown legs value", &m);
    let mut m = hybrid.clone();
    set(&mut m, "/passages/0/lexical_score", json!("high"));
    assert_invalid("knowledge_search_passages", "lexical_score: string", &m);

    // The rerank / expansion reports are free-form and appear only on staged paths that need a
    // configured model; the schema's optional-object slots are checked with a sample.
    let mut staged = r.clone();
    staged["rerank"] = json!({"candidates": 3, "anything": ["goes"]});
    staged["expansion"] = json!({"variants": 2});
    assert_valid("knowledge_search_passages", &staged);
    // Both are omitted when the stage did not run; the handler never sends `null` for them.
    let mut m = r.clone();
    m["rerank"] = Value::Null;
    assert_invalid("knowledge_search_passages", "rerank: null", &m);
    let mut m = r.clone();
    m["expansion"] = Value::Null;
    assert_invalid("knowledge_search_passages", "expansion: null", &m);
    let mut m = r.clone();
    m["rerank"] = json!("not an object");
    assert_invalid("knowledge_search_passages", "rerank: string", &m);

    // Drift.
    let mut m = r.clone();
    rename_key(&mut m, "/passages/0", "score", "relevance");
    assert_invalid("knowledge_search_passages", "score renamed", &m);
    let mut m = r.clone();
    set(&mut m, "/passages/0/origin", json!(1));
    assert_invalid("knowledge_search_passages", "origin retyped", &m);
    let mut m = r.clone();
    remove_key(&mut m, "", "count");
    assert_invalid("knowledge_search_passages", "count dropped", &m);
    let mut m = r.clone();
    set(&mut m, "/passages/0/chunk_id", json!(5));
    assert_invalid("knowledge_search_passages", "chunk_id retyped", &m);
}

// ── knowledge_lint ──────────────────────────────────────────────────────────────────────────

/// Validates a lint response and returns it, asserting each category's findings were produced.
fn assert_lint(r: &Value, category: &str) -> Vec<Value> {
    assert_valid("knowledge_lint", r);
    let f: Vec<Value> = findings_of(r, category).into_iter().cloned().collect();
    assert!(
        !f.is_empty(),
        "scenario produced no {category} finding: {r}"
    );
    f
}

#[tokio::test]
async fn lint_empty_graph_and_unavailable_categories_validate() {
    let (state, _db, _d) = state_with(vec![], None);
    let r = lint(&state, json!({})).await;
    assert_eq!(r["count"], 0);
    assert_valid("knowledge_lint", &r);
    // contradiction / unlinked_mention are unavailable without configuration: summary carries a
    // reason.
    assert_eq!(
        r["summary"]["categories"]["contradiction"]["status"],
        "unavailable"
    );
    assert!(r["summary"]["categories"]["contradiction"]["reason"].is_string());
}

#[tokio::test]
async fn lint_stale_derived_page_pins_the_incident_shapes() {
    let (state, _db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres (page)"),
            fact("X", "MySQL", "X uses MySQL"),
        ],
        None,
    );
    put(&state, "note", None, Some("note"), json!({})).await;
    put(&state, "wiki", Some("derived"), Some("page"), json!({})).await;
    put(&state, "note", None, Some("note"), json!({})).await;
    let r = lint(&state, json!({"categories": ["stale_derived_page"]})).await;
    let f = assert_lint(&r, "stale_derived_page");

    // The shapes Liminis Notes guessed wrong (#852): derived_document, a causes map, and (for
    // weak support) the kind in finding.reason.
    assert_eq!(f[0]["details"]["derived_document"], "page");
    assert!(f[0]["details"]["causes"].is_object());
    assert_eq!(f[0]["reason"], "lost_corroboration");

    let idx = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .position(|x| x["category"] == "stale_derived_page")
        .unwrap();
    let base = format!("/findings/{idx}");
    let mut m = r.clone();
    rename_key(
        &mut m,
        &format!("{base}/details"),
        "derived_document",
        "derived_doc",
    );
    assert_invalid("knowledge_lint", "details.derived_document renamed", &m);
    let mut m = r.clone();
    set(
        &mut m,
        &format!("{base}/details/causes"),
        json!("document_superseded"),
    );
    assert_invalid("knowledge_lint", "details.causes as a string", &m);
    let mut m = r.clone();
    rename_key(&mut m, &format!("{base}/details"), "causes", "cause");
    assert_invalid("knowledge_lint", "details.causes renamed to cause", &m);
    let mut m = r.clone();
    set(&mut m, &format!("{base}/reason"), json!("derived_only"));
    assert_invalid(
        "knowledge_lint",
        "stale page with a weak-support reason",
        &m,
    );
    let mut m = r.clone();
    set(
        &mut m,
        &format!("{base}/details/derived_document"),
        json!(5),
    );
    assert_invalid("knowledge_lint", "derived_document retyped", &m);
    // derived_document may legitimately be null.
    let mut nullable = r.clone();
    set(
        &mut nullable,
        &format!("{base}/details/derived_document"),
        Value::Null,
    );
    assert_valid("knowledge_lint", &nullable);
}

#[tokio::test]
async fn lint_stale_page_after_document_supersession_validates() {
    let (state, _db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres (page)"),
            fact("X", "MySQL", "X uses MySQL"),
        ],
        None,
    );
    put(&state, "old", None, Some("old"), json!({})).await;
    put(&state, "wiki", Some("derived"), Some("page"), json!({})).await;
    put(&state, "new", None, Some("new"), json!({})).await;
    call(
        "knowledge_supersede_document",
        json!({
            "group_id": G, "superseding_doc_id": "new", "superseded_doc_id": "old",
            "effective_at": rfc3339(now_us() - 1_000_000),
        }),
        &state,
    )
    .await;
    let r = lint(&state, json!({"categories": ["stale_derived_page"]})).await;
    let f = assert_lint(&r, "stale_derived_page");
    assert_eq!(f[0]["details"]["causes"]["document_superseded"], 1);
}

#[tokio::test]
async fn lint_weak_support_pins_reason_and_support_tally() {
    let (state, _db, _d) = state_with(
        vec![
            fact("A", "B", "A uses B"),
            fact("C", "D", "C uses D"),
            fact("C", "D", "C uses D (2)"),
            fact("E", "F", "E uses F"),
        ],
        None,
    );
    put(&state, "c1", None, Some("d1"), json!({})).await;
    put(&state, "c2", None, Some("d2"), json!({})).await;
    put(&state, "c3", None, Some("d3"), json!({})).await;
    put(&state, "c4", Some("derived"), Some("d4"), json!({})).await;
    let r = lint(&state, json!({"categories": ["weak_support"]})).await;
    let f = assert_lint(&r, "weak_support");
    let reasons: Vec<&str> = f.iter().map(|x| x["reason"].as_str().unwrap()).collect();
    assert!(
        reasons.contains(&"single_source") && reasons.contains(&"derived_only"),
        "{reasons:?}"
    );
    // The tally is an object of counts; the kind is finding.reason, not details.support.
    assert!(f[0]["details"]["support"].is_object());

    let idx = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .position(|x| x["category"] == "weak_support")
        .unwrap();
    let base = format!("/findings/{idx}");
    let mut m = r.clone();
    set(
        &mut m,
        &format!("{base}/details/support"),
        json!("single_source"),
    );
    assert_invalid("knowledge_lint", "details.support as a kind string", &m);
    let mut m = r.clone();
    remove_key(&mut m, &base, "reason");
    assert_invalid("knowledge_lint", "weak_support without finding.reason", &m);
    let mut m = r.clone();
    set(
        &mut m,
        &format!("{base}/reason"),
        json!("lost_corroboration"),
    );
    assert_invalid("knowledge_lint", "weak_support with an unknown reason", &m);
    let mut m = r.clone();
    remove_key(&mut m, &format!("{base}/details/support"), "derived");
    assert_invalid("knowledge_lint", "support tally missing derived", &m);
}

#[tokio::test]
async fn lint_contradiction_validates() {
    let (state, _db, _d) = state_with(
        vec![
            fact("Alice", "Berlin", "Alice lives in Berlin"),
            fact("Alice", "Paris", "Alice lives in Paris"),
            fact("Alice", "Paris", "Alice lives in Paris (revised)"),
        ],
        None,
    );
    put(&state, "a", None, Some("da"), json!({})).await;
    put(&state, "b", None, Some("db"), json!({})).await;
    let r = lint(
        &state,
        json!({"categories": ["contradiction"], "single_valued_relations": ["related_to"]}),
    )
    .await;
    let f = assert_lint(&r, "contradiction");
    assert_eq!(f[0]["reason"], "single_valued_conflict");
    assert_eq!(r["summary"]["categories"]["contradiction"]["status"], "ok");

    let mut m = r.clone();
    rename_key(&mut m, "/findings/0/details", "sides", "targets");
    assert_invalid("knowledge_lint", "contradiction details.sides renamed", &m);
    let mut m = r.clone();
    set(&mut m, "/findings/0/details/sides/0/facts", json!("f1"));
    assert_invalid("knowledge_lint", "contradiction side facts retyped", &m);
}

#[tokio::test]
async fn lint_orphans_and_near_duplicates_validate() {
    let (state, _db, _d) = state_with(
        vec![
            fact("Old1", "Old2", "Old1 uses Old2"),
            fact("New1", "New2", "New1 uses New2"),
            extraction(&[
                ("Tim Cook", "Apple", "Tim Cook runs Apple"),
                ("Tim Cooke", "Apple", "Tim Cooke runs Apple"),
                ("Zed", "Apple", "Zed uses Apple"),
            ]),
        ],
        None,
    );
    put(&state, "c1", None, Some("d1"), json!({})).await;
    put(&state, "c1", None, Some("d1"), json!({})).await;
    put(&state, "c2", None, Some("d2"), json!({})).await;
    let r = lint(
        &state,
        json!({"categories": ["orphan_entity", "near_duplicate_entity"], "near_duplicate_threshold": 0.85}),
    )
    .await;
    let orphans = assert_lint(&r, "orphan_entity");
    assert!(orphans
        .iter()
        .all(|f| f["details"]["superseded_mentions"].is_u64()));
    let dups = assert_lint(&r, "near_duplicate_entity");
    assert!(
        dups[0]["details"]["embedding_cosine"].is_number()
            || dups[0]["details"]["embedding_cosine"].is_null()
    );

    let idx = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .position(|x| x["category"] == "near_duplicate_entity")
        .unwrap();
    let base = format!("/findings/{idx}/details");
    let mut m = r.clone();
    remove_key(&mut m, &base, "embedding_cosine");
    assert_invalid(
        "knowledge_lint",
        "near-duplicate without embedding_cosine",
        &m,
    );
    let mut m = r.clone();
    set(&mut m, &format!("{base}/names"), json!("a, b"));
    assert_invalid("knowledge_lint", "near-duplicate names retyped", &m);
    let oidx = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .position(|x| x["category"] == "orphan_entity")
        .unwrap();
    let mut m = r.clone();
    rename_key(
        &mut m,
        &format!("/findings/{oidx}/details"),
        "superseded_mentions",
        "mentions",
    );
    assert_invalid("knowledge_lint", "orphan details renamed", &m);
}

#[tokio::test]
async fn lint_unlinked_mention_and_missing_page_validate() {
    let (state, _db, _d) = state_with(
        vec![
            fact("Postgres", "Db", "Postgres is a Db"),
            fact("Postgres", "Svc1", "Svc1 uses Postgres"),
            fact("Postgres", "Svc2", "Svc2 uses Postgres"),
            fact("Redis", "Svc3", "Svc3 uses Redis"),
            fact("Redis", "Svc4", "Svc4 uses Redis"),
            fact("Redis", "Svc5", "Svc5 uses Redis"),
        ],
        None,
    );
    put(
        &state,
        "pg",
        None,
        Some("page-postgres"),
        json!({"page_for": "Postgres"}),
    )
    .await;
    put(&state, "s1", None, Some("svc1"), json!({"links": []})).await;
    put(
        &state,
        "s2",
        None,
        Some("svc2"),
        json!({"links": ["page-postgres"]}),
    )
    .await;
    put(&state, "s3", None, Some("svc3"), json!({})).await;
    put(&state, "s4", None, Some("svc4"), json!({})).await;
    put(&state, "s5", None, Some("svc5"), json!({})).await;
    let r = lint(&state, json!({"categories": ["unlinked_mention"]})).await;
    let f = assert_lint(&r, "unlinked_mention");
    let reasons: Vec<&str> = f.iter().map(|x| x["reason"].as_str().unwrap()).collect();
    assert!(
        reasons.contains(&"unlinked_mention") && reasons.contains(&"missing_page"),
        "{reasons:?}"
    );
    let pos = |reason: &str| {
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .position(|x| x["reason"] == reason)
            .unwrap()
    };
    let (u, mp) = (pos("unlinked_mention"), pos("missing_page"));
    let mut m = r.clone();
    rename_key(&mut m, &format!("/findings/{u}/details"), "page", "page_id");
    assert_invalid(
        "knowledge_lint",
        "unlinked_mention details.page renamed",
        &m,
    );
    let mut m = r.clone();
    rename_key(
        &mut m,
        &format!("/findings/{mp}/details"),
        "min_mentions",
        "min",
    );
    assert_invalid(
        "knowledge_lint",
        "missing_page details.min_mentions renamed",
        &m,
    );
    let mut m = r.clone();
    set(
        &mut m,
        &format!("/findings/{mp}/reason"),
        json!("not_a_reason"),
    );
    assert_invalid("knowledge_lint", "unknown unlinked_mention reason", &m);
    // `reason` is coupled to the `details` shape: a mismatched pair must fail.
    let mut m = r.clone();
    set(
        &mut m,
        &format!("/findings/{mp}/reason"),
        json!("unlinked_mention"),
    );
    assert_invalid(
        "knowledge_lint",
        "missing_page details under reason unlinked_mention",
        &m,
    );
    let mut m = r.clone();
    set(
        &mut m,
        &format!("/findings/{u}/reason"),
        json!("missing_page"),
    );
    assert_invalid(
        "knowledge_lint",
        "unlinked_mention details under reason missing_page",
        &m,
    );
}

#[tokio::test]
async fn lint_envelope_drift_is_rejected() {
    let (state, _db, _d) = state_with(vec![fact("A", "B", "A uses B")], None);
    put(&state, "c1", None, Some("d1"), json!({})).await;
    let r = lint(&state, json!({"categories": ["weak_support"], "limit": 1})).await;
    assert_valid("knowledge_lint", &r);
    let mut m = r.clone();
    remove_key(&mut m, "", "next_cursor");
    assert_invalid("knowledge_lint", "next_cursor dropped", &m);
    let mut m = r.clone();
    set(&mut m, "/summary/total", json!("1"));
    assert_invalid("knowledge_lint", "summary.total retyped", &m);
    let mut m = r.clone();
    remove_key(&mut m, "/effective", "limit");
    assert_invalid("knowledge_lint", "effective.limit dropped", &m);
    let mut m = r.clone();
    set(&mut m, "/findings/0/category", json!("invented_category"));
    assert_invalid("knowledge_lint", "unknown category", &m);
    let mut m = r.clone();
    remove_key(&mut m, "/findings/0/citations", "episodes_total");
    assert_invalid("knowledge_lint", "citations.episodes_total dropped", &m);
}

// ── additive changes stay valid (compatibility policy) ──────────────────────────────────────

#[tokio::test]
async fn additional_properties_are_tolerated() {
    let (db, _d) = make_db();
    let state = simple_state(Some(db));
    let mut h = health(&state).await;
    h["a_future_field"] = json!({"anything": true});
    assert_valid("health_check", &h);
    let mut s = call("knowledge_status", json!({}), &state).await;
    s["a_future_field"] = json!(1);
    s["wal"]["a_future_field"] = json!(1);
    assert_valid("knowledge_status", &s);
}

// ── omitted-only fields are not nullable ────────────────────────────────────────────────────

/// The handlers *omit* optional keys when absent; they never send `null`. A handler change that
/// starts emitting an explicit `null` for one is a shape change and must fail validation, rather
/// than slip through because `Option<T>` was rendered as nullable.
#[tokio::test]
async fn omitted_only_fields_reject_explicit_null() {
    let (state, _db, _d, _wal) = seeded().await;

    let with = call(
        "knowledge_find_entities",
        json!({"query": "Postgres", "group_ids": [G], "include_episodes": true}),
        &state,
    )
    .await;
    let mut m = with.clone();
    m["nodes"][0]["episodes_truncated"] = json!(true);
    assert_valid("knowledge_find_entities", &m);
    m["nodes"][0]["episodes_truncated"] = Value::Null;
    assert_invalid("knowledge_find_entities", "episodes_truncated: null", &m);
    let mut m = with.clone();
    set(&mut m, "/nodes/0/source_files", Value::Null);
    assert_invalid("knowledge_find_entities", "source_files: null", &m);

    let believed = call(
        "knowledge_find_relationships",
        json!({"query": "Postgres", "group_ids": [G], "believed_only": true}),
        &state,
    )
    .await;
    let mut m = believed.clone();
    set(&mut m, "/edges/0/belief", Value::Null);
    assert_invalid("knowledge_find_relationships", "belief: null", &m);

    let (chunk_state, _cdb, _cd) = state_with(vec![fact("Alpha", "Beta", "Alpha uses Beta")], None);
    let chunk = call(
        "knowledge_process_chunk",
        json!({
            "chunk_text": "Alpha uses Beta", "chunk_id": "c1", "source_file": "notes/a.md",
            "group_id": G, "reference_time": "2026-08-04T00:00:00Z",
        }),
        &chunk_state,
    )
    .await;
    let mut m = chunk.clone();
    m["warning"] = Value::Null;
    assert_invalid("knowledge_process_chunk", "warning: null", &m);
    let mut m = chunk.clone();
    m["left_alone_episode_uuids"] = Value::Null;
    assert_invalid(
        "knowledge_process_chunk",
        "left_alone_episode_uuids: null",
        &m,
    );

    let status = call("knowledge_status", json!({}), &state).await;
    let mut m = status.clone();
    set(&mut m, "/wal/scope", Value::Null);
    assert_invalid("knowledge_status", "wal.scope: null", &m);
}
