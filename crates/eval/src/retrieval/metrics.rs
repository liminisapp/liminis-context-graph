//! Pure retrieval metrics (FR-002, FR-013, FR-014, FR-017).
//!
//! Rules, recorded in ADR-0754:
//! * The ranked note list is de-duplicated keeping the first occurrence, then truncated to k.
//!   Fewer than k results are not padded.
//! * Questions with no relevant note ("negative") are **excluded** from recall/MRR/nDCG means and
//!   scored separately by `false_positive` (1 when anything is returned, else 0).
//! * nDCG uses graded gain `2^gain - 1` with a log2 discount; IDCG is over the judged set at k.
//! * `stale_above_current`: the best-ranked stale note outranks the best-ranked current note. A
//!   stale hit with no current hit counts as true; no stale hit counts as false.
//! * No function here divides by zero; undefined values are `None`.

use std::collections::{BTreeMap, HashSet};

use super::fixture::{Category, Question, Relevant};

/// De-duplicates keeping first occurrence and keeps at most `k`.
pub fn dedupe_truncate(ranked: &[String], k: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for n in ranked {
        if seen.insert(n.as_str()) {
            out.push(n.clone());
            if out.len() == k {
                break;
            }
        }
    }
    out
}

pub fn recall_at_k(ranked: &[String], rel: &[Relevant]) -> Option<f64> {
    if rel.is_empty() {
        return None;
    }
    let found = rel.iter().filter(|r| ranked.contains(&r.note)).count();
    Some(found as f64 / rel.len() as f64)
}

pub fn mrr(ranked: &[String], rel: &[Relevant]) -> Option<f64> {
    if rel.is_empty() {
        return None;
    }
    let pos = ranked.iter().position(|n| rel.iter().any(|r| r.note == *n));
    Some(pos.map(|p| 1.0 / (p as f64 + 1.0)).unwrap_or(0.0))
}

fn gain_value(g: u32) -> f64 {
    (2f64).powi(g as i32) - 1.0
}

pub fn ndcg_at_k(ranked: &[String], rel: &[Relevant], k: usize) -> Option<f64> {
    if rel.is_empty() {
        return None;
    }
    let dcg: f64 = ranked
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, n)| {
            let g = rel
                .iter()
                .find(|r| r.note == *n)
                .map(|r| gain_value(r.gain))
                .unwrap_or(0.0);
            g / ((i as f64 + 2.0).log2())
        })
        .sum();
    let mut gains: Vec<f64> = rel.iter().map(|r| gain_value(r.gain)).collect();
    gains.sort_by(|a, b| b.partial_cmp(a).expect("gains are finite"));
    let idcg: f64 = gains
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, g)| g / ((i as f64 + 2.0).log2()))
        .sum();
    if idcg == 0.0 {
        None
    } else {
        Some(dcg / idcg)
    }
}

/// 1.0 when a negative question received any result, else 0.0.
pub fn false_positive(ranked: &[String]) -> f64 {
    if ranked.is_empty() {
        0.0
    } else {
        1.0
    }
}

fn best_rank(ranked: &[String], set: &[String]) -> Option<usize> {
    ranked.iter().position(|n| set.contains(n))
}

pub fn stale_above_current(ranked: &[String], current: &[String], stale: &[String]) -> bool {
    match (best_rank(ranked, stale), best_rank(ranked, current)) {
        (Some(_), None) => true,
        (Some(s), Some(c)) => s < c,
        _ => false,
    }
}

pub fn stale_in_top_k(ranked: &[String], stale: &[String]) -> bool {
    best_rank(ranked, stale).is_some()
}

