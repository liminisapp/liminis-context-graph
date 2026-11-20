//! Belief- and recency-aware blending of cross-encoder scores for passage search (issue #796).
//!
//! A cross-encoder scores text match only; it cannot tell a superseded note from its current
//! replacement (qmd's reranker collapsed to nDCG 0.293 on temporal questions for exactly this
//! reason). This module combines the reranker's logit with LCG's belief state (ADR-0705) and a
//! small recency prior, then enforces a hard guarantee that a replacement ranks above the note it
//! superseded whenever both are in the reranked head.
//!
//! Everything here is pure: no I/O, no clock (the caller passes `now_us`), fully deterministic.
//!
//! ## Score
//!
//! ```text
//! p(r)   = (1/rank - 1/n) / (1 - 1/n)           // first-stage reciprocal rank, 1 at the top, 0 at the end of the head
//! s      = a(r) * p(r) + (1 - a(r)) * sigmoid(logit)
//! s     *= superseded_factor   if superseded    // default 0.5
//! s     += recency_weight * t                   // default 0.05; t in 0..1, rank-normalised
//! ```
//!
//! `a(r)` is the weight of the first stage at 1-based rank `r`: `0.30` for ranks 1-3, `0.20` for
//! ranks 4-10 and `0.10` beyond (all [`BlendConfig`] fields; tuned on corpus v2 with
//! mxbai-rerank-large-v1 at N = 50, see ADR-0796). A cross-encoder alone can bury a
//! right answer the first stage already ranked first (bge-reranker-base put the right note first
//! for only 5 of 40 paraphrase questions), so the first-stage rank is mixed in, most strongly at
//! the top of the list; a large logit gap still promotes a candidate from deep in the head.
//!
//! `t` is the candidate's rank-normalised timestamp *within the head* (`valid_at`, falling back to
//! `created_at`); a missing timestamp, or a head of one, gives the neutral `0.5`. The recency term
//! is a near-tiebreak on purpose: implicit revisions (a plain later note with no supersession
//! record) can only be recognised by recency, but a newer irrelevant note must not beat a clearly
//! better text match. Ties break by first-stage rank, then uuid.
//!
//! The band edges make `a(r)` a step function, so with *equal* logits the order is preserved
//! within a band but can shift across a band edge (a deeper candidate's smaller first-stage weight
//! lets the shared logit count for more). Real logits rarely tie; the weights are tuning knobs.

use std::collections::HashMap;

use crate::belief::EpisodeSupersession;

/// Default multiplier applied to a superseded (or pending) passage's whole blended score (rank prior and squashed logit).
pub const DEFAULT_SUPERSEDED_FACTOR: f64 = 0.5;
/// Default weight of the recency prior.
pub const DEFAULT_RECENCY_WEIGHT: f64 = 0.05;

/// Default first-stage weight at ranks 1-3.
pub const DEFAULT_RANK_WEIGHT_TOP: f64 = 0.30;
/// Default first-stage weight at ranks 4-10.
pub const DEFAULT_RANK_WEIGHT_MID: f64 = 0.20;
/// Default first-stage weight beyond rank 10.
pub const DEFAULT_RANK_WEIGHT_TAIL: f64 = 0.10;
/// Last 1-based rank of the "top" band.
pub const DEFAULT_TOP_RANKS: usize = 3;
/// Last 1-based rank of the "mid" band.
pub const DEFAULT_MID_RANKS: usize = 10;

/// Tunables for [`blend`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlendConfig {
    pub superseded_factor: f64,
    pub recency_weight: f64,
    /// Weight of the first-stage rank prior at ranks `1..=top_ranks`.
    pub rank_weight_top: f64,
    /// Weight at ranks `top_ranks+1..=mid_ranks`.
    pub rank_weight_mid: f64,
    /// Weight beyond `mid_ranks`.
    pub rank_weight_tail: f64,
    pub top_ranks: usize,
    pub mid_ranks: usize,
}

