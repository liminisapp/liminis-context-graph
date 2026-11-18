//! Retrieval-quality eval mode (`lcg-eval retrieval`, issue #754).
//!
//! Measures whether LCG's read side (and qmd as an external baseline) returns the right notes and
//! grounded evidence for a checked-in question set over a notebook corpus. Separate from the
//! extraction eval; see `docs/retrieval-quality-evaluation.md` and ADR-0754.
//!
//! Pipeline: load + validate fixture → (dry-run stops here, via the same [`resolve`]) → ingest into
//! LCG / index in qmd → warm up → score every configuration at the same k → report.

pub mod cli;
pub mod evidence_judge;
pub mod fixture;
pub mod lcg;
pub mod mapping;
pub mod metrics;
pub mod qmd;
pub mod report;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

use self::cli::{parse_args, usage, RetrievalArgs, RetrievalMode};
use self::evidence_judge::{CachedEvidenceJudge, EvidenceJudge};
use self::fixture::{Category, Fixture, Question};
use self::metrics::{score_question, summarize_by_category, QuestionScore};
use self::report::{
    percentile, ConfigOutcome, ConfigRun, Grounded, IngestCost, Report, RunMeta, Status,
};

/// Minimum questions per category before a plan warns that per-category figures are thin (SC-003).
pub const MIN_QUESTIONS_PER_CATEGORY: usize = 5;

/// What one system returned for one question.
#[derive(Debug, Clone)]
pub struct RankedResult {
    /// Note ids in rank order, duplicates allowed (metrics dedupe, keeping first occurrence).
    pub notes: Vec<String>,
    /// Returned evidence texts in rank order, for grounded judging.
    pub evidence: Vec<String>,
    /// Query-time model calls; `None` when the system does not expose them.
    pub model_calls: Option<u64>,
    /// Non-model lookup RPCs made to resolve results (counted, never hidden).
    pub lookup_calls: u64,
    /// Returned hits that carried no evidence text (qmd hits without a snippet). They stay in the
    /// ranking as empty evidence, are judged unsupported, and are counted so the report can say so.
    pub evidence_gaps: u64,
    pub latency: Duration,
}

pub trait Retriever: Send + Sync {
    fn id(&self) -> &str;
    fn description(&self) -> String;
    /// True when every query spawns a fresh process (qmd). A full warm-up pass cannot make such a
    /// retriever warm — each timed query reloads its models regardless — so it gets a single
    /// warm-up query (absorbing one-off model download / disk-cache cost) instead of a whole pass.
    fn per_process(&self) -> bool {
        false
    }
    fn retrieve<'a>(
        &'a self,
        question: &'a str,
        k: usize,
    ) -> BoxFuture<'a, Result<RankedResult, String>>;
}

/// Everything `--dry-run` prints and the real run starts from (ADR-0052: one resolution path).
#[derive(Debug, Clone)]
pub struct Plan {
    pub configs: Vec<String>,
    pub k: usize,
    pub corpus_version: String,
    pub corpus_hash: String,
    pub questions_version: String,
    pub questions_hash: String,
    pub notes: usize,
    pub category_counts: BTreeMap<Category, usize>,
    pub qmd_index: String,
    pub needs_lcg: bool,
    pub needs_qmd: bool,
    pub warnings: Vec<String>,
}

/// Default qmd index name. Includes a corpus-hash prefix because qmd keeps a collection's original
/// directory: reusing a name across a different or edited corpus would score qmd against the old
/// one while the report prints the new corpus hash.
pub fn default_qmd_index(fx: &Fixture) -> String {
    let hash: String = fx.corpus_hash.chars().take(8).collect();
    format!("lcg-eval-{}-{hash}", fx.manifest.corpus_version)
}

