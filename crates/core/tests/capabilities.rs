//! `knowledge_capabilities` and the version fields on `health_check` (issue #854, ADR-0854).

mod common;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    capabilities,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::MockExtractor,
    handlers,
    ipc::IpcRequest,
    telemetry::{NoopSink, TelemetrySink},
    write_policy::WritableGroups,
};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// A degraded state (no DB loaded): capabilities needs no database.
fn degraded_state() -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(None),
        degraded_reason: Arc::new(Mutex::new(Some("corrupt wal".to_string()))),
        embedder: Arc::new(MockEmbedder::new(4)),
        extractor: Arc::new(MockExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: None,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
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

async fn call(
    state: &Arc<AppState>,
    policy: Option<&WritableGroups>,
    method: &str,
    params: Value,
) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    let fut = handlers::dispatch_with_policy(req, Arc::clone(state), None, policy);
    let resp = tokio::time::timeout(Duration::from_secs(2), fut)
        .await
        .expect("must answer promptly");
    serde_json::to_value(resp).unwrap()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn reports_version_protocol_and_seeded_features() {
    let state = degraded_state();
    let v = call(&state, None, "knowledge_capabilities", json!({})).await;
    let r = &v["result"];
    assert_eq!(r["server_version"], env!("CARGO_PKG_VERSION"), "{v}");
    assert_eq!(r["protocol_version"], 1, "{v}");
    let features = strings(&r["features"]);
    for f in [
        "process_document",
        "retire_document",
        "chunk_supersede",
        "document_max_bytes_env",
        "walk",
        "walk_provenance",
        "origin",
        "lint",
        "writable_groups",
        "hybrid_passage_search",
        "entity_provenance",
        "status_episode_breakdown",
        "unchanged_noop",
        "search_min_score_env",
        "ready_for_reads",
        "test_providers",
        "exact_source_match",
        "episode_source_fields",
        "passage_legs",
        "implicit_default_group_warning",
        "zero_vector_record",
        "chunk_progress",
        "cancel_on_disconnect",
    ] {
        assert!(features.iter().any(|x| x == f), "missing feature {f}");
    }
}

#[tokio::test]
async fn features_and_methods_are_sorted_and_duplicate_free() {
    let state = degraded_state();
    let v = call(&state, None, "knowledge_capabilities", json!({})).await;
    for key in ["features", "methods"] {
        let list = strings(&v["result"][key]);
        let mut expect = list.clone();
        expect.sort();
        expect.dedup();
        assert_eq!(list, expect, "{key} must be sorted and duplicate-free");
    }
}

/// FR-017 / Story 4: every dispatched method is advertised, and nothing else is.
#[tokio::test]
async fn methods_match_the_dispatcher_in_both_directions() {
    let state = degraded_state();
    let v = call(&state, None, "knowledge_capabilities", json!({})).await;
    let advertised: BTreeSet<String> = strings(&v["result"]["methods"]).into_iter().collect();
    let dispatched = common::dispatched_methods();
    assert!(dispatched.len() > 40, "dispatch extraction broken");
    let missing: Vec<_> = dispatched.difference(&advertised).collect();
    let stale: Vec<_> = advertised.difference(&dispatched).collect();
    assert!(
        missing.is_empty(),
        "dispatcher accepts methods missing from `methods`: {missing:?}"
    );
    assert!(
        stale.is_empty(),
        "`methods` names methods the dispatcher does not accept: {stale:?}"
    );
    assert!(advertised.contains("health_check"));
    assert!(advertised.contains("knowledge_capabilities"));
}

/// SC-004: answers in degraded mode, with extra params ignored, and under a write policy.
#[tokio::test]
async fn answers_in_degraded_mode_ignores_params_and_passes_write_policy() {
    let state = degraded_state();
    let v = call(&state, None, "knowledge_capabilities", json!({"bogus": 1})).await;
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["result"]["protocol_version"], 1);

    let policy = WritableGroups::parse("a").unwrap();
    let v = call(&state, Some(&policy), "knowledge_capabilities", json!({})).await;
    assert!(v.get("error").is_none(), "refused under write policy: {v}");
}

/// SC-004: answers promptly while a writer holds the write lock.
#[tokio::test]
async fn answers_while_the_write_lock_is_held() {
    let state = degraded_state();
    let _w = state.write_lock.write().await;
    let v = call(&state, None, "knowledge_capabilities", json!({})).await;
    assert_eq!(v["result"]["server_version"], capabilities::SERVER_VERSION);
}

/// SC-005 (degraded shape): `health_check` carries the two new fields next to the old ones.
#[tokio::test]
async fn health_check_degraded_carries_version_fields() {
    let state = degraded_state();
    let v = call(&state, None, "health_check", json!({})).await;
    let r = &v["result"];
    assert_eq!(r["state"], "degraded");
    assert_eq!(r["reason"], "corrupt wal");
    assert_eq!(r["server_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["protocol_version"], 1);
}

// ── registry vs docs/ipc-mcp-reference.md (issue #885) ────────────────────────────────────────

const REFERENCE: &str = include_str!("../../../docs/ipc-mcp-reference.md");

/// Rows of the "Feature strings" table: `(feature, introduced cell)`.
fn documented_features() -> Vec<(String, String)> {
    let start = REFERENCE
        .find("#### Feature strings")
        .expect("`#### Feature strings` missing from docs/ipc-mcp-reference.md");
    let section = &REFERENCE[start + 4..];
    let end = section.find("\n#").unwrap_or(section.len());
    section[..end]
        .lines()
        .filter_map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            let f = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
            Some((f.to_string(), cells.get(2)?.to_string()))
        })
        .collect()
}

#[test]
fn reference_lists_every_feature_and_nothing_else() {
    let rows = documented_features();
    let documented: BTreeSet<String> = rows.iter().map(|(f, _)| f.clone()).collect();
    assert_eq!(
        documented.len(),
        rows.len(),
        "a feature is documented twice"
    );
    let registry: BTreeSet<String> = capabilities::FEATURES
        .iter()
        .map(|f| f.to_string())
        .collect();
    let undocumented: Vec<_> = registry.difference(&documented).collect();
    let stale: Vec<_> = documented.difference(&registry).collect();
    assert!(
        undocumented.is_empty(),
        "feature strings missing from the `#### Feature strings` table in \
         docs/ipc-mcp-reference.md: {undocumented:?}"
    );
    assert!(
        stale.is_empty(),
        "docs/ipc-mcp-reference.md documents feature strings not in FEATURES: {stale:?}"
    );
}

#[test]
fn reference_rows_cite_an_introducing_issue() {
    for (f, introduced) in documented_features() {
        let cited = introduced
            .split('#')
            .skip(1)
            .any(|t| t.starts_with(|c: char| c.is_ascii_digit()));
        assert!(
            cited,
            "feature `{f}` has no `#<issue>` in its Introduced cell"
        );
    }
}

#[test]
fn reference_example_response_shows_every_feature() {
    let start = REFERENCE
        .find("{\"server_version\"")
        .expect("example capabilities response missing");
    let example = &REFERENCE[start..start + REFERENCE[start..].find("```").unwrap()];
    for f in capabilities::FEATURES {
        assert!(
            example.contains(&format!("\"{f}\"")),
            "example `knowledge_capabilities` response in docs/ipc-mcp-reference.md omits `{f}`"
        );
    }
}