impl Default for BlendConfig {
    fn default() -> Self {
        Self {
            superseded_factor: DEFAULT_SUPERSEDED_FACTOR,
            recency_weight: DEFAULT_RECENCY_WEIGHT,
            rank_weight_top: DEFAULT_RANK_WEIGHT_TOP,
            rank_weight_mid: DEFAULT_RANK_WEIGHT_MID,
            rank_weight_tail: DEFAULT_RANK_WEIGHT_TAIL,
            top_ranks: DEFAULT_TOP_RANKS,
            mid_ranks: DEFAULT_MID_RANKS,
        }
    }
}

impl BlendConfig {
    /// First-stage weight at 1-based `rank`.
    fn rank_weight(&self, rank: usize) -> f64 {
        if rank <= self.top_ranks {
            self.rank_weight_top
        } else if rank <= self.mid_ranks {
            self.rank_weight_mid
        } else {
            self.rank_weight_tail
        }
    }

    /// No first-stage prior: candidates are ordered by the cross-encoder alone (plus belief and
    /// recency). The pre-position-aware behaviour, kept for tests and for ablation.
    pub fn cross_encoder_only() -> Self {
        Self {
            rank_weight_top: 0.0,
            rank_weight_mid: 0.0,
            rank_weight_tail: 0.0,
            ..Self::default()
        }
    }
}

/// First-stage reciprocal-rank prior in `0..=1` for the 0-based `index` in a head of `n`:
/// `1` at the top, `0` at the end, `1` for a head of one.
fn rank_prior(index: usize, n: usize) -> f64 {
    if n < 2 {
        return 1.0;
    }
    let inv_n = 1.0 / n as f64;
    ((1.0 / (index + 1) as f64 - inv_n) / (1.0 - inv_n)).clamp(0.0, 1.0)
}

/// A passage's belief state at query time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeliefState {
    /// Visible and not superseded as of now (a supersession with a future effective date is still
    /// in force — ADR-0707).
    Current,
    /// Superseded as of now, or written under a document revision that never committed.
    Superseded,
    /// No belief information (absent from the lookup): neither boosted nor demoted.
    Unknown,
}

impl BeliefState {
    /// Wire value, `None` for [`BeliefState::Unknown`] (omitted from the response).
    pub fn as_wire(self) -> Option<&'static str> {
        match self {
            BeliefState::Current => Some("current"),
            BeliefState::Superseded => Some("superseded"),
            BeliefState::Unknown => None,
        }
    }
}

/// One first-stage candidate handed to the reranker.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub uuid: String,
    /// Raw cross-encoder logit.
    pub logit: f64,
    /// `valid_at` (falling back to `created_at`) in microseconds since the epoch, if known.
    pub time_us: Option<i64>,
    /// Supersession row, `None` when the passage is absent from the lookup.
    pub belief: Option<EpisodeSupersession>,
}

/// A candidate's blended outcome. `index` refers to the input slice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blended {
    pub index: usize,
    pub score: f64,
    pub state: BeliefState,
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Belief state of one candidate at `now_us`.
pub fn belief_state(belief: Option<&EpisodeSupersession>, now_us: i64) -> BeliefState {
    match belief {
        None => BeliefState::Unknown,
        Some(ep) if ep.is_pending() || !ep.live_at(now_us) => BeliefState::Superseded,
        Some(_) => BeliefState::Current,
    }
}

