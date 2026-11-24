//! The graph-qa run (FR-013..FR-022): plans, then every runner's context, then one fixed reader
//! prompt over every context, then scoring.
//!
//! Every RPC goes through an [`RpcTransport`] and every model call through a [`ModelClient`] behind
//! the [`LlmCache`], so the whole run is testable with canned responders and scripted models.
//! Identical reader prompts are sent once (as the probe's `map_calls` did), so the same context
//! always gets the same answer. Model calls run bounded-parallel but results are indexed, so the
//! output is independent of completion order.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::{StreamExt, TryStreamExt};
use serde_json::{json, Value};

use super::cache::LlmCache;
use super::plans::{self, Plan, PlansFile};
use super::prompts::{self, PassageItem};
use super::scorer;
use crate::llm_client::{ModelClient, ModelRequest};
use crate::retrieval::generator::graph_questions::GraphQuestion;
use crate::retrieval::lcg::{LcgPassagesStaged, PassageMode, RpcTransport, EVAL_GROUP};
use crate::retrieval::mapping::note_of_source_description;
use crate::retrieval::qmd::{QmdRetriever, QmdRunner};
use crate::retrieval::Retriever;

/// Every runner, in report order.
pub const RUNNERS: [&str; 5] = ["passages", "passages-belief-rerank", "qmd", "walk", "route"];
/// The runners a run measures by default (the others need a reranker or a qmd install).
pub const DEFAULT_RUNNERS: [&str; 3] = ["passages", "walk", "route"];

/// qmd for the `qmd` runner: the index to query and the corpus notes the reader gets whole.
pub struct QmdSource {
    pub runner: Arc<dyn QmdRunner>,
    pub index: String,
    /// note id → markdown body.
    pub notes: BTreeMap<String, String>,
    /// note id → reference time (the passage date the reader sees).
    pub dates: BTreeMap<String, String>,
}

pub enum QmdSetup {
    Ready(QmdSource),
    /// Skipped with this reason, as `retrieval` does (FR-014).
    Skipped(String),
}

pub struct Inputs<'a> {
    pub questions: &'a [GraphQuestion],
    /// Requested runner ids (each one of [`RUNNERS`]).
    pub runners: &'a [String],
    pub transport: Arc<dyn RpcTransport>,
    /// The workspace's `knowledge_get_ontology` result.
    pub ontology_view: &'a Value,
    pub plans_file: Option<&'a PlansFile>,
    /// Fail when a question has no recorded plan, rather than calling the planner.
    pub plans_only: bool,
    /// `None`: no planner configured. Cached planner responses are still used; questions with no
    /// recorded or cached plan fail the run.
    pub planner: Option<&'a dyn ModelClient>,
    pub reader: &'a dyn ModelClient,
    pub planner_model: &'a str,
    pub reader_model: &'a str,
    pub cache: &'a LlmCache,
    /// `None` when `qmd` is not requested.
    pub qmd: Option<QmdSetup>,
    pub k: usize,
    pub concurrency: usize,
}

