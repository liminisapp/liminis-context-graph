//! Issue #755 / ADR-0755: `knowledge_lint`, the deterministic, LLM-free, read-only notebook health
//! report. DB-backed behaviour: summary/paging, stable ids (also across a WAL rebuild and unrelated
//! ingests), stale-page detection reacting to a correction and a document supersession with no
//! re-ingest, each category on a seeded notebook, read-only-ness and validation.

use std::collections::HashMap;
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
        "source_file": "notes/a.md", "group_id": G,
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

fn graph_counts(db: &Db) -> (usize, usize, usize) {
    let conn = db.connect().unwrap();
    let n = |q: &str| -> usize {
        conn.query_cypher_raw(q)
            .unwrap()
            .next()
            .map(|r| r[0].to_string().parse().unwrap())
            .unwrap_or(0)
    };
    (
        n("MATCH (e:Episodic) RETURN count(e)"),
        n("MATCH (e:Entity) RETURN count(e)"),
        n("MATCH (r:RelatesToNode_) RETURN count(r)"),
    )
}

// ── envelope: summary, paging, determinism ──────────────────────────────────────────────────

#[tokio::test]
async fn summary_matches_paged_totals_and_repeat_calls_are_identical() {
    let facts: Vec<ExtractionResult> = (0..5)
        .map(|i| fact("Hub", &format!("T{i}"), &format!("Hub uses T{i}")))
        .collect();
    let (state, _db, _d) = state_with(facts, None);
    for i in 0..5 {
        put(
            &state,
            &format!("c{i}"),
            None,
            Some(&format!("doc{i}")),
            json!({}),
        )
        .await;
    }

    let full = lint(&state, json!({"categories": ["weak_support"]})).await;
    assert_eq!(full["summary"]["categories"]["weak_support"]["count"], 5);
    assert_eq!(full["summary"]["total"], 5);
    assert_eq!(full["count"], 5);
    assert!(full["next_cursor"].is_null());
    assert_eq!(full["effective"]["near_duplicate_threshold"], 0.9);
    let again = lint(&state, json!({"categories": ["weak_support"]})).await;
    assert_eq!(
        serde_json::to_string(&full).unwrap(),
        serde_json::to_string(&again).unwrap(),
        "byte-identical on an unchanged graph"
    );

    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    let mut pages = 0;
    loop {
        let r = lint(
            &state,
            json!({"categories": ["weak_support"], "limit": 2, "cursor": cursor}),
        )
        .await;
        assert_eq!(r["summary"], full["summary"], "summary is page-independent");
        assert_eq!(r["count"], r["findings"].as_array().unwrap().len());
        seen.extend(
            r["findings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f["id"].clone()),
        );
        pages += 1;
        cursor = r["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(pages, 3);
    let want: Vec<Value> = full["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].clone())
        .collect();
    assert_eq!(seen, want, "every finding exactly once, same order");

    // A cursor replayed against another query shape is rejected.
    let r = lint(&state, json!({"categories": ["weak_support"], "limit": 2})).await;
    let c = r["next_cursor"].as_str().unwrap().to_string();
    for other in [
        json!({"categories": ["orphan_entity"], "limit": 2, "cursor": c}),
        json!({"categories": ["weak_support"], "limit": 2, "cursor": c, "group_ids": ["elsewhere"]}),
    ] {
        let v = try_call("knowledge_lint", other, &state).await;
        assert!(
            v["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("invalid cursor")),
            "{v}"
        );
    }
}

#[tokio::test]
async fn empty_unknown_group_and_validation() {
    let (state, _db, _d) = state_with(vec![], None);
    let r = call("knowledge_lint", json!({"group_ids": ["nope"]}), &state).await;
    assert_eq!(r["count"], 0);
    assert_eq!(r["summary"]["total"], 0);
    assert!(r["next_cursor"].is_null());
    assert_eq!(r["summary"]["categories"]["weak_support"]["status"], "ok");
    // Contradiction needs a designation; unlinked needs page data: both `unavailable`, not 0/ok.
    assert_eq!(
        r["summary"]["categories"]["contradiction"]["status"],
        "unavailable"
    );
    assert_eq!(
        r["summary"]["categories"]["unlinked_mention"]["status"],
        "unavailable"
    );
    // Omitted group_ids reads broad.
    let r = call("knowledge_lint", json!({}), &state).await;
    assert_eq!(r["count"], 0);

    for bad in [
        json!({"categories": ["bogus"]}),
        json!({"limit": 0}),
        json!({"limit": 100000}),
        json!({"near_duplicate_threshold": 2}),
        json!({"min_mentions": 0}),
        json!({"cursor": "not-hex"}),
    ] {
        let v = try_call("knowledge_lint", bad.clone(), &state).await;
        assert!(v.get("error").is_some(), "{bad} must be rejected: {v}");
    }
}

#[tokio::test]
async fn degraded_db_fails_like_other_reads() {
    let (state, _db, _d) = state_with(vec![], None);
    state.db.store(None);
    *state.degraded_reason.lock().unwrap() = Some("test".to_string());
    let v = try_call("knowledge_lint", json!({}), &state).await;
    assert!(v.get("error").is_some(), "{v}");
    assert!(v.get("result").is_none() || v["result"].is_null());
}

// ── stale derived pages (SC-005 / FR-011) ───────────────────────────────────────────────────

#[tokio::test]
async fn stale_page_reacts_to_a_correction_without_reingest() {
    let (state, _db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres (page)"),
            fact("X", "MySQL", "X uses MySQL"), // the corrected note
        ],
        None,
    );
    put(&state, "note", None, Some("note"), json!({})).await;
    put(&state, "wiki", Some("derived"), Some("page"), json!({})).await;
    let before = lint(&state, json!({"categories": ["stale_derived_page"]})).await;
    assert_eq!(
        before["summary"]["categories"]["stale_derived_page"]["count"],
        0
    );

    // The human corrects the note: same chunk_id, new text (ADR-0705).
    put(&state, "note", None, Some("note"), json!({})).await;
    let after = lint(&state, json!({"categories": ["stale_derived_page"]})).await;
    let f = findings_of(&after, "stale_derived_page");
    assert_eq!(f.len(), 1, "{after}");
    assert_eq!(f[0]["reason"], "lost_corroboration");
    assert_eq!(f[0]["details"]["causes"]["corrected"], 1, "{}", f[0]);
    assert_eq!(f[0]["details"]["derived_document"], "page");
    let docs = f[0]["citations"]["documents"].as_array().unwrap();
    assert!(
        docs.contains(&json!("page")) && docs.contains(&json!("note")),
        "{docs:?}"
    );
    assert!(!f[0]["citations"]["facts"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn stale_page_reacts_to_document_supersession() {
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
    assert_eq!(
        lint(&state, json!({"categories": ["stale_derived_page"]})).await["summary"]["total"],
        0
    );
    let r = try_call(
        "knowledge_supersede_document",
        json!({
            "group_id": G, "superseding_doc_id": "new", "superseded_doc_id": "old",
            "effective_at": rfc3339(now_us() - 1_000_000),
        }),
        &state,
    )
    .await;
    assert!(r.get("error").is_none(), "{r}");
    let after = lint(&state, json!({"categories": ["stale_derived_page"]})).await;
    let f = findings_of(&after, "stale_derived_page");
    assert_eq!(f.len(), 1, "{after}");
    assert_eq!(
        f[0]["details"]["causes"]["document_superseded"], 1,
        "{}",
        f[0]
    );
}

#[tokio::test]
async fn stale_page_not_flagged_while_other_primary_support_is_live() {
    let (state, _db, _d) = state_with(
        vec![
            fact("X", "Postgres", "X uses Postgres"),
            fact("X", "Postgres", "X uses Postgres (second note)"),
            fact("X", "Postgres", "X uses Postgres (page)"),
            fact("X", "MySQL", "X uses MySQL"),
        ],
        None,
    );
    put(&state, "n1", None, Some("n1"), json!({})).await;
    put(&state, "n2", None, Some("n2"), json!({})).await;
    put(&state, "wiki", Some("derived"), Some("page"), json!({})).await;
    put(&state, "n1", None, Some("n1"), json!({})).await; // correct n1; n2 still backs the fact
    let r = lint(&state, json!({"categories": ["stale_derived_page"]})).await;
    assert_eq!(
        r["summary"]["categories"]["stale_derived_page"]["count"], 0,
        "{r}"
    );
}

// ── weak support, contradiction, orphans, near-duplicates, unlinked mentions ────────────────

#[tokio::test]
async fn weak_support_origin_breakdown() {
    let (state, _db, _d) = state_with(
        vec![
            fact("A", "B", "A uses B"), // one primary doc → single_source
            fact("C", "D", "C uses D"), // two primary docs → fine
            fact("C", "D", "C uses D (2)"),
            fact("E", "F", "E uses F"), // derived only
        ],
        None,
    );
    put(&state, "c1", None, Some("d1"), json!({})).await;
    put(&state, "c2", None, Some("d2"), json!({})).await;
    put(&state, "c3", None, Some("d3"), json!({})).await;
    put(&state, "c4", Some("derived"), Some("d4"), json!({})).await;
    let r = lint(&state, json!({"categories": ["weak_support"]})).await;
    let mut got: Vec<(String, String)> = findings_of(&r, "weak_support")
        .iter()
        .map(|f| {
            (
                f["reason"].as_str().unwrap().to_string(),
                f["citations"]["facts"][0]["source"]["name"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("derived_only".to_string(), "E".to_string()),
            ("single_source".to_string(), "A".to_string())
        ]
    );
    let single = findings_of(&r, "weak_support")
        .into_iter()
        .find(|f| f["reason"] == "single_source")
        .unwrap();
    assert_eq!(single["details"]["support"]["primary"], 1);
    assert!(single["citations"]["facts"][0]["relation"].is_string());
}

#[tokio::test]
async fn contradiction_needs_designation_and_follows_belief() {
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

    let r = lint(&state, json!({"categories": ["contradiction"]})).await;
    assert_eq!(
        r["summary"]["categories"]["contradiction"]["status"],
        "unavailable"
    );
    assert_eq!(
        r["summary"]["categories"]["contradiction"]["reason"],
        "no_single_valued_relations"
    );

    // Extraction records the canonical `relation_type` (here RELATED_TO); `name` is per-edge
    // text. The designation matches either, case-insensitively.
    let designated =
        json!({"categories": ["contradiction"], "single_valued_relations": ["related_to"]});
    let r = lint(&state, designated.clone()).await;
    let f = findings_of(&r, "contradiction");
    assert_eq!(f.len(), 1, "{r}");
    assert_eq!(f[0]["reason"], "single_valued_conflict");
    assert_eq!(f[0]["citations"]["facts"].as_array().unwrap().len(), 2);
    let names: Vec<&str> = f[0]["citations"]["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Alice") && names.contains(&"Berlin") && names.contains(&"Paris"));

    // A not-designated relation reports nothing.
    let r = lint(
        &state,
        json!({"categories": ["contradiction"], "single_valued_relations": ["other"]}),
    )
    .await;
    assert_eq!(r["summary"]["categories"]["contradiction"]["count"], 0);

    // Correct the Berlin note: the conflict disappears.
    put(&state, "a", None, Some("da"), json!({})).await;
    let r = lint(&state, designated).await;
    assert_eq!(
        r["summary"]["categories"]["contradiction"]["count"], 0,
        "{r}"
    );
}

#[tokio::test]
async fn orphans_and_near_duplicates() {
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
    put(&state, "c1", None, Some("d1"), json!({})).await; // correction: Old1/Old2 lose their support
    put(&state, "c2", None, Some("d2"), json!({})).await;

    let r = lint(&state, json!({"categories": ["orphan_entity", "near_duplicate_entity"], "near_duplicate_threshold": 0.85})).await;
    let orphans = findings_of(&r, "orphan_entity");
    let mut names: Vec<&str> = orphans
        .iter()
        .map(|f| f["citations"]["entities"][0]["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Old1", "Old2"], "{r}");
    assert!(orphans.iter().all(|f| f["reason"] == "support_lost"));
    assert!(orphans
        .iter()
        .all(|f| !f["citations"]["episodes"].as_array().unwrap().is_empty()));

    let dups = findings_of(&r, "near_duplicate_entity");
    assert_eq!(dups.len(), 1, "{r}");
    let d = dups[0];
    assert_eq!(d["reason"], "similar_name");
    assert!(d["details"]["score"].as_f64().unwrap() >= 0.85);
    assert_eq!(d["citations"]["entities"].as_array().unwrap().len(), 2);
    assert_eq!(r["effective"]["near_duplicate_threshold"], 0.85);
    // Raising the threshold shrinks the set.
    let r = lint(
        &state,
        json!({"categories": ["near_duplicate_entity"], "near_duplicate_threshold": 0.95}),
    )
    .await;
    assert_eq!(
        r["summary"]["categories"]["near_duplicate_entity"]["count"],
        0
    );
}

#[tokio::test]
async fn unlinked_mentions_and_missing_pages_from_attributes_convention() {
    let (state, _db, _d) = state_with(
        vec![
            fact("Postgres", "Db", "Postgres is a Db"), // the page for Postgres
            fact("Postgres", "Svc1", "Svc1 uses Postgres"),
            fact("Postgres", "Svc2", "Svc2 uses Postgres"), // linked
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
    assert_eq!(
        r["summary"]["categories"]["unlinked_mention"]["status"], "ok",
        "{r}"
    );
    let f = findings_of(&r, "unlinked_mention");
    let unlinked: Vec<_> = f
        .iter()
        .filter(|x| x["reason"] == "unlinked_mention")
        .collect();
    assert_eq!(unlinked.len(), 1, "{r}");
    let docs = unlinked[0]["citations"]["documents"].as_array().unwrap();
    assert!(
        docs.contains(&json!("svc1")) && docs.contains(&json!("page-postgres")),
        "{docs:?}"
    );
    let missing: Vec<_> = f.iter().filter(|x| x["reason"] == "missing_page").collect();
    assert_eq!(missing.len(), 1, "{r}");
    assert_eq!(missing[0]["citations"]["entities"][0]["name"], "Redis");
}

// ── read-only, stable ids, no model calls ───────────────────────────────────────────────────

#[tokio::test]
async fn lint_leaves_graph_and_wal_unchanged_and_needs_no_model() {
    let wal = TempDir::new().unwrap();
    // An empty extractor queue after seeding: any extraction call during lint would error.
    let (state, db, _d) = state_with(
        vec![
            fact("A", "B", "A uses B"),
            fact("A", "B", "A uses B (page)"),
        ],
        Some(wal.path().to_path_buf()),
    );
    put(&state, "c1", None, Some("d1"), json!({})).await;
    put(&state, "c2", Some("derived"), Some("d2"), json!({})).await;
    let counts = graph_counts(&db);
    let bytes = wal_bytes(wal.path());
    let r = lint(&state, json!({"single_valued_relations": ["x"]})).await;
    assert!(r["summary"]["total"].as_u64().unwrap() >= 1);
    assert_eq!(graph_counts(&db), counts);
    assert_eq!(wal_bytes(wal.path()), bytes);
}

#[tokio::test]
async fn ids_survive_unrelated_ingest_and_wal_rebuild() {
    let wal = TempDir::new().unwrap();
    let (state, _db, _d) = state_with(
        vec![
            fact("A", "B", "A uses B"),
            fact("C", "D", "C uses D"),
            fact("Unrelated", "Other", "Unrelated uses Other"),
        ],
        Some(wal.path().to_path_buf()),
    );
    put(&state, "c1", None, Some("d1"), json!({})).await;
    put(&state, "c2", Some("derived"), Some("d2"), json!({})).await;
    let cats = json!({"categories": ["weak_support"]});
    let ids = |r: &Value| -> Vec<String> {
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap().to_string())
            .collect()
    };
    let before = ids(&lint(&state, cats.clone()).await);
    assert_eq!(before.len(), 2);

    // Unrelated ingest adds a finding but leaves the existing ids alone.
    put(&state, "c3", None, Some("d3"), json!({})).await;
    let after = ids(&lint(&state, cats.clone()).await);
    assert_eq!(after.len(), 3);
    for id in &before {
        assert!(
            after.contains(id),
            "id {id} changed after an unrelated ingest"
        );
    }

    // A WAL rebuild reproduces the same ids (uuids replay verbatim).
    let (rstate, _rdb, _rd) = replayed(wal.path()).await;
    let rebuilt = ids(&lint(&rstate, cats).await);
    let mut a = after.clone();
    a.sort();
    let mut b = rebuilt.clone();
    b.sort();
    assert_eq!(a, b, "ids must survive a WAL rebuild");
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

#[tokio::test]
async fn scan_chunk_boundaries_lose_and_duplicate_nothing() {
    let facts: Vec<ExtractionResult> = (0..6)
        .map(|i| {
            fact(
                &format!("S{i}"),
                &format!("T{i}"),
                &format!("S{i} uses T{i}"),
            )
        })
        .collect();
    let (state, db, _d) = state_with(facts, None);
    for i in 0..6 {
        put(
            &state,
            &format!("c{i}"),
            None,
            Some(&format!("d{i}")),
            json!({}),
        )
        .await;
    }
    let conn = db.connect().unwrap();
    let req = lcg_core::lint::LintRequest::parse(&json!({"limit": 500}), None).unwrap();
    let now = now_us();
    let reference = lcg_core::lint::run_lint(&conn, &req, now).unwrap();
    for chunk in [1, 2, 5, 7] {
        let r = lcg_core::lint::run_lint_with_chunk(&conn, &req, now, chunk).unwrap();
        assert_eq!(r, reference, "chunk size {chunk} changed the report");
    }
    assert_eq!(
        reference["summary"]["categories"]["weak_support"]["count"],
        6
    );
}

/// SC-004 probe: a full pass at 10× the episodes should cost no more than about 12× the time (probed here at 5×).
/// Timing-sensitive, so it never runs by default; `cargo test -p lcg-core --test knowledge_lint
/// --release -- --ignored --nocapture` prints the measured ratio. `benches/lint.rs` is the
/// canonical measurement.
#[path = "../benches/common/lint_fixture.rs"]
mod lint_fixture;

#[test]
#[ignore = "timing probe for SC-004; run explicitly"]
fn lint_full_pass_scales_near_linearly() {
    let time_at = |n: usize| {
        let (db, _dir) = lint_fixture::seed(n);
        let req = lcg_core::lint::LintRequest::parse(
            &json!({"limit": 500, "single_valued_relations": ["related_to"]}),
            Some(vec![lint_fixture::GROUP.to_string()]),
        )
        .unwrap();
        let conn = db.connect().unwrap();
        let now = now_us();
        let start = std::time::Instant::now();
        let r = lcg_core::lint::run_lint(&conn, &req, now).unwrap();
        let elapsed = start.elapsed();
        eprintln!(
            "n={n}: {elapsed:?}, total findings {}",
            r["summary"]["total"]
        );
        elapsed
    };
    let small = time_at(1_000);
    let large = time_at(5_000);
    let ratio = large.as_secs_f64() / small.as_secs_f64();
    eprintln!("5x episodes -> {ratio:.1}x time");
    // Near-linear would be ~5x; 10x the size may cost at most ~12x, so 5x may cost ~6x.
    assert!(ratio < 12.0, "ratio {ratio:.1} is far from near-linear");
}

/// An entity anchored only by a cross-group edge keyed on another group (ADR-0615) is not an
/// orphan, whether lint is scoped to its group or not.
#[tokio::test]
async fn cross_group_edge_keeps_scoped_entity_from_being_an_orphan() {
    let (state, db, _d) = state_with(vec![], None);
    {
        let conn = db.connect().unwrap();
        for (uuid, name, group) in [("hub", "hub", "layer"), ("w1", "widget", "other")] {
            conn.insert_entity(&lcg_core::EntityRow {
                uuid: uuid.into(),
                name: name.into(),
                group_id: group.into(),
                labels: vec!["Entity".into()],
                created_at: "2026-01-01 00:00:00".into(),
                name_embedding: vec![1.0, 0.0, 0.0, 0.0],
                summary: "s".into(),
                attributes: "{}".into(),
                ..Default::default()
            })
            .unwrap();
        }
    }
    call(
        "knowledge_add_cross_group_edge",
        json!({
            "name": "REFERENCES", "group_id": "layer", "source": {"uuid": "hub"},
            "target": {"source_group_id": "other", "endpoint_name": "widget"},
            "fact": "hub references widget",
        }),
        &state,
    )
    .await;

    for scope in [
        json!(["other"]),
        json!(["layer"]),
        json!(["layer", "other"]),
    ] {
        let r = call(
            "knowledge_lint",
            json!({"group_ids": scope, "categories": ["orphan_entity"]}),
            &state,
        )
        .await;
        assert_eq!(
            r["summary"]["categories"]["orphan_entity"]["count"], 0,
            "scope {scope}: {r}"
        );
    }
}
