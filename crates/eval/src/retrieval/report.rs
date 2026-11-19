//! Report rendering (`report.md` / `report.json`) for the retrieval eval (FR-015, FR-016, SC-007).
//!
//! #788 adds, all additive: bootstrap 95% confidence intervals on the headline metrics, paired
//! per-question comparisons that label a gap whose interval includes zero "not distinguishable
//! from noise", ceiling and low-n labels per category, and a per-origin section for fixtures that
//! declare origins. Metric *definitions* are unchanged (ADR-0754); only how they are read changes.

use std::collections::BTreeMap;
use std::fmt::Write;

use serde_json::{json, Value};

use super::fixture::Category;
use super::metrics::{QuestionScore, Summary};
use super::origin::OriginData;
use super::stats::{
    at_ceiling, bootstrap_ci, low_n, paired_diff_ci, seed_for, Interval, PairedDiff, Verdict,
    CEILING, DEFAULT_RESAMPLES, DEFAULT_SEED, LEVEL, MIN_CATEGORY_N,
};

/// Bootstrap settings, recorded in the report so a run can be reproduced or changed explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bootstrap {
    pub seed: u64,
    pub resamples: usize,
}

impl Default for Bootstrap {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SEED,
            resamples: DEFAULT_RESAMPLES,
        }
    }
}

/// Reproduction metadata (FR-016).
#[derive(Debug, Clone, Default)]
pub struct RunMeta {
    pub corpus_version: String,
    pub corpus_hash: String,
    pub questions_version: String,
    pub questions_hash: String,
    pub k: usize,
    pub lcg_build: String,
    pub qmd_version: String,
    pub qmd_index: String,
    pub embedder: String,
    pub extractor: String,
    pub qmd_models: String,
    pub judge_model: String,
    pub category_counts: BTreeMap<Category, usize>,
    pub bootstrap: Bootstrap,
    /// Private-notebook run (`--private`): the report carries metrics, counts and hashes only and
    /// withholds the detail of failed queries (a service error can echo query text).
    pub private: bool,
}

#[derive(Debug, Clone)]
pub enum Grounded {
    Judged {
        paid_calls: usize,
        cache_hits: usize,
    },
    Skipped(String),
}

#[derive(Debug, Clone)]
pub struct ConfigRun {
    pub scores: Vec<QuestionScore>,
    pub overall: Summary,
    pub by_category: BTreeMap<Category, Summary>,
    pub latency_p50_ms: Option<f64>,
    pub latency_p95_ms: Option<f64>,
    /// Query-time model calls per query; `None` = not exposed by the system.
    pub model_calls_per_query: Option<f64>,
    /// Service RPCs per query, main reads included (issue #797); `None` = not an LCG service.
    pub service_calls_per_query: Option<f64>,
    pub lookup_calls: u64,
    /// Returned hits with no evidence text, judged unsupported (see `RankedResult::evidence_gaps`).
    pub evidence_gaps: u64,
    pub query_errors: Vec<String>,
    pub grounded: Grounded,
    /// Per-origin reading; `None` when the fixture declares no origins.
    pub origin: Option<OriginData>,
}

#[derive(Debug, Clone)]
pub enum Status {
    Ran(Box<ConfigRun>),
    Skipped(String),
}

#[derive(Debug, Clone)]
pub struct ConfigOutcome {
    pub id: String,
    pub description: String,
    pub status: Status,
}

/// Indexing/ingestion cost, reported separately from per-query cost (FR-003).
#[derive(Debug, Clone, Default)]
pub struct IngestCost {
    pub lcg_seconds: Option<f64>,
    pub lcg_chunks: usize,
    pub lcg_extraction_requests: usize,
    pub lcg_rpc_calls: usize,
    pub lcg_unconfirmed_entity_links: usize,
    /// `knowledge_supersede_document` calls made, and the episodes whose belief they ended
    /// (issue #787). Both zero when no LCG configuration ran.
    pub lcg_supersessions: usize,
    pub lcg_superseded_episodes: usize,
    pub qmd_seconds: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub meta: RunMeta,
    pub ingest: IngestCost,
    pub configs: Vec<ConfigOutcome>,
}

/// Suffix of a caller-supplied-variants passage config (#802): `lcg-passages-variants`,
/// `lcg-passages-belief-variants`, `lcg-passages-hybrid-belief-variants`. A profiled row is
/// `<id>@<profile>`. Each is read against the same config without the suffix.
const VARIANTS_SUFFIX: &str = "-variants";

/// Whether `id` is a variants config (see [`VARIANTS_SUFFIX`]), profiled or not.
pub(crate) fn is_variants_config(id: &str) -> bool {
    id.split('@')
        .next()
        .unwrap_or(id)
        .ends_with(VARIANTS_SUFFIX)
}

/// The config a variants config is read against: the same id without the suffix, with any
/// `@<profile>` kept. `None` for a config that is not a variants config.
pub(crate) fn variants_base_id(id: &str) -> Option<String> {
    let (head, profile) = match id.split_once('@') {
        Some((h, p)) => (h, Some(p)),
        None => (id, None),
    };
    let base = head.strip_suffix(VARIANTS_SUFFIX)?;
    Some(match profile {
        Some(p) => format!("{base}@{p}"),
        None => base.to_string(),
    })
}

/// Nearest-rank percentile over unsorted samples; `None` when empty.
pub fn percentile(samples: &[f64], p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).expect("latencies are finite"));
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    Some(v[rank.min(v.len()) - 1])
}

fn f3(x: Option<f64>) -> String {
    x.map_or("n/a".to_string(), |v| format!("{v:.3}"))
}
fn ms(x: Option<f64>) -> String {
    x.map_or("n/a".to_string(), |v| format!("{v:.0}"))
}
fn ci_txt(iv: Option<Interval>) -> String {
    iv.map_or("[n/a]".to_string(), |i| {
        format!("[{:.3}, {:.3}]", i.lo, i.hi)
    })
}
/// Rounds to 6 decimals so JSON output is stable and short.
fn r6(x: f64) -> Value {
    json!((x * 1e6).round() / 1e6)
}

type QGet = fn(&QuestionScore) -> Option<f64>;

/// Which metric a column is, whether higher or lower is better, and (for the six headline
/// metrics) how to read it off one question for bootstrapping.
#[derive(Clone, Copy)]
struct Col {
    name: &'static str,
    key: &'static str,
    get: fn(&Summary) -> Option<f64>,
    higher_better: bool,
    /// `Some` for headline metrics, which carry a bootstrap CI.
    per_question: Option<QGet>,
}

const COLS: [Col; 8] = [
    Col {
        name: "recall@k",
        key: "recall",
        get: |s| s.recall,
        higher_better: true,
        per_question: Some(|q| q.recall),
    },
    Col {
        name: "MRR",
        key: "mrr",
        get: |s| s.mrr,
        higher_better: true,
        per_question: Some(|q| q.mrr),
    },
    Col {
        name: "nDCG@k",
        key: "ndcg",
        get: |s| s.ndcg,
        higher_better: true,
        per_question: Some(|q| q.ndcg),
    },
    Col {
        name: "grounded-recall@k",
        key: "grounded_recall",
        get: |s| s.grounded_recall,
        higher_better: true,
        per_question: Some(|q| q.grounded_recall),
    },
    Col {
        name: "grounded-MRR",
        key: "grounded_mrr",
        get: |s| s.grounded_mrr,
        higher_better: true,
        per_question: None,
    },
    Col {
        name: "stale>current",
        key: "stale_above_current",
        get: |s| s.stale_above_current_rate,
        higher_better: false,
        per_question: Some(|q| q.stale_above_current.map(|b| b as u8 as f64)),
    },
    Col {
        name: "stale-in-top-k",
        key: "stale_in_top_k",
        get: |s| s.stale_in_top_k_rate,
        higher_better: false,
        per_question: None,
    },
    Col {
        name: "false-pos@k",
        key: "false_positive",
        get: |s| s.false_positive_rate,
        higher_better: false,
        per_question: Some(|q| q.false_positive),
    },
];

const NDCG: QGet = |q| q.ndcg;
const RECALL: QGet = |q| q.recall;

fn best_value(vals: &[Option<f64>], higher: bool) -> Option<f64> {
    let present: Vec<f64> = vals.iter().flatten().copied().collect();
    if present.len() < 2 {
        return None; // nothing to compare against, so nothing to mark
    }
    let best = if higher {
        present.iter().cloned().fold(f64::MIN, f64::max)
    } else {
        present.iter().cloned().fold(f64::MAX, f64::min)
    };
    // Don't mark when every configuration ties.
    if present.iter().all(|v| (*v - best).abs() < 1e-12) {
        None
    } else {
        Some(best)
    }
}

/// How a hypothesis reads once sampling noise is accounted for (FR-020).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// The hypothesis holds: the paired 95% interval is wholly on the stated side.
    Yes,
    /// The opposite holds: the interval is wholly on the other side.
    No,
    /// The paired interval includes zero (or too few shared questions): no difference shown.
    Noise,
    /// A side is missing or skipped.
    CannotRead,
}

impl Reading {
    fn md(self) -> &'static str {
        match self {
            Reading::Yes => "**YES**",
            Reading::No => "**NO** (the opposite holds, 95% interval wholly on the other side)",
            Reading::Noise => {
                "**not distinguishable from noise** (95% paired interval includes zero)"
            }
            Reading::CannotRead => "**cannot be read** (a side is missing or skipped)",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Reading::Yes => "yes",
            Reading::No => "no",
            Reading::Noise => "noise",
            Reading::CannotRead => "cannot_read",
        }
    }
    /// Legacy tri-state for the JSON booleans: true = yes, false = no, null = noise/unreadable.
    fn legacy(self) -> Option<bool> {
        match self {
            Reading::Yes => Some(true),
            Reading::No => Some(false),
            _ => None,
        }
    }
}

/// One paired comparison row.
#[derive(Debug, Clone)]
pub struct Comparison {
    pub a: String,
    pub b: String,
    pub diff: Option<PairedDiff>,
}

/// A skip reason can carry the first raw query error (`all N queries failed; first error: ...`) or
/// a judge error, either of which may echo private text. Under `--private` only the leading,
/// message-free part survives.
fn redact_reason(why: &str, private: bool) -> String {
    if !private {
        return why.to_string();
    }
    match why.split_once("; first error:") {
        Some((head, _)) => format!("{head}; detail withheld"),
        None => "detail withheld".to_string(),
    }
}

/// p50 latency budget for a rerank-enabled passage search at k = 10 on Apple silicon (#796).
pub const RERANK_P50_BUDGET_MS: f64 = 150.0;

/// Outcome of the rerank stage's acceptance criteria (issue #796); see
/// [`Report::rerank_acceptance`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RerankAcceptance {
    pub paraphrase_gain: Option<bool>,
    pub overall_not_lower: Option<bool>,
    pub temporal_no_drop: Option<bool>,
    pub p50_within_budget: Option<bool>,
    pub paraphrase: Option<PairedDiff>,
    pub overall: Option<PairedDiff>,
    pub temporal: Option<PairedDiff>,
    pub p50_ms: Option<f64>,
}