/// Rank-normalised recency in `0..=1` per candidate. Candidates without a timestamp, and every
/// candidate when fewer than two have one, get the neutral `0.5`. Equal timestamps share a value.
fn recency_priors(cands: &[Candidate]) -> Vec<f64> {
    let times: Vec<i64> = cands.iter().filter_map(|c| c.time_us).collect();
    if times.len() < 2 {
        return vec![0.5; cands.len()];
    }
    let denom = (times.len() - 1) as f64;
    cands
        .iter()
        .map(|c| match c.time_us {
            None => 0.5,
            Some(t) => times.iter().filter(|x| **x < t).count() as f64 / denom,
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|r: f64| r.clamp(0.0, 1.0))
        .collect()
}

/// For each candidate, the input indices that replace it (are expected to rank above it).
fn replacements(cands: &[Candidate], states: &[BeliefState]) -> Vec<Vec<usize>> {
    let by_uuid: HashMap<&str, usize> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (c.uuid.as_str(), i))
        .collect();
    cands
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let Some(ep) = c.belief.as_ref() else {
                return Vec::new();
            };
            if states[i] != BeliefState::Superseded || ep.is_pending() {
                return Vec::new();
            }
            let Some(by) = ep.superseded_by.as_deref() else {
                return Vec::new();
            };
            let usable =
                |j: usize| j != i && !cands[j].belief.as_ref().is_some_and(|b| b.is_pending());
            match ep.kind.as_deref() {
                // Episode supersession: `superseded_by` is the replacing episode's uuid.
                Some("episode") => by_uuid
                    .get(by)
                    .copied()
                    .filter(|j| usable(*j))
                    .into_iter()
                    .collect(),
                // Document supersession: `superseded_by` is the replacing document's id; every
                // candidate episode of that document is a replacement.
                Some("document") => cands
                    .iter()
                    .enumerate()
                    .filter(|(j, c2)| {
                        usable(*j)
                            && c2
                                .belief
                                .as_ref()
                                .and_then(|b| b.doc_id.as_deref())
                                .is_some_and(|d| d == by)
                    })
                    .map(|(j, _)| j)
                    .collect(),
                _ => Vec::new(),
            }
        })
        .collect()
}

/// Blends reranker logits with belief and recency and returns the candidates best-first.
///
/// `cands` is in first-stage order (index = first-stage rank). The returned vector is a
/// permutation of `0..cands.len()`.
pub fn blend(cands: &[Candidate], now_us: i64, cfg: BlendConfig) -> Vec<Blended> {
    let states: Vec<BeliefState> = cands
        .iter()
        .map(|c| belief_state(c.belief.as_ref(), now_us))
        .collect();
    let priors = recency_priors(cands);

    let mut out: Vec<Blended> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let a = cfg.rank_weight(i + 1);
            let mut s = a * rank_prior(i, cands.len()) + (1.0 - a) * sigmoid(c.logit);
            if states[i] == BeliefState::Superseded {
                s *= cfg.superseded_factor;
            }
            s += cfg.recency_weight * priors[i];
            Blended {
                index: i,
                score: s,
                state: states[i],
            }
        })
        .collect();

    out.sort_by(|a, b| {
        // `total_cmp` is a total order even for NaN, so the sort is well defined for any input (the
        // production path rejects non-finite logits before they get here).
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.index.cmp(&b.index))
            .then_with(|| cands[a.index].uuid.cmp(&cands[b.index].uuid))
    });

    enforce_replacement_above_superseded(&mut out, &replacements(cands, &states));
    out
}

/// Hard guarantee (FR-005): whenever a superseded passage and one of its replacements are both
/// present, the replacement ranks above it. A violating superseded passage is moved to just below
/// its best-ranked replacement. Superseded passages only ever move down and supersession is
/// acyclic, so this terminates; the pass cap is a belt-and-braces bound.
fn enforce_replacement_above_superseded(order: &mut Vec<Blended>, repl: &[Vec<usize>]) {
    let max_passes = order.len().saturating_mul(order.len()).max(1);
    for _ in 0..max_passes {
        let pos: HashMap<usize, usize> = order
            .iter()
            .enumerate()
            .map(|(p, b)| (b.index, p))
            .collect();
        let violation = order.iter().enumerate().find_map(|(p, b)| {
            // The best-ranked replacement: the guarantee is that *a* replacement ranks above the
            // superseded passage, so one weak chunk of a multi-chunk replacement document must not
            // sink it.
            let best = repl[b.index]
                .iter()
                .filter_map(|r| pos.get(r).copied())
                .min()?;
            (best > p).then_some((p, best))
        });
        match violation {
            None => return,
            Some((from, after)) => {
                let item = order.remove(from);
                // After removal the replacement shifted up by one; insert right below it.
                order.insert(after, item);
            }
        }
    }
}

