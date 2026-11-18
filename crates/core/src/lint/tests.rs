//! Unit tests over hand-built snapshots (no database): each category's rules, the paging
//! contract and request validation. DB-backed behaviour lives in `tests/knowledge_lint.rs`.

use serde_json::{json, Value};

use super::snapshot::{LintEdge, LintEntity, LintEpisode, LintSnapshot};
use super::*;
use crate::belief::EdgeBeliefRow;
use crate::origin::Origin;

const G: &str = "g";
const NOW: i64 = 1_000_000;

fn ep(uuid: &str, doc: Option<&str>, origin: Origin) -> LintEpisode {
    LintEpisode {
        uuid: uuid.into(),
        group_id: G.into(),
        doc_id: doc.map(Into::into),
        origin,
        superseded_at_us: None,
        superseded_by_kind: None,
        supersession_reason: None,
        revision_id_present: false,
        revision_committed_at_us: None,
        attributes: None,
    }
}

fn superseded(mut e: LintEpisode, reason: &str, kind: Option<&str>) -> LintEpisode {
    e.superseded_at_us = Some(NOW - 10);
    e.supersession_reason = Some(reason.into());
    e.superseded_by_kind = kind.map(Into::into);
    e
}

fn edge(uuid: &str, src: &str, dst: &str, name: &str, eps: &[&str]) -> LintEdge {
    LintEdge {
        belief: EdgeBeliefRow {
            uuid: uuid.into(),
            ingested_at_us: Some(1),
            episodes: eps.iter().map(|s| s.to_string()).collect(),
            warrant: None,
            valid_at_source: None,
        },
        group_id: G.into(),
        source: src.into(),
        target: dst.into(),
        source_group: G.into(),
        target_group: G.into(),
        name: name.into(),
        relation_type: None,
    }
}

fn entity(uuid: &str, name: &str) -> LintEntity {
    LintEntity {
        uuid: uuid.into(),
        group_id: G.into(),
        name: name.into(),
        kind: "Entity".into(),
    }
}

fn req(cats: &[Category]) -> LintRequest {
    LintRequest {
        groups: None,
        categories: cats.to_vec(),
        limit: 500,
        single_valued: vec![],
        near_duplicate_threshold: DEFAULT_NEAR_DUPLICATE_THRESHOLD,
        min_mentions: DEFAULT_MIN_MENTIONS,
        after: None,
    }
}

fn run(snap: &LintSnapshot, r: &LintRequest) -> Analysis {
    analyse(snap, r, NOW)
}

fn reasons(a: &Analysis) -> Vec<&'static str> {
    a.page.iter().map(|f| f.reason).collect()
}

// ── weak_support ────────────────────────────────────────────────────────────────────────────

#[test]
fn weak_support_reports_single_source_and_derived_only() {
    let snap = LintSnapshot {
        episodes: vec![
            ep("e1", Some("d1"), Origin::Primary), // fact A: one primary doc
            ep("e2", Some("d2"), Origin::Primary), // fact B: two docs
            ep("e3", Some("d3"), Origin::External),
            ep("e4", Some("d4"), Origin::Derived), // fact C: derived only
            ep("e5", Some("d1"), Origin::Primary), // second chunk of d1: still one doc
        ],
        edges: vec![
            edge("r1", "a", "b", "uses", &["e1"]),
            edge("r1b", "a", "b", "uses", &["e5"]),
            edge("r2", "c", "d", "uses", &["e2"]),
            edge("r2b", "c", "d", "uses", &["e3"]),
            edge("r3", "e", "f", "uses", &["e4"]),
        ],
        ..Default::default()
    };
    let a = run(&snap, &req(&[Category::WeakSupport]));
    let mut r = reasons(&a);
    r.sort();
    assert_eq!(r, vec!["derived_only", "single_source"]);
    assert_eq!(a.summary["categories"]["weak_support"]["count"], 2);
    let single = a.page.iter().find(|f| f.reason == "single_source").unwrap();
    assert_eq!(single.details["support"]["primary"], 1, "chunks count once");
}