impl Report {
    /// The headroom section (issue #797 steer): a per-question oracle that takes, for each
    /// question, the best nDCG@k any configuration in this run reached, and each configuration's
    /// gap to it. It shows how much a composite that picked the right retriever per question could
    /// gain, and how much of that a given configuration already captures. Omitted with fewer than
    /// two configurations, where the oracle is just the one configuration.
    fn oracle_section(&self, o: &mut String) {
        let ran = self.ran();
        if ran.len() < 2 {
            return;
        }
        let runs: Vec<&[QuestionScore]> = ran.iter().map(|(_, r)| r.scores.as_slice()).collect();
        let oracle = oracle_ndcg(&runs);
        let Some(overall) = oracle.overall else {
            return;
        };
        let _ = writeln!(
            o,
            "## Headroom: per-question oracle
"
        );
        let _ = writeln!(o, "The oracle takes, for each question, the best nDCG@{} any configuration in this run reached, then averages. It is an upper bound for a composite that always picked the right retriever, not a configuration. `gap` is the oracle minus the configuration, over the same questions; `captured` is the configuration as a share of the oracle.
", self.meta.k);
        let _ = writeln!(
            o,
            "Oracle nDCG@{}: **{overall:.3}** overall (n = {}).
",
            self.meta.k, oracle.n
        );
        let _ = writeln!(o, "| config | nDCG@{} | gap | captured |", self.meta.k);
        let _ = writeln!(o, "|---|---|---|---|");
        for (c, r) in &ran {
            let mean = mean_over(&r.scores, &oracle.questions);
            let _ = writeln!(
                o,
                "| `{}` | {mean:.3} | {:.3} | {:.0}% |",
                c.id,
                overall - mean,
                if overall > 0.0 {
                    100.0 * mean / overall
                } else {
                    0.0
                }
            );
        }
        let _ = writeln!(
            o,
            "
| category | oracle | best config | best |"
        );
        let _ = writeln!(o, "|---|---|---|---|");
        for (cat, v) in &oracle.by_category {
            let qs: std::collections::BTreeSet<String> = oracle
                .questions
                .iter()
                .filter(|q| oracle.category_of.get(*q) == Some(cat))
                .cloned()
                .collect();
            let best = ran
                .iter()
                .map(|(c, r)| (c.id.as_str(), mean_over(&r.scores, &qs)))
                .fold(None, |b: Option<(&str, f64)>, (id, m)| match b {
                    Some((_, bm)) if bm >= m => b,
                    _ => Some((id, m)),
                });
            if let Some((id, m)) = best {
                let _ = writeln!(o, "| {} | {v:.3} | `{id}` | {m:.3} |", cat.label());
            }
        }
        let _ = writeln!(o);
    }

    fn ran(&self) -> Vec<(&ConfigOutcome, &ConfigRun)> {
        self.configs
            .iter()
            .filter_map(|c| match &c.status {
                Status::Ran(r) => Some((c, r.as_ref())),
                Status::Skipped(_) => None,
            })
            .collect()
    }

    /// Categories shown in the report: the four original ones always, the rest only when present.
    fn listed_categories(&self) -> Vec<Category> {
        Category::EVERY
            .iter()
            .copied()
            .filter(|c| c.is_legacy() || self.meta.category_counts.get(c).copied().unwrap_or(0) > 0)
            .collect()
    }

    fn cat_label(cat: Option<Category>) -> &'static str {
        cat.map_or("overall", Category::label)
    }

    /// `(question id, value)` series of one metric for a configuration, over scored questions.
    fn series(run: &ConfigRun, cat: Option<Category>, get: QGet) -> Vec<(String, f64)> {
        run.scores
            .iter()
            .filter(|s| cat.is_none_or(|c| s.category == c))
            .filter_map(|s| get(s).map(|v| (s.id.clone(), v)))
            .collect()
    }

    fn interval(
        &self,
        config: &str,
        run: &ConfigRun,
        cat: Option<Category>,
        col: &Col,
    ) -> Option<Interval> {
        let get = col.per_question?;
        let vals: Vec<f64> = Self::series(run, cat, get)
            .into_iter()
            .map(|(_, v)| v)
            .collect();
        let label = format!("{config}|{}|{}", Self::cat_label(cat), col.key);
        bootstrap_ci(
            &vals,
            seed_for(self.meta.bootstrap.seed, &label),
            self.meta.bootstrap.resamples,
        )
    }

    fn paired(
        &self,
        a: (&str, &ConfigRun),
        b: (&str, &ConfigRun),
        cat: Option<Category>,
        metric: (&str, QGet),
    ) -> Option<PairedDiff> {
        // Every route to a pair (the comparison table, the headline verdict, H1/H2) must read the
        // same interval, so the bootstrap always runs in configuration order and the result is
        // flipped for display when the caller asked for the opposite orientation.
        let pos = |id: &str| self.configs.iter().position(|c| c.id == id);
        let flipped = pos(a.0) > pos(b.0);
        let (x, y) = if flipped { (b, a) } else { (a, b) };
        let label = format!("{}|{}|{}|{}", x.0, y.0, Self::cat_label(cat), metric.0);
        let d = paired_diff_ci(
            &Self::series(x.1, cat, metric.1),
            &Self::series(y.1, cat, metric.1),
            seed_for(self.meta.bootstrap.seed, &label),
            self.meta.bootstrap.resamples,
        )?;
        Some(if flipped { d.flipped() } else { d })
    }

    /// Paired comparison of every ordered-by-config pair of ran configurations on one metric.
    pub fn comparisons(&self, cat: Option<Category>, metric: &str) -> Vec<Comparison> {
        let get: QGet = if metric == "recall" { RECALL } else { NDCG };
        let ran = self.ran();
        let mut out = Vec::new();
        for i in 0..ran.len() {
            for j in (i + 1)..ran.len() {
                out.push(Comparison {
                    a: ran[i].0.id.clone(),
                    b: ran[j].0.id.clone(),
                    diff: self.paired(
                        (&ran[i].0.id, ran[i].1),
                        (&ran[j].0.id, ran[j].1),
                        cat,
                        (metric, get),
                    ),
                });
            }
        }
        out
    }

