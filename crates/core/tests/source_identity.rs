//! Issue #853 / ADR-0853 (implements #829, #830): episode source identity.
//!
//! The chunk and document ingest paths store `source_file` and `chunk_id` separately from the
//! composed display value `source_description = "{source_file}:{chunk_id}"`, and the two
//! source-scoped operations — `knowledge_delete_by_source` and `knowledge_get_entities_by_source`
//! — match on that identity exactly. Legacy rows (no stored `source_file`) fall back to the safe
//! rule in `lcg_core::source_identity`, which may under-match but never matches another source.

use std::collections::{BTreeSet, HashMap};
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
    ConfigurableExtractor, Db, EpisodicRow, ExtractedEntity, ExtractionResult, WalReplayer,
    WalWriter,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const G: &str = "liminis";
const G2: &str = "other";

// ── Fixture ──────────────────────────────────────────────────────────────────

/// One extraction per entry, each yielding the named entities (no edges). Entity names are kept
/// distinct across sources so dedup never merges two sources' entities by accident.
fn make_state(
    wal_dir: Option<std::path::PathBuf>,
    extractions: &[&[&str]],
) -> (Arc<AppState>, Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
    }
    let writers: HashMap<String, WalWriter> = wal_dir
        .as_ref()
        .and_then(|d| WalWriter::new(d, 10_000, 5 * 1024 * 1024).ok())
        .map(|w| (G.to_string(), w))
        .into_iter()
        .collect();
    let queue: Vec<ExtractionResult> = extractions.iter().map(|names| ents(names)).collect();
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(Arc::clone(&db))),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor: Arc::new(ConfigurableExtractor::new(queue)),
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
    });
    (state, db, dir)
}

