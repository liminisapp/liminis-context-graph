//! Argument parsing for `lcg-eval graph-qa …` (hand-rolled, as the other subcommands).

use std::path::PathBuf;

use lcg_core::anthropic_config::AnthropicCliArgs;

use super::prompts::DEFAULT_MODEL;
use super::runner::{DEFAULT_RUNNERS, RUNNERS};
use crate::retrieval::stats::{DEFAULT_RESAMPLES, DEFAULT_SEED};

pub const DEFAULT_CORPUS: &str = "crates/eval/fixtures/retrieval-v2";
pub const DEFAULT_LLM_CACHE: &str = "graph-qa-llm-cache.jsonl";
pub const DEFAULT_CONCURRENCY: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphQaArgs {
    pub workspace: PathBuf,
    pub corpus: PathBuf,
    /// Default `<corpus>/graph-questions.jsonl`.
    pub questions: Option<PathBuf>,
    pub runners: Vec<String>,
    pub plans: Option<PathBuf>,
    pub plans_only: bool,
    /// `--allow-reader-errors`: report a reader parse-error rate over the limit as a warning
    /// instead of failing the run (exit 1).
    pub allow_reader_errors: bool,
    pub planner_model: String,
    pub reader_model: String,
    pub llm_cache: PathBuf,
    pub anthropic: AnthropicCliArgs,
    pub service_bin: Option<PathBuf>,
    pub embedder_http: Option<String>,
    pub embedder_uds: Option<String>,
    /// `--allow-embedder-mismatch`: warn instead of refusing (exit 3) when the configured embedder
    /// address differs from the one in the workspace manifest.
    pub allow_embedder_mismatch: bool,
    pub embedding_profile: Option<String>,
    pub reranker_http: Option<String>,
    pub reranker_uds: Option<String>,
    pub qmd_bin: PathBuf,
    pub qmd_index: Option<String>,
    pub k: usize,
    pub concurrency: usize,
    pub out: Option<PathBuf>,
    pub bootstrap_seed: u64,
    pub bootstrap_resamples: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Help,
    Run(Box<GraphQaArgs>),
}

pub fn usage() -> String {
    format!(
        "lcg-eval graph-qa — graph question answering over a kept workspace (E13, #821)

USAGE:
    lcg-eval graph-qa --workspace <DIR> [OPTIONS]

Every runner's context goes to one fixed reader prompt; answers are scored per question (single
answer = first answer matches, list/join = set F1, history = exact in-order match) and reported
per shape, per family, overall (E13 and all) and per dev/test half, with paired-bootstrap
differences between runners. The workspace is copied and the service runs on the copy: the
workspace itself (DB, WAL) is never modified.

OPTIONS:
    --workspace <DIR>          An ingested workspace, e.g. from `retrieval --keep-workspace` (required).
    --corpus <DIR>             Fixture directory (default {DEFAULT_CORPUS}); qmd reads its notes.
    --questions <FILE>         Graph question set (default <corpus>/graph-questions.jsonl).
    --runners <LIST>           Comma-separated subset of: {all}.
                               Default: {default}. passages-belief-rerank needs a reranker
                               (--reranker-*); qmd is skipped with a notice when qmd cannot run.
    --plans <FILE>             Recorded plans (the E13 v5-plans.json format) used instead of planner
                               calls for the questions they cover.
    --plans-only               Never call the planner: a question with no recorded plan fails the
                               run (exit 4) before any reader call.
    --allow-reader-errors      Report a runner whose reader replies fail to parse more than 10% of
                               the time as a warning instead of failing the run (exit 1).
    --planner-model <MODEL>    Planner model (default {DEFAULT_MODEL}).
    --reader-model <MODEL>     Reader model (default {DEFAULT_MODEL}).
    --llm-cache <FILE>         Planner/reader response cache, keyed by model + full prompt
                               (default {DEFAULT_LLM_CACHE}). A fully cached run makes no model call.
    --anthropic-base-url <URL> Anthropic-compatible base URL (also ANTHROPIC_BASE_URL), e.g. the
    --anthropic-endpoint <URL> subscription bridge; full endpoint form also LCG_ANTHROPIC_ENDPOINT.
                               Credentials: ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN (optional for
                               a bridge).
    --service-bin <PATH>       liminis-context-graph binary (default: alongside lcg-eval, else PATH).
    --embedder-http <URL>      The embedder the workspace was ingested with (required unless
    --embedder-uds <SOCKET>    LCG_EMBEDDING_URL is set; checked against the workspace manifest).
    --allow-embedder-mismatch  Warn instead of refusing (exit 3) when the embedder address differs
                               from the manifest's (loopback aliases and a trailing slash already
                               compare equal). Only for the same embedding model at a new address.
    --embedding-profile <P>    LCG_EMBEDDING_PROFILE for the service (default: the ingest profile
                               recorded in the workspace manifest; a different one is refused).
    --reranker-http <URL>      Reranker for passages-belief-rerank.
    --reranker-uds <SOCKET>
    --qmd-bin <PATH>           qmd executable (default: qmd).
    --qmd-index <NAME>         qmd index (default: lcg-eval-<corpus_version>-<corpus_hash8>).
    --k <N>                    Passages per question (default 10).
    --concurrency <N>          Parallel model calls (default {DEFAULT_CONCURRENCY}).
    --out <DIR>                Write graph-qa.md and graph-qa.json here.
    --bootstrap-seed <N>       Base seed for the paired bootstrap (default {DEFAULT_SEED}).
    --bootstrap-resamples <N>  Resamples per interval (default {DEFAULT_RESAMPLES}, 100..=1000000).
    -h, --help                 Print this help.