pub fn resolve(args: &RetrievalArgs, fx: &Fixture) -> Plan {
    let mut category_counts: BTreeMap<Category, usize> =
        Category::ALL.iter().map(|c| (*c, 0)).collect();
    for q in &fx.questions {
        *category_counts.entry(q.category).or_default() += 1;
    }
    let mut warnings = Vec::new();
    for (c, n) in &category_counts {
        if *n < MIN_QUESTIONS_PER_CATEGORY {
            warnings.push(format!(
                "category '{}' has {n} question(s), fewer than {MIN_QUESTIONS_PER_CATEGORY}: its per-category figures are thin",
                c.label()
            ));
        }
    }
    Plan {
        configs: args.configs.clone(),
        k: args.k,
        corpus_version: fx.manifest.corpus_version.clone(),
        corpus_hash: fx.corpus_hash.clone(),
        questions_version: fx.questions_version.clone(),
        questions_hash: fx.questions_hash.clone(),
        notes: fx.manifest.notes.len(),
        category_counts,
        qmd_index: args
            .qmd_index
            .clone()
            .unwrap_or_else(|| default_qmd_index(fx)),
        needs_lcg: args.configs.iter().any(|c| c.starts_with("lcg-")),
        needs_qmd: args.configs.iter().any(|c| c.starts_with("qmd")),
        warnings,
    }
}

pub fn render_plan(p: &Plan) -> String {
    let mut s = String::from("lcg-eval retrieval — resolved plan\n");
    s += &format!(
        "  corpus:     {} ({} notes, hash {})\n",
        p.corpus_version, p.notes, p.corpus_hash
    );
    s += &format!(
        "  questions:  {} (hash {})\n",
        p.questions_version, p.questions_hash
    );
    for (c, n) in &p.category_counts {
        s += &format!("    {:<11} {n}\n", c.label());
    }
    s += &format!("  k:          {} (shared by every configuration)\n", p.k);
    s += &format!("  configs:    {}\n", p.configs.join(", "));
    if p.needs_qmd {
        s += &format!("  qmd index:  {}\n", p.qmd_index);
    }
    for w in &p.warnings {
        s += &format!("  WARNING: {w}\n");
    }
    s
}

/// Scores one configuration: one unmeasured warm-up pass over every question (a single query for
/// per-process retrievers, see [`Retriever::per_process`]), then one measured pass (warm latency
/// convention). A question whose query errors is listed in `query_errors` and
/// left out of the means — never scored as zero. If *every* query fails the configuration is
/// reported as skipped.
pub async fn score_config(
    r: &dyn Retriever,
    questions: &[Question],
    k: usize,
    judge: Option<&CachedEvidenceJudge<'_>>,
    judge_skip_reason: &str,
) -> Status {
    let warm_n = if r.per_process() { 1 } else { questions.len() };
    for q in questions.iter().take(warm_n) {
        let _ = r.retrieve(&q.text, k).await;
    }
    let mut scores: Vec<QuestionScore> = Vec::new();
    let mut latencies = Vec::new();
    let mut calls: Vec<Option<u64>> = Vec::new();
    let mut lookup_calls = 0;
    let mut evidence_gaps = 0;
    let mut errors = Vec::new();
    let mut judge_failure: Option<String> = None;

    for q in questions {
        let res = match r.retrieve(&q.text, k).await {
            Ok(res) => res,
            Err(e) => {
                errors.push(format!("{}: {e}", q.id));
                continue;
            }
        };
        latencies.push(res.latency.as_secs_f64() * 1000.0);
        calls.push(res.model_calls);
        lookup_calls += res.lookup_calls;
        evidence_gaps += res.evidence_gaps;

        let mut ranks: Option<Vec<Option<usize>>> = None;
        if !q.evidence.is_empty() {
            if let (Some(j), None) = (judge, &judge_failure) {
                let texts: Vec<String> = res.evidence.iter().take(k).cloned().collect();
                let mut v = Vec::new();
                for spec in &q.evidence {
                    match j.first_supporting_rank(&spec.key_fact, &texts).await {
                        Ok(rank) => v.push(rank),
                        Err(e) => {
                            judge_failure = Some(e);
                            break;
                        }
                    }
                }
                if judge_failure.is_none() {
                    ranks = Some(v);
                }
            }
        }
        scores.push(score_question(q, &res.notes, k, ranks.as_deref()));
    }

    if scores.is_empty() {
        return Status::Skipped(format!(
            "all {} queries failed; first error: {}",
            questions.len(),
            errors.first().map(String::as_str).unwrap_or("none")
        ));
    }
    // A judge failure part-way through leaves only the earlier answer questions grounded-scored;
    // drop those so the configuration is cleanly "grounded: skipped", not a partial mean that is
    // not comparable with other configurations.
    if judge_failure.is_some() {
        for s in &mut scores {
            s.grounded_recall = None;
            s.grounded_mrr = None;
        }
    }
    let (overall, by_category) = summarize_by_category(&scores);
    let grounded = match (judge, judge_failure) {
        (None, _) => Grounded::Skipped(judge_skip_reason.to_string()),
        (Some(_), Some(e)) => Grounded::Skipped(format!("judge failed: {e}")),
        (Some(j), None) => Grounded::Judged {
            paid_calls: j.paid_calls(),
            cache_hits: j.cache_hits(),
        },
    };
    let model_calls_per_query = if calls.iter().all(Option::is_some) && !calls.is_empty() {
        Some(calls.iter().flatten().sum::<u64>() as f64 / calls.len() as f64)
    } else {
        None
    };
    Status::Ran(Box::new(ConfigRun {
        scores,
        overall,
        by_category,
        latency_p50_ms: percentile(&latencies, 50.0),
        latency_p95_ms: percentile(&latencies, 95.0),
        model_calls_per_query,
        lookup_calls,
        evidence_gaps,
        query_errors: errors,
        grounded,
    }))
}