#[test]
fn weak_support_skips_superseded_and_asserted_facts() {
    let mut asserted = edge("r2", "c", "d", "uses", &[]);
    asserted.belief.warrant = Some("asserted".into());
    let snap = LintSnapshot {
        episodes: vec![superseded(
            ep("e1", Some("d1"), Origin::Primary),
            "revision",
            None,
        )],
        edges: vec![edge("r1", "a", "b", "uses", &["e1"]), asserted],
        ..Default::default()
    };
    let a = run(&snap, &req(&[Category::WeakSupport]));
    assert!(
        a.page.is_empty(),
        "superseded and asserted facts are not weak: {:?}",
        reasons(&a)
    );
}

#[test]
fn weak_support_ignores_cross_group_edges() {
    let mut e = edge("r1", "a", "b", "uses", &["e1"]);
    e.target_group = "other".into();
    let snap = LintSnapshot {
        episodes: vec![ep("e1", Some("d1"), Origin::Primary)],
        edges: vec![e],
        ..Default::default()
    };
    assert!(run(&snap, &req(&[Category::WeakSupport])).page.is_empty());
}

// ── stale_derived_page ──────────────────────────────────────────────────────────────────────

fn stale_snapshot(prim: LintEpisode) -> LintSnapshot {
    LintSnapshot {
        episodes: vec![prim, ep("d1", Some("page"), Origin::Derived)],
        edges: vec![
            edge("rp", "a", "b", "uses", &["p1"]),
            edge("rd", "a", "b", "uses", &["d1"]),
        ],
        ..Default::default()
    }
}

#[test]
fn stale_page_not_reported_while_primary_support_is_live() {
    let snap = stale_snapshot(ep("p1", Some("note"), Origin::Primary));
    assert!(run(&snap, &req(&[Category::StaleDerivedPage]))
        .page
        .is_empty());
}

#[test]
fn stale_page_not_reported_when_an_asserted_sibling_still_backs_the_fact() {
    // Same-fact class (a, b, uses): superseded primary p1, derived restatement d1, and an
    // asserted edge with no producing episode (primary under ADR-0752) that is still believed.
    let mut snap = stale_snapshot(superseded(
        ep("p1", Some("note"), Origin::Primary),
        "revision",
        None,
    ));
    snap.edges.push(edge("ra", "a", "b", "uses", &[]));
    assert!(
        run(&snap, &req(&[Category::StaleDerivedPage]))
            .page
            .is_empty(),
        "an episode-less believed edge keeps the fact corroborated"
    );
}

#[test]
fn stale_page_cause_mapping() {
    for (reason, kind, want) in [
        ("revision", None, "corrected"),
        ("chunk_removed", None, "removed"),
        (
            "document_superseded",
            Some("document"),
            "document_superseded",
        ),
    ] {
        let snap = stale_snapshot(superseded(
            ep("p1", Some("note"), Origin::Primary),
            reason,
            kind,
        ));
        let a = run(&snap, &req(&[Category::StaleDerivedPage]));
        assert_eq!(a.page.len(), 1, "{reason}");
        let f = &a.page[0];
        assert_eq!(f.reason, "lost_corroboration");
        assert_eq!(f.details["causes"][want], 1, "{reason}: {}", f.details);
        assert_eq!(f.details["affected_facts"], 1);
        assert_eq!(f.details["total_facts"], 1);
        assert!(f.citations.documents.contains("page"));
        assert!(f.citations.documents.contains("note"));
        assert!(f.citations.episodes.contains("p1"));
    }
}

#[test]
fn stale_page_needs_formerly_corroborated_fact() {
    // The derived fact never had primary support: weak, not stale.
    let snap = LintSnapshot {
        episodes: vec![ep("d1", Some("page"), Origin::Derived)],
        edges: vec![edge("rd", "a", "b", "uses", &["d1"])],
        ..Default::default()
    };
    assert!(run(&snap, &req(&[Category::StaleDerivedPage]))
        .page
        .is_empty());
    assert_eq!(run(&snap, &req(&[Category::WeakSupport])).page.len(), 1);
}

