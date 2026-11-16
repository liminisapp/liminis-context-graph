//! Stale derived pages (FR-010, FR-011): a document made only of `derived` episodes that restates
//! a fact whose corroborating (primary / external) support has since been superseded, corrected or
//! removed, so nothing live backs the fact any more.
//!
//! LCG records no "derived from" link, so the relation is inferred through shared facts using the
//! ADR-0752 same-fact key. Everything is read from supersession state at the evaluated instant, so
//! a correction or document supersession is reflected the moment it is recorded, with no re-ingest
//! of the derived page. A corroborating episode that was hard-deleted has no readable origin and is
//! ignored (documented in ADR-0755).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::json;

use crate::origin::Origin;

use super::classes::{ClassKey, Ctx};
use super::ids::finding_id;
use super::snapshot::LintEpisode;
use super::{Category, CategoryResult, Citations, Finding};

/// Why a formerly supporting episode no longer counts.
fn cause(ep: &LintEpisode) -> &'static str {
    if ep.superseded_by_kind.as_deref() == Some("document") {
        return "document_superseded";
    }
    match ep.supersession_reason.as_deref() {
        Some("revision") => "corrected",
        Some("chunk_removed") => "removed",
        _ => "superseded",
    }
}

struct DocInfo<'a> {
    all_derived: bool,
    live: Vec<&'a LintEpisode>,
    any: Option<&'a LintEpisode>,
}

pub fn analyse(ctx: &Ctx<'_>) -> CategoryResult {
    // Documents: every episode (live or superseded) decides whether the document is wholly derived.
    let mut docs: BTreeMap<(String, String), DocInfo<'_>> = BTreeMap::new();
    for ep in &ctx.snap.episodes {
        let d = docs
            .entry((ep.group_id.clone(), ep.doc_key()))
            .or_insert_with(|| DocInfo {
                all_derived: true,
                live: Vec::new(),
                any: None,
            });
        d.any.get_or_insert(ep);
        if ep.origin != Origin::Derived {
            d.all_derived = false;
        }
        if ep.is_live(ctx.as_of_us) {
            d.live.push(ep);
        }
    }

    // episode uuid → same-group edges it produced → their classes.
    let mut edges_of: HashMap<&str, Vec<usize>> = HashMap::new();
    for idxs in ctx.classes.values() {
        for &i in idxs {
            for u in &ctx.edge(i).belief.episodes {
                edges_of.entry(u.as_str()).or_default().push(i);
            }
        }
    }
    let class_of: HashMap<usize, &ClassKey> = ctx
        .classes
        .iter()
        .flat_map(|(k, v)| v.iter().map(move |i| (*i, k)))
        .collect();

    // Lost-corroboration verdict per class: the formerly supporting episodes when the class had
    // corroborating support and none of it is live any more.
    let mut lost: HashMap<&ClassKey, Vec<&LintEpisode>> = HashMap::new();
    for (key, idxs) in &ctx.classes {
        // A believed edge with no producing episode (asserted, cross-group warrant, legacy) is
        // `primary` under ADR-0752 and still backs the fact, so the class has not lost support
        // (same guard as `weak.rs`).
        if idxs
            .iter()
            .any(|&i| ctx.believed[i] && ctx.edge(i).belief.episodes.is_empty())
        {
            continue;
        }
        let mut corroborating: Vec<&LintEpisode> = Vec::new();
        for &i in idxs {
            for u in &ctx.edge(i).belief.episodes {
                if let Some(ep) = ctx.ep_by_uuid.get(u.as_str()) {
                    if ep.origin.is_corroborating() {
                        corroborating.push(ep);
                    }
                }
            }
        }
        if !corroborating.is_empty() && !corroborating.iter().any(|e| e.is_live(ctx.as_of_us)) {
            lost.insert(key, corroborating);
        }
    }

    let mut out = Vec::new();
    for ((group, key), d) in &docs {
        if !d.all_derived || d.live.is_empty() {
            continue;
        }
        let mut total: BTreeSet<&ClassKey> = BTreeSet::new();
        let mut affected: BTreeMap<&ClassKey, Vec<&LintEpisode>> = BTreeMap::new();
        let mut derived_edges: BTreeSet<usize> = BTreeSet::new();
        for ep in &d.live {
            for &i in edges_of.get(ep.uuid.as_str()).into_iter().flatten() {
                let Some(&class) = class_of.get(&i) else {
                    continue;
                };
                total.insert(class);
                if let Some(former) = lost.get(class) {
                    affected.insert(class, former.clone());
                    derived_edges.insert(i);
                }
            }
        }
        if affected.is_empty() {
            continue;
        }

        let mut cites = Citations::default();
        let mut former_eps: BTreeMap<&str, &LintEpisode> = BTreeMap::new();
        for eps in affected.values() {
            for ep in eps {
                former_eps.insert(ep.uuid.as_str(), ep);
            }
        }
        for ep in former_eps.values() {
            cites.episode(ep);
        }
        for ep in &d.live {
            cites.episode(ep);
        }
        for &i in &derived_edges {
            let e = ctx.edge(i);
            cites.fact(e);
            cites.entities.insert(e.source.clone());
            cites.entities.insert(e.target.clone());
        }
        let causes: BTreeMap<&str, usize> =
            former_eps.values().fold(BTreeMap::new(), |mut m, ep| {
                *m.entry(cause(ep)).or_insert(0) += 1;
                m
            });
        let derived_document = d.any.and_then(|e| e.doc_id.clone());
        out.push(Finding {
            id: finding_id(Category::StaleDerivedPage.as_str(), group, &[key]),
            category: Category::StaleDerivedPage,
            group_id: group.clone(),
            reason: "lost_corroboration",
            details: json!({
                "derived_document": derived_document,
                "affected_facts": affected.len(),
                "total_facts": total.len(),
                "causes": causes,
                "formerly_supporting": former_eps.values().take(super::CITATION_CAP).map(|ep| json!({
                    "episode": ep.uuid,
                    "document": ep.doc_id,
                    "origin": ep.origin.as_str(),
                    "cause": cause(ep),
                })).collect::<Vec<_>>(),
            }),
            citations: cites,
        });
    }
    CategoryResult::ok(out)
}