/// Assembles the final ordering: the reranked head first, then the un-reranked tail in
/// first-stage order, truncated to `num_results`. Indices refer to the first-stage list; `head`
/// is the best-first permutation of `0..head_len` and the tail is `head_len..total`. Duplicates
/// are impossible by construction but guarded anyway.
pub fn assemble(
    head_order: &[usize],
    head_len: usize,
    total: usize,
    num_results: usize,
) -> Vec<usize> {
    let mut seen = std::collections::HashSet::new();
    head_order
        .iter()
        .copied()
        .chain(head_len..total)
        .filter(|i| seen.insert(*i))
        .take(num_results)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000_000_000;

    fn live() -> EpisodeSupersession {
        EpisodeSupersession::default()
    }

    fn superseded_by_episode(by: &str) -> EpisodeSupersession {
        EpisodeSupersession {
            superseded_at_us: Some(NOW - 10),
            kind: Some("episode".into()),
            superseded_by: Some(by.into()),
            ..Default::default()
        }
    }

    fn cand(uuid: &str, logit: f64, belief: Option<EpisodeSupersession>) -> Candidate {
        Candidate {
            uuid: uuid.into(),
            logit,
            time_us: None,
            belief,
        }
    }

    /// The cross-encoder alone, so the older tests keep pinning what they were written for.
    fn pure() -> BlendConfig {
        BlendConfig::cross_encoder_only()
    }

    /// The first-stage-heavy weights the position-aware tests below were written for (the shipped
    /// defaults are lighter, see ADR-0796); the properties they pin are about the mechanism.
    fn heavy() -> BlendConfig {
        BlendConfig {
            rank_weight_top: 0.75,
            rank_weight_mid: 0.60,
            rank_weight_tail: 0.40,
            ..BlendConfig::default()
        }
    }

    fn order(b: &[Blended]) -> Vec<usize> {
        b.iter().map(|x| x.index).collect()
    }

    /// Superseded at first-stage rank 1 with no replacement link, so only the score can demote it:
    /// the factor must apply to the whole blended score (rank prior included), not just the logit.
    #[test]
    fn superseded_factor_applies_to_the_whole_blended_score() {
        let mut c = vec![cand("old", 0.0, Some(superseded_by_episode("elsewhere")))];
        for u in ["b", "c", "d"] {
            c.push(cand(u, 0.0, Some(live())));
        }
        c.push(cand("new", 0.0, Some(live()))); // rank 5, equal logits
        let b = blend(&c, NOW, BlendConfig::default());
        let pos = |i: usize| b.iter().position(|x| x.index == i).unwrap();
        assert!(
            pos(4) < pos(0),
            "replacement must end above the superseded rank-1 note: {b:?}"
        );
    }

    /// A document revision whose replacing document contributes a good chunk and a weak one: the
    /// superseded passage only has to rank below the best replacement, not the worst.
    #[test]
    fn superseded_sinks_only_below_its_best_replacement() {
        let old = EpisodeSupersession {
            superseded_at_us: Some(NOW - 10),
            kind: Some("document".into()),
            superseded_by: Some("doc2".into()),
            ..Default::default()
        };
        let chunk = |doc: &str| EpisodeSupersession {
            doc_id: Some(doc.into()),
            ..Default::default()
        };
        let mut c = vec![
            cand("new1", 6.0, Some(chunk("doc2"))),
            cand("old", 8.0, Some(old)),
        ];
        for i in 0..20 {
            c.push(cand(&format!("x{i:02}"), 0.0, Some(live())));
        }
        c.push(cand("new2", -9.0, Some(chunk("doc2")))); // unrelated chunk, deep in the head
        let b = blend(&c, NOW, pure());
        let pos = |i: usize| b.iter().position(|x| x.index == i).unwrap();
        assert!(
            pos(0) < pos(1),
            "best replacement above the superseded note"
        );
        assert!(
            pos(1) < pos(22),
            "superseded note not sunk below the weak chunk: {:?}",
            order(&b)
        );
    }

    /// A NaN logit must not break the sort: the result is still a permutation of the input.
    #[test]
    fn a_nan_logit_still_yields_a_permutation() {
        let c = vec![
            cand("a", 1.0, None),
            cand("b", f64::NAN, None),
            cand("c", -1.0, None),
            cand("d", 0.5, None),
        ];
        let b = blend(&c, NOW, pure());
        let mut idx = order(&b);
        idx.sort_unstable();
        assert_eq!(idx, vec![0, 1, 2, 3]);
        assert_eq!(order(&blend(&c, NOW, pure())), order(&b), "deterministic");
    }

    #[test]
    fn higher_logit_wins_when_beliefs_equal() {
        let c = vec![
            cand("a", 0.0, None),
            cand("b", 3.0, None),
            cand("c", -2.0, None),
        ];
        assert_eq!(order(&blend(&c, NOW, pure())), vec![1, 0, 2]);
    }

    #[test]
    fn superseded_is_demoted_below_a_moderately_worse_current_note() {
        // sigmoid(1.0)*0.5 = 0.365 < sigmoid(0.2) = 0.55
        let c = vec![
            cand("old", 1.0, Some(superseded_by_episode("elsewhere"))),
            cand("cur", 0.2, Some(live())),
        ];
        let b = blend(&c, NOW, pure());
        assert_eq!(order(&b), vec![1, 0]);
        assert_eq!(b[0].state, BeliefState::Current);
        assert_eq!(b[1].state, BeliefState::Superseded);
    }

    #[test]
    fn replacement_is_forced_above_superseded_even_with_a_large_text_gap() {
        let c = vec![
            cand("old", 8.0, Some(superseded_by_episode("new"))),
            cand("new", -4.0, Some(live())),
        ];
        assert_eq!(order(&blend(&c, NOW, pure())), vec![1, 0]);
    }

    #[test]
    fn document_replacement_matched_by_doc_id() {
        let old = EpisodeSupersession {
            superseded_at_us: Some(NOW - 1),
            kind: Some("document".into()),
            superseded_by: Some("doc-v2".into()),
            doc_id: Some("doc-v1".into()),
            ..Default::default()
        };
        let new = EpisodeSupersession {
            doc_id: Some("doc-v2".into()),
            ..Default::default()
        };
        let c = vec![cand("old", 9.0, Some(old)), cand("new", -9.0, Some(new))];
        assert_eq!(order(&blend(&c, NOW, pure())), vec![1, 0]);
    }

    #[test]
    fn future_effective_date_is_still_current() {
        let future = EpisodeSupersession {
            superseded_at_us: Some(NOW + 1_000_000),
            kind: Some("document".into()),
            superseded_by: Some("doc-v2".into()),
            doc_id: Some("doc-v1".into()),
            ..Default::default()
        };
        assert_eq!(belief_state(Some(&future), NOW), BeliefState::Current);
        let c = vec![cand("a", 0.0, Some(future)), cand("b", 2.0, Some(live()))];
        let b = blend(&c, NOW, pure());
        assert_eq!(
            b.iter().find(|x| x.index == 0).unwrap().state,
            BeliefState::Current
        );
        // No demotion, so plain text match decides.
        assert_eq!(order(&b), vec![1, 0]);
    }

    #[test]
    fn pending_revision_is_demoted_and_never_a_replacement() {
        let pending = EpisodeSupersession {
            revision_id_present: true,
            revision_committed_at_us: None,
            ..Default::default()
        };
        assert_eq!(belief_state(Some(&pending), NOW), BeliefState::Superseded);
        let c = vec![
            cand("old", 3.0, Some(superseded_by_episode("pend"))),
            cand("pend", 5.0, Some(pending)),
        ];
        let b = blend(&c, NOW, pure());
        // The pending passage is not a replacement, so the fix-up does not lift it above `old`;
        // both are superseded-state, so text match decides.
        assert_eq!(order(&b), vec![1, 0]);
        let repl = replacements(&c, &[BeliefState::Superseded, BeliefState::Superseded]);
        assert!(repl[0].is_empty());
    }

    #[test]
    fn missing_belief_is_neutral() {
        let c = vec![cand("a", 1.0, None), cand("b", 1.0, Some(live()))];
        let b = blend(&c, NOW, pure());
        assert_eq!(
            b.iter().find(|x| x.index == 0).unwrap().state,
            BeliefState::Unknown
        );
        assert_eq!(BeliefState::Unknown.as_wire(), None);
        // Same logit, same score: first-stage rank decides.
        assert_eq!(order(&b), vec![0, 1]);
    }

    #[test]
    fn recency_breaks_near_ties_for_implicit_revisions() {
        let mut older = cand("older", 1.0, Some(live()));
        older.time_us = Some(100);
        let mut newer = cand("newer", 1.0, Some(live()));
        newer.time_us = Some(200);
        let c = vec![older, newer];
        assert_eq!(order(&blend(&c, NOW, pure())), vec![1, 0]);
    }

    #[test]
    fn recency_does_not_promote_a_newer_irrelevant_note() {
        let mut relevant = cand("relevant", 4.0, Some(live()));
        relevant.time_us = Some(100);
        let mut irrelevant = cand("irrelevant", -4.0, Some(live()));
        irrelevant.time_us = Some(999);
        let c = vec![relevant, irrelevant];
        assert_eq!(order(&blend(&c, NOW, pure())), vec![0, 1]);
    }

    #[test]
    fn recency_prior_is_neutral_for_missing_or_single_timestamps() {
        let mut one = cand("a", 0.0, None);
        one.time_us = Some(5);
        let none = cand("b", 0.0, None);
        assert_eq!(recency_priors(&[one, none]), vec![0.5, 0.5]);
        let mut x = cand("x", 0.0, None);
        x.time_us = Some(1);
        let mut y = cand("y", 0.0, None);
        y.time_us = Some(1);
        let mut z = cand("z", 0.0, None);
        z.time_us = Some(2);
        assert_eq!(recency_priors(&[x, y, z]), vec![0.0, 0.0, 1.0]);
    }

    #[test]
    fn ties_are_deterministic() {
        let c = vec![cand("b", 1.0, None), cand("a", 1.0, None)];
        // Equal score and equal index impossible; first-stage rank decides.
        assert_eq!(order(&blend(&c, NOW, pure())), vec![0, 1]);
        assert_eq!(
            order(&blend(&c, NOW, pure())),
            order(&blend(&c, NOW, pure()))
        );
    }

    #[test]
    fn chain_of_supersession_is_ordered_newest_first() {
        // v1 superseded by v2, v2 superseded by v3; v1 has the best text match.
        let c = vec![
            cand("v1", 6.0, Some(superseded_by_episode("v2"))),
            cand("v2", 3.0, Some(superseded_by_episode("v3"))),
            cand("v3", -3.0, Some(live())),
        ];
        assert_eq!(order(&blend(&c, NOW, pure())), vec![2, 1, 0]);
    }

    #[test]
    fn empty_and_single_inputs() {
        assert!(blend(&[], NOW, pure()).is_empty());
        let b = blend(&[cand("a", 0.0, None)], NOW, pure());
        assert_eq!(order(&b), vec![0]);
    }

    #[test]
    fn non_finite_logits_do_not_panic() {
        let c = vec![
            cand("a", f64::INFINITY, None),
            cand("b", f64::NEG_INFINITY, None),
        ];
        assert_eq!(order(&blend(&c, NOW, pure())), vec![0, 1]);
    }

    #[test]
    fn assemble_appends_tail_in_first_stage_order_without_duplicates() {
        // head of 3 reranked to [2,0,1], total 6, k=5
        assert_eq!(assemble(&[2, 0, 1], 3, 6, 5), vec![2, 0, 1, 3, 4]);
        // k smaller than the head
        assert_eq!(assemble(&[2, 0, 1], 3, 6, 2), vec![2, 0]);
        // fewer candidates than N: tail empty
        assert_eq!(assemble(&[1, 0], 2, 2, 10), vec![1, 0]);
        // none
        assert!(assemble(&[], 0, 0, 10).is_empty());
        // defensive duplicate guard
        assert_eq!(assemble(&[0, 0, 1], 2, 4, 10), vec![0, 1, 2, 3]);
    }

    #[test]
    fn with_equal_logits_the_first_stage_order_is_preserved() {
        // Within one weight band (ranks 4-10) equal logits keep first-stage order, and so does the
        // whole top of a short head.
        let c: Vec<Candidate> = (0..10).map(|i| cand(&format!("u{i}"), 0.0, None)).collect();
        let cfg = BlendConfig {
            recency_weight: 0.0,
            ..heavy()
        };
        assert_eq!(order(&blend(&c, NOW, cfg)), (0..10).collect::<Vec<_>>());
        // Across the 3/4 band edge too, for a neutral or negative logit (the first-stage weight
        // steps down there, so a *high* shared logit can shift a deeper candidate ahead; see the
        // module docs).
        for logit in [0.0, -2.0] {
            let c: Vec<Candidate> = (0..6)
                .map(|i| cand(&format!("u{i}"), logit, None))
                .collect();
            assert_eq!(order(&blend(&c, NOW, cfg)), (0..6).collect::<Vec<_>>());
        }
    }

    #[test]
    fn a_right_answer_ranked_first_survives_a_mediocre_logit() {
        // The cross-encoder-only blend buries rank 1 under a candidate with a higher logit; the
        // position-aware blend keeps it on top unless the logit gap is large.
        let c = vec![
            cand("first", -1.0, None),
            cand("second", 0.5, None),
            cand("third", 0.0, None),
        ];
        assert_eq!(order(&blend(&c, NOW, pure()))[0], 1);
        assert_eq!(order(&blend(&c, NOW, heavy()))[0], 0);
    }

    #[test]
    fn a_large_logit_gap_still_promotes_a_candidate_from_deep_in_the_head() {
        let mut c: Vec<Candidate> = (0..12)
            .map(|i| cand(&format!("u{i}"), -6.0, None))
            .collect();
        c[9].logit = 8.0;
        let b = blend(&c, NOW, heavy());
        // Rank 9 (0-based) jumps over every other candidate except the first-stage top-1, whose
        // weight (0.75) is high enough that no deeper candidate can displace it.
        assert_eq!(order(&b)[..2], [0, 9]);
        // With the first stage switched off it would win outright.
        assert_eq!(blend(&c, NOW, pure())[0].index, 9);
    }

    #[test]
    fn replacement_rule_still_holds_after_position_aware_blending() {
        // `old` is first-stage rank 1 with the best logit; `new` is deep. The hard rule puts the
        // replacement above it whatever the blended scores say.
        let mut c = vec![
            cand("old", 8.0, Some(superseded_by_episode("new"))),
            cand("x", 0.0, None),
            cand("y", 0.0, None),
            cand("new", -8.0, Some(live())),
        ];
        c[0].time_us = Some(1);
        let b = blend(&c, NOW, BlendConfig::default());
        let pos = |idx: usize| b.iter().position(|x| x.index == idx).unwrap();
        assert!(pos(3) < pos(0), "{:?}", order(&b));
    }

    #[test]
    fn rank_prior_spans_zero_to_one() {
        assert_eq!(rank_prior(0, 25), 1.0);
        assert!(rank_prior(24, 25).abs() < 1e-12);
        assert_eq!(rank_prior(0, 1), 1.0);
        assert!(rank_prior(1, 25) > rank_prior(2, 25));
    }

    #[test]
    fn rank_weights_follow_the_bands() {
        let cfg = BlendConfig::default();
        assert_eq!(cfg.rank_weight(1), 0.30);
        assert_eq!(cfg.rank_weight(3), 0.30);
        assert_eq!(cfg.rank_weight(4), 0.20);
        assert_eq!(cfg.rank_weight(10), 0.20);
        assert_eq!(cfg.rank_weight(11), 0.10);
    }
}