#[test]
fn stale_page_live_sibling_support_keeps_it_fresh_and_partial_staleness_is_counted() {
    let snap = LintSnapshot {
        episodes: vec![
            superseded(ep("p1", Some("n1"), Origin::Primary), "revision", None),
            ep("p2", Some("n2"), Origin::External), // keeps fact (c,d) alive
            ep("d1", Some("page"), Origin::Derived),
        ],
        edges: vec![
            edge("r1", "a", "b", "uses", &["p1"]),
            edge("r2", "a", "b", "uses", &["d1"]), // stale
            edge("r3", "c", "d", "uses", &["p2"]),
            edge("r4", "c", "d", "uses", &["d1"]), // fresh
        ],
        ..Default::default()
    };
    let a = run(&snap, &req(&[Category::StaleDerivedPage]));
    assert_eq!(a.page.len(), 1);
    assert_eq!(a.page[0].details["affected_facts"], 1);
    assert_eq!(a.page[0].details["total_facts"], 2);
}

#[test]
fn stale_page_ignores_mixed_origin_documents_and_hard_deleted_support() {
    // A document with a primary episode is not a derived page.
    let mut mixed = stale_snapshot(superseded(
        ep("p1", Some("note"), Origin::Primary),
        "revision",
        None,
    ));
    mixed.episodes.push(ep("x", Some("page"), Origin::Primary));
    assert!(run(&mixed, &req(&[Category::StaleDerivedPage]))
        .page
        .is_empty());

    // The primary episode is gone entirely: its origin is unknowable, so nothing is reported.
    let gone = LintSnapshot {
        episodes: vec![ep("d1", Some("page"), Origin::Derived)],
        edges: vec![
            edge("rp", "a", "b", "uses", &["p1"]),
            edge("rd", "a", "b", "uses", &["d1"]),
        ],
        ..Default::default()
    };
    assert!(run(&gone, &req(&[Category::StaleDerivedPage]))
        .page
        .is_empty());
}

#[test]
fn stale_page_docless_episode_is_its_own_document() {
    let snap = LintSnapshot {
        episodes: vec![
            superseded(ep("p1", None, Origin::Primary), "chunk_removed", None),
            ep("d1", None, Origin::Derived),
        ],
        edges: vec![
            edge("rp", "a", "b", "uses", &["p1"]),
            edge("rd", "a", "b", "uses", &["d1"]),
        ],
        ..Default::default()
    };
    let a = run(&snap, &req(&[Category::StaleDerivedPage]));
    assert_eq!(a.page.len(), 1);
    assert!(a.page[0].details["derived_document"].is_null());
    assert!(a.page[0].citations.episodes.contains("d1"));
}

// ── contradiction ───────────────────────────────────────────────────────────────────────────

fn contradiction_req(rel: &[&str]) -> LintRequest {
    let mut r = req(&[Category::Contradiction]);
    r.single_valued = rel.iter().map(|s| s.to_ascii_lowercase()).collect();
    r
}

#[test]
fn contradiction_unavailable_without_designation() {
    let a = run(&LintSnapshot::default(), &contradiction_req(&[]));
    assert_eq!(
        a.summary["categories"]["contradiction"],
        json!({"status": "unavailable", "count": 0, "reason": "no_single_valued_relations"})
    );
}

