//! Issue #850, FR-002 / SC-001: with `LCG_SUMMARY_UNITS` unset (the default) ingest makes no
//! summarize call, stores no summary unit and writes no `SummaryUnit` WAL line, and the delete and
//! purge paths add no `SummaryUnit` line either. This binary never enables the switch.

mod summary_support;

use std::sync::atomic::Ordering;

use serde_json::json;
use summary_support::*;

const T1: &str = "2026-03-01T00:00:00Z";

#[tokio::test]
async fn off_by_default_no_call_no_rows_no_wal_lines() {
    assert!(std::env::var("LCG_SUMMARY_UNITS").is_err());
    let f = fixture();
    let a = put(
        &f.state,
        G,
        "kiln",
        "notes on kiln drying of timber",
        None,
        T1,
    )
    .await;
    put(
        &f.state,
        G,
        "owl",
        "field notes about an owl at dusk",
        None,
        T1,
    )
    .await;
    assert_eq!(f.summarize_calls.load(Ordering::SeqCst), 0);
    assert!(units(&f.state).is_empty());

    call(
        "knowledge_delete_episode",
        json!({"episode_uuid": a}),
        &f.state,
    )
    .await;
    call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "owl", "group_ids": [G]}),
        &f.state,
    )
    .await;
    put(
        &f.state,
        G,
        "tide",
        "a log of the tide tables for May",
        None,
        T1,
    )
    .await;
    call(
        "knowledge_delete_by_source",
        json!({"source_file": "notes/tide.md", "group_ids": [G]}),
        &f.state,
    )
    .await;
    call(
        "knowledge_delete_by_group",
        json!({"group_ids": [G], "confirm": true}),
        &f.state,
    )
    .await;

    let wal = wal_text(f.wal_dir.path());
    assert!(!wal.is_empty());
    assert!(!wal.contains("SummaryUnit"), "{wal}");

    // A summaries search on a workspace with no units is an ordinary search plus a `no_units`
    // block, never an error.
    put(
        &f.state,
        G,
        "kiln",
        "notes on kiln drying of timber",
        None,
        T1,
    )
    .await;
    let r = call(
        "knowledge_search_passages",
        json!({"query": "KILN", "group_ids": [G], "min_score": 0.0, "summaries": true}),
        &f.state,
    )
    .await;
    assert_eq!(r["summaries"]["applied"], false, "{r}");
    assert_eq!(r["summaries"]["reason"], "no_units");
    assert_eq!(r["count"], 1);
}