    /// Configuration(s) with the best nDCG@k *point estimate* in a category (ties all returned).
    /// A point estimate is not a win; see [`Report::verdict_line`].
    pub fn winners(&self, cat: Option<Category>) -> Vec<String> {
        let ran = self.ran();
        let pick = |r: &ConfigRun| match cat {
            Some(c) => r.by_category.get(&c).and_then(|s| s.ndcg),
            None => r.overall.ndcg,
        };
        let best = ran
            .iter()
            .filter_map(|(_, r)| pick(r))
            .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |x| x.max(v))));
        match best {
            None => Vec::new(),
            Some(b) => ran
                .iter()
                .filter(|(_, r)| pick(r).is_some_and(|v| (v - b).abs() < 1e-12))
                .map(|(c, _)| c.id.clone())
                .collect(),
        }
    }

    /// Names a winner only when the top nDCG@k point estimate is distinguishable from every other
    /// configuration; otherwise says which ones it cannot be told apart from (FR-020).
    pub fn verdict_line(&self, cat: Option<Category>) -> String {
        let w = self.winners(cat);
        let Some(top) = w.first() else {
            return "Winner by nDCG@k: none (no configuration produced a score).".to_string();
        };
        if w.len() > 1 {
            return format!(
                "Tie by nDCG@k: {} (identical point estimates); no winner is named.",
                fmt_winners(&w)
            );
        }
        let ran = self.ran();
        let Some((_, top_run)) = ran.iter().find(|(c, _)| &c.id == top) else {
            return String::new();
        };
        let mut indistinct = Vec::new();
        let mut compared = 0;
        for (c, r) in ran.iter().filter(|(c, _)| &c.id != top) {
            let has = match cat {
                Some(k) => r.by_category.get(&k).and_then(|s| s.ndcg).is_some(),
                None => r.overall.ndcg.is_some(),
            };
            if !has {
                continue;
            }
            compared += 1;
            let d = self.paired((top, top_run), (&c.id, r), cat, ("ndcg", NDCG));
            if !matches!(d.map(|d| d.verdict), Some(Verdict::AHigher)) {
                indistinct.push(format!("`{}`", c.id));
            }
        }
        if compared == 0 {
            format!("Winner by nDCG@k: `{top}` (the only configuration with a score).")
        } else if indistinct.is_empty() {
            format!("Winner by nDCG@k: `{top}` (distinguishable from every other configuration; paired 95% interval above zero).")
        } else {
            format!(
                "Highest nDCG@k: `{top}` — **not distinguishable from noise** against {}; no winner is named.",
                indistinct.join(", ")
            )
        }
    }

    fn best_nd(&self, cat: Category, prefix: &str) -> Option<(String, f64, Option<f64>)> {
        self.ran()
            .into_iter()
            .filter(|(c, _)| c.id.starts_with(prefix))
            // The variants config is read in its own section against `lcg-passages` (#802); it
            // is not a separate system for the LCG-vs-qmd hypotheses.
            .filter(|(c, _)| !is_variants_config(&c.id))
            .filter_map(|(c, r)| {
                let s = r.by_category.get(&cat)?;
                Some((c.id.clone(), s.ndcg?, s.stale_above_current_rate))
            })
            .fold(None, |m: Option<(String, f64, Option<f64>)>, x| match m {
                Some(cur) if cur.1 >= x.1 => Some(cur),
                _ => Some(x),
            })
    }

    fn read_pair(&self, cat: Category, a_prefix: &str, b_prefix: &str) -> Reading {
        let (Some(a), Some(b)) = (self.best_nd(cat, a_prefix), self.best_nd(cat, b_prefix)) else {
            return Reading::CannotRead;
        };
        let ran = self.ran();
        let find = |id: &str| {
            ran.iter()
                .find(|(c, _)| c.id == id)
                .map(|(c, r)| (c.id.as_str(), *r))
        };
        let (Some(ra), Some(rb)) = (find(&a.0), find(&b.0)) else {
            return Reading::CannotRead;
        };
        match self
            .paired(ra, rb, Some(cat), ("ndcg", NDCG))
            .map(|d| d.verdict)
        {
            Some(Verdict::AHigher) => Reading::Yes,
            Some(Verdict::BHigher) => Reading::No,
            _ => Reading::Noise,
        }
    }

    /// Reading of the two stated hypotheses (SC-007), using the same 95% paired-interval rule as
    /// every other comparison (FR-020). The best configuration on each side is picked by point
    /// estimate.
    pub fn hypotheses(&self) -> (Reading, Reading) {
        (
            self.read_pair(Category::Temporal, "lcg", "qmd"),
            self.read_pair(Category::Paraphrase, "qmd", "lcg"),
        )
    }

    /// `false-pos@k` is 1 whenever anything is returned, and every configuration runs with its score
    /// floor at 0, so a vector search returns k results for any query. When every configuration
    /// that has negative questions is pinned at 1.0 the column carries no signal; say so.
    pub fn false_positive_note(&self) -> Option<String> {
        let rates: Vec<f64> = self
            .ran()
            .iter()
            .filter_map(|(_, r)| r.overall.false_positive_rate)
            .collect();
        if !rates.is_empty() && rates.iter().all(|v| (*v - 1.0).abs() < 1e-12) {
            Some("> **false-pos@k is non-discriminating in this run:** every configuration scored 1.0 because score floors are 0 (so each system returns results for any query). Do not read it as a quality difference.".to_string())
        } else {
            None
        }
    }

    fn total_questions(&self) -> usize {
        self.meta.category_counts.values().sum()
    }

    /// Notes on the opt-in graph-aware configurations (issue #797), printed only when one ran or
    /// was requested: what each ablation row means, the plain-passage mode, the fairer
    /// entities+facts variant, and the reranker (#796) status (FR-019, FR-020).
    fn graph_section(&self, o: &mut String) {
        let ids: Vec<&str> = self
            .configs
            .iter()
            .map(|c| c.id.split('@').next().unwrap_or(&c.id))
            .collect();
        let graph = ids.iter().any(|i| i.starts_with("lcg-graph"));
        let rrf = ids.contains(&"lcg-entities-facts-rrf");
        if !graph && !rrf {
            return;
        }
        let _ = writeln!(o, "## Graph-aware search (#797)\n");
        if graph {
            let _ = writeln!(o, "- `lcg-graph*` drive `knowledge_graph_search` (one service call, one query embedding, no LLM). Ablation rows: `lcg-graph-seeds` = passages mentioning the linked seed entities only; `lcg-graph-expand` = + two-hop expansion (supporting passages and facts); `lcg-graph` = + plain passage search, fused with RRF.");
            let _ = writeln!(o, "- The plain-passage list inside `lcg-graph` is `knowledge_search_passages` with `mode: \"vector\"` and `belief_aware: true` (#789), the best passage list measured on v2; hybrid scored below vector (ADR-0789). Each response reports it as `stats.passage_mode`.");
            let _ = writeln!(o, "- `knowledge_graph_search` is believed-only by default: superseded facts and passages are neither returned nor traversed.");
            let _ = writeln!(
                o,
                "- Local reranker (#796): **not measured** — it has not landed on this build."
            );
        }
        if rrf {
            let _ = writeln!(o, "- `lcg-entities-facts-rrf` is a fairer merge of the same two reads as `lcg-entities-facts`: each read asks for k (not ceil(k/2)), each entity contributes at most 2 notes (no hub flooding in note-id order), unmapped results take no slot, and the two note lists are fused with RRF instead of strictly alternated. `lcg-entities-facts` itself is unchanged, so historical numbers stay comparable.");
        }
        let _ = writeln!(o);
    }

    fn disclaimer(&self) -> String {
        let corpus = if self.meta.private {
            "The corpus is a private notebook: these results describe that notebook and its question set only, not retrieval in general"
        } else if self.total_questions() >= 100 {
            "The corpus is synthetic (generated structure, LLM-written prose) and more uniform than a real notebook, so direct questions may sit near the ceiling. Treat results as a controlled comparison, not a forecast"
        } else {
            "The corpus is small and hand-built: results are indicative, not general"
        };
        let ran = |id: &str| self.ran().iter().any(|(c, _)| c.id == id);
        // `lcg-passages` sends neither `mode` nor `belief_aware` (#806), and the eval removes the
        // `LCG_PASSAGE_*` overrides from the spawned service's environment, so it measures the
        // shipped default.
        let mut passages = String::from(
            " `lcg-passages` is the service's shipped default (`knowledge_search_passages` with no `mode` or `belief_aware`: belief-aware vector, no FTS, no RRF; a derived page ranks below the primary note it cites, a superseded passage below its successor).",
        );
        if ran("lcg-passages-raw") {
            passages.push_str(
                " `lcg-passages-raw` is the pre-#806 baseline: `belief_aware: false`, cosine only.",
            );
        }
        if ran("lcg-passages-hybrid") || ran("lcg-passages-hybrid-belief") {
            passages.push_str(" `lcg-passages-hybrid` fuses BM25 over episode text with the vector ranking by weighted RRF (the lexical list weighs less than the vector list); `lcg-passages-hybrid-belief` adds belief-aware ranking (a derived page ranks below the primary note it cites, a superseded passage below its successor).");
        }
        if ran("lcg-passages-belief") {
            passages.push_str(
                " `lcg-passages-belief` is the vector ranking with the same belief-aware rules.",
            );
        }
        if self.ran().iter().any(|(c, _)| is_variants_config(&c.id)) {
            passages.push_str(" Each `*-variants` configuration adds the fixture's caller-supplied query variants to the same configuration: every variant runs the request's full pipeline (vector, or BM25 + vector), the per-query rankings are fused with weighted RRF (the primary query weighs 2, the variants share a total of 1), and belief-aware ranking, where on, runs once on the fused list.");
        }
        format!("> **Read with care.** LCG and qmd use different embedding (and reranker) models, so this compares *systems*, not retrieval algorithms. {corpus}.{passages}\n")
    }

    /// `nDCG@k` ceiling statement for a scope: every ran configuration at or above [`CEILING`].
    fn ceiling_note(&self, cat: Option<Category>) -> Option<String> {
        let pts: Vec<f64> = self
            .ran()
            .iter()
            .filter_map(|(_, r)| match cat {
                Some(c) => r.by_category.get(&c).and_then(|s| s.ndcg),
                None => r.overall.ndcg,
            })
            .collect();
        if !at_ceiling(&pts) {
            return None;
        }
        Some(format!(
            "> **At the ceiling:** every configuration scores nDCG@k ≥ {CEILING:.2} here (intervals above). This scope cannot rank them; do not read the point estimates as a difference."
        ))
    }

    fn run_of(&self, id: &str) -> Option<&ConfigRun> {
        self.ran()
            .into_iter()
            .find(|(c, _)| c.id == id)
            .map(|(_, r)| r)
    }

    /// Paired nDCG@k difference `a − b` between two configurations that both ran.
    fn stage_pair(&self, a: &str, b: &str, cat: Option<Category>) -> Option<PairedDiff> {
        self.paired(
            (a, self.run_of(a)?),
            (b, self.run_of(b)?),
            cat,
            ("ndcg", NDCG),
        )
    }

    /// The rerank stage's acceptance criteria (issue #796) against `lcg-passages`. `None` unless
    /// both configurations ran. Each check is `None` when it cannot be read (a category without
    /// enough shared questions, or no latency samples).
    ///
    /// With several embedding profiles the configuration ids carry a `@<profile>` suffix (see
    /// `profiled_id`); this reads the first profile that has both. Use
    /// [`Report::rerank_acceptance_for`] to pick one.
    pub fn rerank_acceptance(&self) -> Option<RerankAcceptance> {
        self.stage_suffixes()
            .iter()
            .find_map(|s| self.rerank_acceptance_for(s))
    }

    /// Id suffixes (`""`, or `@<profile>` when several embedding profiles were measured) under
    /// which the `lcg-passages` baseline ran, in report order.
    fn stage_suffixes(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (c, _) in self.ran() {
            // Any `lcg-passages*` configuration that can appear in the stage section counts: a run
            // with the stage configs and a matching baseline (`lcg-passages-raw`) but without
            // `lcg-passages` must still get its section.
            let Some(rest) = c.id.strip_prefix("lcg-passages") else {
                continue;
            };
            let (kind, suffix) = match rest.split_once('@') {
                Some((k, profile)) => (k, format!("@{profile}")),
                None => (rest, String::new()),
            };
            if ![
                "",
                "-raw",
                "-belief",
                "-rerank",
                "-belief-rerank",
                "-expand",
                "-expand-rerank",
            ]
            .contains(&kind)
            {
                continue;
            }
            let suffix = suffix.as_str();
            if !out.iter().any(|s| s == suffix) {
                out.push(suffix.to_string());
            }
        }
        out
    }

    /// The un-staged configuration a stage configuration should be paired with, so the paired delta
    /// isolates the stage: the stage configs that pin a raw cosine first stage (`-rerank`,
    /// `-expand`, `-expand-rerank`) pair with `lcg-passages-raw`, and `lcg-passages-belief-rerank`
    /// (belief-aware first stage) with `lcg-passages-belief`. Since #806 `lcg-passages` is the
    /// belief-aware default, so it is only the fallback when the matching baseline did not run.
    fn baseline_for(&self, stage_base: &str, suffix: &str) -> String {
        let preferred = if stage_base == "lcg-passages-belief-rerank" {
            "lcg-passages-belief"
        } else {
            "lcg-passages-raw"
        };
        let preferred = format!("{preferred}{suffix}");
        if self.run_of(&preferred).is_some() {
            preferred
        } else {
            format!("lcg-passages{suffix}")
        }
    }

    /// [`Report::rerank_acceptance`] for the configurations carrying id suffix `suffix`.
    pub fn rerank_acceptance_for(&self, suffix: &str) -> Option<RerankAcceptance> {
        // Pairs with the matching baseline; `run_of(base)?` below requires one to have run.
        let base_id = self.baseline_for("lcg-passages-rerank", suffix);
        let stage_id = format!("lcg-passages-rerank{suffix}");
        let (base, stage) = (base_id.as_str(), stage_id.as_str());
        self.run_of(base)?;
        let run = self.run_of(stage)?;
        let para = self.stage_pair(stage, base, Some(Category::Paraphrase));
        let overall = self.stage_pair(stage, base, None);
        let temporal = self.stage_pair(stage, base, Some(Category::Temporal));
        Some(RerankAcceptance {
            // SC-001: the paired interval is wholly above zero.
            paraphrase_gain: para.map(|d| d.lo > 0.0),
            // SC-002: overall nDCG@k is not lower (point estimate).
            overall_not_lower: overall.map(|d| d.mean_diff >= 0.0),
            // SC-003: the paired interval includes or exceeds zero, i.e. no supported drop.
            temporal_no_drop: temporal.map(|d| d.hi >= 0.0),
            // SC-004: p50 total latency within the 150 ms budget.
            p50_within_budget: run.latency_p50_ms.map(|p| p <= RERANK_P50_BUDGET_MS),
            paraphrase: para,
            overall,
            temporal,
            p50_ms: run.latency_p50_ms,
        })
    }

    /// "Passage stages" section (issue #796): each stage configuration against `lcg-passages` per
    /// category with a paired interval, expansion on top of rerank, and the rerank acceptance
    /// checks. Emitted only when a stage configuration ran.
    fn stage_section(&self, o: &mut String) {
        let suffixes = self.stage_suffixes();
        let multi = suffixes.iter().any(|s| !s.is_empty());
        for suffix in &suffixes {
            self.stage_section_for(o, suffix, multi);
        }
    }

    /// One "Passage stages" section for the configurations carrying id suffix `suffix`; `multi`
    /// is true when several embedding profiles were measured, so the heading names the profile.
    fn stage_section_for(&self, o: &mut String, suffix: &str, multi: bool) {
        let ids: Vec<String> = [
            "lcg-passages-rerank",
            "lcg-passages-belief-rerank",
            "lcg-passages-expand",
            "lcg-passages-expand-rerank",
        ]
        .iter()
        .map(|s| format!("{s}{suffix}"))
        .collect();
        let stages: Vec<&str> = ids.iter().map(String::as_str).collect();
        let base_id = self.baseline_for("lcg-passages-rerank", suffix);
        let rerank_id = format!("lcg-passages-rerank{suffix}");
        let bases: Vec<String> = ["lcg-passages-rerank", "lcg-passages-belief-rerank"]
            .iter()
            .chain(["lcg-passages-expand", "lcg-passages-expand-rerank"].iter())
            .map(|s| self.baseline_for(s, suffix))
            .collect();
        // Emit the section when at least one stage ran together with its own matching baseline
        // (a run of only `lcg-passages-belief-rerank` and `lcg-passages-belief` qualifies).
        let any_pair = stages
            .iter()
            .zip(bases.iter())
            .any(|(s, b)| self.run_of(s).is_some() && self.run_of(b).is_some());
        if !any_pair {
            return;
        }
        let heading = if multi {
            format!(
                "\n## Passage stages (rerank / query expansion) — embedding profile `{}`\n",
                suffix.trim_start_matches('@')
            )
        } else {
            "\n## Passage stages (rerank / query expansion)\n".to_string()
        };
        let _ = writeln!(o, "{heading}");
        let _ = writeln!(
            o,
            "Paired nDCG@k difference, stage − baseline, per category ({:.0}% bootstrap interval). Each stage is switchable and measured on its own; expansion is also shown on top of rerank.\n",
            LEVEL * 100.0
        );
        let _ = writeln!(
            o,
            "| stage | baseline | category | n | Δ nDCG@k | interval | reading |"
        );
        let _ = writeln!(o, "|---|---|---|---|---|---|---|");
        let mut pairs: Vec<(&str, &str)> = stages
            .iter()
            .zip(bases.iter())
            .map(|(s, b)| (*s, b.as_str()))
            .collect();
        pairs.push((stages[3], rerank_id.as_str()));
        for (stage, base) in pairs {
            if self.run_of(stage).is_none() || self.run_of(base).is_none() {
                continue;
            }
            let mut cats: Vec<Option<Category>> = vec![None];
            cats.extend(self.listed_categories().into_iter().map(Some));
            for cat in cats {
                match self.stage_pair(stage, base, cat) {
                    None => {
                        let _ = writeln!(
                            o,
                            "| `{stage}` | `{base}` | {} | <2 | n/a | n/a | not distinguishable from noise (too few shared questions) |",
                            Self::cat_label(cat)
                        );
                    }
                    Some(d) => {
                        let reading = match d.verdict {
                            Verdict::AHigher => format!("`{stage}` higher"),
                            Verdict::BHigher => format!("`{base}` higher"),
                            Verdict::Noise => Verdict::Noise.label().to_string(),
                        };
                        let _ = writeln!(
                            o,
                            "| `{stage}` | `{base}` | {} | {} | {:+.3} | [{:+.3}, {:+.3}] | {reading} |",
                            Self::cat_label(cat),
                            d.n,
                            d.mean_diff,
                            d.lo,
                            d.hi
                        );
                    }
                }
            }
        }
        if let Some(a) = self.rerank_acceptance_for(suffix) {
            let mark = |v: Option<bool>| match v {
                Some(true) => "met",
                Some(false) => "**not met**",
                None => "unreadable",
            };
            let _ = writeln!(o, "\nRerank acceptance (`{rerank_id}` vs `{base_id}`):\n");
            let _ = writeln!(
                o,
                "- Paraphrase nDCG@k improves, paired interval above zero: {}.",
                mark(a.paraphrase_gain)
            );
            let _ = writeln!(
                o,
                "- Overall nDCG@k does not drop (point estimate): {}.",
                mark(a.overall_not_lower)
            );
            let _ = writeln!(
                o,
                "- Temporal nDCG@k: paired interval includes or exceeds zero (no supported drop): {}.",
                mark(a.temporal_no_drop)
            );
            let _ = writeln!(
                o,
                "- p50 latency ≤ {RERANK_P50_BUDGET_MS:.0} ms{}: {}.",
                a.p50_ms
                    .map_or(String::new(), |p| format!(" (measured {p:.0} ms)")),
                mark(a.p50_within_budget)
            );
        }
    }

    pub fn render_markdown(&self) -> String {
        let m = &self.meta;
        let mut o = String::new();
        let _ = writeln!(o, "# Retrieval quality eval\n");
        let _ = writeln!(
            o,
            "Corpus `{}` (hash `{}`), questions `{}` (hash `{}`), shared k = {}.\n",
            m.corpus_version, m.corpus_hash, m.questions_version, m.questions_hash, m.k
        );
        let _ = writeln!(o, "{}", self.disclaimer());
        let _ = writeln!(
            o,
            "> **Reading the intervals.** `[lo, hi]` is a {:.0}% percentile bootstrap interval over questions ({} resamples, seed {}). A difference between two configurations whose paired {:.0}% interval includes zero is labelled *not distinguishable from noise*, never a winner. No multiplicity correction is applied across the many pairs shown. Bold marks the best point estimate, which is not by itself a finding.\n",
            LEVEL * 100.0,
            m.bootstrap.resamples,
            m.bootstrap.seed,
            LEVEL * 100.0
        );

        let _ = writeln!(o, "## Configurations\n");
        for c in &self.configs {
            match &c.status {
                Status::Ran(_) => {
                    let _ = writeln!(o, "- `{}`: {}", c.id, c.description);
                }
                Status::Skipped(why) => {
                    let why = redact_reason(why, m.private);
                    let _ = writeln!(o, "- `{}`: **skipped** — {why}", c.id);
                }
            }
        }

        let _ = writeln!(o, "\n## Overall\n");
        self.table(&mut o, None);
        let _ = writeln!(o, "\n{}", self.verdict_line(None));
        self.comparison_table(&mut o, None);
        if let Some(n) = self.ceiling_note(None) {
            let _ = writeln!(o, "\n{n}");
        }

        if let Some(note) = self.false_positive_note() {
            let _ = writeln!(o, "\n{note}");
        }

        let _ = writeln!(o, "\n## Per category\n");
        for cat in self.listed_categories() {
            let n = m.category_counts.get(&cat).copied().unwrap_or(0);
            let flag = if n > 0 && low_n(n) {
                format!(" — **low n** (< {MIN_CATEGORY_N}): intervals are wide")
            } else {
                String::new()
            };
            let _ = writeln!(o, "### {} (n = {n}){flag}\n", cat.label());
            if n == 0 {
                let _ = writeln!(o, "_No questions in this category._\n");
                continue;
            }
            self.table(&mut o, Some(cat));
            let _ = writeln!(o, "\n{}", self.verdict_line(Some(cat)));
            self.comparison_table(&mut o, Some(cat));
            if let Some(note) = self.ceiling_note(Some(cat)) {
                let _ = writeln!(o, "\n{note}");
            }
            let _ = writeln!(o);
        }

        self.oracle_section(&mut o);
        self.origin_section(&mut o);
        self.variants_section(&mut o);
        self.graph_section(&mut o);

        let _ = writeln!(o, "## Hypotheses\n");
        let (h1, h2) = self.hypotheses();
        let _ = writeln!(
            o,
            "- H1 — LCG beats qmd on temporal/superseded questions: {} (best `lcg-*` vs best `qmd*` by temporal nDCG@k, paired {:.0}% interval; see the stale>current column — the belief-aware facts variant is the only one that can filter superseded content, ADR-0705).",
            h1.md(),
            LEVEL * 100.0
        );
        let _ = writeln!(
            o,
            "- H2 — qmd beats LCG on paraphrase questions: {} (best `qmd*` vs best `lcg-*` by paraphrase nDCG@k, paired {:.0}% interval).",
            h2.md(),
            LEVEL * 100.0
        );

        self.stage_section(&mut o);

        let _ = writeln!(o, "\n## Cost\n");
        let _ = writeln!(o, "Per query, **warm** (indices built and one unmeasured warm-up pass before timing). qmd runs as one process per query, so its latency is **per-process** (it includes model load) and it gets a single warm-up query rather than a full pass.\n");
        let _ = writeln!(o, "| config | p50 ms | p95 ms | model calls/query | service calls/query | lookup calls | query errors | grounded judge |");
        let _ = writeln!(o, "|---|---|---|---|---|---|---|---|");
        for (c, r) in self.ran() {
            let calls = r
                .model_calls_per_query
                .map_or("n/a (not exposed)".to_string(), |v| format!("{v:.1}"));
            let service = r
                .service_calls_per_query
                .map_or("n/a".to_string(), |v| format!("{v:.1}"));
            let g = match &r.grounded {
                Grounded::Judged {
                    paid_calls,
                    cache_hits,
                } => {
                    format!("{paid_calls} paid, {cache_hits} cached")
                }
                Grounded::Skipped(why) => format!("skipped ({})", redact_reason(why, m.private)),
            };
            let _ = writeln!(
                o,
                "| `{}` | {} | {} | {calls} | {service} | {} | {} | {g} |",
                c.id,
                ms(r.latency_p50_ms),
                ms(r.latency_p95_ms),
                r.lookup_calls,
                r.query_errors.len()
            );
        }
        let i = &self.ingest;
        let _ = writeln!(
            o,
            "\nIndexing / ingestion (reported separately from per-query cost):"
        );
        let _ = writeln!(
            o,
            "- LCG ingest: {} s, {} chunks, {} extraction requests (each ≥ 1 LLM call), {} RPC calls.",
            i.lcg_seconds.map_or("n/a".into(), |s| format!("{s:.1}")),
            i.lcg_chunks,
            i.lcg_extraction_requests,
            i.lcg_rpc_calls
        );
        if i.lcg_supersessions > 0 {
            let _ = writeln!(
                o,
                "- Belief ended for {} episode(s) across {} document supersession(s).",
                i.lcg_superseded_episodes, i.lcg_supersessions
            );
        }
        if i.lcg_unconfirmed_entity_links > 0 {
            let _ = writeln!(
                o,
                "- **Caution:** {} entity→note link(s) were accepted without `episode_uuids` confirmation (the `source` filter is a substring match), so `lcg-entities-facts*` note credit may be over-attributed.",
                i.lcg_unconfirmed_entity_links
            );
        }
        let _ = writeln!(
            o,
            "- qmd index: {} s.",
            i.qmd_seconds.map_or("n/a".into(), |s| format!("{s:.1}"))
        );
        for (c, r) in self.ran() {
            if r.evidence_gaps > 0 {
                let _ = writeln!(
                    o,
                    "- `{}`: {} returned hit(s) carried no snippet. They were counted as ungrounded (no whole-note fallback), so this configuration's grounded figures are a lower bound.",
                    c.id, r.evidence_gaps
                );
            }
            for e in r.query_errors.iter().take(3) {
                if m.private {
                    let id = e.split(':').next().unwrap_or("?");
                    let _ = writeln!(o, "- `{}` query error: {id}: error (detail withheld)", c.id);
                } else {
                    let _ = writeln!(o, "- `{}` query error: {e}", c.id);
                }
            }
        }

        let _ = writeln!(o, "\n## Reproduction\n");
        let _ = writeln!(o, "- LCG build: {}", m.lcg_build);
        let _ = writeln!(
            o,
            "- qmd version: {} (index `{}`)",
            m.qmd_version, m.qmd_index
        );
        let _ = writeln!(o, "- LCG embedder: {}", m.embedder);
        let _ = writeln!(o, "- LCG extraction: {}", m.extractor);
        let _ = writeln!(
            o,
            "- qmd models (expansion / embedding / rerank): {}",
            m.qmd_models
        );
        let _ = writeln!(o, "- Grounded-evidence judge: {}", m.judge_model);
        let _ = writeln!(
            o,
            "- Bootstrap: percentile, {:.0}% level, {} resamples over questions, base seed {} (each interval's seed is the base seed XOR an FNV-1a hash of its label, so adding a configuration changes no other interval).",
            LEVEL * 100.0,
            m.bootstrap.resamples,
            m.bootstrap.seed
        );
        let _ = writeln!(
            o,
            "- Latency convention: warm, per query, p50/p95 over one measured pass."
        );
        let _ = writeln!(o, "- Determinism: relevance metrics are exactly reproducible for a fixed LCG graph and embedder; the intervals are exactly reproducible from the per-question scores, seed and resample count. LCG ingest is deterministic only with a replayed cassette; qmd query expansion and reranking are model-dependent; the grounded judge is cached by (model, key fact, text).");
        o
    }

    /// Paired nDCG@k comparison table for a scope; skipped when fewer than two configs ran.
    fn comparison_table(&self, o: &mut String, cat: Option<Category>) {
        let rows = self.comparisons(cat, "ndcg");
        if rows.is_empty() {
            return;
        }
        let _ = writeln!(
            o,
            "\nPaired nDCG@k comparison (difference a − b, {:.0}% bootstrap interval):\n",
            LEVEL * 100.0
        );
        let _ = writeln!(o, "| a | b | n | Δ nDCG@k | interval | reading |");
        let _ = writeln!(o, "|---|---|---|---|---|---|");
        for c in rows {
            match c.diff {
                None => {
                    let _ = writeln!(
                        o,
                        "| `{}` | `{}` | <2 | n/a | n/a | not distinguishable from noise (too few shared questions) |",
                        c.a, c.b
                    );
                }
                Some(d) => {
                    let reading = match d.verdict {
                        Verdict::AHigher => format!("`{}` higher", c.a),
                        Verdict::BHigher => format!("`{}` higher", c.b),
                        Verdict::Noise => Verdict::Noise.label().to_string(),
                    };
                    let _ = writeln!(
                        o,
                        "| `{}` | `{}` | {} | {:+.3} | [{:+.3}, {:+.3}] | {reading} |",
                        c.a, c.b, d.n, d.mean_diff, d.lo, d.hi
                    );
                }
            }
        }
    }

    /// A variants config against the same config without variants (#802): per-category paired nDCG@k
    /// difference with its interval, latency, and the merge-bar reading. A profiled variants row
    /// is paired with the same profile's `lcg-passages` row.
    fn variants_section(&self, o: &mut String) {
        let ran = self.ran();
        let pairs: Vec<_> = ran
            .iter()
            .filter_map(|(c, r)| {
                let base_id = variants_base_id(&c.id)?;
                let base = ran.iter().find(|(b, _)| b.id == base_id)?;
                Some((c.id.as_str(), *r, base.0.id.as_str(), base.1))
            })
            .collect();
        for (vid, vrun, bid, brun) in pairs {
            let _ = writeln!(o, "## Query variants vs `{bid}`\n");
            let _ = writeln!(
                o,
                "`{vid}` is `{bid}` plus the question's variants from `query-variants.jsonl`, sent as `queries` (primary query weighted 2, the variants sharing a total of 1). Paired nDCG@k difference, `{vid}` − `{bid}`, {:.0}% bootstrap interval:\n",
                LEVEL * 100.0
            );
            let _ = writeln!(
                o,
                "| scope | n | `{bid}` | `{vid}` | Δ nDCG@k | interval | reading |"
            );
            let _ = writeln!(o, "|---|---|---|---|---|---|---|");
            let mut scopes: Vec<Option<Category>> = vec![None];
            scopes.extend(
                self.listed_categories()
                    .into_iter()
                    .filter(|c| self.meta.category_counts.get(c).copied().unwrap_or(0) > 0)
                    .map(Some),
            );
            let mean = |r: &ConfigRun, cat: Option<Category>| match cat {
                Some(c) => r.by_category.get(&c).and_then(|s| s.ndcg),
                None => r.overall.ndcg,
            };
            let mut paraphrase_up = None;
            let mut regressions: Vec<&'static str> = Vec::new();
            for cat in scopes {
                let d = self.paired((vid, vrun), (bid, brun), cat, ("ndcg", NDCG));
                let (n, delta, iv, reading) = match d {
                    None => (
                        "<2".to_string(),
                        "n/a".to_string(),
                        "n/a".to_string(),
                        "not distinguishable from noise (too few shared questions)".to_string(),
                    ),
                    Some(d) => (
                        d.n.to_string(),
                        format!("{:+.3}", d.mean_diff),
                        format!("[{:+.3}, {:+.3}]", d.lo, d.hi),
                        match d.verdict {
                            Verdict::AHigher => format!("`{vid}` higher"),
                            Verdict::BHigher => format!("`{bid}` higher"),
                            Verdict::Noise => Verdict::Noise.label().to_string(),
                        },
                    ),
                };
                if let Some(c) = cat {
                    if c == Category::Paraphrase {
                        paraphrase_up =
                            Some(matches!(d.map(|d| d.verdict), Some(Verdict::AHigher)));
                    }
                    if matches!(d.map(|d| d.verdict), Some(Verdict::BHigher)) {
                        regressions.push(c.label());
                    }
                }
                let _ = writeln!(
                    o,
                    "| {} | {n} | {} | {} | {delta} | {iv} | {reading} |",
                    Self::cat_label(cat),
                    f3(mean(brun, cat)),
                    f3(mean(vrun, cat)),
                );
            }
            let _ = writeln!(
                o,
                "\nLatency (warm, over the socket): `{bid}` p50 {} ms / p95 {} ms; `{vid}` p50 {} ms / p95 {} ms.\n",
                ms(brun.latency_p50_ms),
                ms(brun.latency_p95_ms),
                ms(vrun.latency_p50_ms),
                ms(vrun.latency_p95_ms)
            );
            let _ = writeln!(o, "Merge bar (#802):");
            let _ = writeln!(
                o,
                "- paraphrase nDCG@k improves with the interval above zero: {}",
                match paraphrase_up {
                    Some(true) => "**met**",
                    Some(false) => "**not met**",
                    None => "cannot be read (no paraphrase questions)",
                }
            );
            let _ = writeln!(
                o,
                "- no category regresses beyond noise (no interval wholly below zero): {}\n",
                if regressions.is_empty() {
                    "**met**".to_string()
                } else {
                    format!("**not met** ({})", regressions.join(", "))
                }
            );
            // Latency is reported above, not gated: variants are deliberate agent searches (#802,
            // SC-004 dropped its p50 ceiling).
        }
    }

    fn origin_values(run: &ConfigRun) -> Option<(Vec<f64>, Vec<f64>, Vec<f64>)> {
        let od = run.origin.as_ref()?;
        let share = od
            .per_question
            .iter()
            .map(|(_, q)| q.restating_share)
            .collect();
        let outrank = od
            .per_question
            .iter()
            .filter_map(|(_, q)| q.outranks_primary.map(|b| b as u8 as f64))
            .collect();
        let only = od
            .per_question
            .iter()
            .map(|(_, q)| q.derived_only as u8 as f64)
            .collect();
        Some((share, outrank, only))
    }

    fn origin_ci(&self, cfg: &str, name: &str, vals: &[f64]) -> Option<Interval> {
        bootstrap_ci(
            vals,
            seed_for(self.meta.bootstrap.seed, &format!("{cfg}|origin|{name}")),
            self.meta.bootstrap.resamples,
        )
    }

    fn mean_txt(vals: &[f64]) -> String {
        if vals.is_empty() {
            "n/a".to_string()
        } else {
            format!("{:.3}", vals.iter().sum::<f64>() / vals.len() as f64)
        }
    }

    /// Per-origin section (FR-022); only when some configuration carries origin data.
    fn origin_section(&self, o: &mut String) {
        let ran = self.ran();
        if !ran.iter().any(|(_, r)| r.origin.is_some()) {
            return;
        }
        let _ = writeln!(o, "## By origin of returned notes\n");
        let _ = writeln!(
            o,
            "Eligible questions are non-negative ones whose relevant `primary` note is restated by at least one `derived` page (a *restating* note: it cites that primary note and is not itself relevant). *Restating share@k* is the fraction of the returned top-k that are restating notes. *Outrank rate* is, over eligible questions where both the primary note and a restating note were returned, how often the best restating note sits above the primary one. *Derived-only* is how often a restating note was returned and no relevant primary note was. ADR-0752: derived pages are ingested as `derived`, so they cannot corroborate themselves.\n"
        );
        let _ = writeln!(
            o,
            "| config | eligible n | restating share@k | outrank rate (pairs) | derived-only rate |"
        );
        let _ = writeln!(o, "|---|---|---|---|---|");
        for (c, r) in &ran {
            let Some((share, outrank, only)) = Self::origin_values(r) else {
                continue;
            };
            let _ = writeln!(
                o,
                "| `{}` | {} | {} {} | {} {} ({}) | {} {} |",
                c.id,
                share.len(),
                Self::mean_txt(&share),
                ci_txt(self.origin_ci(&c.id, "share", &share)),
                Self::mean_txt(&outrank),
                ci_txt(self.origin_ci(&c.id, "outrank", &outrank)),
                outrank.len(),
                Self::mean_txt(&only),
                ci_txt(self.origin_ci(&c.id, "derived_only", &only)),
            );
        }
        let _ = writeln!(
            o,
            "\nReturned top-k by origin (share of all returned results over non-negative questions):\n"
        );
        let origins = ["primary", "external", "derived", "answer", "undeclared"];
        let _ = writeln!(o, "| config | {} |", origins.join(" | "));
        let _ = writeln!(o, "|---|{}", "---|".repeat(origins.len()));
        for (c, r) in &ran {
            let Some(od) = &r.origin else { continue };
            let cells: Vec<String> = origins
                .iter()
                .map(|k| {
                    if od.returned == 0 {
                        "n/a".to_string()
                    } else {
                        format!(
                            "{:.3}",
                            od.mix.get(*k).copied().unwrap_or(0) as f64 / od.returned as f64
                        )
                    }
                })
                .collect();
            let _ = writeln!(o, "| `{}` | {} |", c.id, cells.join(" | "));
        }
        let _ = writeln!(o);
    }

    fn table(&self, o: &mut String, cat: Option<Category>) {
        let rows: Vec<(&ConfigOutcome, Option<(&ConfigRun, &Summary)>)> = self
            .configs
            .iter()
            .map(|c| {
                let s = match &c.status {
                    Status::Ran(r) => match cat {
                        Some(cat) => r.by_category.get(&cat).map(|s| (r.as_ref(), s)),
                        None => Some((r.as_ref(), &r.overall)),
                    },
                    Status::Skipped(_) => None,
                };
                (c, s)
            })
            .collect();
        let _ = write!(o, "| config |");
        for c in COLS {
            let _ = write!(o, " {} |", c.name);
        }
        let _ = writeln!(o);
        let _ = writeln!(o, "|---|{}", "---|".repeat(COLS.len()));
        let bests: Vec<Option<f64>> = COLS
            .iter()
            .map(|c| {
                let vals: Vec<Option<f64>> = rows
                    .iter()
                    .map(|(_, s)| s.and_then(|(_, s)| (c.get)(s)))
                    .collect();
                best_value(&vals, c.higher_better)
            })
            .collect();
        for (c, s) in &rows {
            let id = &c.id;
            let _ = write!(o, "| `{id}` |");
            match s {
                None => {
                    let _ = write!(o, " skipped |{}", " |".repeat(COLS.len() - 1));
                }
                Some((run, s)) => {
                    for (ci, col) in COLS.iter().enumerate() {
                        let v = (col.get)(s);
                        let txt = f3(v);
                        let marked = match (v, bests[ci]) {
                            (Some(v), Some(b)) if (v - b).abs() < 1e-12 => format!("**{txt}**"),
                            _ => txt,
                        };
                        let cell = if col.per_question.is_some() && v.is_some() {
                            format!("{marked} {}", ci_txt(self.interval(id, run, cat, col)))
                        } else {
                            marked
                        };
                        let _ = write!(o, " {cell} |");
                    }
                }
            }
            let _ = writeln!(o);
        }
        // Sample sizes behind the means (negatives, temporal and grounded subsets differ).
        if let Some((_, Some((_, s)))) = rows.iter().find(|(_, s)| s.is_some()) {
            let _ = writeln!(
                o,
                "\n_n: {} questions; {} with relevance judgements; {} negative; {} temporal; {} grounded._",
                s.questions, s.n_relevance, s.n_negative, s.n_temporal, s.n_grounded
            );
        }
    }

    fn ci_json(&self, cfg: &str, run: &ConfigRun, cat: Option<Category>) -> Value {
        let mut m = serde_json::Map::new();
        for col in COLS.iter().filter(|c| c.per_question.is_some()) {
            let v = match self.interval(cfg, run, cat, col) {
                Some(i) => json!({"n": i.n, "mean": r6(i.mean), "lo": r6(i.lo), "hi": r6(i.hi)}),
                None => Value::Null,
            };
            m.insert(col.key.to_string(), v);
        }
        Value::Object(m)
    }

    fn comparisons_json(&self) -> Vec<Value> {
        let mut out = Vec::new();
        let mut scopes: Vec<Option<Category>> = vec![None];
        scopes.extend(self.listed_categories().into_iter().map(Some));
        for scope in scopes {
            if let Some(c) = scope {
                if self.meta.category_counts.get(&c).copied().unwrap_or(0) == 0 {
                    continue;
                }
            }
            for metric in ["ndcg", "recall"] {
                for c in self.comparisons(scope, metric) {
                    out.push(match c.diff {
                        Some(d) => json!({
                            "scope": Self::cat_label(scope), "metric": metric, "a": c.a, "b": c.b,
                            "n": d.n, "mean_diff": r6(d.mean_diff), "lo": r6(d.lo), "hi": r6(d.hi),
                            "reading": match d.verdict {
                                Verdict::AHigher => "a_higher",
                                Verdict::BHigher => "b_higher",
                                Verdict::Noise => "not_distinguishable_from_noise",
                            },
                        }),
                        None => json!({
                            "scope": Self::cat_label(scope), "metric": metric, "a": c.a, "b": c.b,
                            "n": null, "reading": "not_distinguishable_from_noise",
                        }),
                    });
                }
            }
        }
        out
    }

    pub fn to_json(&self) -> Value {
        let m = &self.meta;
        let cat_counts: serde_json::Map<String, Value> = m
            .category_counts
            .iter()
            .map(|(c, n)| (c.label().to_string(), json!(n)))
            .collect();
        let summ = |s: &Summary| {
            json!({
                "questions": s.questions, "n_relevance": s.n_relevance,
                "recall": s.recall, "mrr": s.mrr, "ndcg": s.ndcg,
                "n_negative": s.n_negative, "false_positive_rate": s.false_positive_rate,
                "n_temporal": s.n_temporal,
                "stale_above_current_rate": s.stale_above_current_rate,
                "stale_in_top_k_rate": s.stale_in_top_k_rate,
                "n_grounded": s.n_grounded,
                "grounded_recall": s.grounded_recall, "grounded_mrr": s.grounded_mrr,
            })
        };
        let configs: Vec<Value> = self
            .configs
            .iter()
            .map(|c| match &c.status {
                Status::Skipped(why) => json!({"id": c.id, "description": c.description, "skipped": redact_reason(why, m.private)}),
                Status::Ran(r) => {
                    let by: serde_json::Map<String, Value> = r
                        .by_category
                        .iter()
                        .map(|(k, s)| (k.label().to_string(), summ(s)))
                        .collect();
                    let ci_by: serde_json::Map<String, Value> = r
                        .by_category
                        .keys()
                        .map(|k| (k.label().to_string(), self.ci_json(&c.id, r, Some(*k))))
                        .collect();
                    let origins = match (&r.origin, Self::origin_values(r)) {
                        (Some(od), Some((share, outrank, only))) => {
                            let part = |name: &str, v: &[f64]| {
                                json!({
                                    "n": v.len(),
                                    "mean": if v.is_empty() { Value::Null } else { r6(v.iter().sum::<f64>() / v.len() as f64) },
                                    "ci": self.origin_ci(&c.id, name, v).map(|i| json!({"lo": r6(i.lo), "hi": r6(i.hi)})),
                                })
                            };
                            json!({
                                "eligible_questions": share.len(),
                                "restating_share": part("share", &share),
                                "outrank_rate": part("outrank", &outrank),
                                "derived_only_rate": part("derived_only", &only),
                                "returned": od.returned,
                                "returned_by_origin": od.mix,
                            })
                        }
                        _ => Value::Null,
                    };
                    json!({
                        "id": c.id, "description": c.description,
                        "overall": summ(&r.overall), "by_category": by,
                        "ci": {"overall": self.ci_json(&c.id, r, None), "by_category": ci_by},
                        "origins": origins,
                        "latency_p50_ms": r.latency_p50_ms, "latency_p95_ms": r.latency_p95_ms,
                        "model_calls_per_query": r.model_calls_per_query,
                        "service_calls_per_query": r.service_calls_per_query,
                        "lookup_calls": r.lookup_calls,
                        "evidence_gaps": r.evidence_gaps,
                        "query_errors": if m.private {
                            r.query_errors.iter().map(|e| json!(format!("{}: error (detail withheld)", e.split(':').next().unwrap_or("?")))).collect::<Vec<_>>()
                        } else {
                            r.query_errors.iter().map(|e| json!(e)).collect::<Vec<_>>()
                        },
                        "grounded": match &r.grounded {
                            Grounded::Judged { paid_calls, cache_hits } => json!({"paid_calls": paid_calls, "cache_hits": cache_hits}),
                            Grounded::Skipped(w) => json!({"skipped": redact_reason(w, m.private)}),
                        },
                        "questions": r.scores.iter().map(|s| json!({
                            "id": s.id, "category": s.category.label(),
                            "recall": s.recall, "mrr": s.mrr, "ndcg": s.ndcg,
                            "false_positive": s.false_positive,
                            "stale_above_current": s.stale_above_current,
                            "stale_in_top_k": s.stale_in_top_k,
                            "grounded_recall": s.grounded_recall, "grounded_mrr": s.grounded_mrr,
                        })).collect::<Vec<_>>(),
                    })
                }
            })
            .collect();
        let (h1, h2) = self.hypotheses();
        let winners: BTreeMap<String, Vec<String>> = self
            .listed_categories()
            .iter()
            .map(|c| (c.label().to_string(), self.winners(Some(*c))))
            .collect();
        json!({
            "meta": {
                "corpus_version": m.corpus_version, "corpus_hash": m.corpus_hash,
                "questions_version": m.questions_version, "questions_hash": m.questions_hash,
                "k": m.k, "lcg_build": m.lcg_build, "qmd_version": m.qmd_version,
                "qmd_index": m.qmd_index, "embedder": m.embedder, "extractor": m.extractor,
                "qmd_models": m.qmd_models, "judge_model": m.judge_model,
                "category_counts": cat_counts, "latency_convention": "warm",
            },
            "bootstrap": {
                "method": "percentile bootstrap over questions",
                "level": LEVEL, "seed": m.bootstrap.seed, "resamples": m.bootstrap.resamples,
                "min_category_n": MIN_CATEGORY_N, "ceiling_ndcg": CEILING,
            },
            "ingest": {
                "lcg_seconds": self.ingest.lcg_seconds, "lcg_chunks": self.ingest.lcg_chunks,
                "lcg_extraction_requests": self.ingest.lcg_extraction_requests,
                "lcg_rpc_calls": self.ingest.lcg_rpc_calls,
                "lcg_unconfirmed_entity_links": self.ingest.lcg_unconfirmed_entity_links,
                "lcg_supersessions": self.ingest.lcg_supersessions,
                "lcg_superseded_episodes": self.ingest.lcg_superseded_episodes,
                "qmd_seconds": self.ingest.qmd_seconds,
            },
            "configs": configs,
            "comparisons": self.comparisons_json(),
            "winners": winners,
            "hypotheses": {
                "h1_lcg_beats_qmd_on_temporal": h1.legacy(),
                "h2_qmd_beats_lcg_on_paraphrase": h2.legacy(),
                "h1_reading": h1.key(),
                "h2_reading": h2.key(),
            },
        })
    }
}