#[test]
fn contradiction_two_live_targets_one_finding_and_supersession_removes_it() {
    let live = |p2: LintEpisode| LintSnapshot {
        episodes: vec![ep("e1", Some("d1"), Origin::Primary), p2],
        edges: vec![
            edge("r1", "alice", "berlin", "lives_in", &["e1"]),
            edge("r2", "alice", "paris", "LIVES_IN", &["e2"]),
            edge("r3", "alice", "x", "knows", &["e1"]),
            edge("r4", "alice", "y", "knows", &["e1"]),
        ],
        ..Default::default()
    };
    let snap = live(ep("e2", Some("d2"), Origin::Primary));
    let a = run(&snap, &contradiction_req(&["Lives_In"]));
    assert_eq!(a.page.len(), 1, "non-designated `knows` is not reported");
    let f = &a.page[0];
    assert_eq!(f.reason, "single_valued_conflict");
    assert_eq!(f.citations.facts.len(), 2);
    assert_eq!(
        f.details["sides"][0]["target"], "berlin",
        "sides ordered by target uuid"
    );
    assert_eq!(f.details["sides"][1]["target"], "paris");
    let id = f.id.clone();
    // Stable id.
    assert_eq!(run(&snap, &contradiction_req(&["lives_in"])).page[0].id, id);

    let gone = live(superseded(
        ep("e2", Some("d2"), Origin::Primary),
        "revision",
        None,
    ));
    assert!(run(&gone, &contradiction_req(&["lives_in"]))
        .page
        .is_empty());
}

#[test]
fn contradiction_skips_and_counts_subjects_with_too_many_targets() {
    let n = contradiction::MAX_SIDES + 1;
    let mut edges: Vec<_> = (0..n)
        .map(|i| {
            edge(
                &format!("r{i:03}"),
                "alice",
                &format!("t{i:03}"),
                "related_to",
                &["e1"],
            )
        })
        .collect();
    edges.push(edge("s1", "bob", "x", "related_to", &["e1"]));
    edges.push(edge("s2", "bob", "y", "related_to", &["e1"]));
    let snap = LintSnapshot {
        episodes: vec![ep("e1", Some("d1"), Origin::Primary)],
        edges,
        ..Default::default()
    };
    let a = run(&snap, &contradiction_req(&["related_to"]));
    let c = &a.summary["categories"]["contradiction"];
    assert_eq!(c["skipped_subjects"], 1, "alice is skipped");
    assert_eq!(c["count"], 1, "bob's single pair is still reported");
}

#[test]
fn contradiction_merges_sibling_edges_into_one_side() {
    let snap = LintSnapshot {
        episodes: vec![
            ep("e1", Some("d1"), Origin::Primary),
            ep("e2", Some("d2"), Origin::Primary),
            ep("e3", Some("d3"), Origin::Primary),
        ],
        edges: vec![
            edge("r1", "alice", "berlin", "lives_in", &["e1"]),
            edge("r1b", "alice", "berlin", "lives_in", &["e2"]),
            edge("r2", "alice", "paris", "lives_in", &["e3"]),
        ],
        ..Default::default()
    };
    let a = run(&snap, &contradiction_req(&["lives_in"]));
    assert_eq!(a.page.len(), 1, "siblings do not multiply findings");
    assert_eq!(a.page[0].details["sides"][0]["corroboration"], 2);
}

// ── orphan / near-duplicate ─────────────────────────────────────────────────────────────────

#[test]
fn orphans_need_no_live_mention_and_no_believed_edge() {
    let snap = LintSnapshot {
        episodes: vec![
            ep("e1", Some("d1"), Origin::Primary),
            superseded(ep("e2", Some("d2"), Origin::Primary), "revision", None),
        ],
        entities: vec![
            entity("live", "Live"),
            entity("lost", "Lost"),
            entity("bare", "Bare"),
            entity("linked", "Linked"),
        ],
        mentions: vec![("e1".into(), "live".into()), ("e2".into(), "lost".into())],
        edges: vec![edge("r1", "linked", "live", "uses", &["e1"])],
    };
    let a = run(&snap, &req(&[Category::OrphanEntity]));
    let got: Vec<(String, &str)> = a
        .page
        .iter()
        .map(|f| {
            (
                f.citations.entities.iter().next().unwrap().clone(),
                f.reason,
            )
        })
        .collect();
    assert!(got.contains(&("lost".to_string(), "support_lost")));
    assert!(got.contains(&("bare".to_string(), "never_supported")));
    assert_eq!(got.len(), 2, "{got:?}");
    let lost = a.page.iter().find(|f| f.reason == "support_lost").unwrap();
    assert!(lost.citations.episodes.contains("e2"));
}

