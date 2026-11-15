//! Weakly supported facts (FR-012, FR-016): a believed fact whose corroboration — distinct
//! primary + external documents, ADR-0752 — is zero (`derived_only`) or one (`single_source`).

use serde_json::json;

use super::classes::Ctx;
use super::ids::finding_id;
use super::{Category, CategoryResult, Citations, Finding};

pub fn analyse(ctx: &Ctx<'_>) -> CategoryResult {
    let mut out = Vec::new();
    for ((group, src, dst, name), idxs) in &ctx.classes {
        let believed: Vec<usize> = idxs.iter().copied().filter(|i| ctx.believed[*i]).collect();
        if believed.is_empty() {
            continue;
        }
        // A believed edge with no producing episode (asserted, cross-group warrant, legacy) is
        // `primary` under ADR-0752; a zero tally would misreport it, so the class is skipped.
        if believed
            .iter()
            .any(|i| ctx.edge(*i).belief.episodes.is_empty())
        {
            continue;
        }
        let tally = ctx.tally(idxs);
        let corroboration = tally.corroboration();
        let reason = match corroboration {
            0 => "derived_only",
            1 => "single_source",
            _ => continue,
        };
        let mut cites = Citations::default();
        for &i in &believed {
            let e = ctx.edge(i);
            cites.fact(e);
            cites.entities.insert(e.source.clone());
            cites.entities.insert(e.target.clone());
        }
        for &i in idxs {
            for u in &ctx.edge(i).belief.episodes {
                if let Some(ep) = ctx.live_episode(u) {
                    cites.episode(ep);
                }
            }
        }
        out.push(Finding {
            id: finding_id(Category::WeakSupport.as_str(), group, &[src, dst, name]),
            category: Category::WeakSupport,
            group_id: group.clone(),
            reason,
            details: json!({
                "support": tally.to_json(),
                "corroboration": corroboration,
            }),
            citations: cites,
        });
    }
    CategoryResult::ok(out)
}
