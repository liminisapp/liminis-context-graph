//! The graph-qa report (FR-023..FR-026): mean score per runner per shape, overall and per dev/test
//! half, an E13 overall kept apart from the inverse family, per-family means, paired-bootstrap
//! differences between every pair of runners, diagnostics and per-question detail.
//!
//! `scores` in the JSON holds only what the questions, plans and model responses determine, so a
//! second fully cached run gives a byte-identical `scores` object (SC-005); cache counts and other
//! run facts live in `meta`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use serde_json::{json, Value};

use super::cache::Counts;
use super::runner::RunOutput;
use crate::retrieval::generator::graph_questions::GraphQuestion;
use crate::retrieval::stats::{paired_diff_ci, seed_for, LEVEL};
use crate::retrieval::workspace::WorkspaceManifest;

/// Shapes in report order.
pub const SHAPES: [&str; 6] = ["chain-2", "chain-3", "list", "join", "history", "inverse"];

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub workspace: String,
    /// `ontology` or `free-form`.
    pub workspace_kind: String,
    pub ontology_hash: String,
    pub ontology_mode: Option<String>,
    pub manifest: Option<WorkspaceManifest>,
    pub lcg_build: String,
    pub embedder: String,
    pub questions_file: String,
    pub planner_model: String,
    pub reader_model: String,
    pub provider: String,
    pub plans_file: Option<String>,
    pub plans_model: Option<String>,
    pub plans_ontology: Option<String>,
    pub k: usize,
    pub seed: u64,
    pub resamples: usize,
    pub cache: BTreeMap<String, Counts>,
}

pub struct Report<'a> {
    pub meta: Meta,
    pub run: &'a RunOutput,
    pub questions: &'a [GraphQuestion],
    /// The plans file's recorded reference scores (`route_planned5_scores`), if any.
    pub reference: Option<&'a BTreeMap<String, f64>>,
}

/// A named subset of the questions.
struct Slice {
    name: String,
    keep: Box<dyn Fn(&GraphQuestion) -> bool>,
}

fn slice(name: &str, keep: impl Fn(&GraphQuestion) -> bool + 'static) -> Slice {
    Slice {
        name: name.to_string(),
        keep: Box::new(keep),
    }
}

fn halves(base: &str, keep: impl Fn(&GraphQuestion) -> bool + Clone + 'static) -> Vec<Slice> {
    let (k1, k2) = (keep.clone(), keep.clone());
    vec![
        slice(base, keep),
        slice(&format!("{base}/dev"), move |q| k1(q) && q.split == "dev"),
        slice(&format!("{base}/test"), move |q| k2(q) && q.split == "test"),
    ]
}