#[test]
fn near_duplicates_threshold_monotone_same_kind_and_group_once() {
    let mut other_kind = entity("e5", "Tim Cook");
    other_kind.kind = "Person".into();
    let mut other_group = entity("e6", "Tim Cook");
    other_group.group_id = "elsewhere".into();
    let snap = LintSnapshot {
        entities: vec![
            entity("e1", "Tim Cook"),
            entity("e2", "tim cook"),
            entity("e3", "Tim Cooke"),
            entity("e4", "Sundar Pichai"),
            other_kind,
            other_group,
        ],
        ..Default::default()
    };
    let at = |t: f64| {
        let mut r = req(&[Category::NearDuplicateEntity]);
        r.near_duplicate_threshold = t;
        run(&snap, &r)
    };
    let mut prev = usize::MAX;
    for t in [0.5, 0.8, 0.9, 0.95, 1.0] {
        let n = at(t).page.len();
        assert!(n <= prev, "threshold {t}: {n} > {prev}");
        prev = n;
    }
    assert_eq!(
        at(0.9).page.len(),
        1,
        "only the identical-after-normalisation pair"
    );
    let a = at(0.85);
    // e1/e2 identical after normalisation, e1/e3 and e2/e3 close; nothing involves e4, e5, e6.
    assert_eq!(a.page.len(), 3);
    for f in &a.page {
        let ids: Vec<&String> = f.citations.entities.iter().collect();
        assert!(ids[0] < ids[1], "canonical order");
        assert!(!f
            .citations
            .entities
            .iter()
            .any(|u| ["e4", "e5", "e6"].contains(&u.as_str())));
    }
    assert_eq!(
        a.summary["categories"]["near_duplicate_entity"]["skipped_blocks"],
        0
    );
}

#[test]
fn near_duplicates_skip_and_count_oversize_blocks() {
    let entities: Vec<LintEntity> = (0..(entities::MAX_BLOCK + 5))
        .map(|i| entity(&format!("e{i:04}"), &format!("Zzzz {i:04}")))
        .collect();
    let snap = LintSnapshot {
        entities,
        ..Default::default()
    };
    let a = run(&snap, &req(&[Category::NearDuplicateEntity]));
    assert!(
        a.summary["categories"]["near_duplicate_entity"]["skipped_blocks"]
            .as_u64()
            .unwrap()
            >= 1
    );
}

// ── unlinked_mention ────────────────────────────────────────────────────────────────────────

fn with_attrs(mut e: LintEpisode, attrs: Value) -> LintEpisode {
    e.attributes = Some(attrs.to_string());
    e
}

#[test]
fn unlinked_mentions_unavailable_without_page_or_link_data() {
    let snap = LintSnapshot {
        episodes: vec![ep("e1", Some("d1"), Origin::Primary)],
        entities: vec![entity("x", "X")],
        mentions: vec![("e1".into(), "x".into())],
        ..Default::default()
    };
    let a = run(&snap, &req(&[Category::UnlinkedMention]));
    assert_eq!(
        a.summary["categories"]["unlinked_mention"]["status"],
        "unavailable"
    );
    assert_eq!(
        a.summary["categories"]["unlinked_mention"]["reason"],
        "no_page_data"
    );
    assert_eq!(a.summary["total"], 0);

    let mut pages_only = snap;
    pages_only.episodes.push(with_attrs(
        ep("p", Some("pagex"), Origin::Primary),
        json!({"page_for": "x"}),
    ));
    let a = run(&pages_only, &req(&[Category::UnlinkedMention]));
    assert_eq!(
        a.summary["categories"]["unlinked_mention"]["reason"],
        "no_link_data"
    );
}

