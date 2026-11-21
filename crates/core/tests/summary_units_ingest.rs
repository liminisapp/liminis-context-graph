//! Issue #850: summary-unit generation at ingest, with `LCG_SUMMARY_UNITS=on` for this whole
//! binary. One unit per episode with cleaned text, failure handling (a provider error leaves no
//! unit; a cassette miss fails the ingest), revision, the WAL line, delete/purge cleanup, and a
//! dump → replay round trip that rebuilds the units with no LLM call.

mod summary_support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use lcg_core::{Db, Error, WalReplayer};
use serde_json::json;
use summary_support::*;
use tempfile::TempDir;

const T1: &str = "2026-03-01T00:00:00Z";
const T2: &str = "2026-04-01T00:00:00Z";

#[tokio::test]
async fn one_clean_summary_unit_per_episode() {
    enable_summary_units();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let f = fixture_with(
        Some(Arc::new(move |body: &str| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(format!(
                "## Summary\nTopics: kilns\nThis note describes {}.",
                body.to_uppercase()
            ))
        })),
        Arc::clone(&calls),
    );
    let ep1 = put(
        &f.state,
        G,
        "kiln",
        "notes on kiln drying of timber",
        None,
        T1,
    )
    .await;
    let ep2 = put(
        &f.state,
        G,
        "owl",
        "field notes about an owl at dusk",
        None,
        T1,
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let units = units(&f.state);
    assert_eq!(units.len(), 2);
    let by_ep: std::collections::HashMap<&str, &str> = units
        .iter()
        .map(|(_, ep, _, content)| (ep.as_str(), content.as_str()))
        .collect();
    assert_eq!(
        by_ep[ep1.as_str()],
        "kilns This note describes NOTES ON KILN DRYING OF TIMBER.",
        "the heading line is dropped and the label removed; the label's text is kept"
    );
    assert!(by_ep.contains_key(ep2.as_str()));
    for (_, _, gid, content) in &units {
        assert_eq!(gid, G);
        for label in ["Summary", "Topics:", "#"] {
            assert!(!content.contains(label), "{label} in {content:?}");
        }
    }

    // The unit is a WAL line in the same group stream, with its vector stripped.
    let wal = wal_text(f.wal_dir.path());
    let lines: Vec<&str> = wal.lines().filter(|l| l.contains("SummaryUnit")).collect();
    assert_eq!(lines.len(), 2, "{wal}");
    for l in lines {
        assert!(l.contains("CREATE (:SummaryUnit"));
        let v: serde_json::Value = serde_json::from_str(l).unwrap();
        assert!(v["params"].get("content_embedding").is_none(), "{l}");
        assert!(v["params"]["content"]
            .as_str()
            .unwrap()
            .contains("This note describes"));
    }
}

#[tokio::test]
async fn summary_units_are_not_episodes_anywhere_else() {
    enable_summary_units();
    let f = fixture();
    put(
        &f.state,
        G,
        "kiln",
        "notes on kiln drying of timber",
        None,
        T1,
    )
    .await;
    let eps = call(
        "knowledge_get_episodes",
        json!({"group_ids": [G], "last_n": 10}),
        &f.state,
    )
    .await;
    let eps = eps["episodes"].as_array().unwrap();
    assert_eq!(eps.len(), 1);
    assert!(!eps[0].to_string().contains("THIS NOTE DESCRIBES"));
    let status = call("knowledge_status", json!({}), &f.state).await;
    assert_eq!(status["episode_count"], 1, "{status}");
}

#[tokio::test]
async fn a_short_body_gets_no_summarize_call() {
    enable_summary_units();
    let f = fixture();
    put(&f.state, G, "tiny", "ok", None, T1).await;
    assert_eq!(f.summarize_calls.load(Ordering::SeqCst), 0);
    assert!(units(&f.state).is_empty());
}

#[tokio::test]
async fn a_provider_failure_ingests_the_episode_without_a_unit() {
    enable_summary_units();
    let calls = Arc::new(AtomicUsize::new(0));
    let f = fixture_with(
        Some(Arc::new(|_: &str| {
            Err(Error::Ipc("summarize HTTP 500".into()))
        })),
        calls,
    );
    let ep = put(
        &f.state,
        G,
        "kiln",
        "notes on kiln drying of timber",
        None,
        T1,
    )
    .await;
    assert!(!ep.is_empty());
    assert!(units(&f.state).is_empty());
    let status = call("knowledge_status", json!({}), &f.state).await;
    assert_eq!(status["episode_count"], 1);

    // An empty reply is a failure too, not an empty unit.
    let f = fixture_with(Some(Arc::new(|_: &str| Ok("  \n".into()))), Arc::default());
    put(
        &f.state,
        G,
        "kiln",
        "notes on kiln drying of timber",
        None,
        T1,
    )
    .await;
    assert!(units(&f.state).is_empty());
}

/// Extraction fails at once; summarize never returns. Stands in for a hung or slow provider.
struct FailingExtractHungSummarize;

impl lcg_core::extractor::Extractor for FailingExtractHungSummarize {
    fn extract<'a>(
        &'a self,
        _opts: lcg_core::extractor::ExtractOptions<'a>,
    ) -> futures::future::BoxFuture<'a, Result<lcg_core::types::ExtractionOutcome, Error>> {
        Box::pin(async { Err(Error::Ipc("extract HTTP 500".into())) })
    }

    fn classify_entities<'a>(
        &'a self,
        _entities: &'a [(&'a str, &'a str)],
        _allowed_types: Option<&'a [String]>,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async { Ok(vec![]) })
    }

    fn classify_relations<'a>(
        &'a self,
        _edges: &'a [(&'a str, &'a str)],
        _allowed_types: &'a [(String, Option<String>)],
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async { Ok(vec![]) })
    }

    fn summarize<'a>(
        &'a self,
        _body: &'a str,
        _chunk_key: Option<&'a str>,
    ) -> futures::future::BoxFuture<'a, Result<String, Error>> {
        Box::pin(futures::future::pending())
    }
}

