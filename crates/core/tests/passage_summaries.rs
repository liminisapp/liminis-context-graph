//! Issue #850: summary units as candidates in `knowledge_search_passages`, end to end through
//! `dispatch`. Summary generation is on for this whole binary; each test's summarizer upper-cases
//! the note body, so the keyword embedder sees a summary's keywords while its passage (lower
//! case) matches nothing — the paraphrase case.

mod summary_support;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use lcg_core::{
    query_expand::{MockQueryExpander, QueryVariant, VariantKind},
    rerank::MockReranker,
    search_stages::{SearchStages, SearchStagesConfig},
};
use serde_json::{json, Value};
use summary_support::*;

const T1: &str = "2026-03-01T00:00:00Z";
const T2: &str = "2026-04-01T00:00:00Z";

async fn search(f: &Fixture, extra: Value) -> Value {
    let mut params =
        json!({"query": "KILN", "group_ids": [G], "min_score": 0.0, "num_results": 10});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    call("knowledge_search_passages", params, &f.state).await
}

async fn search_err(f: &Fixture, extra: Value) -> String {
    let mut params = json!({"query": "KILN", "group_ids": [G], "min_score": 0.0});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    let v = try_call("knowledge_search_passages", params, &f.state).await;
    assert!(v.get("error").is_some(), "expected an error: {v}");
    v["error"].to_string()
}

fn passages(r: &Value) -> &Vec<Value> {
    r["passages"].as_array().unwrap()
}

fn uuids_via(r: &Value) -> Vec<(String, Option<String>)> {
    passages(r)
        .iter()
        .map(|p| {
            (
                p["uuid"].as_str().unwrap().to_string(),
                p["via"].as_str().map(str::to_string),
            )
        })
        .collect()
}

fn via_count(r: &Value) -> usize {
    passages(r).iter().filter(|p| p["via"] == "summary").count()
}

/// The kiln note (matches `KILN` only through its summary) plus three fillers.
async fn corpus(f: &Fixture) -> String {
    enable_summary_units();
    let kiln = put(
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
        G,
        "garden",
        "planting plan for the garden beds",
        None,
        T1,
    )
    .await;
    kiln
}

fn install(f: &Fixture, reranker: Option<MockReranker>, expander: Option<MockQueryExpander>) {
    f.state.set_search_stages(SearchStages::new(
        SearchStagesConfig::default(),
        reranker.map(|r| Arc::new(r) as Arc<dyn lcg_core::rerank::Reranker>),
        expander.map(|e| Arc::new(e) as Arc<dyn lcg_core::query_expand::QueryExpander>),
    ));
}

/// Drops the wall-clock `elapsed_ms` from every stage block (`rerank`, `expansion`, …).
/// Two otherwise identical calls can straddle a millisecond tick, so the timing is the
/// one field a byte-identity comparison must not include; everything else is compared.
fn without_timings(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut() {
        for block in obj.values_mut() {
            if let Some(b) = block.as_object_mut() {
                b.remove("elapsed_ms");
            }
        }
    }
    v
}

#[tokio::test]
async fn without_the_flag_every_mode_is_unchanged() {
    let f = fixture();
    corpus(&f).await;
    install(&f, Some(MockReranker::new(|_, _| 0.0)), None);
    for base in [
        json!({}),
        json!({"belief_aware": false}),
        json!({"mode": "hybrid"}),
        json!({"queries": ["owl at dusk"]}),
        json!({"rerank": true}),
    ] {
        let plain = search(&f, base.clone()).await;
        for flag in [json!(false), Value::Null] {
            let mut with = base.clone();
            with["summaries"] = flag.clone();
            // A stray `max_summary_slots` is not even validated when summaries are off.
            with["max_summary_slots"] = json!("ignored");
            assert_eq!(
                without_timings(search(&f, with).await),
                without_timings(plain.clone()),
                "{base} summaries={flag}"
            );
        }
        assert!(plain.get("summaries").is_none());
        assert_eq!(via_count(&plain), 0);
        assert!(!plain.to_string().contains("\"via\""));
    }
}

#[tokio::test]
async fn without_the_flag_the_response_matches_the_direct_search() {
    let f = fixture();
    corpus(&f).await;
    let r = search(&f, json!({})).await;
    let direct = lcg_core::search::search_passages_vector_belief(
        f.state.db.load_full().unwrap(),
        Arc::clone(&f.state.embedder),
        "KILN",
        Some(vec![G.to_string()]),
        10,
        0.0,
        None,
    )
    .await
    .unwrap();
    assert_eq!(r["passages"], serde_json::to_value(&direct).unwrap());
}