#[test]
fn doc_less_page_declaration_is_ignored() {
    let snap = LintSnapshot {
        episodes: vec![
            with_attrs(
                ep("p", None, Origin::Primary),
                json!({"page_for": "x-uuid"}),
            ),
            with_attrs(ep("e1", Some("d1"), Origin::Primary), json!({"links": []})),
        ],
        entities: vec![entity("x-uuid", "X")],
        mentions: vec![("e1".into(), "x-uuid".into())],
        ..Default::default()
    };
    let a = run(&snap, &req(&[Category::UnlinkedMention]));
    assert_eq!(
        a.summary["categories"]["unlinked_mention"]["reason"], "no_page_data",
        "a page without a doc_id cannot be linked to, so it is not a page"
    );
    assert!(a.page.is_empty());
}

#[test]
fn unlinked_mentions_and_missing_pages() {
    let snap = LintSnapshot {
        episodes: vec![
            with_attrs(
                ep("p", Some("page-x"), Origin::Primary),
                json!({"page_for": "x-uuid"}),
            ),
            with_attrs(ep("e1", Some("d1"), Origin::Primary), json!({"links": []})), // no link to page-x
            with_attrs(
                ep("e2", Some("d2"), Origin::Primary),
                json!({"links": ["page-x"]}),
            ),
            ep("e3", Some("d3"), Origin::Primary), // not link-tracked
            ep("e4", Some("d4"), Origin::Primary),
            ep("e5", Some("d5"), Origin::Primary),
        ],
        entities: vec![entity("x-uuid", "X"), entity("y-uuid", "Y")],
        mentions: vec![
            ("e1".into(), "x-uuid".into()),
            ("e2".into(), "x-uuid".into()),
            ("e3".into(), "x-uuid".into()),
            ("e3".into(), "y-uuid".into()),
            ("e4".into(), "y-uuid".into()),
            ("e5".into(), "y-uuid".into()),
        ],
        ..Default::default()
    };
    let a = run(&snap, &req(&[Category::UnlinkedMention]));
    let mut r = reasons(&a);
    r.sort();
    assert_eq!(r, vec!["missing_page", "unlinked_mention"], "{:?}", a.page);
    let un = a
        .page
        .iter()
        .find(|f| f.reason == "unlinked_mention")
        .unwrap();
    assert!(un.citations.documents.contains("d1") && un.citations.documents.contains("page-x"));
    let missing = a.page.iter().find(|f| f.reason == "missing_page").unwrap();
    assert!(missing.citations.entities.contains("y-uuid"));
    assert_eq!(missing.citations.documents.len(), 3);

    // min_mentions boundary: 3 documents mention Y, so 4 hides the finding.
    let mut strict = req(&[Category::UnlinkedMention]);
    strict.min_mentions = 4;
    let a = run(&snap, &strict);
    assert_eq!(reasons(&a), vec!["unlinked_mention"]);
}

// ── paging and validation ───────────────────────────────────────────────────────────────────