fn mean(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

fn fmt(x: Option<f64>) -> String {
    x.map_or("—".into(), |v| format!("{v:.3}"))
}

/// Reader responses that did not parse, per runner that ran: `(errors, answered)`.
pub fn reader_parse_errors(run: &RunOutput) -> BTreeMap<String, (usize, usize)> {
    let mut m: BTreeMap<String, (usize, usize)> =
        run.runners.iter().map(|r| (r.clone(), (0, 0))).collect();
    for q in &run.questions {
        for (r, a) in &q.per_runner {
            let e = m.entry(r.clone()).or_default();
            e.1 += 1;
            if a.reader_parse_error {
                e.0 += 1;
            }
        }
    }
    m
}

impl Report<'_> {
    fn by_id(&self) -> BTreeMap<&str, &GraphQuestion> {
        self.questions.iter().map(|q| (q.id.as_str(), q)).collect()
    }

    /// `(question id, score)` of `runner` over the questions `keep` selects.
    fn series(&self, runner: &str, keep: &dyn Fn(&GraphQuestion) -> bool) -> Vec<(String, f64)> {
        let by = self.by_id();
        self.run
            .questions
            .iter()
            .filter(|r| by.get(r.id.as_str()).is_some_and(|q| keep(q)))
            .filter_map(|r| r.per_runner.get(runner).map(|a| (r.id.clone(), a.score)))
            .collect()
    }

    fn mean_of(&self, runner: &str, keep: &dyn Fn(&GraphQuestion) -> bool) -> (Option<f64>, usize) {
        let s: Vec<f64> = self.series(runner, keep).into_iter().map(|x| x.1).collect();
        (mean(&s), s.len())
    }

    fn shapes_present(&self) -> Vec<&'static str> {
        SHAPES
            .iter()
            .copied()
            .filter(|s| self.questions.iter().any(|q| q.shape == *s))
            .collect()
    }

    fn families(&self) -> Vec<String> {
        let mut seen = Vec::new();
        for q in self.questions {
            if !seen.contains(&q.family) {
                seen.push(q.family.clone());
            }
        }
        seen
    }

    fn headline_slices(&self) -> Vec<Slice> {
        let mut v = halves("e13", |q: &GraphQuestion| q.set == "e13");
        v.extend(halves("all", |_: &GraphQuestion| true));
        v
    }

    fn shape_slices(&self) -> Vec<Slice> {
        self.shapes_present()
            .into_iter()
            .flat_map(|s| halves(&format!("shape:{s}"), move |q: &GraphQuestion| q.shape == s))
            .collect()
    }

    fn bootstrap_slices(&self) -> Vec<Slice> {
        let mut v = vec![
            slice("e13", |q: &GraphQuestion| q.set == "e13"),
            slice("all", |_: &GraphQuestion| true),
            slice("e13/dev", |q: &GraphQuestion| {
                q.set == "e13" && q.split == "dev"
            }),
            slice("e13/test", |q: &GraphQuestion| {
                q.set == "e13" && q.split == "test"
            }),
            slice("all/dev", |q: &GraphQuestion| q.split == "dev"),
            slice("all/test", |q: &GraphQuestion| q.split == "test"),
        ];
        for s in self.shapes_present() {
            v.push(slice(&format!("shape:{s}"), move |q: &GraphQuestion| {
                q.shape == s
            }));
        }
        v
    }

    fn pairs(&self) -> Vec<(String, String)> {
        let r = &self.run.runners;
        let mut out = Vec::new();
        for i in 0..r.len() {
            for j in i + 1..r.len() {
                out.push((r[i].clone(), r[j].clone()));
            }
        }
        out
    }

    fn means_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        for r in &self.run.runners {
            let mut slices = serde_json::Map::new();
            for s in self
                .headline_slices()
                .into_iter()
                .chain(self.shape_slices())
            {
                let (v, n) = self.mean_of(r, &s.keep);
                slices.insert(s.name, json!({"mean": v, "n": n}));
            }
            for f in self.families() {
                let ff = f.clone();
                let (v, n) = self.mean_of(r, &move |q: &GraphQuestion| q.family == ff);
                slices.insert(format!("family:{f}"), json!({"mean": v, "n": n}));
            }
            m.insert(r.clone(), Value::Object(slices));
        }
        Value::Object(m)
    }

    fn bootstrap_json(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for (a, b) in self.pairs() {
            for s in self.bootstrap_slices() {
                let sa = self.series(&a, &s.keep);
                let sb = self.series(&b, &s.keep);
                let label = format!("graph-qa:{a}|{b}|{}", s.name);
                let d = paired_diff_ci(
                    &sa,
                    &sb,
                    seed_for(self.meta.seed, &label),
                    self.meta.resamples,
                );
                out.push(match d {
                    Some(d) => json!({"a": a, "b": b, "slice": s.name, "n": d.n,
                        "mean_diff": d.mean_diff, "lo": d.lo, "hi": d.hi, "verdict": d.verdict.label()}),
                    None => json!({"a": a, "b": b, "slice": s.name, "n": 0, "mean_diff": null,
                        "lo": null, "hi": null, "verdict": "too few questions"}),
                });
            }
        }
        out
    }

    fn diagnostics_json(&self) -> Value {
        let by = self.by_id();
        let mut routes: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        let mut sources: BTreeMap<String, usize> = BTreeMap::new();
        let mut plan_errors: BTreeMap<String, usize> = BTreeMap::new();
        let mut unresolved: BTreeMap<String, usize> = BTreeMap::new();
        let mut parse_errors: BTreeMap<String, usize> = BTreeMap::new();
        for r in &self.run.questions {
            let shape = by.get(r.id.as_str()).map_or("?", |q| q.shape.as_str());
            *sources.entry(r.plan_source.to_string()).or_default() += 1;
            if r.plan_source != "none" {
                let route = match (&r.route, &r.plan_error) {
                    (Some(x), _) => x.clone(),
                    (None, _) => "plan-error".into(),
                };
                *routes
                    .entry(shape.to_string())
                    .or_default()
                    .entry(route)
                    .or_default() += 1;
            }
            if r.plan_error.is_some() {
                *plan_errors.entry(shape.to_string()).or_default() += 1;
            }
            if r.start_resolved == Some(false) {
                *unresolved.entry(shape.to_string()).or_default() += 1;
            }
            for (runner, a) in &r.per_runner {
                if a.reader_parse_error {
                    *parse_errors.entry(runner.clone()).or_default() += 1;
                }
            }
        }
        let reference = self.reference.map(|scores| {
            let pick = |keep: &dyn Fn(&GraphQuestion) -> bool| {
                let xs: Vec<f64> = self
                    .questions
                    .iter()
                    .filter(|q| keep(q))
                    .filter_map(|q| scores.get(&q.id).copied())
                    .collect();
                json!({"mean": mean(&xs), "n": xs.len()})
            };
            json!({"source": "route_planned5_scores in the plans file",
                   "e13": pick(&|q| q.set == "e13"),
                   "e13/test": pick(&|q| q.set == "e13" && q.split == "test")})
        });
        json!({
            "routes_by_shape": routes,
            "plan_sources": sources,
            "plan_errors_by_shape": plan_errors,
            "unresolved_starts_by_shape": unresolved,
            "reader_parse_errors_by_runner": parse_errors,
            "plans_file_unknown_ids": self.run.plan_warnings,
            "reference": reference,
        })
    }

    fn per_question_json(&self) -> Vec<Value> {
        let by = self.by_id();
        self.run
            .questions
            .iter()
            .map(|r| {
                let q = by[r.id.as_str()];
                let runners: serde_json::Map<String, Value> = r
                    .per_runner
                    .iter()
                    .map(|(k, a)| {
                        (
                            k.clone(),
                            json!({"answers": a.answers, "score": a.score,
                                   "reader_parse_error": a.reader_parse_error}),
                        )
                    })
                    .collect();
                json!({"id": r.id, "set": q.set, "shape": q.shape, "family": q.family,
                       "split": q.split, "text": q.text, "gold": q.answers, "scoring": q.scoring,
                       "plan_source": r.plan_source, "plan": r.plan, "plan_error": r.plan_error,
                       "route": r.route, "start_resolved": r.start_resolved, "runners": runners})
            })
            .collect()
    }

    /// The deterministic part of the report.
    pub fn scores_json(&self) -> Value {
        json!({
            "runners": self.run.runners,
            "skipped": self.run.skipped,
            "means": self.means_json(),
            "bootstrap": self.bootstrap_json(),
            "diagnostics": self.diagnostics_json(),
            "per_question": self.per_question_json(),
        })
    }

    pub fn to_json(&self) -> Value {
        let m = &self.meta;
        json!({
            "meta": {
                "workspace": m.workspace, "workspace_kind": m.workspace_kind,
                "ontology_hash": m.ontology_hash, "ontology_mode": m.ontology_mode,
                "workspace_manifest": m.manifest, "lcg_build": m.lcg_build, "embedder": m.embedder,
                "questions_file": m.questions_file, "question_count": self.questions.len(),
                "planner_model": m.planner_model, "reader_model": m.reader_model,
                "provider": m.provider,
                "planner_prompt_version": super::prompts::PLANNER_PROMPT_VERSION,
                "reader_prompt_version": super::prompts::READER_PROMPT_VERSION,
                "plans_file": m.plans_file, "plans_model": m.plans_model,
                "plans_ontology": m.plans_ontology, "k": m.k,
                "bootstrap": {"method": "paired percentile bootstrap over questions",
                              "level": LEVEL, "seed": m.seed, "resamples": m.resamples},
                "cache": m.cache,
            },
            "scores": self.scores_json(),
        })
    }

    pub fn render_markdown(&self) -> String {
        let m = &self.meta;
        let mut o = String::new();
        let _ = writeln!(o, "# Graph question answering (graph-qa)\n");
        let _ = writeln!(
            o,
            "Workspace `{}` ({}; ontology hash `{}`{}), {} questions from `{}`, k = {}.\n",
            m.workspace,
            m.workspace_kind,
            m.ontology_hash,
            m.ontology_mode
                .as_deref()
                .map(|x| format!(", mode {x}"))
                .unwrap_or_default(),
            self.questions.len(),
            m.questions_file,
            m.k
        );
        let _ = writeln!(
            o,
            "> One reader prompt for every runner. Scores: single answer = first answer matches; list and join = set F1; history = exact in-order match. **E13** is the 129-question E13 set; **all** adds the inverse family. `[lo, hi]` intervals are {:.0}% paired percentile bootstrap intervals over questions ({} resamples, base seed {}); an interval that includes zero is not distinguishable from noise.\n",
            LEVEL * 100.0,
            m.resamples,
            m.seed
        );
        for (r, why) in &self.run.skipped {
            let _ = writeln!(o, "- `{r}`: **skipped** — {why}");
        }
        if !self.run.skipped.is_empty() {
            o.push('\n');
        }

        let _ = writeln!(o, "## Overall\n");
        let heads = self.headline_slices();
        let _ = writeln!(
            o,
            "| runner | {} |",
            heads
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
                .join(" | ")
        );
        let _ = writeln!(o, "|---|{}", "---:|".repeat(heads.len()));
        for r in &self.run.runners {
            let cells: Vec<String> = heads
                .iter()
                .map(|s| {
                    let (v, n) = self.mean_of(r, &s.keep);
                    format!("{} (n={n})", fmt(v))
                })
                .collect();
            let _ = writeln!(o, "| `{r}` | {} |", cells.join(" | "));
        }
        // A reader that cannot be parsed scores every question 0: say so beside the numbers, so
        // an all-zero table is never mistaken for a measurement.
        let errs = reader_parse_errors(self.run);
        let _ = writeln!(
            o,
            "\nReader responses that did not parse (scored 0): {}",
            errs.iter()
                .map(|(r, (e, n))| format!("`{r}` {e}/{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );

        let _ = writeln!(o, "\n## By shape\n");
        let shapes = self.shapes_present();
        for half in ["", "/dev", "/test"] {
            let title = match half {
                "" => "all questions",
                "/dev" => "dev half",
                _ => "test half",
            };
            let _ = writeln!(o, "**{title}**\n");
            let _ = writeln!(o, "| runner | {} |", shapes.join(" | "));
            let _ = writeln!(o, "|---|{}", "---:|".repeat(shapes.len()));
            for r in &self.run.runners {
                let cells: Vec<String> = shapes
                    .iter()
                    .map(|s| {
                        let s = s.to_string();
                        let h = half.to_string();
                        let (v, _) = self.mean_of(r, &move |q: &GraphQuestion| {
                            q.shape == s && (h.is_empty() || q.split == h[1..])
                        });
                        fmt(v)
                    })
                    .collect();
                let _ = writeln!(o, "| `{r}` | {} |", cells.join(" | "));
            }
            o.push('\n');
        }

        let _ = writeln!(o, "## By family\n");
        let _ = writeln!(
            o,
            "| family | n | {} |",
            self.run
                .runners
                .iter()
                .map(|r| format!("`{r}`"))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        let _ = writeln!(o, "|---|---:|{}", "---:|".repeat(self.run.runners.len()));
        for f in self.families() {
            let ff = f.clone();
            let n = self.questions.iter().filter(|q| q.family == f).count();
            let cells: Vec<String> = self
                .run
                .runners
                .iter()
                .map(|r| {
                    let ff = ff.clone();
                    fmt(self.mean_of(r, &move |q: &GraphQuestion| q.family == ff).0)
                })
                .collect();
            let _ = writeln!(o, "| {f} | {n} | {} |", cells.join(" | "));
        }

        let boot = self.bootstrap_json();
        if !boot.is_empty() {
            let _ = writeln!(o, "\n## Paired differences (a − b)\n");
            let _ = writeln!(
                o,
                "| a | b | slice | n | mean diff | 95% interval | verdict |"
            );
            let _ = writeln!(o, "|---|---|---|---:|---:|---|---|");
            for d in &boot {
                let f3 = |k: &str| d[k].as_f64().map_or("—".into(), |v| format!("{v:+.3}"));
                let _ = writeln!(
                    o,
                    "| `{}` | `{}` | {} | {} | {} | [{}, {}] | {} |",
                    d["a"].as_str().unwrap_or(""),
                    d["b"].as_str().unwrap_or(""),
                    d["slice"].as_str().unwrap_or(""),
                    d["n"],
                    f3("mean_diff"),
                    f3("lo"),
                    f3("hi"),
                    d["verdict"].as_str().unwrap_or("")
                );
            }
        } else {
            let _ = writeln!(o, "\n(Paired differences need two runners; only one ran.)");
        }

        let d = self.diagnostics_json();
        let _ = writeln!(o, "\n## Diagnostics\n");
        let _ = writeln!(o, "- Plan sources: {}", d["plan_sources"]);
        let _ = writeln!(o, "- Routes by shape: {}", d["routes_by_shape"]);
        let _ = writeln!(o, "- Plan errors by shape: {}", d["plan_errors_by_shape"]);
        let _ = writeln!(
            o,
            "- Unresolved walk starts by shape: {}",
            d["unresolved_starts_by_shape"]
        );
        let _ = writeln!(
            o,
            "- Reader responses that did not parse: {}",
            d["reader_parse_errors_by_runner"]
        );
        let warn = &self.run.plan_warnings;
        if !warn.is_empty() {
            let _ = writeln!(
                o,
                "- WARNING: {} plans-file id(s) name no question and were ignored: {}",
                warn.len(),
                warn.join(", ")
            );
        }
        if !d["reference"].is_null() {
            let _ = writeln!(
                o,
                "- Reference (plans file `route_planned5_scores`): E13 {} (n={}), E13 test {} (n={})",
                fmt(d["reference"]["e13"]["mean"].as_f64()),
                d["reference"]["e13"]["n"],
                fmt(d["reference"]["e13/test"]["mean"].as_f64()),
                d["reference"]["e13/test"]["n"]
            );
        }

        let _ = writeln!(o, "\n## Per question\n");
        let _ = writeln!(
            o,
            "| id | shape | half | plan | route | start | {} |",
            self.run
                .runners
                .iter()
                .map(|r| format!("`{r}`"))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        let _ = writeln!(
            o,
            "|---|---|---|---|---|---|{}",
            "---:|".repeat(self.run.runners.len())
        );
        let by = self.by_id();
        for r in &self.run.questions {
            let q = by[r.id.as_str()];
            let cells: Vec<String> = self
                .run
                .runners
                .iter()
                .map(|x| {
                    r.per_runner
                        .get(x)
                        .map_or("—".into(), |a| format!("{:.2}", a.score))
                })
                .collect();
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {} | {} |",
                r.id,
                q.shape,
                q.split,
                if r.plan_error.is_some() {
                    format!("{} (error)", r.plan_source)
                } else {
                    r.plan_source.to_string()
                },
                r.route.as_deref().unwrap_or("—"),
                match r.start_resolved {
                    Some(true) => "resolved",
                    Some(false) => "unresolved",
                    None => "—",
                },
                cells.join(" | ")
            );
        }

        let _ = writeln!(o, "\n## Reproduction\n");
        let _ = writeln!(o, "- LCG build: {}", m.lcg_build);
        let _ = writeln!(o, "- Embedder: {}", m.embedder);
        match &m.manifest {
            Some(x) => {
                let _ = writeln!(
                    o,
                    "- Workspace built by `retrieval --keep-workspace`: corpus `{}` (hash `{}`), extraction {}{}",
                    x.corpus_version,
                    x.corpus_hash,
                    x.extractor,
                    x.cassette
                        .as_deref()
                        .map(|c| format!(", cassette `{c}`"))
                        .unwrap_or_default()
                );
            }
            None => {
                let _ = writeln!(
                    o,
                    "- Workspace provenance: unknown (no lcg-eval-workspace.json)"
                );
            }
        }
        let _ = writeln!(
            o,
            "- Planner: {} (prompt {}); reader: {} (prompt {}); provider: {}",
            m.planner_model,
            super::prompts::PLANNER_PROMPT_VERSION,
            m.reader_model,
            super::prompts::READER_PROMPT_VERSION,
            m.provider
        );
        if let Some(p) = &m.plans_file {
            let _ = writeln!(
                o,
                "- Plans file: `{p}` (model {}, ontology {})",
                m.plans_model.as_deref().unwrap_or("?"),
                m.plans_ontology.as_deref().unwrap_or("?")
            );
        }
        let counts: Vec<String> = m
            .cache
            .iter()
            .map(|(role, c)| format!("{role}: {} cached, {} paid", c.cache_hits, c.paid_calls))
            .collect();
        let _ = writeln!(
            o,
            "- Model calls: {}",
            if counts.is_empty() {
                "none".to_string()
            } else {
                counts.join("; ")
            }
        );
        let ids: BTreeSet<&str> = self.questions.iter().map(|q| q.id.as_str()).collect();
        let _ = writeln!(o, "- Questions: {}", ids.len());
        o
    }
}