#[tokio::test]
async fn a_paraphrase_only_summary_brings_in_its_notes_passage() {
    let f = fixture();
    let kiln = corpus(&f).await;
    let before = f.summarize_calls.load(Ordering::SeqCst);

    let off = search(&f, json!({"num_results": 1})).await;
    assert_ne!(passages(&off)[0]["uuid"], kiln.as_str());

    let r = search(&f, json!({"summaries": true, "num_results": 3})).await;
    let first = &passages(&r)[0];
    assert_eq!(first["uuid"], kiln.as_str());
    assert_eq!(first["via"], "summary");
    assert_eq!(first["content"], "notes on kiln drying of timber");
    assert_eq!(first["name"], "kiln");
    assert!(first["score"].as_f64().unwrap() > 0.99);
    assert!(!r.to_string().contains("THIS NOTE DESCRIBES"), "{r}");
    assert_eq!(r["summaries"]["applied"], true);
    assert!(r["summaries"]["candidates"].as_u64().unwrap() >= 1);
    assert_eq!(r["summaries"]["results"], 1);
    assert_eq!(passages(&r).len(), 3);
    // Results that did not come in through a summary carry no `via`.
    assert!(passages(&r)[1..].iter().all(|p| p.get("via").is_none()));
    // No LLM call on the read path (ADR-0820): the summarizer was not called by the search.
    assert_eq!(f.summarize_calls.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn a_note_hit_directly_and_through_its_summary_appears_once() {
    enable_summary_units();
    let f = fixture();
    let k = put(&f.state, G, "kiln", "KILN drying notes", None, T1).await;
    put(
        &f.state,
        G,
        "owl",
        "field notes about an owl at dusk",
        None,
        T1,
    )
    .await;
    let r = search(&f, json!({"summaries": true})).await;
    let ids = uuids_via(&r);
    assert_eq!(ids.iter().filter(|(u, _)| *u == k).count(), 1, "{r}");
    // The passage tied the summary and ranks first, so it holds the slot on its own.
    assert_eq!(ids[0], (k, None));
}

#[tokio::test]
async fn a_superseded_notes_summary_ranks_below_its_successor() {
    enable_summary_units();
    let f = fixture();
    // Both versions' summaries match KILN; the stale one matches better (cosine 1 vs 0.71).
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
        "kiln drying now uses a rocket stove",
        None,
        T2,
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

    let pos = |r: &Value, u: &str| {
        uuids_via(r)
            .iter()
            .position(|(x, _)| x == u)
            .unwrap_or_else(|| panic!("{u} missing from {r}"))
    };

    let r = search(&f, json!({"summaries": true})).await;
    assert!(pos(&r, &new) < pos(&r, &old), "first stage: {r}");

    // With rerank, even a cross-encoder that loves the stale summary cannot lift it.
    install(
        &f,
        Some(MockReranker::new(|_, d| {
            if d.contains("TIMBER") {
                10.0
            } else {
                -10.0
            }
        })),
        None,
    );
    let r = search(&f, json!({"summaries": true, "rerank": true})).await;
    assert_eq!(r["rerank"]["applied"], true, "{r}");
    assert!(pos(&r, &new) < pos(&r, &old), "after rerank: {r}");
    let stale = &passages(&r)[pos(&r, &old)];
    assert_eq!(stale["belief"], "superseded");

    // `belief_aware: false`: summaries join the pool without belief rules, as passages do.
    let r = search(&f, json!({"summaries": true, "belief_aware": false})).await;
    assert_eq!(passages(&r)[0]["uuid"], old.as_str());
    assert_eq!(passages(&r)[0]["via"], "summary");
}

#[tokio::test]
async fn filters_bind_a_summary_through_its_episode() {
    enable_summary_units();
    let f = fixture();
    let ours = put(
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
        OTHER,
        "kiln",
        "notes on kiln drying of timber",
        None,
        T1,
    )
    .await;
    call(
        "knowledge_process_chunk",
        json!({"chunk_text": "a derived digest about the kiln", "chunk_id": "digest",
               "source_file": "notes/digest.md", "group_id": G, "reference_time": T1,
               "doc_id": "digest", "origin": "derived"}),
        &f.state,
    )
    .await;

    let r = search(&f, json!({"summaries": true, "origins": ["primary"]})).await;
    assert_eq!(via_count(&r), 0, "{r}");
    assert!(uuids_via(&r).iter().all(|(u, _)| *u == ours));

    let r = search(&f, json!({"summaries": true})).await;
    assert_eq!(
        via_count(&r),
        1,
        "the derived digest's summary is in scope without the filter"
    );
}

#[tokio::test]
async fn a_removed_chunks_summary_yields_nothing() {
    let f = fixture();
    let kiln = corpus(&f).await;
    call(
        "knowledge_mark_chunk_removed",
        json!({"chunk_id": "kiln", "group_ids": [G]}),
        &f.state,
    )
    .await;
    let r = search(&f, json!({"summaries": true})).await;
    assert!(uuids_via(&r)
        .iter()
        .all(|(u, via)| *u != kiln || via.is_none()));
    assert_eq!(via_count(&r), 0, "{r}");
}

#[tokio::test]
async fn the_slot_cap_holds() {
    enable_summary_units();
    let f = fixture();
    for i in 0..5 {
        put(
            &f.state,
            G,
            &format!("k{i}"),
            &format!("kiln note number {i} on drying timber"),
            None,
            T1,
        )
        .await;
    }
    for name in ["owl", "tide", "garden", "moss", "fern"] {
        put(
            &f.state,
            G,
            name,
            &format!("plain notes about the {name} today"),
            None,
            T1,
        )
        .await;
    }
    let r = search(&f, json!({"summaries": true})).await;
    assert_eq!(via_count(&r), 3, "default cap: {r}");
    assert_eq!(passages(&r).len(), 10);
    // The summaries rank first; the remaining slots go to ordinary passages.
    assert!(passages(&r)[..3].iter().all(|p| p["via"] == "summary"));

    let r = search(&f, json!({"summaries": true, "max_summary_slots": 1})).await;
    assert_eq!(via_count(&r), 1);
    assert_eq!(passages(&r).len(), 10);

    let r = search(&f, json!({"summaries": true, "max_summary_slots": 0})).await;
    assert_eq!(via_count(&r), 0);
    assert_eq!(r["summaries"]["applied"], false);
    assert_eq!(r["summaries"]["reason"], "disabled");
    let mut plain = search(&f, json!({})).await;
    plain["summaries"] = r["summaries"].clone();
    assert_eq!(r, plain, "disabled runs the ordinary search");
}

#[tokio::test]
async fn invalid_parameters_are_rejected() {
    let f = fixture();
    corpus(&f).await;
    assert!(search_err(&f, json!({"summaries": "yes"}))
        .await
        .contains("summaries must be a boolean"));
    assert!(search_err(&f, json!({"summaries": 1}))
        .await
        .contains("summaries"));
    for bad in [json!(-1), json!("3"), json!(1.5), Value::Null] {
        let e = search_err(&f, json!({"summaries": true, "max_summary_slots": bad})).await;
        assert!(e.contains("max_summary_slots"), "{bad}: {e}");
    }
}

#[tokio::test]
async fn no_units_in_the_searched_groups_is_reported_not_an_error() {
    let f = fixture();
    corpus(&f).await;
    put(&f.state, OTHER, "x", "x", None, T1).await; // too short to summarize
    let r = call(
        "knowledge_search_passages",
        json!({"query": "KILN", "group_ids": [OTHER], "min_score": 0.0, "summaries": true}),
        &f.state,
    )
    .await;
    assert_eq!(
        r["summaries"],
        json!({"applied": false, "candidates": 0, "reason": "no_units"})
    );
    assert_eq!(r["count"], 1);
}

#[tokio::test]
async fn summaries_compose_with_expand_queries_and_hybrid() {
    let f = fixture();
    let kiln = corpus(&f).await;
    install(
        &f,
        Some(MockReranker::new(|_, _| 0.0)),
        Some(MockQueryExpander::with_variants(vec![QueryVariant {
            kind: VariantKind::Lexical,
            text: "owl".to_string(),
        }])),
    );
    for extra in [
        json!({"summaries": true, "expand": true}),
        json!({"summaries": true, "queries": ["owl at dusk"]}),
        json!({"summaries": true, "mode": "hybrid"}),
        json!({"summaries": true, "mode": "hybrid", "rerank": true, "expand": true}),
        json!({"summaries": true, "belief_aware": false, "queries": ["tide"]}),
    ] {
        let r = search(&f, extra.clone()).await;
        // The kiln note is in the results exactly once. With extra query lists its passage can
        // also be fused in directly and outrank its own summary, in which case it holds the slot
        // without `via` — so only presence is asserted here.
        let hits = uuids_via(&r)
            .into_iter()
            .filter(|(u, _)| *u == kiln)
            .count();
        assert_eq!(hits, 1, "{extra}: {r}");
        assert_eq!(r["summaries"]["applied"], true, "{extra}");
        assert!(!r.to_string().contains("THIS NOTE DESCRIBES"), "{extra}");
    }
}