/// `support_ranks[i]` is the 0-based rank of the first returned item the judge accepted for
/// evidence spec `i`, or `None`. Returns (grounded recall, grounded MRR).
pub fn grounded(support_ranks: &[Option<usize>]) -> Option<(f64, f64)> {
    if support_ranks.is_empty() {
        return None;
    }
    let supported = support_ranks.iter().filter(|r| r.is_some()).count();
    let first = support_ranks.iter().flatten().min();
    Some((
        supported as f64 / support_ranks.len() as f64,
        first.map(|r| 1.0 / (*r as f64 + 1.0)).unwrap_or(0.0),
    ))
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuestionScore {
    pub id: String,
    pub category: Category,
    pub recall: Option<f64>,
    pub mrr: Option<f64>,
    pub ndcg: Option<f64>,
    pub false_positive: Option<f64>,
    pub stale_above_current: Option<bool>,
    pub stale_in_top_k: Option<bool>,
    pub grounded_recall: Option<f64>,
    pub grounded_mrr: Option<f64>,
}

/// Scores one question. `grounded_ranks` is `None` when grounded judging was not run (no judge
/// available, or not an answer question).
pub fn score_question(
    q: &Question,
    result_notes: &[String],
    k: usize,
    grounded_ranks: Option<&[Option<usize>]>,
) -> QuestionScore {
    let ranked = dedupe_truncate(result_notes, k);
    let (g_recall, g_mrr) = grounded_ranks
        .and_then(grounded)
        .map(|(a, b)| (Some(a), Some(b)))
        .unwrap_or((None, None));
    let temporal = q.category == Category::Temporal;
    QuestionScore {
        id: q.id.clone(),
        category: q.category,
        recall: recall_at_k(&ranked, &q.relevant),
        mrr: mrr(&ranked, &q.relevant),
        ndcg: ndcg_at_k(&ranked, &q.relevant, k),
        false_positive: q.negative.then(|| false_positive(&ranked)),
        stale_above_current: temporal.then(|| stale_above_current(&ranked, &q.current, &q.stale)),
        stale_in_top_k: temporal.then(|| stale_in_top_k(&ranked, &q.stale)),
        grounded_recall: g_recall,
        grounded_mrr: g_mrr,
    }
}

/// Means over the questions for which each metric is defined, with the sample size of each.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    pub questions: usize,
    pub n_relevance: usize,
    pub recall: Option<f64>,
    pub mrr: Option<f64>,
    pub ndcg: Option<f64>,
    pub n_negative: usize,
    pub false_positive_rate: Option<f64>,
    pub n_temporal: usize,
    pub stale_above_current_rate: Option<f64>,
    pub stale_in_top_k_rate: Option<f64>,
    pub n_grounded: usize,
    pub grounded_recall: Option<f64>,
    pub grounded_mrr: Option<f64>,
}

fn mean(xs: impl Iterator<Item = f64>) -> (usize, Option<f64>) {
    let v: Vec<f64> = xs.collect();
    if v.is_empty() {
        (0, None)
    } else {
        (v.len(), Some(v.iter().sum::<f64>() / v.len() as f64))
    }
}

pub fn summarize(scores: &[&QuestionScore]) -> Summary {
    let (n_relevance, recall) = mean(scores.iter().filter_map(|s| s.recall));
    let (_, mrr_m) = mean(scores.iter().filter_map(|s| s.mrr));
    let (_, ndcg) = mean(scores.iter().filter_map(|s| s.ndcg));
    let (n_negative, fp) = mean(scores.iter().filter_map(|s| s.false_positive));
    let (n_temporal, stale) = mean(
        scores
            .iter()
            .filter_map(|s| s.stale_above_current)
            .map(|b| b as u8 as f64),
    );
    let (_, stale_k) = mean(
        scores
            .iter()
            .filter_map(|s| s.stale_in_top_k)
            .map(|b| b as u8 as f64),
    );
    let (n_grounded, g_recall) = mean(scores.iter().filter_map(|s| s.grounded_recall));
    let (_, g_mrr) = mean(scores.iter().filter_map(|s| s.grounded_mrr));
    Summary {
        questions: scores.len(),
        n_relevance,
        recall,
        mrr: mrr_m,
        ndcg,
        n_negative,
        false_positive_rate: fp,
        n_temporal,
        stale_above_current_rate: stale,
        stale_in_top_k_rate: stale_k,
        n_grounded,
        grounded_recall: g_recall,
        grounded_mrr: g_mrr,
    }
}