fn fmt_winners(w: &[String]) -> String {
    if w.is_empty() {
        "none (no configuration produced a score)".to_string()
    } else {
        w.iter()
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join(" = ")
    }
}

/// The per-question oracle over several runs: for each question any run scored (nDCG@k present),
/// the best nDCG@k among them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Oracle {
    /// Mean oracle nDCG@k over [`Oracle::questions`]; `None` when no run scored any question.
    pub overall: Option<f64>,
    pub by_category: BTreeMap<Category, f64>,
    /// Ids of the questions the oracle is defined on.
    pub questions: std::collections::BTreeSet<String>,
    pub category_of: BTreeMap<String, Category>,
    pub n: usize,
}

pub fn oracle_ndcg(runs: &[&[QuestionScore]]) -> Oracle {
    let mut best: BTreeMap<String, (Category, f64)> = BTreeMap::new();
    for run in runs {
        for q in run.iter() {
            if let Some(v) = q.ndcg {
                let e = best.entry(q.id.clone()).or_insert((q.category, v));
                if v > e.1 {
                    e.1 = v;
                }
            }
        }
    }
    let n = best.len();
    if n == 0 {
        return Oracle::default();
    }
    let mut by_cat: BTreeMap<Category, (f64, usize)> = BTreeMap::new();
    for (cat, v) in best.values() {
        let e = by_cat.entry(*cat).or_insert((0.0, 0));
        e.0 += v;
        e.1 += 1;
    }
    Oracle {
        overall: Some(best.values().map(|(_, v)| v).sum::<f64>() / n as f64),
        by_category: by_cat
            .into_iter()
            .map(|(c, (s, k))| (c, s / k as f64))
            .collect(),
        questions: best.keys().cloned().collect(),
        category_of: best.iter().map(|(q, (c, _))| (q.clone(), *c)).collect(),
        n,
    }
}