#[derive(Debug)]
pub enum RunError {
    /// Questions with no recorded (or, without a planner, cached) plan and no planner to ask
    /// (exit 4), before any reader call.
    Uncovered(Vec<String>),
    Failed(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Uncovered(ids) => write!(
                f,
                "{} question(s) have no recorded plan and no planner may be called (--plans-only, \
                 or no model provider): {}",
                ids.len(),
                ids.join(", ")
            ),
            RunError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl From<String> for RunError {
    fn from(s: String) -> Self {
        RunError::Failed(s)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunnerAnswer {
    pub answers: Vec<String>,
    pub score: f64,
    /// The reader answered, but not with parseable JSON (scored as no answer).
    pub reader_parse_error: bool,
}

#[derive(Debug, Clone)]
pub struct QuestionRun {
    pub id: String,
    /// The raw plan (as recorded or as the planner returned it).
    pub plan: Option<Value>,
    /// `file`, `planner` or `none` (no runner needed a plan).
    pub plan_source: &'static str,
    pub plan_error: Option<String>,
    /// The planner's route (`graph` / `passages`) for a valid plan.
    pub route: Option<String>,
    /// Whether the walk's start resolved, when a walk ran.
    pub start_resolved: Option<bool>,
    pub per_runner: BTreeMap<String, RunnerAnswer>,
}

#[derive(Debug, Clone, Default)]
pub struct RunOutput {
    pub questions: Vec<QuestionRun>,
    /// Runners that ran, in report order.
    pub runners: Vec<String>,
    /// Runners that were requested but skipped, with the reason.
    pub skipped: BTreeMap<String, String>,
    /// Plans-file ids that name no question.
    pub plan_warnings: Vec<String>,
}

fn passage_items(resp: &Value) -> Vec<PassageItem> {
    resp["passages"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(|p| PassageItem {
            note: p["source_description"]
                .as_str()
                .and_then(note_of_source_description)
                .unwrap_or("")
                .to_string(),
            valid_at: p["valid_at"].as_str().unwrap_or("").to_string(),
            text: p["content"].as_str().unwrap_or("").to_string(),
        })
        .collect()
}

async fn passages_ctx(t: &dyn RpcTransport, q: &str, k: usize) -> Result<String, String> {
    let r = t
        .call(
            "knowledge_search_passages",
            PassageMode::Default.request(q, k),
        )
        .await?;
    Ok(prompts::passages_context(&passage_items(&r)))
}

async fn belief_rerank_ctx(t: &Arc<dyn RpcTransport>, q: &str, k: usize) -> Result<String, String> {
    let staged = LcgPassagesStaged {
        t: t.clone(),
        rerank: true,
        expand: false,
        belief_aware: true,
    };
    let r = t
        .call(
            "knowledge_search_passages",
            json!({"query": q, "num_results": k, "min_score": 0.0, "group_ids": [EVAL_GROUP],
                   "mode": "vector", "belief_aware": true, "rerank": true, "expand": false}),
        )
        .await?;
    staged.check_applied(&r)?;
    Ok(prompts::passages_context(&passage_items(&r)))
}

async fn qmd_ctx(src: &QmdSource, q: &str, k: usize) -> Result<String, String> {
    let r = QmdRetriever {
        runner: src.runner.clone(),
        index_name: src.index.clone(),
        rerank: false,
        id: "qmd",
    }
    .retrieve(q, k)
    .await?;
    // E13 step 3: the reader gets the whole note for each qmd hit.
    let mut seen = BTreeSet::new();
    let items: Vec<PassageItem> = r
        .notes
        .iter()
        .filter(|n| seen.insert(n.to_string()))
        .map(|n| PassageItem {
            note: n.clone(),
            valid_at: src.dates.get(n).cloned().unwrap_or_default(),
            text: src.notes.get(n).cloned().unwrap_or_default(),
        })
        .collect();
    Ok(prompts::passages_context(&items))
}

/// The plan for every question that needs one: from the plans file, else from the planner.
async fn plan_phase(inp: &Inputs<'_>) -> Result<Vec<(Option<Value>, &'static str)>, RunError> {
    let mut out: Vec<(Option<Value>, &'static str)> = Vec::with_capacity(inp.questions.len());
    let mut uncovered = Vec::new();
    for q in inp.questions {
        match inp.plans_file.and_then(|f| f.plans.get(&q.id)) {
            Some(v) => out.push((Some(v.clone()), "file")),
            None => {
                uncovered.push(q.id.clone());
                out.push((None, "planner"));
            }
        }
    }
    if uncovered.is_empty() {
        return Ok(out);
    }
    if inp.plans_only {
        return Err(RunError::Uncovered(uncovered));
    }
    let system = prompts::planner_system(inp.ontology_view);
    let schema = prompts::planner_schema(inp.ontology_view);
    let reqs: Vec<(usize, ModelRequest)> = inp
        .questions
        .iter()
        .enumerate()
        .filter(|(i, _)| out[*i].0.is_none())
        .map(|(i, q)| {
            (
                i,
                ModelRequest {
                    model: inp.planner_model.to_string(),
                    system: Some(system.clone()),
                    user: prompts::planner_user(&q.text),
                    schema: Some(schema.clone()),
                    max_tokens: prompts::PLANNER_MAX_TOKENS,
                },
            )
        })
        .collect();
    let Some(planner) = inp.planner else {
        // No planner to call: a cached plan is still a plan (a fully cached rerun needs no
        // provider, FR-020); only the questions with no cached plan are uncovered.
        let mut missing = Vec::new();
        for (i, req) in &reqs {
            match inp.cache.lookup("planner", req) {
                Some(t) => out[*i].0 = Some(prompts::parse_json_object(&t).unwrap_or(Value::Null)),
                None => missing.push(inp.questions[*i].id.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(RunError::Uncovered(missing));
        }
        return Ok(out);
    };
    // One call per distinct prompt, as the reader phase does: two questions with the same text
    // share a cache key, and running both concurrently would pay twice and leave the cached plan
    // to whichever finished last.
    let mut unique: Vec<ModelRequest> = Vec::new();
    let mut slot_of: BTreeMap<String, usize> = BTreeMap::new();
    let mut targets: Vec<(usize, usize)> = Vec::with_capacity(reqs.len());
    for (i, req) in reqs {
        let slot = *slot_of.entry(super::cache::key(&req)).or_insert_with(|| {
            unique.push(req);
            unique.len() - 1
        });
        targets.push((i, slot));
    }
    let texts: Vec<String> = futures::stream::iter(unique.iter())
        .map(|req| inp.cache.get_or_call("planner", planner, req))
        .buffered(inp.concurrency.max(1))
        .try_collect()
        .await
        .map_err(|e| RunError::Failed(format!("planner call failed: {e}")))?;
    for (i, slot) in targets {
        // An unparseable response is a plan error (validated below), never a re-prompt.
        out[i].0 = Some(prompts::parse_json_object(&texts[slot]).unwrap_or(Value::Null));
    }
    Ok(out)
}

pub async fn run(inp: Inputs<'_>) -> Result<RunOutput, RunError> {
    let has = |r: &str| inp.runners.iter().any(|x| x == r);
    let mut out = RunOutput::default();
    let ids: BTreeSet<&str> = inp.questions.iter().map(|q| q.id.as_str()).collect();
    if let Some(f) = inp.plans_file {
        out.plan_warnings = f.unknown_ids(&ids);
    }
    let qmd_src = match &inp.qmd {
        Some(QmdSetup::Ready(s)) => Some(s),
        Some(QmdSetup::Skipped(why)) => {
            out.skipped.insert("qmd".into(), why.clone());
            None
        }
        None => None,
    };
    out.runners = RUNNERS
        .iter()
        .filter(|r| has(r) && !out.skipped.contains_key(**r))
        .map(|r| r.to_string())
        .collect();
    let needs_plans = has("walk") || has("route");

    // ── plans ──────────────────────────────────────────────────────────────────
    let raw_plans = if needs_plans {
        plan_phase(&inp).await?
    } else {
        vec![(None, "none"); inp.questions.len()]
    };
    let rels: BTreeSet<String> = prompts::ontology_menu(inp.ontology_view)
        .0
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let ents: BTreeSet<String> = inp.ontology_view["entity_types"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["name"].as_str().map(String::from))
        .collect();

    // ── contexts ───────────────────────────────────────────────────────────────
    let t = inp.transport.as_ref();
    let mut contexts: Vec<Vec<(String, String)>> = Vec::with_capacity(inp.questions.len());
    for (q, (raw, source)) in inp.questions.iter().zip(&raw_plans) {
        let mut qr = QuestionRun {
            id: q.id.clone(),
            plan: raw.clone(),
            plan_source: source,
            plan_error: None,
            route: None,
            start_resolved: None,
            per_runner: BTreeMap::new(),
        };
        let plan: Option<Plan> = if needs_plans {
            match plans::validate(raw.as_ref().unwrap_or(&Value::Null), &rels, &ents) {
                Ok(p) => {
                    qr.route = Some(p.route.clone());
                    Some(p)
                }
                Err(e) => {
                    qr.plan_error = Some(e);
                    None
                }
            }
        } else {
            None
        };
        let routed_to_passages = plan.as_ref().is_some_and(|p| p.route == "passages");
        let mut ctx: Vec<(String, String)> = Vec::new();
        let pass = if has("passages") || (has("route") && routed_to_passages) {
            Some(passages_ctx(t, &q.text, inp.k).await?)
        } else {
            None
        };
        let walk = if has("walk") || (has("route") && !routed_to_passages) {
            Some(match &plan {
                Some(p) if !p.hops.is_empty() => {
                    let r = t
                        .call("knowledge_walk", plans::walk_request(p, &q.text))
                        .await
                        .map_err(|e| format!("knowledge_walk for {}: {e}", q.id))?;
                    qr.start_resolved = Some(r["found"].as_bool().unwrap_or(true));
                    prompts::facts_context(r["facts_text"].as_str().unwrap_or(""))
                }
                _ => prompts::facts_context(""),
            })
        } else {
            None
        };
        if has("passages") {
            ctx.push(("passages".into(), pass.clone().unwrap_or_default()));
        }
        if has("passages-belief-rerank") {
            let c = belief_rerank_ctx(&inp.transport, &q.text, inp.k)
                .await
                .map_err(|e| format!("passages-belief-rerank for {}: {e}", q.id))?;
            ctx.push(("passages-belief-rerank".into(), c));
        }
        if let Some(src) = qmd_src {
            let c = qmd_ctx(src, &q.text, inp.k)
                .await
                .map_err(|e| format!("qmd for {}: {e}", q.id))?;
            ctx.push(("qmd".into(), c));
        }
        if has("walk") {
            ctx.push(("walk".into(), walk.clone().unwrap_or_default()));
        }
        if has("route") {
            let c = if routed_to_passages { pass } else { walk };
            ctx.push(("route".into(), c.unwrap_or_default()));
        }
        contexts.push(ctx);
        out.questions.push(qr);
    }

    // ── reader ─────────────────────────────────────────────────────────────────
    let mut uniq: Vec<String> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (q, ctx) in inp.questions.iter().zip(&contexts) {
        for (_, c) in ctx {
            let user = prompts::reader_user(c, &q.text);
            if !index.contains_key(&user) {
                index.insert(user.clone(), uniq.len());
                uniq.push(user);
            }
        }
    }
    let schema = prompts::read_schema();
    let reqs: Vec<ModelRequest> = uniq
        .into_iter()
        .map(|user| ModelRequest {
            model: inp.reader_model.to_string(),
            system: Some(prompts::READ_SYS.to_string()),
            user,
            schema: Some(schema.clone()),
            max_tokens: prompts::READER_MAX_TOKENS,
        })
        .collect();
    let reads: Vec<String> = futures::stream::iter(reqs.iter())
        .map(|req| inp.cache.get_or_call("reader", inp.reader, req))
        .buffered(inp.concurrency.max(1))
        .try_collect()
        .await
        .map_err(|e| RunError::Failed(format!("reader call failed: {e}")))?;

    // ── scoring ────────────────────────────────────────────────────────────────
    for ((q, ctx), qr) in inp
        .questions
        .iter()
        .zip(&contexts)
        .zip(out.questions.iter_mut())
    {
        for (runner, c) in ctx {
            let text = &reads[index[&prompts::reader_user(c, &q.text)]];
            let parsed = prompts::parse_answers(text);
            let answers = parsed.clone().unwrap_or_default();
            let score = scorer::score(&q.scoring, &q.answers, &q.optional_answers, &answers);
            qr.per_runner.insert(
                runner.clone(),
                RunnerAnswer {
                    answers,
                    score,
                    reader_parse_error: parsed.is_none(),
                },
            );
        }
    }
    Ok(out)
}