/// A configuration slot: either something to run, or a reason it cannot be.
pub enum Slot {
    Run(Box<dyn Retriever>),
    Skipped {
        id: String,
        description: String,
        reason: String,
    },
}

/// Runs every slot in order at the same k over the same questions.
pub async fn evaluate(
    fx: &Fixture,
    k: usize,
    slots: Vec<Slot>,
    judge: Option<&CachedEvidenceJudge<'_>>,
    judge_skip_reason: &str,
) -> Vec<ConfigOutcome> {
    let mut out = Vec::new();
    for slot in slots {
        match slot {
            Slot::Skipped {
                id,
                description,
                reason,
            } => out.push(ConfigOutcome {
                id,
                description,
                status: Status::Skipped(reason),
            }),
            Slot::Run(r) => {
                let status =
                    score_config(r.as_ref(), &fx.questions, k, judge, judge_skip_reason).await;
                out.push(ConfigOutcome {
                    id: r.id().to_string(),
                    description: r.description(),
                    status,
                });
            }
        }
    }
    out
}

fn default_service_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("liminis-context-graph");
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from("liminis-context-graph")
}

fn skipped_slot(id: &str, reason: &str) -> Slot {
    let description = match id {
        "qmd" => "qmd query --no-rerank",
        "qmd-rerank" => "qmd query with reranker",
        other => other,
    };
    Slot::Skipped {
        id: id.to_string(),
        description: description.to_string(),
        reason: reason.to_string(),
    }
}

/// Entry point for `lcg-eval retrieval …`. Returns the process exit code.
pub async fn run_cli(args: &[String]) -> i32 {
    let a = match parse_args(args) {
        Ok(RetrievalMode::Help) => {
            println!("{}", usage());
            return 0;
        }
        Ok(RetrievalMode::Run(a)) => *a,
        Err(e) => {
            eprintln!("lcg-eval retrieval: {e}");
            return 2;
        }
    };
    match run(a).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lcg-eval retrieval: {e}");
            1
        }
    }
}