fn ents(names: &[&str]) -> ExtractionResult {
    ExtractionResult {
        entities: names
            .iter()
            .map(|n| ExtractedEntity {
                name: n.to_string(),
                entity_type: "Thing".to_string(),
                summary: format!("{n} summary"),
                original_entity_type: None,
            })
            .collect(),
        edges: vec![],
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

/// Issue #858: an identical resubmission is a no-op, so each submission gets distinct text; a
/// repeat of a `chunk_id` here is then a genuine correction, as these tests intend.
fn next_submission() -> usize {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

async fn chunk_in(state: &Arc<AppState>, group: &str, source_file: &str, chunk_id: &str) -> String {
    let r = call(
        "knowledge_process_chunk",
        json!({
            "chunk_text": format!("text of {source_file} {chunk_id} (submission {})", next_submission()),
            "chunk_id": chunk_id,
            "source_file": source_file,
            "group_id": group,
            "reference_time": "2026-11-01T00:00:00Z",
        }),
        state,
    )
    .await;
    assert_eq!(r["success"], true, "{r}");
    r["episode_uuid"].as_str().unwrap().to_string()
}

async fn chunk(state: &Arc<AppState>, source_file: &str, chunk_id: &str) -> String {
    chunk_in(state, G, source_file, chunk_id).await
}

async fn delete_by_source(state: &Arc<AppState>, source_file: &str, groups: &[&str]) -> Value {
    call(
        "knowledge_delete_by_source",
        json!({"source_file": source_file, "group_ids": groups}),
        state,
    )
    .await
}

async fn entity_names_by_source(state: &Arc<AppState>, source: &str) -> BTreeSet<String> {
    let r = call(
        "knowledge_get_entities_by_source",
        json!({"source": source}),
        state,
    )
    .await;
    r["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap().to_string())
        .collect()
}

fn names(xs: &[&str]) -> BTreeSet<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

/// Every episode, by uuid.
fn episodes(db: &Db) -> HashMap<String, EpisodicRow> {
    db.connect()
        .unwrap()
        .retrieve_episodes_page(None, 1000, None, None)
        .unwrap()
        .into_iter()
        .map(|(e, _)| (e.uuid.clone(), e))
        .collect()
}

/// A legacy row (no stored `source_file`), as a pre-#853 release or a raw `add_episode` left it.
fn legacy_row(db: &Db, uuid: &str, name: &str, source_description: &str, write_path: Option<&str>) {
    db.connect()
        .unwrap()
        .insert_episodic(&EpisodicRow {
            uuid: uuid.into(),
            name: name.into(),
            group_id: G.into(),
            created_at: "2026-01-01 00:00:00".into(),
            source: "text".into(),
            source_description: source_description.into(),
            content: "legacy".into(),
            content_embedding: vec![0.0; DIM],
            valid_at: "2026-01-01 00:00:00".into(),
            attributes: "{}".into(),
            write_path: write_path.map(str::to_string),
            ..Default::default()
        })
        .unwrap();
}

fn replayed(source: &std::path::Path) -> (Db, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Db::open(dir.path().join("r.db").to_str().unwrap()).unwrap();
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        WalReplayer::new(source)
            .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
            .unwrap();
    }
    (db, dir)
}

fn identity(db: &Db) -> HashMap<String, (Option<String>, Option<String>, String)> {
    episodes(db)
        .into_values()
        .map(|e| (e.uuid, (e.source_file, e.chunk_id, e.source_description)))
        .collect()
}

// ── Write path (FR-001..FR-003) ──────────────────────────────────────────────

/// US3 AC1: a URL with a port is stored exactly, and the display value is unchanged.
#[tokio::test]
async fn process_chunk_stores_source_file_and_chunk_id() {
    let (state, db, _d) = make_state(None, &[&["A"]]);
    let uuid = chunk(&state, "https://a.com:8443/x", "c1").await;
    let ep = &episodes(&db)[&uuid];
    assert_eq!(ep.source_file.as_deref(), Some("https://a.com:8443/x"));
    assert_eq!(ep.chunk_id.as_deref(), Some("c1"));
    assert_eq!(ep.source_description, "https://a.com:8443/x:c1");
}

/// FR-002: document ingest stamps the same identity; the chunk ID is the minted
/// `{doc_id}::{hex}` name, which contains colons.
#[tokio::test]
async fn process_document_stores_source_identity() {
    let (state, db, _d) = make_state(None, &[&["DocEnt"], &["DocEnt2"]]);
    let r = call(
        "knowledge_process_document",
        json!({
            "doc_id": "doc-1",
            "group_id": G,
            "content": "# Title\n\nSome text about a thing.\n",
            "title": "Design note",
            "source_path": "https://a.com:8443/doc",
            "reference_time": "2026-11-01T00:00:00Z",
        }),
        &state,
    )
    .await;
    assert!(r.get("revision_id").is_some(), "{r}");
    let eps = episodes(&db);
    assert!(!eps.is_empty());
    for ep in eps.values() {
        assert_eq!(ep.source_file.as_deref(), Some("https://a.com:8443/doc"));
        let cid = ep.chunk_id.as_deref().unwrap();
        assert_eq!(cid, ep.name);
        assert!(cid.starts_with("doc-1::"), "{cid}");
        assert_eq!(
            ep.source_description,
            format!("https://a.com:8443/doc:{cid}")
        );
    }
}

/// Raw `knowledge_add_episode` takes no structured identity: its episode stays a legacy row.
#[tokio::test]
async fn add_episode_stays_legacy() {
    let (state, db, _d) = make_state(None, &[&["A"]]);
    let r = call(
        "knowledge_add_episode",
        json!({"name": "ep", "episode_body": "body", "group_id": G,
               "source_description": "doc.md:section1",
               "reference_time": "2026-11-01T00:00:00Z"}),
        &state,
    )
    .await;
    let ep = &episodes(&db)[r["episode_uuid"].as_str().unwrap()];
    assert_eq!(ep.source_file, None);
    assert_eq!(ep.chunk_id, None);
}

// ── WAL and dump (FR-009, FR-010, SC-004) ────────────────────────────────────

/// US4 AC1: a rebuild from the WAL, and from a dumped/compacted WAL, restores both fields —
/// and a legacy row's NULLs stay NULL rather than becoming empty strings.
#[tokio::test]
async fn source_identity_survives_wal_replay_and_dump() {
    let wal = TempDir::new().unwrap();
    let (state, db, _d) = make_state(Some(wal.path().to_path_buf()), &[&["A"], &["B"]]);
    chunk(&state, "https://a.com:8443/x", "c1").await;
    chunk(&state, "notes/today.md", "n:1").await;
    call(
        "knowledge_add_episode",
        json!({"name": "raw", "episode_body": "raw body", "group_id": G,
               "source_description": "doc.md", "reference_time": "2026-11-01T00:00:00Z"}),
        &state,
    )
    .await;
    let expected = identity(&db);
    assert_eq!(expected.len(), 3);

    let (db2, _d2) = replayed(wal.path());
    assert_eq!(identity(&db2), expected, "WAL rebuild");

    let out = TempDir::new().unwrap();
    let target = out.path().join("dump");
    let r = call(
        "knowledge_dump_wal",
        json!({"target_dir": target.to_str().unwrap()}),
        &state,
    )
    .await;
    assert_eq!(r["success"], true, "{r}");
    let (db3, _d3) = replayed(&target);
    assert_eq!(identity(&db3), expected, "dumped-WAL rebuild");
}

/// US4 AC2 / FR-010: a WAL written before #853 (no `source_file`/`chunk_id` in the `CREATE`
/// template or its params) replays, and its episodes are legacy rows that the legacy rule still
/// attributes to their source.
#[tokio::test]
async fn pre_change_wal_replays_as_legacy() {
    let wal = TempDir::new().unwrap();
    let (state, _db, _d) = make_state(Some(wal.path().to_path_buf()), &[&["A"]]);
    let uuid = chunk(&state, "https://a.com:8443/x", "c1").await;
    let mut stack = vec![wal.path().to_path_buf()];
    let mut stripped_any = false;
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            stack.extend(std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
        } else if let Ok(text) = std::fs::read_to_string(&p) {
            let stripped = text
                .replace(", source_file: $source_file, chunk_id: $chunk_id", "")
                .replace(r#","source_file":"https://a.com:8443/x""#, "")
                .replace(r#""source_file":"https://a.com:8443/x","#, "")
                .replace(r#","chunk_id":"c1""#, "")
                .replace(r#""chunk_id":"c1","#, "");
            stripped_any |= stripped != text;
            assert!(
                !stripped.contains("source_file"),
                "strip incomplete: {stripped}"
            );
            std::fs::write(&p, stripped).unwrap();
        }
    }
    assert!(stripped_any, "the WAL must have carried the new fields");
    let (db2, _d2) = replayed(wal.path());
    let ep = &episodes(&db2)[&uuid];
    assert_eq!(ep.source_file, None);
    assert_eq!(ep.chunk_id, None);
    assert_eq!(ep.source_description, "https://a.com:8443/x:c1");
}

// ── delete_by_source (FR-004, FR-005, FR-008, SC-001, SC-006) ────────────────

/// US1 AC1 / SC-001: deleting `https://a.com` never deletes `https://a.com:8443/x`.
#[tokio::test]
async fn delete_by_source_does_not_delete_a_colon_prefixed_url() {
    let (state, db, _d) = make_state(None, &[&["A1"], &["B1"], &["B2"]]);
    let a = chunk(&state, "https://a.com", "a-1").await;
    let b1 = chunk(&state, "https://a.com:8443/x", "b-1").await;
    let b2 = chunk(&state, "https://a.com:8443/x", "b-2").await;
    let r = delete_by_source(&state, "https://a.com", &[G]).await;
    assert_eq!(r["deleted_count"], 1, "{r}");
    assert_eq!(r["deleted_uuids"], json!([a]));
    let left = episodes(&db);
    assert!(left.contains_key(&b1) && left.contains_key(&b2));
    assert!(!left.contains_key(&a));
}

/// US1 AC2 and AC4: a multi-chunk non-URL source is fully deleted; a string-prefix sibling is not.
#[tokio::test]
async fn delete_by_source_full_source_and_prefix_sibling() {
    let (state, db, _d) = make_state(None, &[&["T1"], &["T2"], &["A"], &["AB"]]);
    chunk(&state, "notes/today.md", "t-1").await;
    chunk(&state, "notes/today.md", "t-2").await;
    chunk(&state, "notes/a", "a-1").await;
    let ab = chunk(&state, "notes/ab", "ab-1").await;
    let r = delete_by_source(&state, "notes/today.md", &[G]).await;
    assert_eq!(r["deleted_count"], 2, "{r}");
    let r = delete_by_source(&state, "notes/a", &[G]).await;
    assert_eq!(r["deleted_count"], 1, "{r}");
    let left = episodes(&db);
    assert_eq!(left.len(), 1);
    assert!(left.contains_key(&ab));
}

/// US1 AC3 / SC-006: legacy rows are deleted for their own source and never for another one —
/// URLs with ports, chunk IDs containing colons, prefix-related sources, raw `add_episode` rows.
#[tokio::test]
async fn delete_by_source_legacy_rows_never_over_match() {
    let (state, db, _d) = make_state(None, &[]);
    // (uuid, name, source_description, write_path) — the source each belongs to in a comment.
    legacy_row(&db, "l-port", "c1", "https://a.com:8443/x:c1", None); // https://a.com:8443/x
    legacy_row(
        &db,
        "l-chunk",
        "c2",
        "https://a.com:8443/x:c2",
        Some("chunk"),
    ); // same
    legacy_row(&db, "l-colon", "sec:1", "notes/a.md:sec:1", Some("chunk")); // notes/a.md
    legacy_row(
        &db,
        "l-doc",
        "d::0123",
        "https://a.com:d::0123",
        Some("document"),
    ); // https://a.com
    legacy_row(&db, "l-ep", "ep-a2", "doc.md:section1", Some("episode")); // doc.md:section1
    legacy_row(&db, "l-exact", "free", "doc.md", None); // doc.md
    legacy_row(&db, "l-trap", "8443", "https://a.com:8443", None); // https://a.com
    for wrong in [
        "https://a.com:8443",
        "https://a.com:8443/x:c1",
        "notes/a.md:sec",
        "notes/a",
        "https://a.com:d:",
        "https://a",
        "doc",
    ] {
        let r = delete_by_source(&state, wrong, &[G]).await;
        assert_eq!(r["deleted_count"], 0, "{wrong} must delete nothing: {r}");
    }
    assert_eq!(episodes(&db).len(), 7);

    let deleted = |src: &'static str| {
        let state = Arc::clone(&state);
        async move {
            let r = delete_by_source(&state, src, &[G]).await;
            let mut v: Vec<String> = r["deleted_uuids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|u| u.as_str().unwrap().to_string())
                .collect();
            v.sort();
            v
        }
    };
    assert_eq!(deleted("doc.md").await, vec!["l-exact"]);
    assert_eq!(deleted("https://a.com").await, vec!["l-doc", "l-trap"]);
    assert_eq!(
        deleted("https://a.com:8443/x").await,
        vec!["l-chunk", "l-port"]
    );
    assert_eq!(deleted("notes/a.md").await, vec!["l-colon"]);
    assert_eq!(deleted("doc.md:section1").await, vec!["l-ep"]);
    assert!(episodes(&db).is_empty());
}

/// Edge case: new and legacy rows of the same source are both deleted.
#[tokio::test]
async fn delete_by_source_covers_new_and_legacy_rows_together() {
    let (state, db, _d) = make_state(None, &[&["A"]]);
    let new = chunk(&state, "https://a.com", "n-1").await;
    legacy_row(&db, "old", "o-1", "https://a.com:o-1", None);
    let r = delete_by_source(&state, "https://a.com", &[G]).await;
    assert_eq!(r["deleted_count"], 2, "{r}");
    let mut got: Vec<String> = r["deleted_uuids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u.as_str().unwrap().to_string())
        .collect();
    got.sort();
    let mut want = vec![new, "old".to_string()];
    want.sort();
    assert_eq!(got, want);
}

/// FR-008: the caller's group scope is honoured.
#[tokio::test]
async fn delete_by_source_honours_group_scope() {
    let (state, db, _d) = make_state(None, &[&["A"], &["B"]]);
    chunk_in(&state, G, "https://a.com", "a-1").await;
    let other = chunk_in(&state, G2, "https://a.com", "a-2").await;
    let r = delete_by_source(&state, "https://a.com", &[G]).await;
    assert_eq!(r["deleted_count"], 1, "{r}");
    assert!(episodes(&db).contains_key(&other));
}

/// Edge case: delete-by-source still removes the source's superseded episodes.
#[tokio::test]
async fn delete_by_source_deletes_superseded_episodes_too() {
    let (state, db, _d) = make_state(None, &[&["V1"], &["V2"]]);
    let v1 = chunk(&state, "https://a.com", "a-1").await;
    let v2 = chunk(&state, "https://a.com", "a-1").await;
    assert!(
        episodes(&db)[&v1].superseded_at.is_some(),
        "re-ingest supersedes"
    );
    let r = delete_by_source(&state, "https://a.com", &[G]).await;
    assert_eq!(r["deleted_count"], 2, "{r}");
    let left = episodes(&db);
    assert!(!left.contains_key(&v1) && !left.contains_key(&v2));
}

#[tokio::test]
async fn delete_by_source_still_rejects_empty_source() {
    let (state, _db, _d) = make_state(None, &[]);
    let v = try_call(
        "knowledge_delete_by_source",
        json!({"source_file": "", "group_ids": [G]}),
        &state,
    )
    .await;
    assert!(v.get("error").is_some(), "{v}");
}

// ── get_entities_by_source (FR-006, FR-007, SC-002, SC-003) ──────────────────

/// US2 AC1, AC2 / SC-002: `https://a.com/p` returns only `/p`'s entities.
#[tokio::test]
async fn entities_by_source_is_exact() {
    let (state, _db, _d) = make_state(
        None,
        &[&["Pee"], &["Page"], &["Query"], &["Sub"], &["PortP"]],
    );
    chunk(&state, "https://a.com/p", "p-1").await;
    chunk(&state, "https://a.com/page", "page-1").await;
    chunk(&state, "https://a.com/p?x=1", "q-1").await;
    chunk(&state, "https://a.com/p/sub", "s-1").await;
    chunk(&state, "https://a.com:8443/p", "port-1").await;
    assert_eq!(
        entity_names_by_source(&state, "https://a.com/p").await,
        names(&["Pee"])
    );
    assert_eq!(
        entity_names_by_source(&state, "https://a.com/page").await,
        names(&["Page"])
    );
    assert!(entity_names_by_source(&state, "https://a.com")
        .await
        .is_empty());
    assert!(entity_names_by_source(&state, "a.com/p").await.is_empty());
}

/// US2 AC2: an entity a variant source shares with a live `/p` episode is still returned.
#[tokio::test]
async fn entities_by_source_returns_a_shared_entity_through_the_live_source() {
    let (state, _db, _d) = make_state(None, &[&["Shared", "OnlyP"], &["Shared", "OnlyPage"]]);
    chunk(&state, "https://a.com/p", "p-1").await;
    chunk(&state, "https://a.com/page", "page-1").await;
    assert_eq!(
        entity_names_by_source(&state, "https://a.com/p").await,
        names(&["Shared", "OnlyP"])
    );
}

/// US2 AC3, AC4 / SC-003: superseded episodes do not count; an entity a live episode still
/// supports does.
#[tokio::test]
async fn entities_by_source_counts_live_episodes_only() {
    let (state, _db, _d) = make_state(None, &[&["Old", "Kept"], &["New", "Kept"], &["Other"]]);
    chunk(&state, "https://a.com/p", "p-1").await;
    // Re-ingesting `p-1` supersedes its first episode.
    chunk(&state, "https://a.com/p", "p-1").await;
    chunk(&state, "https://a.com/p", "p-2").await;
    assert_eq!(
        entity_names_by_source(&state, "https://a.com/p").await,
        names(&["New", "Kept", "Other"])
    );
    // Marking `p-2` removed supersedes its only episode, so `Other` drops out too.
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "p-2", "group_ids": [G]}),
        &state,
    )
    .await;
    assert_eq!(
        entity_names_by_source(&state, "https://a.com/p").await,
        names(&["New", "Kept"])
    );
}

/// Live also excludes a pending (never committed) document revision's episodes.
#[tokio::test]
async fn entities_by_source_ignores_uncommitted_document_revisions() {
    let (state, db, _d) = make_state(None, &[&["Committed"]]);
    chunk(&state, "https://a.com/doc", "c-1").await;
    let conn = db.connect().unwrap();
    conn.insert_episodic(&EpisodicRow {
        uuid: "pending".into(),
        name: "d::1".into(),
        group_id: G.into(),
        created_at: "2026-01-01 00:00:00".into(),
        source: "text".into(),
        source_description: "https://a.com/doc:d::1".into(),
        content: "pending".into(),
        content_embedding: vec![0.0; DIM],
        valid_at: "2026-01-01 00:00:00".into(),
        attributes: "{}".into(),
        write_path: Some("document".into()),
        revision_id: Some("rev-1".into()),
        source_file: Some("https://a.com/doc".into()),
        chunk_id: Some("d::1".into()),
        ..Default::default()
    })
    .unwrap();
    conn.run_cypher("CREATE (:Entity {uuid: 'pe', name: 'PendingOnly', group_id: 'liminis'})")
        .unwrap();
    conn.run_cypher(
        "MATCH (ep:Episodic {uuid: 'pending'}), (e:Entity {uuid: 'pe'}) \
         CREATE (ep)-[:MENTIONS {group_id: 'liminis'}]->(e)",
    )
    .unwrap();
    assert_eq!(
        entity_names_by_source(&state, "https://a.com/doc").await,
        names(&["Committed"])
    );
}

/// Legacy rows are matched by the same safe rule as delete-by-source.
#[tokio::test]
async fn entities_by_source_matches_legacy_rows_safely() {
    let (state, db, _d) = make_state(None, &[]);
    legacy_row(&db, "l-port", "c1", "https://a.com:8443/x:c1", None);
    let conn = db.connect().unwrap();
    conn.run_cypher("CREATE (:Entity {uuid: 'le', name: 'Legacy', group_id: 'liminis'})")
        .unwrap();
    conn.run_cypher(
        "MATCH (ep:Episodic {uuid: 'l-port'}), (e:Entity {uuid: 'le'}) \
         CREATE (ep)-[:MENTIONS {group_id: 'liminis'}]->(e)",
    )
    .unwrap();
    assert!(entity_names_by_source(&state, "https://a.com")
        .await
        .is_empty());
    assert!(entity_names_by_source(&state, "https://a.com:8443")
        .await
        .is_empty());
    assert_eq!(
        entity_names_by_source(&state, "https://a.com:8443/x").await,
        names(&["Legacy"])
    );
}

/// FR-008: the optional group filter still applies to the entity's group.
#[tokio::test]
async fn entities_by_source_honours_group_filter() {
    let (state, _db, _d) = make_state(None, &[&["InG"], &["InG2"]]);
    chunk_in(&state, G, "https://a.com/p", "p-1").await;
    chunk_in(&state, G2, "https://a.com/p", "p-2").await;
    let r = call(
        "knowledge_get_entities_by_source",
        json!({"source": "https://a.com/p", "group_ids": [G2]}),
        &state,
    )
    .await;
    let got: BTreeSet<String> = r["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(got, names(&["InG2"]));
    assert_eq!(
        entity_names_by_source(&state, "https://a.com/p").await,
        names(&["InG", "InG2"])
    );
}

/// `LIMIT` applies after the source filter: many non-matching candidate rows (legacy rows whose
/// description starts with `src:` but belong to other sources) cannot under-fill the result.
#[tokio::test]
async fn entities_by_source_limit_applies_after_filtering() {
    let (state, db, _d) = make_state(None, &[&["E1", "E2", "E3"]]);
    let conn = db.connect().unwrap();
    for i in 0..20 {
        let name = format!("{i}/y:c");
        let uuid = format!("decoy-{i}");
        // Belongs to `https://a.com:{i}/y`, but is a candidate for `https://a.com`.
        legacy_row(
            &db,
            &uuid,
            "c",
            &format!("https://a.com:{name}"),
            Some("chunk"),
        );
        conn.run_cypher(&format!(
            "CREATE (:Entity {{uuid: 'de-{i}', name: 'Decoy{i}', group_id: 'liminis'}})"
        ))
        .unwrap();
        conn.run_cypher(&format!(
            "MATCH (ep:Episodic {{uuid: '{uuid}'}}), (e:Entity {{uuid: 'de-{i}'}}) \
             CREATE (ep)-[:MENTIONS {{group_id: 'liminis'}}]->(e)"
        ))
        .unwrap();
    }
    chunk(&state, "https://a.com", "real-1").await;
    let r = call(
        "knowledge_get_entities_by_source",
        json!({"source": "https://a.com", "num_results": 3}),
        &state,
    )
    .await;
    assert_eq!(r["count"], 3, "{r}");
}

// ── Response fields (FR-013) ─────────────────────────────────────────────────

/// US3 AC1, AC3: get-episodes carries the identity, is projectable, and omits it on legacy rows.
#[tokio::test]
async fn get_episodes_reports_source_identity() {
    let (state, db, _d) = make_state(None, &[&["A"]]);
    let uuid = chunk(&state, "https://a.com:8443/x", "c1").await;
    legacy_row(&db, "legacy", "c9", "https://a.com:c9", None);
    let r = call(
        "knowledge_get_episodes",
        json!({"group_ids": [G], "last_n": 10}),
        &state,
    )
    .await;
    let items = r["episodes"].as_array().expect("episodes array");
    let new = items.iter().find(|e| e["uuid"] == uuid.as_str()).unwrap();
    assert_eq!(new["source_file"], "https://a.com:8443/x");
    assert_eq!(new["chunk_id"], "c1");
    assert_eq!(new["source_description"], "https://a.com:8443/x:c1");
    let old = items.iter().find(|e| e["uuid"] == "legacy").unwrap();
    assert!(old.get("source_file").is_none(), "{old}");
    assert!(old.get("chunk_id").is_none(), "{old}");

    let r = call(
        "knowledge_get_episodes",
        json!({"group_ids": [G], "last_n": 10, "fields": ["uuid", "source_file", "chunk_id"]}),
        &state,
    )
    .await;
    let items = r["episodes"].as_array().expect("episodes array");
    let new = items.iter().find(|e| e["uuid"] == uuid.as_str()).unwrap();
    assert_eq!(new["source_file"], "https://a.com:8443/x");
    assert_eq!(new["chunk_id"], "c1");
}

/// US3 AC2: search-passages carries the identity.
#[tokio::test]
async fn search_passages_reports_source_identity() {
    let (state, _db, _d) = make_state(None, &[&["A"]]);
    let uuid = chunk(&state, "https://a.com:8443/x", "c1").await;
    let r = call(
        "knowledge_search_passages",
        json!({"query": "text", "group_ids": [G], "min_score": 0.0, "num_results": 10}),
        &state,
    )
    .await;
    let passages = r["passages"].as_array().expect("passages array");
    let p = passages
        .iter()
        .find(|p| p["uuid"] == uuid.as_str())
        .unwrap_or_else(|| panic!("passage returned: {r}"));
    assert_eq!(p["source_file"], "https://a.com:8443/x");
    assert_eq!(p["chunk_id"], "c1");
}

// ── Upgrade (FR-011, SC-005) ─────────────────────────────────────────────────

/// The `Episodic` DDL of v0.19.1, verbatim from `git show v0.19.1:crates/core/src/schema.rs`
/// (dimension inlined).
const V0191_EPISODIC_DDL: &str = "CREATE NODE TABLE IF NOT EXISTS Episodic (\
     uuid STRING PRIMARY KEY, name STRING, group_id STRING, created_at TIMESTAMP, \
     source STRING, source_description STRING, content STRING, content_embedding FLOAT[4], \
     valid_at TIMESTAMP, entity_edges STRING[], attributes STRING, ingested_at TIMESTAMP, \
     superseded_at TIMESTAMP, superseded_by STRING, superseded_by_kind STRING, \
     supersession_reason STRING, doc_id STRING, origin STRING, write_path STRING)";

/// v0.19.1's `insert_episodic` `CREATE` template (from `git show v0.19.1:crates/core/src/db.rs`),
/// with its parameters inlined as literals.
fn v0191_insert(uuid: &str, name: &str, source_description: &str) -> String {
    format!(
        "CREATE (:Episodic {{uuid: '{uuid}', name: '{name}', group_id: '{G}', \
         created_at: timestamp('2026-01-01'), source: 'text', \
         source_description: '{source_description}', content: 'c', \
         content_embedding: [0.0, 0.0, 0.0, 0.0], valid_at: timestamp('2026-01-01'), \
         entity_edges: [], attributes: '{{}}', ingested_at: timestamp('2026-01-01'), \
         superseded_at: NULL, superseded_by: NULL, superseded_by_kind: NULL, \
         supersession_reason: NULL, doc_id: NULL, origin: NULL, write_path: 'chunk'}})"
    )
}

/// v0.19.1's `retrieve_episodes_page` read (no filters).
const V0191_RETRIEVE: &str = "MATCH (ep:Episodic) \
     RETURN ep.uuid, ep.name, ep.group_id, ep.created_at, ep.source, \
     ep.source_description, ep.content, ep.valid_at, ep.entity_edges, ep.attributes, \
     ep.ingested_at, ep.superseded_at, ep.superseded_by, ep.superseded_by_kind, \
     ep.supersession_reason, ep.doc_id, ep.origin \
     ORDER BY ep.created_at DESC, ep.uuid DESC LIMIT 100";

/// US4 AC3, AC4 / SC-005: the upgrade test. Reproduces the v0.19.1 open/operate path in-tree
/// (the spec's assumption: no downloaded release binary in CI). The lbug pin is unchanged since
/// v0.19.1, so the storage format version is identical; what this checks is the schema layer.
///
/// 1. A database in the v0.19.1 layout, holding a v0.19.1-written row, opens under this release:
///    the migration adds the two columns as NULL and changes nothing else.
/// 2. This release writes a new-template row.
/// 3. v0.19.1's own statements — its `CREATE TABLE IF NOT EXISTS`, insert template, episode read
///    and delete-by-source match — all still bind and run against the upgraded database,
///    ignoring the new columns.
#[tokio::test]
async fn v0_19_1_database_upgrades_and_stays_readable_by_v0_19_1() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("old.db");
    {
        let db = Db::open(path.to_str().unwrap()).unwrap();
        let conn = db.connect().unwrap();
        conn.run_cypher(V0191_EPISODIC_DDL).unwrap();
        conn.run_cypher(&v0191_insert("old-1", "c1", "https://a.com:8443/x:c1"))
            .unwrap();
        assert!(
            conn.run_cypher("MATCH (n:Episodic) RETURN n.source_file LIMIT 0")
                .is_err(),
            "precondition: the v0.19.1 layout has no source_file"
        );
    }
    // This release opens it.
    let db = Db::open(path.to_str().unwrap()).unwrap();
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        let rows = conn
            .cypher_query(
                "MATCH (n:Episodic {uuid: 'old-1'}) RETURN n.source_file IS NULL, \
                 n.chunk_id IS NULL, n.source_description, n.write_path",
            )
            .unwrap();
        assert_eq!(
            rows[0],
            vec!["True", "True", "https://a.com:8443/x:c1", "chunk"]
        );
        let new_row = EpisodicRow {
            uuid: "new-1".into(),
            name: "c2".into(),
            group_id: G.into(),
            created_at: "2026-01-02 00:00:00".into(),
            source: "text".into(),
            source_description: "https://a.com:c2".into(),
            content: "c".into(),
            content_embedding: vec![0.0; DIM],
            valid_at: "2026-01-02 00:00:00".into(),
            attributes: "{}".into(),
            write_path: Some("chunk".into()),
            source_file: Some("https://a.com".into()),
            chunk_id: Some("c2".into()),
            ..Default::default()
        };
        conn.insert_episodic(&new_row).unwrap();
        // Deleting `https://a.com` takes the new row and leaves the upgraded legacy row (which
        // the legacy rule attributes to `https://a.com:8443/x`) alone.
        let (deleted, _) = conn
            .remove_episodes_by_source("https://a.com", &[G])
            .unwrap();
        assert_eq!(deleted, vec!["new-1".to_string()]);
        conn.insert_episodic(&new_row).unwrap();
    }
    drop(db);

    // v0.19.1 opens the upgraded database and operates on it.
    let db = Db::open(path.to_str().unwrap()).unwrap();
    let conn = db.connect().unwrap();
    conn.run_cypher(V0191_EPISODIC_DDL)
        .expect("v0.19.1's IF NOT EXISTS DDL is a no-op on the upgraded table");
    conn.run_cypher(&v0191_insert("old-2", "c3", "notes/a.md:c3"))
        .expect("v0.19.1's insert template binds, leaving the new columns NULL");
    let rows = conn.cypher_query(V0191_RETRIEVE).unwrap();
    let uuids: BTreeSet<String> = rows.iter().map(|r| r[0].clone()).collect();
    assert_eq!(uuids, names(&["old-1", "old-2", "new-1"]));
    let rows = conn
        .cypher_query(&format!(
            "MATCH (ep:Episodic) WHERE (ep.source_description = 'notes/a.md' \
             OR ep.source_description STARTS WITH 'notes/a.md:') AND ep.group_id IN ['{G}'] \
             RETURN ep.uuid"
        ))
        .expect("v0.19.1's delete-by-source match binds");
    assert_eq!(rows, vec![vec!["old-2".to_string()]]);
}