fn many_weak(n: usize) -> LintSnapshot {
    LintSnapshot {
        episodes: (0..n)
            .map(|i| ep(&format!("e{i}"), Some(&format!("d{i}")), Origin::Primary))
            .collect(),
        edges: (0..n)
            .map(|i| {
                edge(
                    &format!("r{i}"),
                    "a",
                    &format!("t{i}"),
                    "uses",
                    &[&format!("e{i}")],
                )
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn paging_returns_every_finding_once_in_stable_order() {
    let snap = many_weak(7);
    let mut r = req(&[Category::WeakSupport, Category::OrphanEntity]);
    r.limit = 3;
    let all = run(&snap, &{
        let mut x = r.clone();
        x.limit = 500;
        x
    });
    let want: Vec<String> = all.page.iter().map(|f| f.id.clone()).collect();
    assert_eq!(want.len(), 7);
    let mut sorted = want.clone();
    sorted.sort();
    assert_eq!(want, sorted, "ordered by id within a category");

    let mut got = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut q = r.clone();
        if let Some(c) = &cursor {
            q = LintRequest::parse(
                &json!({"cursor": c, "limit": 3, "categories": ["weak_support", "orphan_entity"]}),
                None,
            )
            .unwrap();
        }
        let a = run(&snap, &q);
        assert_eq!(
            a.summary["categories"]["weak_support"]["count"], 7,
            "summary is page-independent"
        );
        got.extend(a.page.iter().map(|f| f.id.clone()));
        pages += 1;
        match a.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(got, want);
}

#[test]
fn runs_are_deterministic() {
    let snap = many_weak(5);
    let r = req(&ALL_CATEGORIES);
    let a = run(&snap, &r);
    let b = run(&snap, &r);
    assert_eq!(
        a.page
            .iter()
            .map(|f| (&f.id, &f.details))
            .collect::<Vec<_>>(),
        b.page
            .iter()
            .map(|f| (&f.id, &f.details))
            .collect::<Vec<_>>()
    );
    assert_eq!(a.summary, b.summary);
}

#[test]
fn parse_validates_before_any_work() {
    let bad = |p: Value| LintRequest::parse(&p, None).unwrap_err().to_string();
    assert!(bad(json!({"categories": ["nope"]})).contains("unknown category"));
    assert!(bad(json!({"categories": []})).contains("must not be empty"));
    assert!(bad(json!({"categories": "weak_support"})).contains("array"));
    assert!(bad(json!({"limit": 0})).contains("limit"));
    assert!(bad(json!({"limit": 501})).contains("limit"));
    assert!(bad(json!({"limit": "ten"})).contains("limit"));
    assert!(bad(json!({"near_duplicate_threshold": 0})).contains("threshold"));
    assert!(bad(json!({"near_duplicate_threshold": 1.5})).contains("threshold"));
    assert!(bad(json!({"min_mentions": 0})).contains("min_mentions"));
    assert!(bad(json!({"single_valued_relations": ["", "x"]})).contains("single_valued_relations"));
    assert!(bad(json!({"single_valued_relations": "x"})).contains("single_valued_relations"));
    assert!(bad(json!({"cursor": "zz"})).contains("invalid cursor"));
    assert!(bad(json!({"cursor": 3})).contains("cursor"));
    let ok = LintRequest::parse(&json!({"near_duplicate_threshold": 1}), None).unwrap();
    assert_eq!(ok.near_duplicate_threshold, 1.0);
    assert_eq!(ok.limit, DEFAULT_LIMIT);
    assert_eq!(ok.categories.len(), 6);
}

#[test]
fn cursor_is_bound_to_the_query_shape() {
    let snap = many_weak(4);
    let mut r = req(&[Category::WeakSupport]);
    r.limit = 2;
    let c = run(&snap, &r).next_cursor.expect("more pages");
    let params = |extra: Value| {
        let mut p = json!({"cursor": c, "categories": ["weak_support"], "limit": 2});
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        p
    };
    assert!(LintRequest::parse(&params(json!({})), None).is_ok());
    // Different category selection, group scope, threshold, min_mentions or designation.
    assert!(LintRequest::parse(&params(json!({"categories": ["orphan_entity"]})), None).is_err());
    assert!(LintRequest::parse(&params(json!({})), Some(vec!["other".into()])).is_err());
    assert!(LintRequest::parse(&params(json!({"near_duplicate_threshold": 0.8})), None).is_err());
    assert!(LintRequest::parse(&params(json!({"min_mentions": 9})), None).is_err());
    assert!(LintRequest::parse(&params(json!({"single_valued_relations": ["x"]})), None).is_err());
    // The page limit is not part of the shape.
    assert!(LintRequest::parse(&params(json!({"limit": 50})), None).is_ok());
}

#[test]
fn only_selected_categories_load_data() {
    use snapshot::Needs;
    let n = Needs::for_categories(&[Category::NearDuplicateEntity]);
    assert!(n.entities && !n.episodes && !n.edges && !n.mentions && !n.attributes);
    let n = Needs::for_categories(&[Category::WeakSupport]);
    assert!(n.episodes && n.edges && !n.entities && !n.mentions);
    let n = Needs::for_categories(&[Category::UnlinkedMention]);
    assert!(n.attributes && n.mentions && !n.edges);
}