async fn run(a: RetrievalArgs) -> Result<(), String> {
    let fx = fixture::load(&a.corpus)?;
    let plan = resolve(&a, &fx);
    if a.dry_run {
        println!("{}", render_plan(&plan));
        return Ok(());
    }
    for w in &plan.warnings {
        eprintln!("lcg-eval retrieval: WARNING {w}");
    }

    let mut meta = RunMeta {
        corpus_version: plan.corpus_version.clone(),
        corpus_hash: plan.corpus_hash.clone(),
        questions_version: plan.questions_version.clone(),
        questions_hash: plan.questions_hash.clone(),
        k: a.k,
        qmd_version: "not run".into(),
        qmd_index: plan.qmd_index.clone(),
        qmd_models: "not run".into(),
        lcg_build: "not run".into(),
        embedder: "not run".into(),
        extractor: "not run".into(),
        judge_model: a.judge_model.clone(),
        category_counts: plan.category_counts.clone(),
    };
    let mut ingest_cost = IngestCost::default();
    let mut lcg_slots: BTreeMap<String, Slot> = BTreeMap::new();
    let mut qmd_slots: BTreeMap<String, Slot> = BTreeMap::new();

    // ── LCG ─────────────────────────────────────────────────────────────────
    // Keeps the service alive until the report is written.
    let _service_guard = if plan.needs_lcg {
        let opts = lcg::ServiceOpts {
            bin: a.service_bin.clone().unwrap_or_else(default_service_bin),
            embedder_http: a.embedder_http.clone(),
            embedder_uds: a.embedder_uds.clone(),
            extractor_http: a.extractor_http.clone(),
            extractor_uds: a.extractor_uds.clone(),
            replay_cassette: a.replay_cassette.clone(),
            record_cassette: a.record_cassette.clone(),
        };
        eprintln!(
            "lcg-eval retrieval: starting LCG service ({})",
            opts.bin.display()
        );
        let guard = lcg::spawn_service(&opts).await?;
        meta.lcg_build = guard.version.clone();
        meta.embedder = a
            .embedder_http
            .clone()
            .or(a.embedder_uds.clone())
            .or_else(|| std::env::var("LCG_EMBEDDING_URL").ok())
            .unwrap_or_default();
        meta.extractor = if let Some(p) = &a.replay_cassette {
            format!("cassette replay ({p})")
        } else if let Some(u) = a.extractor_http.clone().or(a.extractor_uds.clone()) {
            format!("OpenAI-compatible endpoint ({u})")
        } else {
            "service default (Anthropic via ANTHROPIC_API_KEY, or LCG_EXTRACTION_URL)".into()
        };
        eprintln!(
            "lcg-eval retrieval: ingesting {} notes into LCG",
            plan.notes
        );
        let transport: Arc<dyn lcg::RpcTransport> = guard.client.clone();
        let map = Arc::new(lcg::ingest(transport.as_ref(), &fx).await?);
        ingest_cost.lcg_seconds = Some(map.seconds);
        ingest_cost.lcg_chunks = map.chunks;
        ingest_cost.lcg_extraction_requests = map.extraction_requests;
        ingest_cost.lcg_rpc_calls = map.rpc_calls;
        ingest_cost.lcg_unconfirmed_entity_links = map.unconfirmed_entity_links;
        ingest_cost.lcg_supersessions = map.supersessions;
        ingest_cost.lcg_superseded_episodes = map.superseded_episodes;
        for id in a.configs.iter().filter(|c| c.starts_with("lcg-")) {
            let r: Box<dyn Retriever> = match id.as_str() {
                "lcg-passages" => Box::new(lcg::LcgPassages {
                    t: transport.clone(),
                }),
                "lcg-entities-facts" => Box::new(lcg::LcgEntitiesFacts {
                    t: transport.clone(),
                    map: map.clone(),
                    belief: false,
                }),
                _ => Box::new(lcg::LcgEntitiesFacts {
                    t: transport.clone(),
                    map: map.clone(),
                    belief: true,
                }),
            };
            lcg_slots.insert(id.clone(), Slot::Run(r));
        }
        Some(guard)
    } else {
        None
    };

    // ── qmd ─────────────────────────────────────────────────────────────────
    if plan.needs_qmd {
        let runner: Arc<dyn qmd::QmdRunner> = Arc::new(qmd::ProcessRunner {
            bin: a.qmd_bin.clone(),
        });
        eprintln!(
            "lcg-eval retrieval: indexing corpus in qmd (index '{}')",
            plan.qmd_index
        );
        match qmd::prepare(runner.as_ref(), &plan.qmd_index, &fx.notes_dir()).await {
            Ok(info) => {
                if info.reused_collection {
                    eprintln!(
                        "lcg-eval retrieval: WARNING: qmd index '{}' already existed and keeps its original directory; \
                         if --corpus changed or notes were edited, use a new --qmd-index (or `qmd collection remove`) \
                         so qmd is scored against this corpus",
                        info.index_name
                    );
                }
                meta.qmd_version = info.version.clone();
                ingest_cost.qmd_seconds = Some(info.index_seconds);
                meta.qmd_models = match runner
                    .run(&["--index".into(), plan.qmd_index.clone(), "status".into()])
                    .await
                {
                    Ok(o) if o.success => {
                        let lines: Vec<&str> = o
                            .stdout
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .take(6)
                            .collect();
                        if lines.is_empty() {
                            "status gave no output".into()
                        } else {
                            lines.join("; ")
                        }
                    }
                    _ => "not exposed (see `qmd status`)".into(),
                };
                for id in a.configs.iter().filter(|c| c.starts_with("qmd")) {
                    qmd_slots.insert(
                        id.clone(),
                        Slot::Run(Box::new(qmd::QmdRetriever {
                            runner: runner.clone(),
                            index_name: plan.qmd_index.clone(),
                            rerank: id == "qmd-rerank",
                            id: if id == "qmd-rerank" {
                                "qmd-rerank"
                            } else {
                                "qmd"
                            },
                        })),
                    );
                }
            }
            Err(qmd::Skipped(reason)) => {
                eprintln!("lcg-eval retrieval: qmd configurations skipped: {reason}");
                for id in a.configs.iter().filter(|c| c.starts_with("qmd")) {
                    qmd_slots.insert(id.clone(), skipped_slot(id, &reason));
                }
            }
        }
    }

    // ── Judge ───────────────────────────────────────────────────────────────
    let anthropic = if a.no_grounded {
        None
    } else {
        std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .map(|k| crate::judge::AnthropicJudgeClient::new(k, a.judge_model.clone()))
    };
    let skip_reason = if a.no_grounded {
        "--no-grounded"
    } else {
        "ANTHROPIC_API_KEY not set"
    };
    let cached = match &anthropic {
        Some(j) => Some(CachedEvidenceJudge::load(
            j as &dyn EvidenceJudge,
            &a.judge_cache,
        )?),
        None => None,
    };

    // Slots in the order the user asked for them.
    let mut slots = Vec::new();
    for id in &a.configs {
        if let Some(s) = lcg_slots.remove(id).or_else(|| qmd_slots.remove(id)) {
            slots.push(s);
        }
    }
    let outcomes = evaluate(&fx, a.k, slots, cached.as_ref(), skip_reason).await;
    let report = Report {
        meta,
        ingest: ingest_cost,
        configs: outcomes,
    };

    println!("{}", report.render_markdown());
    if let Some(dir) = &a.out {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create --out {}: {e}", dir.display()))?;
        std::fs::write(dir.join("report.md"), report.render_markdown())
            .map_err(|e| format!("cannot write report.md: {e}"))?;
        std::fs::write(
            dir.join("report.json"),
            serde_json::to_string_pretty(&report.to_json()).expect("report serializes"),
        )
        .map_err(|e| format!("cannot write report.json: {e}"))?;
        eprintln!(
            "lcg-eval retrieval: wrote report.md and report.json to {}",
            dir.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::evidence_judge::testing::SubstringJudge;
    use crate::retrieval::fixture::{EvidenceSpec, Relevant};

    struct Canned {
        id: &'static str,
        notes: Vec<String>,
        evidence: Vec<String>,
        fail: bool,
    }

    /// Counts calls and reports itself as process-per-query.
    struct CountingPerProcess {
        calls: std::sync::atomic::AtomicUsize,
        per_process: bool,
    }
    impl Retriever for CountingPerProcess {
        fn id(&self) -> &str {
            "counting"
        }
        fn description(&self) -> String {
            "counting".into()
        }
        fn per_process(&self) -> bool {
            self.per_process
        }
        fn retrieve<'a>(
            &'a self,
            _q: &'a str,
            _k: usize,
        ) -> BoxFuture<'a, Result<RankedResult, String>> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Box::pin(async {
                Ok(RankedResult {
                    notes: vec!["a".into()],
                    evidence: vec![],
                    model_calls: None,
                    lookup_calls: 0,
                    evidence_gaps: 0,
                    latency: Duration::from_millis(1),
                })
            })
        }
    }

    #[tokio::test]
    async fn per_process_retrievers_get_one_warm_up_query_not_a_full_pass() {
        let qs = qs();
        let n = qs.len();
        let full = CountingPerProcess {
            calls: Default::default(),
            per_process: false,
        };
        let _ = score_config(&full, &qs, 10, None, "").await;
        assert_eq!(full.calls.load(std::sync::atomic::Ordering::Relaxed), 2 * n);
        let spawn = CountingPerProcess {
            calls: Default::default(),
            per_process: true,
        };
        let _ = score_config(&spawn, &qs, 10, None, "").await;
        assert_eq!(
            spawn.calls.load(std::sync::atomic::Ordering::Relaxed),
            1 + n,
            "one warm-up query plus the measured pass"
        );
    }

    impl Retriever for Canned {
        fn id(&self) -> &str {
            self.id
        }
        fn description(&self) -> String {
            "canned".into()
        }
        fn retrieve<'a>(
            &'a self,
            _q: &'a str,
            _k: usize,
        ) -> BoxFuture<'a, Result<RankedResult, String>> {
            let r = if self.fail {
                Err("backend down".to_string())
            } else {
                Ok(RankedResult {
                    notes: self.notes.clone(),
                    evidence: self.evidence.clone(),
                    model_calls: Some(1),
                    lookup_calls: 0,
                    evidence_gaps: 0,
                    latency: Duration::from_millis(5),
                })
            };
            Box::pin(async move { r })
        }
    }

    fn qs() -> Vec<Question> {
        vec![
            Question {
                id: "d1".into(),
                category: Category::Direct,
                text: "t".into(),
                relevant: vec![Relevant {
                    note: "a".into(),
                    gain: 1,
                }],
                evidence: vec![],
                current: vec![],
                stale: vec![],
                negative: false,
            },
            Question {
                id: "a1".into(),
                category: Category::Answer,
                text: "t".into(),
                relevant: vec![Relevant {
                    note: "a".into(),
                    gain: 1,
                }],
                evidence: vec![EvidenceSpec {
                    note: "a".into(),
                    key_fact: "blue".into(),
                }],
                current: vec![],
                stale: vec![],
                negative: false,
            },
        ]
    }

    #[tokio::test]
    async fn note_hit_but_grounded_miss() {
        // Story 1, scenario 3: right note, wrong passage.
        let r = Canned {
            id: "c",
            notes: vec!["a".into()],
            evidence: vec!["the sky is red".into()],
            fail: false,
        };
        let d = tempfile::tempdir().unwrap();
        let inner = SubstringJudge::new();
        let j = CachedEvidenceJudge::load(&inner, &d.path().join("c.jsonl")).unwrap();
        let Status::Ran(run) = score_config(&r, &qs(), 10, Some(&j), "").await else {
            panic!()
        };
        assert_eq!(run.overall.recall, Some(1.0));
        assert_eq!(run.overall.grounded_recall, Some(0.0));
        assert_eq!(run.model_calls_per_query, Some(1.0));
        assert!(matches!(run.grounded, Grounded::Judged { .. }));
    }

    #[tokio::test]
    async fn grounded_hit_when_text_supports() {
        let r = Canned {
            id: "c",
            notes: vec!["a".into()],
            evidence: vec!["x".into(), "the sky is BLUE".into()],
            fail: false,
        };
        let d = tempfile::tempdir().unwrap();
        let inner = SubstringJudge::new();
        let j = CachedEvidenceJudge::load(&inner, &d.path().join("c.jsonl")).unwrap();
        let Status::Ran(run) = score_config(&r, &qs(), 10, Some(&j), "").await else {
            panic!()
        };
        assert_eq!(run.overall.grounded_recall, Some(1.0));
        assert_eq!(run.overall.grounded_mrr, Some(0.5));
    }

    #[tokio::test]
    async fn no_judge_means_grounded_skipped_not_zero() {
        let r = Canned {
            id: "c",
            notes: vec!["a".into()],
            evidence: vec![],
            fail: false,
        };
        let Status::Ran(run) = score_config(&r, &qs(), 10, None, "no key").await else {
            panic!()
        };
        assert_eq!(run.overall.grounded_recall, None);
        assert!(matches!(run.grounded, Grounded::Skipped(ref w) if w == "no key"));
    }

    #[tokio::test]
    async fn total_failure_is_skipped_never_zero() {
        let r = Canned {
            id: "c",
            notes: vec![],
            evidence: vec![],
            fail: true,
        };
        match score_config(&r, &qs(), 10, None, "").await {
            Status::Skipped(w) => {
                assert!(w.contains("all 2 queries failed") && w.contains("backend down"))
            }
            _ => panic!("expected skipped"),
        }
    }

    #[tokio::test]
    async fn scoring_is_reproducible() {
        let mk = || Canned {
            id: "c",
            notes: vec!["x".into(), "a".into()],
            evidence: vec![],
            fail: false,
        };
        let (Status::Ran(a), Status::Ran(b)) = (
            score_config(&mk(), &qs(), 10, None, "").await,
            score_config(&mk(), &qs(), 10, None, "").await,
        ) else {
            panic!()
        };
        assert_eq!(a.scores, b.scores);
        assert_eq!(a.overall, b.overall);
    }

    /// Judge that works once and then fails, like one transient API error mid-run.
    struct FlakyJudge(std::sync::atomic::AtomicUsize);
    impl EvidenceJudge for FlakyJudge {
        fn model(&self) -> &str {
            "flaky"
        }
        fn supports<'a>(&'a self, k: &'a str, e: &'a str) -> BoxFuture<'a, Result<bool, String>> {
            let n = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let r = if n >= 1 {
                Err("transient 529".to_string())
            } else {
                Ok(e.contains(k))
            };
            Box::pin(async move { r })
        }
    }

    #[tokio::test]
    async fn mid_run_judge_failure_drops_partial_grounded_scores() {
        let mut questions = qs();
        let mut second = questions[1].clone();
        second.id = "a2".into();
        second.evidence[0].key_fact = "green".into();
        questions.push(second);
        let r = Canned {
            id: "c",
            notes: vec!["a".into()],
            evidence: vec!["the sky is blue".into()],
            fail: false,
        };
        let d = tempfile::tempdir().unwrap();
        let inner = FlakyJudge(Default::default());
        let j = CachedEvidenceJudge::load(&inner, &d.path().join("c.jsonl")).unwrap();
        let Status::Ran(run) = score_config(&r, &questions, 10, Some(&j), "").await else {
            panic!()
        };
        // a1 was judged before the failure, but its grounded score must not survive alone.
        assert_eq!(run.overall.grounded_recall, None);
        assert_eq!(run.overall.n_grounded, 0);
        assert!(run
            .scores
            .iter()
            .all(|s| s.grounded_recall.is_none() && s.grounded_mrr.is_none()));
        assert!(matches!(run.grounded, Grounded::Skipped(ref w) if w.contains("judge failed")));
        // Note-level metrics are unaffected.
        assert_eq!(run.overall.recall, Some(1.0));
    }
}