/// Overall summary plus one per category (ordered by `Category`).
pub fn summarize_by_category(scores: &[QuestionScore]) -> (Summary, BTreeMap<Category, Summary>) {
    let all: Vec<&QuestionScore> = scores.iter().collect();
    let mut by: BTreeMap<Category, Vec<&QuestionScore>> = BTreeMap::new();
    for s in scores {
        by.entry(s.category).or_default().push(s);
    }
    (
        summarize(&all),
        by.into_iter().map(|(c, v)| (c, summarize(&v))).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: &str, g: u32) -> Relevant {
        Relevant {
            note: n.into(),
            gain: g,
        }
    }
    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn dedupe_keeps_first_and_truncates() {
        assert_eq!(
            dedupe_truncate(&v(&["a", "b", "a", "c", "d"]), 3),
            v(&["a", "b", "c"])
        );
        assert_eq!(dedupe_truncate(&v(&["a"]), 5), v(&["a"]));
        assert!(dedupe_truncate(&[], 5).is_empty());
    }

    #[test]
    fn recall_handles_multiple_empty_and_short() {
        let rel = [r("a", 1), r("b", 1)];
        assert_eq!(recall_at_k(&v(&["a", "x"]), &rel), Some(0.5));
        assert_eq!(recall_at_k(&[], &rel), Some(0.0));
        assert_eq!(recall_at_k(&v(&["a"]), &[]), None);
    }

    #[test]
    fn mrr_uses_first_relevant_rank() {
        let rel = [r("b", 1)];
        assert_eq!(mrr(&v(&["a", "b"]), &rel), Some(0.5));
        assert_eq!(mrr(&v(&["a"]), &rel), Some(0.0));
        assert_eq!(mrr(&v(&["a"]), &[]), None);
    }

    #[test]
    fn ndcg_perfect_and_imperfect() {
        let rel = [r("a", 2), r("b", 1)];
        assert!((ndcg_at_k(&v(&["a", "b"]), &rel, 10).unwrap() - 1.0).abs() < 1e-12);
        let swapped = ndcg_at_k(&v(&["b", "a"]), &rel, 10).unwrap();
        assert!(swapped < 1.0 && swapped > 0.0);
        assert_eq!(ndcg_at_k(&v(&["x"]), &rel, 10), Some(0.0));
        assert_eq!(ndcg_at_k(&v(&["a"]), &[], 10), None);
    }

    #[test]
    fn ndcg_respects_k_cutoff() {
        let rel = [r("a", 1)];
        assert_eq!(ndcg_at_k(&v(&["x", "a"]), &rel, 1), Some(0.0));
    }

    #[test]
    fn stale_measure_cases() {
        let cur = v(&["new"]);
        let stale = v(&["old"]);
        assert!(stale_above_current(&v(&["old", "new"]), &cur, &stale));
        assert!(!stale_above_current(&v(&["new", "old"]), &cur, &stale));
        assert!(
            stale_above_current(&v(&["old"]), &cur, &stale),
            "stale only"
        );
        assert!(
            !stale_above_current(&v(&["new"]), &cur, &stale),
            "current only"
        );
        assert!(!stale_above_current(&[], &cur, &stale), "neither");
        assert!(stale_in_top_k(&v(&["new", "old"]), &stale));
        assert!(!stale_in_top_k(&v(&["new"]), &stale));
    }

    #[test]
    fn grounded_metrics() {
        assert_eq!(grounded(&[]), None);
        assert_eq!(grounded(&[Some(0), None]), Some((0.5, 1.0)));
        assert_eq!(grounded(&[None]), Some((0.0, 0.0)));
        assert_eq!(grounded(&[Some(3), Some(1)]), Some((1.0, 0.5)));
    }

    fn q(id: &str, cat: Category, rel: Vec<Relevant>, negative: bool) -> Question {
        Question {
            id: id.into(),
            category: cat,
            text: "t".into(),
            relevant: rel,
            evidence: vec![],
            current: if cat == Category::Temporal {
                v(&["new"])
            } else {
                vec![]
            },
            stale: if cat == Category::Temporal {
                v(&["old"])
            } else {
                vec![]
            },
            negative,
        }
    }

    #[test]
    fn negatives_are_excluded_from_relevance_means() {
        let qs = [
            q("q1", Category::Direct, vec![r("a", 1)], false),
            q("q2", Category::Direct, vec![], true),
        ];
        let scores: Vec<_> = qs
            .iter()
            .zip([v(&["a"]), v(&["zzz"])])
            .map(|(q, res)| score_question(q, &res, 10, None))
            .collect();
        let (all, by) = summarize_by_category(&scores);
        assert_eq!(all.n_relevance, 1);
        assert_eq!(all.recall, Some(1.0));
        assert_eq!(all.n_negative, 1);
        assert_eq!(all.false_positive_rate, Some(1.0));
        assert_eq!(by[&Category::Direct].questions, 2);
        assert!(!by.contains_key(&Category::Temporal));
    }

    #[test]
    fn temporal_question_gets_stale_measures() {
        let t = q("t1", Category::Temporal, vec![r("new", 1)], false);
        let s = score_question(&t, &v(&["old", "new"]), 10, None);
        assert_eq!(s.stale_above_current, Some(true));
        assert_eq!(s.stale_in_top_k, Some(true));
        assert_eq!(s.recall, Some(1.0));
        let (_, by) = summarize_by_category(&[s]);
        assert_eq!(by[&Category::Temporal].stale_above_current_rate, Some(1.0));
    }

    #[test]
    fn scoring_is_bit_deterministic() {
        let t = q(
            "t1",
            Category::Temporal,
            vec![r("new", 2), r("x", 1)],
            false,
        );
        let res = v(&["old", "x", "new", "x"]);
        let a = score_question(&t, &res, 3, Some(&[Some(1)]));
        let b = score_question(&t, &res, 3, Some(&[Some(1)]));
        assert_eq!(a, b);
        assert_eq!(a.ndcg.unwrap().to_bits(), b.ndcg.unwrap().to_bits());
    }

    #[test]
    fn empty_input_summarises_without_panic() {
        let (all, by) = summarize_by_category(&[]);
        assert_eq!(all.questions, 0);
        assert_eq!(all.recall, None);
        assert!(by.is_empty());
    }
}
