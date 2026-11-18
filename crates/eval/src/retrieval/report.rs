//! Report rendering (`report.md` / `report.json`) for the retrieval eval (FR-015, FR-016, SC-007).

use std::collections::BTreeMap;
use std::fmt::Write;

use serde_json::{json, Value};

use super::fixture::Category;
use super::metrics::{QuestionScore, Summary};

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
    pub lookup_calls: u64,
    /// Returned hits with no evidence text, judged unsupported (see `RankedResult::evidence_gaps`).
    pub evidence_gaps: u64,
    pub query_errors: Vec<String>,
    pub grounded: Grounded,
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

/// Which metric a "best" mark is for, and whether higher or lower is better.
#[derive(Clone, Copy)]
struct Col {
    name: &'static str,
    get: fn(&Summary) -> Option<f64>,
    higher_better: bool,
}

const COLS: [Col; 8] = [
    Col {
        name: "recall@k",
        get: |s| s.recall,
        higher_better: true,
    },
    Col {
        name: "MRR",
        get: |s| s.mrr,
        higher_better: true,
    },
    Col {
        name: "nDCG@k",
        get: |s| s.ndcg,
        higher_better: true,
    },
    Col {
        name: "grounded-recall@k",
        get: |s| s.grounded_recall,
        higher_better: true,
    },
    Col {
        name: "grounded-MRR",
        get: |s| s.grounded_mrr,
        higher_better: true,
    },
    Col {
        name: "stale>current",
        get: |s| s.stale_above_current_rate,
        higher_better: false,
    },
    Col {
        name: "stale-in-top-k",
        get: |s| s.stale_in_top_k_rate,
        higher_better: false,
    },
    Col {
        name: "false-pos@k",
        get: |s| s.false_positive_rate,
        higher_better: false,
    },
];

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

impl Report {
    fn ran(&self) -> Vec<(&ConfigOutcome, &ConfigRun)> {
        self.configs
            .iter()
            .filter_map(|c| match &c.status {
                Status::Ran(r) => Some((c, r.as_ref())),
                Status::Skipped(_) => None,
            })
            .collect()
    }

    /// Configuration(s) with the best nDCG@k in a category (ties all returned).
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

    fn best_nd(&self, cat: Category, prefix: &str) -> Option<(String, f64, Option<f64>)> {
        self.ran()
            .into_iter()
            .filter(|(c, _)| c.id.starts_with(prefix))
            .filter_map(|(c, r)| {
                let s = r.by_category.get(&cat)?;
                Some((c.id.clone(), s.ndcg?, s.stale_above_current_rate))
            })
            .fold(None, |m: Option<(String, f64, Option<f64>)>, x| match m {
                Some(cur) if cur.1 >= x.1 => Some(cur),
                _ => Some(x),
            })
    }

