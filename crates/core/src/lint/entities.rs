//! Orphan and near-duplicate entities (FR-013, FR-014).
//!
//! Orphans are read from belief and mention liveness. Near-duplicates use stored names only for
//! membership: a deterministic blocking scheme (independent of the threshold, so raising the
//! threshold can only shrink the result) bounds the comparisons, and a normalised Levenshtein ratio
//! scores each candidate pair. The stored `name_embedding` cosine is reported on the returned page
//! (see `lint::hydrate`) but never decides membership — no embedding is ever computed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::json;

use super::classes::Ctx;
use super::ids::finding_id;
use super::snapshot::LintEpisode;
use super::{Category, CategoryResult, Citations, Finding};

/// A block larger than this is skipped (and counted) rather than compared all-pairs.
pub const MAX_BLOCK: usize = 200;

/// Entities with no live mention and no believed relationship.
///
/// A live mention counts whatever its origin: orphanhood is about the entity having *any* live
/// footing in the graph, not about corroboration (that is `weak_support`'s question).
pub fn analyse_orphans(ctx: &Ctx<'_>) -> CategoryResult {
    let mut live: HashSet<&str> = HashSet::new();
    let mut superseded: HashMap<&str, Vec<&LintEpisode>> = HashMap::new();
    for (ep_uuid, ent) in &ctx.snap.mentions {
        let Some(ep) = ctx.ep_by_uuid.get(ep_uuid.as_str()) else {
            continue;
        };
        if ep.is_live(ctx.as_of_us) {
            live.insert(ent.as_str());
        } else {
            superseded.entry(ent.as_str()).or_default().push(ep);
        }
    }
    let mut endpoints: HashSet<&str> = HashSet::new();
    for (i, e) in ctx.snap.edges.iter().enumerate() {
        if ctx.believed[i] {
            endpoints.insert(e.source.as_str());
            endpoints.insert(e.target.as_str());
        }
    }

    let mut out = Vec::new();
    for ent in &ctx.snap.entities {
        if live.contains(ent.uuid.as_str()) || endpoints.contains(ent.uuid.as_str()) {
            continue;
        }
        let former = superseded.get(ent.uuid.as_str());
        let mut cites = Citations::default();
        cites.entities.insert(ent.uuid.clone());
        for ep in former.into_iter().flatten() {
            cites.episode(ep);
        }
        out.push(Finding {
            id: finding_id(Category::OrphanEntity.as_str(), &ent.group_id, &[&ent.uuid]),
            category: Category::OrphanEntity,
            group_id: ent.group_id.clone(),
            reason: if former.is_some() {
                "support_lost"
            } else {
                "never_supported"
            },
            details: json!({
                "superseded_mentions": former.map_or(0, Vec::len),
            }),
            citations: cites,
        });
    }
    CategoryResult::ok(out)
}

/// Lowercase, punctuation stripped, whitespace collapsed, tokens sorted.
pub fn normalise_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect();
    let mut tokens: Vec<&str> = cleaned.split_whitespace().collect();
    tokens.sort_unstable();
    tokens.join(" ")
}

/// `1 - edit_distance / max_len` over chars; 1.0 for identical strings.
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let max = a.len().max(b.len());
    if max == 0 {
        return 1.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    1.0 - prev[b.len()] as f64 / max as f64
}

/// Blocking keys of a normalised name: each token of length ≥ 4, and the first three characters of
/// the sorted-token key. Independent of the threshold.
fn block_keys(norm: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for t in norm.split(' ').filter(|t| t.chars().count() >= 4) {
        keys.insert(format!("t:{t}"));
    }
    if !norm.is_empty() {
        keys.insert(format!("p:{}", norm.chars().take(3).collect::<String>()));
    }
    keys
}

/// Candidate merge pairs at or above `threshold`, same group and same kind, each emitted once in
/// canonical `(uuid_a < uuid_b)` order.
pub fn analyse_near_duplicates(ctx: &Ctx<'_>, threshold: f64) -> CategoryResult {
    let mut ents: Vec<(&str, &str, &str, &str, String)> = ctx
        .snap
        .entities
        .iter()
        .map(|e| {
            (
                e.group_id.as_str(),
                e.kind.as_str(),
                e.uuid.as_str(),
                e.name.as_str(),
                normalise_name(&e.name),
            )
        })
        .filter(|e| !e.4.is_empty())
        .collect();
    ents.sort_by(|a, b| a.2.cmp(b.2));

    // (group, kind, key) → entity indices (ascending, since `ents` is sorted by uuid).
    let mut blocks: BTreeMap<(&str, &str, String), Vec<usize>> = BTreeMap::new();
    for (i, e) in ents.iter().enumerate() {
        for k in block_keys(&e.4) {
            blocks.entry((e.0, e.1, k)).or_default().push(i);
        }
    }

    let mut skipped_blocks = 0usize;
    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for members in blocks.values() {
        if members.len() > MAX_BLOCK {
            skipped_blocks += 1;
            continue;
        }
        for (x, &i) in members.iter().enumerate() {
            for &j in &members[x + 1..] {
                pairs.insert((i, j));
            }
        }
    }

    let mut out = Vec::new();
    for (i, j) in pairs {
        let (a, b) = (&ents[i], &ents[j]);
        // Exact upper bound on the score from lengths alone: skip hopeless pairs cheaply.
        let (la, lb) = (a.4.chars().count(), b.4.chars().count());
        let bound = la.min(lb) as f64 / la.max(lb) as f64;
        if bound < threshold {
            continue;
        }
        let score = similarity(&a.4, &b.4);
        if score < threshold {
            continue;
        }
        let mut cites = Citations::default();
        cites.entities.insert(a.2.to_string());
        cites.entities.insert(b.2.to_string());
        out.push(Finding {
            id: finding_id(Category::NearDuplicateEntity.as_str(), a.0, &[a.2, b.2]),
            category: Category::NearDuplicateEntity,
            group_id: a.0.to_string(),
            reason: "similar_name",
            details: json!({
                "score": (score * 1e6).round() / 1e6,
                "signal": "normalized_name",
                "kind": a.1,
                "names": [a.3, b.3],
            }),
            citations: cites,
        });
    }
    let mut r = CategoryResult::ok(out);
    r.extra = Some(json!({"skipped_blocks": skipped_blocks}));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation_ignores_case_punctuation_and_token_order() {
        assert_eq!(normalise_name("Cook, Tim"), normalise_name("tim  COOK"));
        assert_eq!(normalise_name("  "), "");
    }

    #[test]
    fn similarity_bounds() {
        assert_eq!(similarity("abc", "abc"), 1.0);
        assert!(similarity("tim cook", "tim cooke") > 0.85);
        assert!(similarity("alice", "zzzzz") < 0.2);
        assert_eq!(similarity("", ""), 1.0);
    }
}
