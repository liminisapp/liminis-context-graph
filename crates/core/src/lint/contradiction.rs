//! Live contradictions (FR-009): two currently-believed facts with the same source entity and a
//! relation designated single-valued, but different target entities.
//!
//! LCG stores no contradiction signal (ingest never invalidates one fact because of another), so
//! the only deterministic, non-noisy definition is this narrow rule. The designation comes from the
//! request (`single_valued_relations`); without one the category is `unavailable`.

use std::collections::BTreeMap;

use serde_json::json;

use super::classes::Ctx;
use super::ids::finding_id;
use super::{Category, CategoryResult, Citations, Finding};

/// The designation `edge` falls under: its `name` first, then its `relation_type`, compared
/// trimmed and ASCII-case-insensitively against the normalised set.
fn designation(
    single_valued: &[String],
    name: &str,
    relation_type: Option<&str>,
) -> Option<String> {
    let n = name.trim().to_ascii_lowercase();
    if single_valued.contains(&n) {
        return Some(n);
    }
    let r = relation_type?.trim().to_ascii_lowercase();
    single_valued.contains(&r).then_some(r)
}

pub fn analyse(ctx: &Ctx<'_>, single_valued: &[String]) -> CategoryResult {
    if single_valued.is_empty() {
        return CategoryResult::unavailable("no_single_valued_relations");
    }
    // (group, source, designation) → target → believed sibling edges to that target (a "side").
    type Sides = BTreeMap<String, Vec<usize>>;
    let mut by_subject: BTreeMap<(String, String, String), Sides> = BTreeMap::new();
    for (i, e) in ctx.snap.edges.iter().enumerate() {
        if !ctx.believed[i] || !e.same_group() {
            continue;
        }
        let Some(d) = designation(single_valued, &e.name, e.relation_type.as_deref()) else {
            continue;
        };
        by_subject
            .entry((e.group_id.clone(), e.source.clone(), d))
            .or_default()
            .entry(e.target.clone())
            .or_default()
            .push(i);
    }

    let mut out = Vec::new();
    for ((group, src, desig), sides) in &by_subject {
        let targets: Vec<(&String, &Vec<usize>)> = sides.iter().collect();
        for a in 0..targets.len() {
            for b in (a + 1)..targets.len() {
                let (ta, ea) = targets[a];
                let (tb, eb) = targets[b];
                let mut cites = Citations::default();
                cites.entities.insert(src.clone());
                cites.entities.insert(ta.clone());
                cites.entities.insert(tb.clone());
                for &i in ea.iter().chain(eb.iter()) {
                    let e = ctx.edge(i);
                    cites.fact(e);
                    for u in &e.belief.episodes {
                        if let Some(ep) = ctx.live_episode(u) {
                            cites.episode(ep);
                        }
                    }
                }
                let side = |t: &String, es: &[usize]| {
                    let tally = ctx.tally(es);
                    json!({
                        "target": t,
                        "facts": es.iter().map(|i| ctx.edge(*i).uuid()).collect::<Vec<_>>(),
                        "support": tally.to_json(),
                        "corroboration": tally.corroboration(),
                    })
                };
                out.push(Finding {
                    id: finding_id(
                        Category::Contradiction.as_str(),
                        group,
                        &[src, desig, ta, tb],
                    ),
                    category: Category::Contradiction,
                    group_id: group.clone(),
                    reason: "single_valued_conflict",
                    details: json!({
                        "relation": desig,
                        "source": src,
                        "sides": [side(ta, ea), side(tb, eb)],
                    }),
                    citations: cites,
                });
            }
        }
    }
    CategoryResult::ok(out)
}