/// A run's mean nDCG@k over `questions`; a question the run did not score counts as 0, so every
/// configuration is averaged over the same question set as the oracle.
fn mean_over(scores: &[QuestionScore], questions: &std::collections::BTreeSet<String>) -> f64 {
    if questions.is_empty() {
        return 0.0;
    }
    let by_id: BTreeMap<&str, f64> = scores
        .iter()
        .filter_map(|q| q.ndcg.map(|v| (q.id.as_str(), v)))
        .collect();
    questions
        .iter()
        .map(|q| by_id.get(q.as_str()).copied().unwrap_or(0.0))
        .sum::<f64>()
        / questions.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::metrics::summarize_by_category;
    use crate::retrieval::origin::{OriginData, OriginQuestion};

    const N: usize = 30;

    fn score(cat: Category, i: usize, ndcg: f64, stale: Option<bool>) -> QuestionScore {
        QuestionScore {
            id: format!("{}-{i:02}", cat.label()),
            category: cat,
            recall: Some(ndcg),
            mrr: Some(ndcg),
            ndcg: Some(ndcg),
            false_positive: None,
            stale_above_current: stale,
            stale_in_top_k: stale,
            grounded_recall: None,
            grounded_mrr: None,
            ranked: vec![],
        }
    }

    /// `n` questions of one category around `base` with deterministic spread.
    fn cat_scores(cat: Category, n: usize, base: f64, stale: Option<bool>) -> Vec<QuestionScore> {
        (0..n)
            .map(|i| {
                let v = (base + ((i * 7) % 5) as f64 * 0.02 - 0.04).clamp(0.0, 1.0);
                score(cat, i, v, stale)
            })
            .collect()
    }

    fn run(scores: Vec<QuestionScore>) -> Status {
        let (overall, by_category) = summarize_by_category(&scores);
        Status::Ran(Box::new(ConfigRun {
            scores,
            overall,
            by_category,
            latency_p50_ms: Some(12.0),
            latency_p95_ms: Some(30.0),
            model_calls_per_query: None,
            service_calls_per_query: None,
            lookup_calls: 0,
            evidence_gaps: 0,
            query_errors: vec![],
            grounded: Grounded::Skipped("no judge".into()),
            origin: None,
        }))
    }

    fn two(a: (f64, f64), b: (f64, f64)) -> (Vec<QuestionScore>, Vec<QuestionScore>) {
        let mk = |t: f64, p: f64, stale: bool| {
            let mut v = cat_scores(Category::Temporal, N, t, Some(stale));
            v.extend(cat_scores(Category::Paraphrase, N, p, None));
            v
        };
        (mk(a.0, a.1, false), mk(b.0, b.1, true))
    }

    fn report_with(lcg: Vec<QuestionScore>, qmd: Vec<QuestionScore>) -> Report {
        let mut meta = RunMeta {
            corpus_version: "nb-1".into(),
            k: 10,
            bootstrap: Bootstrap {
                seed: 788,
                resamples: 300,
            },
            ..Default::default()
        };
        meta.category_counts.insert(Category::Temporal, N);
        meta.category_counts.insert(Category::Paraphrase, N);
        Report {
            meta,
            ingest: IngestCost::default(),
            configs: vec![
                ConfigOutcome {
                    id: "lcg-entities-facts-belief".into(),
                    description: "d".into(),
                    status: run(lcg),
                },
                ConfigOutcome {
                    id: "qmd".into(),
                    description: "d".into(),
                    status: run(qmd),
                },
                ConfigOutcome {
                    id: "qmd-rerank".into(),
                    description: "d".into(),
                    status: Status::Skipped("model download unavailable".into()),
                },
            ],
        }
    }

    /// LCG clearly better on temporal, qmd clearly better on paraphrase.
    fn report() -> Report {
        let (l, q) = two((0.9, 0.4), (0.5, 0.8));
        report_with(l, q)
    }

    /// `lcg-passages` and `lcg-passages-variants` (#802) over temporal + paraphrase questions.
    fn variants_report(base: (f64, f64), variants: (f64, f64), variants_p50: f64) -> Report {
        let (b, _) = two(base, base);
        let (v, _) = two(variants, variants);
        let mut r = report_with(b.clone(), b.clone());
        r.configs.insert(
            0,
            ConfigOutcome {
                id: "lcg-passages".into(),
                description: "d".into(),
                status: run(b),
            },
        );
        let mut vs = run(v);
        if let Status::Ran(run) = &mut vs {
            run.latency_p50_ms = Some(variants_p50);
        }
        r.configs.insert(
            1,
            ConfigOutcome {
                id: "lcg-passages-variants".into(),
                description: "d".into(),
                status: vs,
            },
        );
        r
    }

    #[test]
    fn variants_section_reports_a_paraphrase_gain_and_the_merge_bar() {
        let md = variants_report((0.7, 0.2), (0.7, 0.6), 40.0).render_markdown();
        assert!(md.contains("## Query variants vs `lcg-passages`"), "{md}");
        let row = md
            .lines()
            .find(|l| l.starts_with("| paraphrase |") && l.contains("lcg-passages-variants"))
            .unwrap_or_else(|| panic!("no paraphrase row in {md}"));
        assert!(row.contains("`lcg-passages-variants` higher"), "{row}");
        assert!(
            md.contains("paraphrase nDCG@k improves with the interval above zero: **met**"),
            "{md}"
        );
        assert!(
            md.contains("no interval wholly below zero): **met**"),
            "{md}"
        );
        assert!(
            !md.contains("p50 ≤"),
            "latency is reported, not gated: {md}"
        );
        assert!(md.contains("p50 40 ms"), "{md}");
    }

    #[test]
    fn variants_section_flags_a_regressing_category_and_does_not_gate_latency() {
        // Temporal regresses clearly; paraphrase does not move.
        let md = variants_report((0.8, 0.5), (0.5, 0.5), 150.0).render_markdown();
        assert!(
            md.contains("paraphrase nDCG@k improves with the interval above zero: **not met**"),
            "{md}"
        );
        assert!(md.contains("**not met** (temporal)"), "{md}");
        assert!(md.contains("p50 150 ms"), "{md}");
        assert!(
            !md.contains("p50 ≤"),
            "latency is reported, not gated: {md}"
        );
    }

    #[test]
    fn variants_section_absent_without_both_configs_and_hypotheses_ignore_the_variants_row() {
        assert!(!report().render_markdown().contains("Query variants vs"));
        // Adding a much better variants row must not change what H1/H2 read: they compare the
        // best `lcg-*` with `qmd*`, and the variants row is read on its own.
        let plain = report();
        let (h1, h2) = plain.hypotheses();
        let mut with = report();
        let (v, _) = two((0.99, 0.99), (0.99, 0.99));
        with.configs.insert(
            0,
            ConfigOutcome {
                id: "lcg-passages-variants".into(),
                description: "d".into(),
                status: run(v),
            },
        );
        assert_eq!(with.hypotheses(), (h1, h2));
        // No `lcg-passages` row to pair with: no section, no panic.
        assert!(!with.render_markdown().contains("Query variants vs"));
    }

    #[test]
    fn the_passages_note_is_not_vector_only_when_variants_ran() {
        let md = variants_report((0.7, 0.2), (0.7, 0.6), 40.0).render_markdown();
        assert!(
            md.contains("adds the fixture's caller-supplied query variants"),
            "{md}"
        );
        assert!(!report()
            .render_markdown()
            .contains("caller-supplied query variants"));
    }

    #[test]
    fn a_variants_config_is_read_against_its_own_base() {
        for (id, base) in [
            ("lcg-passages-variants", "lcg-passages"),
            ("lcg-passages-belief-variants", "lcg-passages-belief"),
            (
                "lcg-passages-hybrid-belief-variants",
                "lcg-passages-hybrid-belief",
            ),
            ("lcg-passages-variants@bge", "lcg-passages@bge"),
        ] {
            assert!(is_variants_config(id), "{id}");
            assert_eq!(variants_base_id(id).as_deref(), Some(base), "{id}");
        }
        for id in ["lcg-passages", "lcg-passages-hybrid-belief", "qmd"] {
            assert!(!is_variants_config(id), "{id}");
            assert_eq!(variants_base_id(id), None, "{id}");
        }
    }

    #[test]
    fn percentile_nearest_rank() {
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[5.0], 95.0), Some(5.0));
        let v: Vec<f64> = (1..=10).map(|x| x as f64).collect();
        assert_eq!(percentile(&v, 50.0), Some(5.0));
        assert_eq!(percentile(&v, 95.0), Some(10.0));
        assert_eq!(percentile(&[3.0, 1.0, 2.0], 100.0), Some(3.0));
    }

    #[test]
    fn winners_per_category_and_hypotheses_with_clear_gaps() {
        let r = report();
        assert_eq!(
            r.winners(Some(Category::Temporal)),
            vec!["lcg-entities-facts-belief"]
        );
        assert_eq!(r.winners(Some(Category::Paraphrase)), vec!["qmd"]);
        assert!(r.winners(Some(Category::Direct)).is_empty());
        assert_eq!(r.hypotheses(), (Reading::Yes, Reading::Yes));
        assert!(r
            .verdict_line(Some(Category::Paraphrase))
            .starts_with("Winner by nDCG@k: `qmd`"));
    }

    #[test]
    fn a_gap_within_noise_is_not_a_winner_and_not_a_hypothesis_yes() {
        // Same multiset of per-question scores, permuted: equal means, varied paired differences.
        let (l, mut q) = two((0.62, 0.62), (0.62, 0.62));
        let vals: Vec<Option<f64>> = q.iter().map(|s| s.ndcg).collect();
        for (i, s) in q.iter_mut().enumerate() {
            let v = vals[(i + 1) % vals.len()];
            s.ndcg = v;
            s.recall = v;
            s.mrr = v;
        }
        let r = report_with(l, q);
        assert_eq!(r.hypotheses(), (Reading::Noise, Reading::Noise));
        let md = r.render_markdown();
        assert!(md.contains("not distinguishable from noise"), "{md}");
        assert!(md.contains("no winner is named"), "{md}");
        assert!(
            !md.contains("Winner by nDCG@k: `lcg-entities-facts-belief`"),
            "{md}"
        );
        let j = r.to_json();
        assert_eq!(j["hypotheses"]["h1_reading"], "noise");
        assert!(j["hypotheses"]["h1_lcg_beats_qmd_on_temporal"].is_null());
        assert!(j["comparisons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["reading"] == "not_distinguishable_from_noise"));
    }

    #[test]
    fn reversed_hypothesis_reads_no() {
        // qmd clearly better on temporal: H1 is NO, not "cannot tell".
        let (l, q) = two((0.4, 0.4), (0.9, 0.9));
        assert_eq!(report_with(l, q).hypotheses(), (Reading::No, Reading::Yes));
    }

    #[test]
    fn hypotheses_unreadable_when_a_side_is_skipped() {
        let mut r = report();
        r.configs[1].status = Status::Skipped("qmd not installed".into());
        assert_eq!(r.hypotheses(), (Reading::CannotRead, Reading::CannotRead));
        assert!(r.render_markdown().contains("cannot be read"));
    }

    fn qs(id: &str, cat: Category, ndcg: Option<f64>) -> QuestionScore {
        QuestionScore {
            id: id.into(),
            category: cat,
            recall: None,
            mrr: None,
            ndcg,
            false_positive: None,
            stale_above_current: None,
            stale_in_top_k: None,
            grounded_recall: None,
            grounded_mrr: None,
            ranked: vec![],
        }
    }

    #[test]
    fn oracle_takes_the_best_ndcg_per_question() {
        let a = vec![
            qs("q1", Category::Direct, Some(1.0)),
            qs("q2", Category::Paraphrase, Some(0.0)),
            qs("n1", Category::Negative, None),
        ];
        let b = vec![
            qs("q1", Category::Direct, Some(0.5)),
            qs("q2", Category::Paraphrase, Some(0.6)),
        ];
        let o = oracle_ndcg(&[&a, &b]);
        assert_eq!(o.n, 2, "a question no run scored is not in the oracle");
        assert!((o.overall.unwrap() - 0.8).abs() < 1e-12);
        assert_eq!(o.by_category[&Category::Direct], 1.0);
        assert_eq!(o.by_category[&Category::Paraphrase], 0.6);
        // Each run over the oracle's questions: a = 0.5, b = 0.55.
        assert!((mean_over(&a, &o.questions) - 0.5).abs() < 1e-12);
        assert!((mean_over(&b, &o.questions) - 0.55).abs() < 1e-12);
        assert_eq!(oracle_ndcg(&[]).overall, None);
    }

    #[test]
    fn headroom_section_needs_two_configs() {
        let mut one = report();
        one.configs.truncate(1);
        assert!(
            !one.render_markdown().contains("## Headroom"),
            "one configuration is its own oracle"
        );
        let mut r = report();
        let mut c = r.configs[0].clone();
        c.id = "lcg-graph".into();
        r.configs.push(c);
        let md = r.render_markdown();
        assert!(md.contains("## Headroom: per-question oracle"), "{md}");
        assert!(md.contains("| `lcg-graph` |"), "{md}");
        assert!(
            md.contains("| category | oracle | best config | best |"),
            "{md}"
        );
    }

    #[test]
    fn graph_section_appears_only_for_graph_configs_and_shows_service_calls() {
        let mut r = report();
        let md = r.render_markdown();
        assert!(
            !md.contains("## Graph-aware search"),
            "no graph config, no section"
        );
        assert!(md.contains("service calls/query"));
        let mut c = r.configs[0].clone();
        c.id = "lcg-graph".into();
        if let Status::Ran(run) = &mut c.status {
            run.service_calls_per_query = Some(1.0);
            run.model_calls_per_query = Some(1.0);
        }
        r.configs.push(c);
        let mut e = r.configs[0].clone();
        e.id = "lcg-entities-facts-rrf".into();
        r.configs.push(e);
        let md = r.render_markdown();
        for s in [
            "## Graph-aware search (#797)",
            "`lcg-graph-seeds` = passages mentioning",
            // Text only the graph section writes (the passage-mode bullet), not the unrelated
            // `lcg-passages is vector-only` sentence every report carries.
            "`belief_aware: true` (#789)",
            "Local reranker (#796): **not measured**",
            "`lcg-entities-facts-rrf` is a fairer merge",
            "| `lcg-graph` |",
        ] {
            assert!(md.contains(s), "missing {s:?} in:\n{md}");
        }
        let j = r.to_json();
        let row = j["configs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "lcg-graph")
            .unwrap();
        assert_eq!(row["service_calls_per_query"], 1.0);
    }

    #[test]
    fn markdown_has_required_sections_intervals_and_marks_best_and_skips() {
        let md = report().render_markdown();
        for s in [
            "## Overall",
            "## Per category",
            "### temporal (n = 30)",
            "### direct (n = 0)",
            "## Hypotheses",
            "## Cost",
            "## Reproduction",
            "Winner by nDCG@k: `qmd`",
            "**skipped** — model download unavailable",
            "n/a (not exposed)",
            "percentile bootstrap interval over questions (300 resamples, seed 788)",
            "Paired nDCG@k comparison",
            "- Bootstrap: percentile",
        ] {
            assert!(md.contains(s), "missing {s:?} in:\n{md}");
        }
        assert!(md.contains("different embedding"), "caveat must be printed");
        assert!(md.contains("shipped default"));
        // Point estimate is bolded and followed by its interval.
        let re = md
            .lines()
            .find(|l| l.starts_with("| `qmd` |") && l.contains("**"))
            .unwrap();
        assert!(re.contains("] |") || re.contains("] "), "{re}");
        // v2-scale corpus gets the synthetic-corpus caveat; this fixture (60 questions) keeps v1's.
        assert!(md.contains("small and hand-built"));
    }

    #[test]
    fn private_run_never_claims_the_corpus_is_synthetic() {
        let mut r = report();
        r.meta.category_counts.insert(Category::Direct, 200);
        r.meta.private = true;
        let md = r.render_markdown();
        assert!(md.contains("private notebook"), "{md}");
        assert!(!md.contains("synthetic"), "{md}");
        assert!(!md.contains("hand-built"), "{md}");
    }

    #[test]
    fn private_run_withholds_skip_reasons_that_echo_a_query_error() {
        let secret = "SECRET-PRIVATE-QUERY-TEXT";
        let mut r = report();
        r.configs[1].status = Status::Skipped(format!(
            "all 3 queries failed; first error: q1: bad request for {secret}"
        ));
        let public_md = r.render_markdown();
        assert!(
            public_md.contains(secret),
            "non-private runs keep the detail"
        );
        r.meta.private = true;
        let md = r.render_markdown();
        let js = r.to_json().to_string();
        assert!(!md.contains(secret) && !js.contains(secret), "{md}\n{js}");
        assert!(md.contains("all 3 queries failed; detail withheld"), "{md}");
        assert!(js.contains("all 3 queries failed; detail withheld"), "{js}");
    }

    #[test]
    fn a_pair_reads_the_same_interval_from_either_side() {
        let r = report();
        let ran = r.ran();
        assert!(ran.len() >= 2);
        let (a, b) = (
            (ran[0].0.id.as_str(), ran[0].1),
            (ran[1].0.id.as_str(), ran[1].1),
        );
        for cat in [None, Some(Category::Temporal)] {
            let ab = r.paired(a, b, cat, ("ndcg", NDCG));
            let ba = r.paired(b, a, cat, ("ndcg", NDCG));
            match (ab, ba) {
                (Some(x), Some(y)) => {
                    assert_eq!(x.flipped(), y, "{cat:?}");
                    assert_eq!(x.verdict, y.flipped().verdict);
                }
                (None, None) => {}
                other => panic!("one orientation read a pair the other could not: {other:?}"),
            }
        }
    }

    #[test]
    fn large_corpus_gets_the_synthetic_disclaimer() {
        let mut r = report();
        r.meta.category_counts.insert(Category::Direct, 200);
        let md = r.render_markdown();
        assert!(md.contains("synthetic (generated structure"), "{md}");
        assert!(!md.contains("small and hand-built"));
    }

    #[test]
    fn json_is_well_formed_carries_winners_bootstrap_and_intervals() {
        let j = report().to_json();
        assert_eq!(j["winners"]["paraphrase"][0], "qmd");
        assert_eq!(j["hypotheses"]["h1_lcg_beats_qmd_on_temporal"], true);
        assert_eq!(j["hypotheses"]["h1_reading"], "yes");
        assert_eq!(j["configs"][2]["skipped"], "model download unavailable");
        assert_eq!(j["meta"]["latency_convention"], "warm");
        assert_eq!(j["bootstrap"]["seed"], 788);
        assert_eq!(j["bootstrap"]["resamples"], 300);
        let ci = &j["configs"][0]["ci"]["overall"]["ndcg"];
        let (lo, mean, hi) = (
            ci["lo"].as_f64().unwrap(),
            ci["mean"].as_f64().unwrap(),
            ci["hi"].as_f64().unwrap(),
        );
        assert!(lo <= mean && mean <= hi, "{ci}");
        assert!(j["configs"][0]["ci"]["by_category"]["temporal"]["recall"].is_object());
        assert!(j["configs"][0]["origins"].is_null());
        // The new categories are absent from a v1-shaped report.
        assert!(j["winners"].get("multi-hop").is_none());
        // Pre-existing consumers' keys are all still there.
        for k in [
            "overall",
            "by_category",
            "latency_p50_ms",
            "questions",
            "grounded",
        ] {
            assert!(j["configs"][0].get(k).is_some(), "missing {k}");
        }
    }

    #[test]
    fn rendering_is_deterministic_and_seed_sensitive() {
        assert_eq!(report().render_markdown(), report().render_markdown());
        assert_eq!(
            serde_json::to_string(&report().to_json()).unwrap(),
            serde_json::to_string(&report().to_json()).unwrap()
        );
        let mut other = report();
        other.meta.bootstrap.seed = 7;
        assert_ne!(report().render_markdown(), other.render_markdown());
    }

    #[test]
    fn an_interval_does_not_change_when_another_configuration_is_added() {
        let (l, q) = two((0.9, 0.4), (0.5, 0.8));
        let base = report_with(l.clone(), q.clone());
        let mut more = report_with(l.clone(), q);
        more.configs.push(ConfigOutcome {
            id: "lcg-passages".into(),
            description: "d".into(),
            status: run(l),
        });
        let get = |r: &Report| r.to_json()["configs"][1]["ci"]["overall"]["ndcg"].clone();
        assert_eq!(get(&base), get(&more));
    }

    #[test]
    fn ceiling_scope_is_stated_not_ranked() {
        let mk = |cat| (0..N).map(|i| score(cat, i, 1.0, None)).collect::<Vec<_>>();
        let mut r = report_with(mk(Category::Direct), mk(Category::Direct));
        r.meta.category_counts.insert(Category::Direct, N);
        let md = r.render_markdown();
        assert!(md.contains("At the ceiling"), "{md}");
        assert!(r.ceiling_note(Some(Category::Direct)).is_some());
        assert!(r.ceiling_note(Some(Category::Temporal)).is_none());
    }

    #[test]
    fn small_categories_are_flagged_low_n_with_a_visibly_wide_interval() {
        let mut r = report();
        r.meta.category_counts.insert(Category::Temporal, 5);
        let md = r.render_markdown();
        assert!(md.contains("### temporal (n = 5) — **low n**"), "{md}");
        assert!(md.contains("### paraphrase (n = 30)\n"), "{md}");
    }

    #[test]
    fn a_category_with_one_question_gets_na_not_a_zero_width_interval() {
        let mut scores = cat_scores(Category::Temporal, 1, 0.9, Some(false));
        scores.extend(cat_scores(Category::Paraphrase, N, 0.5, None));
        let mut r = report_with(scores.clone(), scores);
        r.meta.category_counts.insert(Category::Temporal, 1);
        let md = r.render_markdown();
        assert!(md.contains("[n/a]"), "{md}");
        assert!(r.to_json()["configs"][0]["ci"]["by_category"]["temporal"]["ndcg"].is_null());
    }

    #[test]
    fn all_tied_configs_are_not_marked_best() {
        assert_eq!(best_value(&[Some(1.0), Some(1.0)], true), None);
        assert_eq!(best_value(&[Some(1.0), None], true), None);
        assert_eq!(best_value(&[Some(0.2), Some(0.5)], false), Some(0.2));
    }

    #[test]
    fn false_positive_note_only_when_every_config_is_pinned_at_one() {
        let mut r = report();
        for c in &mut r.configs {
            if let Status::Ran(run) = &mut c.status {
                run.overall.false_positive_rate = Some(1.0);
            }
        }
        assert!(r.false_positive_note().is_some());
        assert!(r.render_markdown().contains("non-discriminating"));
        if let Status::Ran(run) = &mut r.configs[0].status {
            run.overall.false_positive_rate = Some(0.5);
        }
        assert!(r.false_positive_note().is_none());
    }

    #[test]
    fn unconfirmed_entity_links_are_surfaced_only_when_present() {
        let mut r = report();
        assert!(!r.render_markdown().contains("without `episode_uuids`"));
        r.ingest.lcg_unconfirmed_entity_links = 4;
        let md = r.render_markdown();
        assert!(md.contains("4 entity→note link(s) were accepted without `episode_uuids`"));
        assert_eq!(r.to_json()["ingest"]["lcg_unconfirmed_entity_links"], 4);
    }

    #[test]
    fn supersession_effect_is_reported_when_supersessions_ran() {
        let mut r = report();
        assert!(!r.render_markdown().contains("Belief ended for"));
        r.ingest.lcg_supersessions = 8;
        r.ingest.lcg_superseded_episodes = 9;
        assert!(r
            .render_markdown()
            .contains("Belief ended for 9 episode(s) across 8 document supersession(s)."));
        assert_eq!(r.to_json()["ingest"]["lcg_superseded_episodes"], 9);
    }

    #[test]
    fn evidence_gaps_are_reported_as_a_grounded_lower_bound() {
        let mut r = report();
        assert!(!r.render_markdown().contains("carried no snippet"));
        if let Status::Ran(run) = &mut r.configs[0].status {
            run.evidence_gaps = 3;
        }
        let md = r.render_markdown();
        assert!(md.contains("3 returned hit(s) carried no snippet"), "{md}");
        assert!(md.contains("lower bound"));
        assert_eq!(r.to_json()["configs"][0]["evidence_gaps"], 3);
    }

    #[test]
    fn failed_queries_are_listed_and_private_mode_withholds_their_detail() {
        let mut r = report();
        if let Status::Ran(run) = &mut r.configs[0].status {
            run.query_errors = vec!["temporal-03: no such passage for 'my secret query'".into()];
        }
        let md = r.render_markdown();
        assert!(md.contains("my secret query"));
        r.meta.private = true;
        let md = r.render_markdown();
        assert!(md.contains("temporal-03: error (detail withheld)"), "{md}");
        assert!(!md.contains("secret"), "{md}");
        assert!(!serde_json::to_string(&r.to_json())
            .unwrap()
            .contains("secret"));
    }

    #[test]
    fn origin_section_appears_only_with_origin_data() {
        let mut r = report();
        assert!(!r.render_markdown().contains("By origin of returned notes"));
        let od = OriginData {
            per_question: vec![
                (
                    "a".into(),
                    OriginQuestion {
                        restating_share: 0.25,
                        outranks_primary: Some(true),
                        derived_only: false,
                    },
                ),
                (
                    "b".into(),
                    OriginQuestion {
                        restating_share: 0.0,
                        outranks_primary: None,
                        derived_only: true,
                    },
                ),
            ],
            mix: [("primary".to_string(), 6), ("derived".to_string(), 2)]
                .into_iter()
                .collect(),
            returned: 8,
        };
        if let Status::Ran(run) = &mut r.configs[0].status {
            run.origin = Some(od);
        }
        let md = r.render_markdown();
        assert!(md.contains("## By origin of returned notes"), "{md}");
        assert!(md.contains("| `lcg-entities-facts-belief` | 2 |"), "{md}");
        assert!(md.contains("0.750"), "primary share 6/8: {md}");
        let j = r.to_json();
        let o = &j["configs"][0]["origins"];
        assert_eq!(o["eligible_questions"], 2);
        assert_eq!(o["outrank_rate"]["n"], 1);
        assert_eq!(o["returned_by_origin"]["derived"], 2);
        assert_eq!(o["derived_only_rate"]["mean"], 0.5);
    }

    #[test]
    fn new_categories_are_listed_only_when_present() {
        let mut r = report();
        assert!(!r.render_markdown().contains("multi-hop"));
        r.meta.category_counts.insert(Category::MultiHop, 30);
        let md = r.render_markdown();
        assert!(md.contains("### multi-hop (n = 30)"), "{md}");
        assert!(r.to_json()["winners"].get("multi-hop").is_some());
    }

    fn staged_scores((t, p): (f64, f64)) -> Vec<QuestionScore> {
        let mut v = cat_scores(Category::Temporal, N, t, Some(false));
        v.extend(cat_scores(Category::Paraphrase, N, p, None));
        v
    }

    /// `lcg-passages` and `lcg-passages-rerank` with the given (temporal, paraphrase) levels.
    fn stage_report(base: (f64, f64), staged: (f64, f64), p50: f64) -> Report {
        let mut r = report();
        r.configs.clear();
        let mut staged_status = run(staged_scores(staged));
        if let Status::Ran(run) = &mut staged_status {
            run.latency_p50_ms = Some(p50);
        }
        for (id, status) in [
            ("lcg-passages", run(staged_scores(base))),
            ("lcg-passages-rerank", staged_status),
        ] {
            r.configs.push(ConfigOutcome {
                id: id.into(),
                description: "d".into(),
                status,
            });
        }
        r
    }

    #[test]
    fn rerank_acceptance_met_when_paraphrase_gains_and_temporal_holds() {
        let r = stage_report((0.6, 0.15), (0.6, 0.5), 90.0);
        let a = r.rerank_acceptance().expect("both configurations ran");
        assert_eq!(a.paraphrase_gain, Some(true));
        assert_eq!(a.overall_not_lower, Some(true));
        assert_eq!(a.temporal_no_drop, Some(true));
        assert_eq!(a.p50_within_budget, Some(true));
        let md = r.render_markdown();
        assert!(md.contains("## Passage stages"), "{md}");
        assert!(
            md.contains("`lcg-passages-rerank` | `lcg-passages` | paraphrase"),
            "{md}"
        );
        assert!(
            md.contains("Paraphrase nDCG@k improves, paired interval above zero: met."),
            "{md}"
        );
    }

    #[test]
    fn rerank_acceptance_flags_a_temporal_collapse_and_a_blown_budget() {
        // The qmd-rerank failure mode: paraphrase up, temporal wrecked, and slow.
        let r = stage_report((0.63, 0.15), (0.29, 0.5), 400.0);
        let a = r.rerank_acceptance().unwrap();
        assert_eq!(a.paraphrase_gain, Some(true));
        assert_eq!(a.temporal_no_drop, Some(false));
        assert_eq!(a.p50_within_budget, Some(false));
        let md = r.render_markdown();
        assert!(md.contains("no supported drop): **not met**"), "{md}");
        assert!(md.contains("measured 400 ms"), "{md}");
    }

    /// "Expansion on top of rerank" must compare `lcg-passages-expand-rerank` with
    /// `lcg-passages-rerank`, not expand-only with rerank.
    #[test]
    fn expansion_on_top_of_rerank_compares_expand_rerank_with_rerank() {
        let mut r = stage_report((0.6, 0.15), (0.6, 0.5), 90.0);
        for id in ["lcg-passages-expand", "lcg-passages-expand-rerank"] {
            r.configs.push(ConfigOutcome {
                id: id.into(),
                description: "d".into(),
                status: run(staged_scores((0.6, 0.6))),
            });
        }
        let md = r.render_markdown();
        assert!(
            md.contains("`lcg-passages-expand-rerank` | `lcg-passages-rerank` |"),
            "{md}"
        );
        assert!(
            !md.contains("`lcg-passages-expand` | `lcg-passages-rerank` |"),
            "{md}"
        );
    }

    /// Stage configs pin a raw first stage, so they pair with `lcg-passages-raw` when it ran (and
    /// the belief variant with `lcg-passages-belief`); `lcg-passages` is only the fallback.
    #[test]
    fn stages_pair_with_their_matching_baseline() {
        let mut r = stage_report((0.6, 0.15), (0.6, 0.5), 90.0);
        for (id, levels) in [
            ("lcg-passages-raw", (0.5, 0.1)),
            ("lcg-passages-belief", (0.7, 0.3)),
            ("lcg-passages-belief-rerank", (0.8, 0.4)),
        ] {
            r.configs.push(ConfigOutcome {
                id: id.into(),
                description: "d".into(),
                status: run(staged_scores(levels)),
            });
        }
        let md = r.render_markdown();
        assert!(
            md.contains("`lcg-passages-rerank` | `lcg-passages-raw` |"),
            "{md}"
        );
        assert!(
            md.contains("`lcg-passages-belief-rerank` | `lcg-passages-belief` |"),
            "{md}"
        );
        assert!(
            !md.contains("`lcg-passages-rerank` | `lcg-passages` |"),
            "{md}"
        );
        assert!(
            md.contains("Rerank acceptance (`lcg-passages-rerank` vs `lcg-passages-raw`)"),
            "{md}"
        );
    }

    /// A run with the stage configs and `lcg-passages-raw` but no `lcg-passages` still gets its
    /// section, paired with the raw baseline.
    #[test]
    fn stage_section_is_emitted_without_lcg_passages() {
        let mut r = report();
        r.configs.clear();
        for (id, levels) in [
            ("lcg-passages-raw", (0.6, 0.15)),
            ("lcg-passages-rerank", (0.6, 0.5)),
        ] {
            r.configs.push(ConfigOutcome {
                id: id.into(),
                description: "d".into(),
                status: run(staged_scores(levels)),
            });
        }
        let md = r.render_markdown();
        assert!(md.contains("## Passage stages"), "{md}");
        assert!(
            md.contains("`lcg-passages-rerank` | `lcg-passages-raw` |"),
            "{md}"
        );
        assert!(r.rerank_acceptance().is_some());
    }

    /// Only the belief variant and its baseline ran: the section is still emitted.
    #[test]
    fn stage_section_is_emitted_for_belief_rerank_with_belief_only() {
        let mut r = report();
        r.configs.clear();
        for (id, levels) in [
            ("lcg-passages-belief", (0.7, 0.3)),
            ("lcg-passages-belief-rerank", (0.8, 0.4)),
        ] {
            r.configs.push(ConfigOutcome {
                id: id.into(),
                description: "d".into(),
                status: run(staged_scores(levels)),
            });
        }
        let md = r.render_markdown();
        assert!(md.contains("## Passage stages"), "{md}");
        assert!(
            md.contains("`lcg-passages-belief-rerank` | `lcg-passages-belief` |"),
            "{md}"
        );
    }

    #[test]
    fn stage_section_is_absent_without_a_stage_configuration() {
        let md = report().render_markdown();
        assert!(!md.contains("Passage stages"));
        assert!(report().rerank_acceptance().is_none());
    }

    #[test]
    fn stage_section_survives_several_embedding_profiles() {
        // With several profiles the ids are `<id>@<profile>`; the section and the rerank
        // acceptance checks must still be produced, once per profile.
        let mut r = report();
        r.configs.clear();
        for (profile, staged) in [("none", (0.6, 0.5)), ("bge-v1.5", (0.3, 0.5))] {
            for (id, levels) in [
                ("lcg-passages", (0.6, 0.15)),
                ("lcg-passages-rerank", staged),
            ] {
                r.configs.push(ConfigOutcome {
                    id: format!("{id}@{profile}"),
                    description: "d".into(),
                    status: run(staged_scores(levels)),
                });
            }
        }
        let md = r.render_markdown();
        assert!(md.contains("embedding profile `none`"), "{md}");
        assert!(md.contains("embedding profile `bge-v1.5`"), "{md}");
        assert!(
            md.contains("`lcg-passages-rerank@none` | `lcg-passages@none` | paraphrase"),
            "{md}"
        );
        assert_eq!(
            r.rerank_acceptance_for("@none").unwrap().temporal_no_drop,
            Some(true)
        );
        assert_eq!(
            r.rerank_acceptance_for("@bge-v1.5")
                .unwrap()
                .temporal_no_drop,
            Some(false)
        );
        assert!(r.rerank_acceptance().is_some());
    }
}