#[tokio::test]
async fn an_extraction_failure_does_not_wait_for_the_summarize_call() {
    enable_summary_units();
    let f = fixture_with_extractor(Arc::new(FailingExtractHungSummarize), Arc::default());
    let params = json!({
        "chunk_text": "notes on kiln drying of timber", "chunk_id": "kiln",
        "source_file": "notes/kiln.md", "group_id": G, "reference_time": T1,
    });
    // With the summary awaited beside (not inside) the failing pair, this never returns.
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        try_call("knowledge_process_chunk", params, &f.state),
    )
    .await
    .expect("an extraction failure must not wait for an in-flight summarize call");
    assert!(r.get("error").is_some(), "{r}");
    assert!(units(&f.state).is_empty());
}

#[tokio::test]
async fn a_cassette_miss_fails_the_ingest() {
    enable_summary_units();
    let f = fixture_with(
        Some(Arc::new(|_: &str| {
            Err(Error::CassetteMiss(
                "no cassette record for summarize call with key abc".into(),
            ))
        })),
        Arc::default(),
    );
    let v = try_call(
        "knowledge_process_chunk",
        json!({"chunk_text": "notes on kiln drying of timber", "chunk_id": "kiln",
               "source_file": "notes/kiln.md", "group_id": G, "reference_time": T1}),
        &f.state,
    )
    .await;
    let err = v["error"].to_string();
    assert!(err.contains("summarize"), "{v}");
    let status = call("knowledge_status", json!({}), &f.state).await;
    assert_eq!(status["episode_count"], 0, "nothing was written");
}

#[tokio::test]
async fn a_revision_writes_a_new_unit_and_keeps_the_old_one() {
    enable_summary_units();
    let f = fixture();
    let old = put(
        &f.state,
        G,
        "kiln",
        "notes on kiln drying of timber",
        None,
        T1,
    )
    .await;
    let new = put(
        &f.state,
        G,
        "kiln",
        "revised notes on kiln drying of oak",
        None,
        T2,
    )
    .await;
    assert_ne!(old, new);
    let eps: Vec<String> = units(&f.state).into_iter().map(|u| u.1).collect();
    assert_eq!(eps.len(), 2);
    assert!(eps.contains(&old) && eps.contains(&new));
}

#[tokio::test]
async fn deletes_and_purge_remove_the_units() {
    enable_summary_units();
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
    put(
        &f.state,
        G,
        "tide",
        "a log of the tide tables for May",
        None,
        T1,
    )
    .await;
    put(
        &f.state,
        OTHER,
        "garden",
        "planting plan for the garden beds",
        None,
        T1,
    )
    .await;
    assert_eq!(units(&f.state).len(), 4);

    call(
        "knowledge_delete_episode",
        json!({"episode_uuid": a}),
        &f.state,
    )
    .await;
    assert_eq!(units(&f.state).len(), 3);
    call(
        "knowledge_delete_chunk_episode",
        json!({"chunk_id": "owl", "group_ids": [G]}),
        &f.state,
    )
    .await;
    assert_eq!(units(&f.state).len(), 2);
    call(
        "knowledge_delete_by_source",
        json!({"source_file": "notes/tide.md", "group_ids": [G]}),
        &f.state,
    )
    .await;
    let left = units(&f.state);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].2, OTHER);
    call(
        "knowledge_delete_by_group",
        json!({"group_ids": [OTHER], "confirm": true}),
        &f.state,
    )
    .await;
    assert!(units(&f.state).is_empty());
}

#[tokio::test]
async fn dump_then_replay_rebuilds_units_without_an_llm_call() {
    enable_summary_units();
    let f = fixture();
    let ep = put(
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
    let before = units(&f.state);
    assert_eq!(before.len(), 2);

    let out = TempDir::new().unwrap();
    let target = out.path().join("dump");
    let r = call(
        "knowledge_dump_wal",
        json!({"target_dir": target.to_str().unwrap()}),
        &f.state,
    )
    .await;
    assert_eq!(r["success"], true, "{r}");

    // Replay into a fresh database. The recompute callback is the only "model" available, and it
    // is an embedder, not an extractor: nothing can summarize here.
    let dir2 = TempDir::new().unwrap();
    let db2 = Db::open(dir2.path().join("r.db").to_str().unwrap()).unwrap();
    let conn = db2.connect().unwrap();
    conn.init_schema(DIM).unwrap();
    WalReplayer::new(&target)
        .replay(
            &conn,
            Box::new(|texts: &[&str]| Ok(texts.iter().map(|t| vector_for(t)).collect())),
            DIM,
        )
        .unwrap();
    conn.create_vector_indexes().unwrap();
    let mut after: Vec<(String, String, String, String)> = conn
        .cypher_query("MATCH (s:SummaryUnit) RETURN s.uuid, s.episode_uuid, s.group_id, s.content")
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone(), r[2].clone(), r[3].clone()))
        .collect();
    after.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(after, before);

    // Searchable: the recomputed vector puts the KILN summary nearest a KILN query.
    let hits = conn
        .vector_search_summary_units(&vector_for("KILN"), Some(&[G]), 1, 10)
        .unwrap();
    assert_eq!(hits[0].episode_uuid, ep);
}