EXIT CODES:
    0 ok · 1 run failed (service, model provider, I/O, or a reader parse-error rate over 10%)
    · 2 usage error · 3 unusable workspace
    (missing, not ingested, embedder mismatch) · 4 questions without a plan (recorded, or cached
    when no provider is set) and no planner
",
        all = RUNNERS.join(", "),
        default = DEFAULT_RUNNERS.join(","),
    )
}

pub fn parse_args(args: &[String]) -> Result<Mode, String> {
    let mut a = GraphQaArgs {
        workspace: PathBuf::new(),
        corpus: PathBuf::from(DEFAULT_CORPUS),
        questions: None,
        runners: DEFAULT_RUNNERS.iter().map(|s| s.to_string()).collect(),
        plans: None,
        plans_only: false,
        allow_reader_errors: false,
        planner_model: DEFAULT_MODEL.into(),
        reader_model: DEFAULT_MODEL.into(),
        llm_cache: PathBuf::from(DEFAULT_LLM_CACHE),
        anthropic: AnthropicCliArgs::default(),
        service_bin: None,
        embedder_http: None,
        embedder_uds: None,
        allow_embedder_mismatch: false,
        embedding_profile: None,
        reranker_http: None,
        reranker_uds: None,
        qmd_bin: PathBuf::from("qmd"),
        qmd_index: None,
        k: 10,
        concurrency: DEFAULT_CONCURRENCY,
        out: None,
        bootstrap_seed: DEFAULT_SEED,
        bootstrap_resamples: DEFAULT_RESAMPLES,
    };
    let mut workspace = None;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let mut val = |name: &str| -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        let num = |name: &str, v: String| -> Result<u64, String> {
            v.parse::<u64>()
                .map_err(|_| format!("{name} must be an unsigned integer, got '{v}'"))
        };
        match flag {
            "-h" | "--help" => return Ok(Mode::Help),
            "--plans-only" => a.plans_only = true,
            "--allow-reader-errors" => a.allow_reader_errors = true,
            "--allow-embedder-mismatch" => a.allow_embedder_mismatch = true,
            "--workspace" => workspace = Some(PathBuf::from(val(flag)?)),
            "--corpus" => a.corpus = PathBuf::from(val(flag)?),
            "--questions" => a.questions = Some(PathBuf::from(val(flag)?)),
            "--runners" => {
                let v = val(flag)?;
                let mut list: Vec<String> = Vec::new();
                for r in v.split(',').map(str::trim).filter(|r| !r.is_empty()) {
                    if !RUNNERS.contains(&r) {
                        return Err(format!(
                            "unknown runner '{r}' (expected one of: {})",
                            RUNNERS.join(", ")
                        ));
                    }
                    if !list.iter().any(|x| x == r) {
                        list.push(r.to_string());
                    }
                }
                if list.is_empty() {
                    return Err("--runners must name at least one runner".into());
                }
                a.runners = list;
            }
            "--plans" => a.plans = Some(PathBuf::from(val(flag)?)),
            "--planner-model" => a.planner_model = val(flag)?,
            "--reader-model" => a.reader_model = val(flag)?,
            "--llm-cache" => a.llm_cache = PathBuf::from(val(flag)?),
            "--anthropic-base-url" => a.anthropic.base_url = Some(val(flag)?),
            "--anthropic-endpoint" => a.anthropic.endpoint = Some(val(flag)?),
            "--service-bin" => a.service_bin = Some(PathBuf::from(val(flag)?)),
            "--embedder-http" => a.embedder_http = Some(val(flag)?),
            "--embedder-uds" => a.embedder_uds = Some(val(flag)?),
            "--embedding-profile" => {
                let p = val(flag)?;
                p.parse::<lcg_core::InstructionProfile>()
                    .map_err(|e| e.to_string())?;
                a.embedding_profile = Some(p);
            }
            "--reranker-http" => a.reranker_http = Some(val(flag)?),
            "--reranker-uds" => a.reranker_uds = Some(val(flag)?),
            "--qmd-bin" => a.qmd_bin = PathBuf::from(val(flag)?),
            "--qmd-index" => a.qmd_index = Some(val(flag)?),
            "--k" => {
                a.k = num(flag, val(flag)?)? as usize;
                if !(1..=100).contains(&a.k) {
                    return Err("--k must be between 1 and 100".into());
                }
            }
            "--concurrency" => {
                a.concurrency = num(flag, val(flag)?)? as usize;
                if !(1..=64).contains(&a.concurrency) {
                    return Err("--concurrency must be between 1 and 64".into());
                }
            }
            "--out" => a.out = Some(PathBuf::from(val(flag)?)),
            "--bootstrap-seed" => a.bootstrap_seed = num(flag, val(flag)?)?,
            "--bootstrap-resamples" => {
                a.bootstrap_resamples = num(flag, val(flag)?)? as usize;
                if !(100..=1_000_000).contains(&a.bootstrap_resamples) {
                    return Err("--bootstrap-resamples must be between 100 and 1000000".into());
                }
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
        i += 1;
    }
    a.workspace = workspace.ok_or("--workspace is required")?;
    for (x, y, n) in [
        (&a.embedder_http, &a.embedder_uds, "--embedder"),
        (&a.reranker_http, &a.reranker_uds, "--reranker"),
    ] {
        if x.is_some() && y.is_some() {
            return Err(format!("{n}-http and {n}-uds are mutually exclusive"));
        }
    }
    Ok(Mode::Run(Box::new(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Mode, String> {
        parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn defaults_and_flags() {
        let Mode::Run(a) = p(&["--workspace", "/w"]).unwrap() else {
            panic!()
        };
        assert_eq!(a.runners, vec!["passages", "walk", "route"]);
        assert_eq!(a.planner_model, DEFAULT_MODEL);
        assert_eq!((a.bootstrap_seed, a.k, a.concurrency), (788, 10, 6));
        let Mode::Run(a) = p(&[
            "--workspace",
            "/w",
            "--runners",
            "route,qmd,route",
            "--plans",
            "p.json",
            "--plans-only",
            "--reader-model",
            "m",
            "--anthropic-base-url",
            "http://b",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(a.runners, vec!["route", "qmd"]);
        assert!(a.plans_only && a.plans.is_some());
        assert_eq!(a.reader_model, "m");
        assert_eq!(a.anthropic.base_url.as_deref(), Some("http://b"));
        assert_eq!(p(&["-h"]).unwrap(), Mode::Help);
        assert!(usage().contains("--plans-only") && usage().contains("EXIT CODES"));
    }

    #[test]
    fn usage_errors() {
        assert!(p(&[]).unwrap_err().contains("--workspace"));
        assert!(p(&["--workspace", "w", "--runners", "walkk"])
            .unwrap_err()
            .contains("walkk"));
        assert!(p(&["--workspace", "w", "--k", "0"]).is_err());
        assert!(p(&["--workspace", "w", "--nope"]).is_err());
        assert!(p(&[
            "--workspace",
            "w",
            "--embedder-http",
            "a",
            "--embedder-uds",
            "b"
        ])
        .is_err());
        assert!(p(&["--workspace", "w", "--bootstrap-resamples", "5"]).is_err());
    }
}