    /// Yes/no reading of the two stated hypotheses (SC-007). `None` = cannot be read.
    pub fn hypotheses(&self) -> (Option<bool>, Option<bool>) {
        let h1 = match (
            self.best_nd(Category::Temporal, "lcg"),
            self.best_nd(Category::Temporal, "qmd"),
        ) {
            (Some(l), Some(q)) => Some(l.1 > q.1),
            _ => None,
        };
        let h2 = match (
            self.best_nd(Category::Paraphrase, "qmd"),
            self.best_nd(Category::Paraphrase, "lcg"),
        ) {
            (Some(q), Some(l)) => Some(q.1 > l.1),
            _ => None,
        };
        (h1, h2)
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

    pub fn render_markdown(&self) -> String {
        let m = &self.meta;
        let mut o = String::new();
        let _ = writeln!(o, "# Retrieval quality eval\n");
        let _ = writeln!(
            o,
            "Corpus `{}` (hash `{}`), questions `{}` (hash `{}`), shared k = {}.\n",
            m.corpus_version, m.corpus_hash, m.questions_version, m.questions_hash, m.k
        );
        let _ = writeln!(
            o,
            "> **Read with care.** LCG and qmd use different embedding (and reranker) models, so this compares *systems*, not retrieval algorithms. The corpus is small and hand-built: results are indicative, not general. `lcg-passages` is vector-only (`knowledge_search_passages` does no FTS or RRF).\n"
        );

        let _ = writeln!(o, "## Configurations\n");
        for c in &self.configs {
            match &c.status {
                Status::Ran(_) => {
                    let _ = writeln!(o, "- `{}`: {}", c.id, c.description);
                }
                Status::Skipped(why) => {
                    let _ = writeln!(o, "- `{}`: **skipped** — {why}", c.id);
                }
            }
        }

        let _ = writeln!(o, "\n## Overall\n");
        self.table(&mut o, None);
        let w = self.winners(None);
        let _ = writeln!(o, "\nBest overall by nDCG@k: {}.", fmt_winners(&w));

        if let Some(note) = self.false_positive_note() {
            let _ = writeln!(o, "\n{note}");
        }

        let _ = writeln!(o, "\n## Per category\n");
        for cat in Category::ALL {
            let n = m.category_counts.get(&cat).copied().unwrap_or(0);
            let _ = writeln!(o, "### {} (n = {n})\n", cat.label());
            if n == 0 {
                let _ = writeln!(o, "_No questions in this category._\n");
                continue;
            }
            self.table(&mut o, Some(cat));
            let _ = writeln!(
                o,
                "\nWinner by nDCG@k: {}.\n",
                fmt_winners(&self.winners(Some(cat)))
            );
        }

        let _ = writeln!(o, "## Hypotheses\n");
        let (h1, h2) = self.hypotheses();
        let yn = |h: Option<bool>| match h {
            Some(true) => "**YES**",
            Some(false) => "**NO**",
            None => "**cannot be read** (a side is missing or skipped)",
        };
        let _ = writeln!(
            o,
            "- H1 — LCG beats qmd on temporal/superseded questions: {} (best `lcg-*` vs best `qmd*` by temporal nDCG@k; see the stale>current column — the belief-aware facts variant is the only one that can filter superseded content, ADR-0705).",
            yn(h1)
        );
        let _ = writeln!(
            o,
            "- H2 — qmd beats LCG on paraphrase questions: {} (best `qmd*` vs best `lcg-*` by paraphrase nDCG@k).",
            yn(h2)
        );

        let _ = writeln!(o, "\n## Cost\n");
        let _ = writeln!(o, "Per query, **warm** (indices built and one unmeasured warm-up pass before timing). qmd runs as one process per query, so its latency is **per-process** (it includes model load) and it gets a single warm-up query rather than a full pass.\n");
        let _ = writeln!(o, "| config | p50 ms | p95 ms | model calls/query | lookup calls | query errors | grounded judge |");
        let _ = writeln!(o, "|---|---|---|---|---|---|---|");
        for (c, r) in self.ran() {
            let calls = r
                .model_calls_per_query
                .map_or("n/a (not exposed)".to_string(), |v| format!("{v:.1}"));
            let g = match &r.grounded {
                Grounded::Judged {
                    paid_calls,
                    cache_hits,
                } => {
                    format!("{paid_calls} paid, {cache_hits} cached")
                }
                Grounded::Skipped(why) => format!("skipped ({why})"),
            };
            let _ = writeln!(
                o,
                "| `{}` | {} | {} | {calls} | {} | {} | {g} |",
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
                let _ = writeln!(o, "- `{}` query error: {e}", c.id);
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
            "- Latency convention: warm, per query, p50/p95 over one measured pass."
        );
        let _ = writeln!(o, "- Determinism: relevance metrics are exactly reproducible for a fixed LCG graph and embedder. LCG ingest is deterministic only with a replayed cassette; qmd query expansion and reranking are model-dependent; the grounded judge is cached by (model, key fact, text).");
        o
    }

    fn table(&self, o: &mut String, cat: Option<Category>) {
        let rows: Vec<(&str, Option<&Summary>)> = self
            .configs
            .iter()
            .map(|c| {
                let s = match &c.status {
                    Status::Ran(r) => match cat {
                        Some(cat) => r.by_category.get(&cat),
                        None => Some(&r.overall),
                    },
                    Status::Skipped(_) => None,
                };
                (c.id.as_str(), s)
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
                    .map(|(_, s)| s.and_then(|s| (c.get)(s)))
                    .collect();
                best_value(&vals, c.higher_better)
            })
            .collect();
        for ((id, s), _) in rows.iter().zip(0..) {
            let _ = write!(o, "| `{id}` |");
            match s {
                None => {
                    let _ = write!(o, " skipped |{}", " |".repeat(COLS.len() - 1));
                }
                Some(s) => {
                    for (ci, c) in COLS.iter().enumerate() {
                        let v = (c.get)(s);
                        let txt = f3(v);
                        let marked = match (v, bests[ci]) {
                            (Some(v), Some(b)) if (v - b).abs() < 1e-12 => format!("**{txt}**"),
                            _ => txt,
                        };
                        let _ = write!(o, " {marked} |");
                    }
                }
            }
            let _ = writeln!(o);
        }
        // Sample sizes behind the means (negatives, temporal and grounded subsets differ).
        if let Some((_, Some(s))) = rows.iter().find(|(_, s)| s.is_some()) {
            let _ = writeln!(
                o,
                "\n_n: {} questions; {} with relevance judgements; {} negative; {} temporal; {} grounded._",
                s.questions, s.n_relevance, s.n_negative, s.n_temporal, s.n_grounded
            );
        }
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
                Status::Skipped(why) => json!({"id": c.id, "description": c.description, "skipped": why}),
                Status::Ran(r) => {
                    let by: serde_json::Map<String, Value> = r
                        .by_category
                        .iter()
                        .map(|(k, s)| (k.label().to_string(), summ(s)))
                        .collect();
                    json!({
                        "id": c.id, "description": c.description,
                        "overall": summ(&r.overall), "by_category": by,
                        "latency_p50_ms": r.latency_p50_ms, "latency_p95_ms": r.latency_p95_ms,
                        "model_calls_per_query": r.model_calls_per_query,
                        "lookup_calls": r.lookup_calls,
                        "evidence_gaps": r.evidence_gaps,
                        "query_errors": r.query_errors,
                        "grounded": match &r.grounded {
                            Grounded::Judged { paid_calls, cache_hits } => json!({"paid_calls": paid_calls, "cache_hits": cache_hits}),
                            Grounded::Skipped(w) => json!({"skipped": w}),
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
        json!({
            "meta": {
                "corpus_version": m.corpus_version, "corpus_hash": m.corpus_hash,
                "questions_version": m.questions_version, "questions_hash": m.questions_hash,
                "k": m.k, "lcg_build": m.lcg_build, "qmd_version": m.qmd_version,
                "qmd_index": m.qmd_index, "embedder": m.embedder, "extractor": m.extractor,
                "qmd_models": m.qmd_models, "judge_model": m.judge_model,
                "category_counts": cat_counts, "latency_convention": "warm",
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
            "winners": Category::ALL.iter().map(|c| (c.label().to_string(), self.winners(Some(*c)))).collect::<BTreeMap<_, _>>(),
            "hypotheses": {"h1_lcg_beats_qmd_on_temporal": h1, "h2_qmd_beats_lcg_on_paraphrase": h2},
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::metrics::summarize_by_category;

    fn score(cat: Category, ndcg: f64, stale: Option<bool>) -> QuestionScore {
        QuestionScore {
            id: format!("{}-{ndcg}", cat.label()),
            category: cat,
            recall: Some(ndcg),
            mrr: Some(ndcg),
            ndcg: Some(ndcg),
            false_positive: None,
            stale_above_current: stale,
            stale_in_top_k: stale,
            grounded_recall: None,
            grounded_mrr: None,
        }
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
            lookup_calls: 0,
            evidence_gaps: 0,
            query_errors: vec![],
            grounded: Grounded::Skipped("no judge".into()),
        }))
    }

    fn report() -> Report {
        let mut meta = RunMeta {
            corpus_version: "nb-1".into(),
            k: 10,
            ..Default::default()
        };
        meta.category_counts.insert(Category::Temporal, 1);
        meta.category_counts.insert(Category::Paraphrase, 1);
        Report {
            meta,
            ingest: IngestCost::default(),
            configs: vec![
                ConfigOutcome {
                    id: "lcg-entities-facts-belief".into(),
                    description: "d".into(),
                    status: run(vec![
                        score(Category::Temporal, 0.9, Some(false)),
                        score(Category::Paraphrase, 0.4, None),
                    ]),
                },
                ConfigOutcome {
                    id: "qmd".into(),
                    description: "d".into(),
                    status: run(vec![
                        score(Category::Temporal, 0.5, Some(true)),
                        score(Category::Paraphrase, 0.8, None),
                    ]),
                },
                ConfigOutcome {
                    id: "qmd-rerank".into(),
                    description: "d".into(),
                    status: Status::Skipped("model download unavailable".into()),
                },
            ],
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
    fn winners_per_category_and_hypotheses() {
        let r = report();
        assert_eq!(
            r.winners(Some(Category::Temporal)),
            vec!["lcg-entities-facts-belief"]
        );
        assert_eq!(r.winners(Some(Category::Paraphrase)), vec!["qmd"]);
        assert!(r.winners(Some(Category::Direct)).is_empty());
        assert_eq!(r.hypotheses(), (Some(true), Some(true)));
    }

    #[test]
    fn hypotheses_unreadable_when_a_side_is_skipped() {
        let mut r = report();
        r.configs[1].status = Status::Skipped("qmd not installed".into());
        assert_eq!(r.hypotheses(), (None, None));
        assert!(r.render_markdown().contains("cannot be read"));
    }

    #[test]
    fn markdown_has_required_sections_and_marks_best_and_skips() {
        let md = report().render_markdown();
        for s in [
            "## Overall",
            "## Per category",
            "### temporal (n = 1)",
            "### direct (n = 0)",
            "## Hypotheses",
            "## Cost",
            "## Reproduction",
            "Winner by nDCG@k: `qmd`",
            "**skipped** — model download unavailable",
            "n/a (not exposed)",
        ] {
            assert!(md.contains(s), "missing {s:?} in:\n{md}");
        }
        assert!(md.contains("**0.900**"), "{md}");
        assert!(md.contains("different embedding"), "caveat must be printed");
        assert!(md.contains("vector-only"));
    }

    #[test]
    fn json_is_well_formed_and_carries_winners() {
        let j = report().to_json();
        assert_eq!(j["winners"]["paraphrase"][0], "qmd");
        assert_eq!(j["hypotheses"]["h1_lcg_beats_qmd_on_temporal"], true);
        assert_eq!(j["configs"][2]["skipped"], "model download unavailable");
        assert_eq!(j["meta"]["latency_convention"], "warm");
    }

    #[test]
    fn rendering_is_deterministic() {
        assert_eq!(report().render_markdown(), report().render_markdown());
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
}
