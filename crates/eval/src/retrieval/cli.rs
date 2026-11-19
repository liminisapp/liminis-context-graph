//! Argument parsing for `lcg-eval retrieval …`. Hand-rolled to match the extraction CLI's style.

use std::path::{Path, PathBuf};

use super::stats::{DEFAULT_RESAMPLES, DEFAULT_SEED};

use crate::cli::DEFAULT_JUDGE_MODEL;

pub const DEFAULT_K: usize = 10;
pub const DEFAULT_FIXTURE_DIR: &str = "crates/eval/fixtures/retrieval";
pub const DEFAULT_EVIDENCE_CACHE: &str = "retrieval_evidence_cache.jsonl";

/// The four headline configurations (FR-004), then the labelled extras: the belief-aware variant
/// and `lcg-passages-variants` (#802, needs `query-variants.jsonl`; a skipped row without it).
pub const HEADLINE_CONFIGS: [&str; 4] = ["lcg-passages", "lcg-entities-facts", "qmd", "qmd-rerank"];
/// The configurations a run measures by default.
pub const DEFAULT_CONFIGS: [&str; 6] = [
    "lcg-passages",
    "lcg-passages-variants",
    "lcg-entities-facts",
    "qmd",
    "qmd-rerank",
    "lcg-entities-facts-belief",
];
/// Passage-search stage configurations (#796). Opt-in via `--configs`: they need a reranker /
/// expansion LLM reachable from the spawned service (normally the CoreML sidecar), which a
/// default run does not assume.
pub const STAGE_CONFIGS: [&str; 4] = [
    "lcg-passages-rerank",
    "lcg-passages-belief-rerank",
    "lcg-passages-expand",
    "lcg-passages-expand-rerank",
];
/// Every accepted configuration id.
pub const ALL_CONFIGS: [&str; 10] = [
    "lcg-passages",
    "lcg-passages-variants",
    "lcg-entities-facts",
    "qmd",
    "qmd-rerank",
    "lcg-entities-facts-belief",
    "lcg-passages-rerank",
    "lcg-passages-belief-rerank",
    "lcg-passages-expand",
    "lcg-passages-expand-rerank",
];

/// Opt-in LCG passage-search variants (#789, #806), measured on one binary by sending `mode` and
/// `belief_aware` explicitly: the raw vector baseline (`belief_aware: false`, the pre-#806
/// default), vector with the belief-aware rules, weighted-RRF hybrid alone, and weighted-RRF
/// hybrid with the belief-aware rules. Not in the default run (`lcg-passages` is the shipped
/// default).
pub const HYBRID_PASSAGE_CONFIGS: [&str; 4] = [
    "lcg-passages-raw",
    "lcg-passages-belief",
    "lcg-passages-hybrid",
    "lcg-passages-hybrid-belief",
];

/// Opt-in caller-supplied-variants configs on top of those (#802): every query runs the same
/// pipeline as the base config, so each is read against it. Needs `query-variants.jsonl`.
pub const VARIANTS_PASSAGE_CONFIGS: [&str; 2] = [
    "lcg-passages-belief-variants",
    "lcg-passages-hybrid-belief-variants",
];

/// Opt-in graph-aware configurations (issue #797): `knowledge_graph_search` per ablation stage,
/// and the fairer `lcg-entities-facts-rrf` variant. Accepted by `--configs` but not part of the
/// default run, so the default report, its row count and every existing ranking are unchanged.
pub const GRAPH_CONFIGS: [&str; 4] = [
    "lcg-graph-seeds",
    "lcg-graph-expand",
    "lcg-graph",
    "lcg-entities-facts-rrf",
];

/// Whether `--configs` accepts `c`: a default configuration or an opt-in passage, variants or graph one.
pub fn is_known_config(c: &str) -> bool {
    ALL_CONFIGS.contains(&c)
        || HYBRID_PASSAGE_CONFIGS.contains(&c)
        || VARIANTS_PASSAGE_CONFIGS.contains(&c)
        || GRAPH_CONFIGS.contains(&c)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalArgs {
    pub corpus: PathBuf,
    pub k: usize,
    pub configs: Vec<String>,
    pub service_bin: Option<PathBuf>,
    pub embedder_http: Option<String>,
    pub embedder_uds: Option<String>,
    /// Instruction profiles to measure (#790). Empty = the service's own default (no
    /// `LCG_EMBEDDING_PROFILE` is set). One entry runs every LCG configuration under that
    /// profile; several expand each LCG configuration to `<id>@<profile>` rows, each profile in a
    /// fresh workspace with a fresh ingest (a document-prefix change means a re-embed).
    pub embedding_profiles: Vec<String>,
    pub extractor_http: Option<String>,
    pub extractor_uds: Option<String>,
    /// Reranker / query-expansion endpoints for the spawned service (#796). Unset: the service
    /// defaults to the embedder's sidecar (and `LCG_EXPANSION_URL` for expansion).
    pub reranker_http: Option<String>,
    pub reranker_uds: Option<String>,
    pub expander_http: Option<String>,
    pub expander_uds: Option<String>,
    pub replay_cassette: Option<String>,
    pub record_cassette: Option<String>,
    pub qmd_bin: PathBuf,
    pub qmd_index: Option<String>,
    pub judge_model: String,
    pub judge_cache: PathBuf,
    pub no_grounded: bool,
    pub out: Option<PathBuf>,
    pub dry_run: bool,
    /// Base seed for the bootstrap confidence intervals (default 788).
    pub bootstrap_seed: u64,
    /// Per-question resamples for every interval (default 10,000).
    pub bootstrap_resamples: usize,
    /// Private-notebook run: results only, outputs must live outside any git tree.
    pub private: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RetrievalMode {
    Help,
    Run(Box<RetrievalArgs>),
}

pub fn usage() -> &'static str {
    "lcg-eval retrieval — retrieval-quality eval (LCG vs qmd) over a notebook corpus

USAGE:
    lcg-eval retrieval [OPTIONS]

OPTIONS:
    --corpus <DIR>             Fixture directory (manifest.json, notes/, questions.jsonl).
                               Default: crates/eval/fixtures/retrieval
    --k <N>                    Shared cut-off for every configuration (1..=100, default 10).
    --configs <LIST>           Comma-separated subset of: lcg-passages, lcg-entities-facts, qmd,
                               qmd-rerank, lcg-entities-facts-belief, lcg-passages-variants.
                               Default: all six. lcg-passages is the service's shipped default
                               (belief-aware vector since #806). lcg-passages-variants reads
                               <DIR>/query-variants.jsonl (see `lcg-eval retrieval-variants`) and is
                               reported as skipped when that file is absent.
                               Opt-in: lcg-passages-raw (belief_aware=false, the pre-#806
                               baseline), (#789) lcg-passages-hybrid (BM25 + vector, weighted RRF),
                               lcg-passages-hybrid-belief (the same plus belief-aware ranking),
                               lcg-passages-belief (vector plus belief-aware ranking).
                               Opt-in (#802, need query-variants.jsonl): lcg-passages-belief-variants
                               and lcg-passages-hybrid-belief-variants (the same, with the variants
                               run through that pipeline and fused once; each is read against its
                               variants-less base).
                               Opt-in passage stages (#796): lcg-passages-rerank, lcg-passages-belief-rerank (rerank over a
                               belief-aware first stage), lcg-passages-expand and
                               lcg-passages-expand-rerank (issue #796; they need a reranker /
                               expansion LLM reachable from the service, normally the CoreML
                               sidecar, and are reported as failed queries if a stage cannot
                               apply — never scored as first-stage results).
    --reranker-http <URL>      Reranker endpoint for the spawned service (default: the embedder's
    --reranker-uds <SOCKET>    sidecar when that is a Unix socket or a loopback URL; a remote
                               --embedder-http is never reused, so name one). Used by the lcg-passages-*rerank configurations.
    --expander-http <URL>      Query-expansion LLM endpoint for the spawned service (default:
    --expander-uds <SOCKET>    LCG_EXPANSION_URL, else the embedder's sidecar under the same loopback-only rule). Used by the
                               lcg-passages-expand* configurations.
                               Opt-in graph-aware configurations (#797): lcg-graph,
                               lcg-graph-seeds, lcg-graph-expand (ablation stages) and
                               lcg-entities-facts-rrf (fairer entities+facts merge).
    --service-bin <PATH>       liminis-context-graph binary to spawn (default: alongside lcg-eval,
                               else `liminis-context-graph` on PATH).
    --embedder-http <URL>      Real embedder for the spawned service (required for LCG configs
    --embedder-uds <SOCKET>    unless LCG_EMBEDDING_URL is set; mock/hash embedders are refused).
    --embedding-profile <LIST> Comma-separated embedding instruction profiles to measure (none,
                               bge-v1.5, nomic-v1.5, e5, embedder), passed to the spawned
                               service as LCG_EMBEDDING_PROFILE. With several, each LCG
                               configuration is reported once per profile as <id>@<profile>,
                               each in a fresh workspace (re-ingest). Default: service default.
    --extractor-http <URL>     Extraction endpoint for the spawned service (else the service's
    --extractor-uds <SOCKET>   own default, e.g. ANTHROPIC_API_KEY).
    --replay-cassette <PATH>   Serve ingest extraction from a recorded cassette (LCG_REPLAY_LLM).
    --record-cassette <PATH>   Record ingest extraction to a cassette (LCG_RECORD_LLM).
    --qmd-bin <PATH>           qmd executable (default: qmd). Installed out of band; if it cannot
                               run, the qmd configurations are reported as skipped.
    --qmd-index <NAME>         qmd named index to create (default: lcg-eval-<corpus_version>-<corpus_hash8>).
    --judge-model <MODEL>      Grounded-evidence judge model (default: claude-sonnet-4-6).
    --judge-cache <PATH>       JSONL verdict cache (default: retrieval_evidence_cache.jsonl).
    --no-grounded              Skip grounded-evidence judging (reported as skipped).
    --out <DIR>                Write report.json and report.md here.
    --bootstrap-seed <N>       Base seed for the 95% bootstrap intervals (default 788).
    --bootstrap-resamples <N>  Per-question resamples per interval (default 10000, 100..=1000000).
    --private                  Private-notebook run: the report carries metrics, counts and hashes
                               only (failed-query detail withheld). Requires --out, and
                               --judge-cache unless --no-grounded, and refuses any output path
                               inside a git working tree so nothing private can be committed.
    --dry-run                  Print the resolved plan (configs, k, hashes, corpus shape) and exit.
    -h, --help                 Print this help.
"
}

pub fn parse_args(args: &[String]) -> Result<RetrievalMode, String> {
    let mut a = RetrievalArgs {
        corpus: PathBuf::from(DEFAULT_FIXTURE_DIR),
        k: DEFAULT_K,
        configs: DEFAULT_CONFIGS.iter().map(|s| s.to_string()).collect(),
        service_bin: None,
        embedder_http: None,
        embedder_uds: None,
        embedding_profiles: Vec::new(),
        extractor_http: None,
        extractor_uds: None,
        reranker_http: None,
        reranker_uds: None,
        expander_http: None,
        expander_uds: None,
        replay_cassette: None,
        record_cassette: None,
        qmd_bin: PathBuf::from("qmd"),
        qmd_index: None,
        judge_model: DEFAULT_JUDGE_MODEL.to_string(),
        judge_cache: PathBuf::from(DEFAULT_EVIDENCE_CACHE),
        no_grounded: false,
        out: None,
        dry_run: false,
        bootstrap_seed: DEFAULT_SEED,
        bootstrap_resamples: DEFAULT_RESAMPLES,
        private: false,
    };
    let mut judge_cache_explicit = false;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let mut val = |name: &str| -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match flag {
            "-h" | "--help" => return Ok(RetrievalMode::Help),
            "--dry-run" => a.dry_run = true,
            "--no-grounded" => a.no_grounded = true,
            "--private" => a.private = true,
            "--bootstrap-seed" => {
                let v = val("--bootstrap-seed")?;
                a.bootstrap_seed = v.parse().map_err(|_| {
                    format!("--bootstrap-seed must be an unsigned integer, got '{v}'")
                })?;
            }
            "--bootstrap-resamples" => {
                let v = val("--bootstrap-resamples")?;
                a.bootstrap_resamples = v
                    .parse()
                    .map_err(|_| format!("--bootstrap-resamples must be an integer, got '{v}'"))?;
                if !(100..=1_000_000).contains(&a.bootstrap_resamples) {
                    return Err("--bootstrap-resamples must be between 100 and 1000000".into());
                }
            }
            "--corpus" => a.corpus = PathBuf::from(val("--corpus")?),
            "--k" => {
                let v = val("--k")?;
                a.k = v
                    .parse()
                    .map_err(|_| format!("--k must be an integer, got '{v}'"))?;
                if !(1..=100).contains(&a.k) {
                    return Err(
                        "--k must be between 1 and 100 (search_passages clamps to 100)".into(),
                    );
                }
            }
            "--configs" => {
                let v = val("--configs")?;
                let mut list = Vec::new();
                for c in v.split(',').map(str::trim).filter(|c| !c.is_empty()) {
                    if !is_known_config(c) {
                        return Err(format!(
                            "unknown configuration '{c}' (expected one of: {}, {}, {}, {})",
                            ALL_CONFIGS.join(", "),
                            HYBRID_PASSAGE_CONFIGS.join(", "),
                            VARIANTS_PASSAGE_CONFIGS.join(", "),
                            GRAPH_CONFIGS.join(", ")
                        ));
                    }
                    if !list.iter().any(|x| x == c) {
                        list.push(c.to_string());
                    }
                }
                if list.is_empty() {
                    return Err("--configs must name at least one configuration".into());
                }
                a.configs = list;
            }
            "--service-bin" => a.service_bin = Some(PathBuf::from(val("--service-bin")?)),
            "--embedder-http" => a.embedder_http = Some(val("--embedder-http")?),
            "--embedder-uds" => a.embedder_uds = Some(val("--embedder-uds")?),
            "--embedding-profile" => {
                let v = val("--embedding-profile")?;
                let mut list = Vec::new();
                for p in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    // Same parser (and same valid-names error) the service uses at startup.
                    p.parse::<lcg_core::InstructionProfile>()
                        .map_err(|e| e.to_string())?;
                    if !list.iter().any(|x| x == p) {
                        list.push(p.to_string());
                    }
                }
                if list.is_empty() {
                    return Err("--embedding-profile must name at least one profile".into());
                }
                a.embedding_profiles = list;
            }
            "--extractor-http" => a.extractor_http = Some(val("--extractor-http")?),
            "--extractor-uds" => a.extractor_uds = Some(val("--extractor-uds")?),
            "--reranker-http" => a.reranker_http = Some(val("--reranker-http")?),
            "--reranker-uds" => a.reranker_uds = Some(val("--reranker-uds")?),
            "--expander-http" => a.expander_http = Some(val("--expander-http")?),
            "--expander-uds" => a.expander_uds = Some(val("--expander-uds")?),
            "--replay-cassette" => a.replay_cassette = Some(val("--replay-cassette")?),
            "--record-cassette" => a.record_cassette = Some(val("--record-cassette")?),
            "--qmd-bin" => a.qmd_bin = PathBuf::from(val("--qmd-bin")?),
            "--qmd-index" => a.qmd_index = Some(val("--qmd-index")?),
            "--judge-model" => a.judge_model = val("--judge-model")?,
            "--judge-cache" => {
                a.judge_cache = PathBuf::from(val("--judge-cache")?);
                judge_cache_explicit = true;
            }
            "--out" => a.out = Some(PathBuf::from(val("--out")?)),
            other => return Err(format!("unknown argument '{other}'")),
        }
        i += 1;
    }
    if a.replay_cassette.is_some() && a.record_cassette.is_some() {
        return Err("--replay-cassette and --record-cassette are mutually exclusive".into());
    }
    if a.embedder_http.is_some() && a.embedder_uds.is_some() {
        return Err("--embedder-http and --embedder-uds are mutually exclusive".into());
    }
    if a.extractor_http.is_some() && a.extractor_uds.is_some() {
        return Err("--extractor-http and --extractor-uds are mutually exclusive".into());
    }
    if a.reranker_http.is_some() && a.reranker_uds.is_some() {
        return Err("--reranker-http and --reranker-uds are mutually exclusive".into());
    }
    if a.expander_http.is_some() && a.expander_uds.is_some() {
        return Err("--expander-http and --expander-uds are mutually exclusive".into());
    }
    if a.private {
        if a.out.is_none() {
            return Err("--private requires an explicit --out (outside any git tree)".into());
        }
        if !a.no_grounded && !judge_cache_explicit {
            return Err("--private requires an explicit --judge-cache (it stores key facts and returned note text) unless --no-grounded".into());
        }
    }
    Ok(RetrievalMode::Run(Box::new(a)))
}

/// True when `path` or any of its ancestors contains `.git` (a working tree or worktree). Works
/// for paths that do not exist yet by starting from the nearest existing ancestor.
pub fn inside_git_tree(path: &Path) -> bool {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut cur: Option<&Path> = Some(abs.as_path());
    let mut resolved: Option<PathBuf> = None;
    while let Some(p) = cur {
        if let Ok(c) = p.canonicalize() {
            resolved = Some(c);
            break;
        }
        cur = p.parent();
    }
    let Some(start) = resolved else { return false };
    start.ancestors().any(|d| d.join(".git").exists())
}

/// `--private` safety guard: outputs (`--out`, `--record-cassette`, and `--judge-cache` unless `--no-grounded`) must not
/// resolve inside a git working tree, so private note text cannot end up in a commit.
pub fn private_guard(a: &RetrievalArgs) -> Result<(), String> {
    let mut paths: Vec<(&str, &Path)> = Vec::new();
    if let Some(o) = &a.out {
        paths.push(("--out", o));
    }
    if !a.no_grounded {
        paths.push(("--judge-cache", &a.judge_cache));
    }
    // A recorded cassette holds the extraction exchanges for the private chunks.
    let record = a.record_cassette.as_deref().map(Path::new);
    if let Some(r) = record {
        paths.push(("--record-cassette", r));
    }
    for (flag, p) in paths {
        if inside_git_tree(p) {
            return Err(format!(
                "--private refuses {flag} {}: it is inside a git working tree, where private text could be committed. Use a path outside the repository (e.g. under ~/.cache or a temp directory)",
                p.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<RetrievalMode, String> {
        parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn graph_configs_are_opt_in_and_accepted() {
        match p(&[
            "--configs",
            "lcg-passages,lcg-graph,lcg-graph-seeds,lcg-entities-facts-rrf",
        ])
        .unwrap()
        {
            RetrievalMode::Run(a) => assert_eq!(
                a.configs,
                vec![
                    "lcg-passages",
                    "lcg-graph",
                    "lcg-graph-seeds",
                    "lcg-entities-facts-rrf"
                ]
            ),
            _ => panic!(),
        }
        for c in GRAPH_CONFIGS {
            assert!(
                !ALL_CONFIGS.contains(&c),
                "{c} must not join the default run"
            );
        }
        let e = p(&["--configs", "lcg-grahp"]).unwrap_err();
        assert!(e.contains("lcg-graph-expand"), "{e}");
    }

    #[test]
    fn defaults() {
        match p(&[]).unwrap() {
            RetrievalMode::Run(a) => {
                assert_eq!(a.k, DEFAULT_K);
                assert_eq!(a.configs.len(), 6);
                assert!(a.configs.iter().any(|c| c == "lcg-passages-variants"));
                assert_eq!(a.corpus, PathBuf::from(DEFAULT_FIXTURE_DIR));
                assert!(!a.dry_run);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn hybrid_passage_configs_are_opt_in() {
        match p(&[]).unwrap() {
            RetrievalMode::Run(a) => {
                assert!(a.configs.iter().all(|c| !c.contains("hybrid")));
            }
            _ => panic!(),
        }
        match p(&[
            "--configs",
            "lcg-passages,lcg-passages-hybrid,lcg-passages-hybrid-belief",
        ])
        .unwrap()
        {
            RetrievalMode::Run(a) => assert_eq!(a.configs.len(), 3),
            _ => panic!(),
        }
    }

    #[test]
    fn raw_passage_config_is_opt_in_and_parses() {
        match p(&[]).unwrap() {
            RetrievalMode::Run(a) => assert!(!a.configs.iter().any(|c| c == "lcg-passages-raw")),
            _ => panic!(),
        }
        match p(&["--configs", "lcg-passages,lcg-passages-raw"]).unwrap() {
            RetrievalMode::Run(a) => {
                assert_eq!(a.configs, vec!["lcg-passages", "lcg-passages-raw"])
            }
            _ => panic!(),
        }
    }

    #[test]
    fn variants_config_is_selectable_and_listed_in_usage() {
        match p(&["--configs", "lcg-passages,lcg-passages-variants"]).unwrap() {
            RetrievalMode::Run(a) => {
                assert_eq!(a.configs, vec!["lcg-passages", "lcg-passages-variants"])
            }
            _ => panic!(),
        }
        assert!(usage().contains("lcg-passages-variants"));
        assert!(p(&["--configs", "lcg-passages-variant"]).is_err());
        match p(&[
            "--configs",
            "lcg-passages-hybrid-belief,lcg-passages-hybrid-belief-variants,lcg-passages-belief-variants",
        ])
        .unwrap()
        {
            RetrievalMode::Run(a) => assert_eq!(a.configs.len(), 3),
            _ => panic!(),
        }
        // Opt-in: not in the default run.
        match p(&[]).unwrap() {
            RetrievalMode::Run(a) => {
                assert!(!a.configs.iter().any(|c| c.contains("belief-variants")))
            }
            _ => panic!(),
        }
    }

    #[test]
    fn flags_parse() {
        match p(&[
            "--k",
            "5",
            "--configs",
            "qmd,lcg-passages,qmd",
            "--dry-run",
            "--no-grounded",
            "--embedder-http",
            "http://e",
        ])
        .unwrap()
        {
            RetrievalMode::Run(a) => {
                assert_eq!(a.k, 5);
                assert_eq!(a.configs, vec!["qmd", "lcg-passages"]);
                assert!(a.dry_run && a.no_grounded);
                assert_eq!(a.embedder_http.as_deref(), Some("http://e"));
            }
            _ => panic!(),
        }
        assert_eq!(p(&["--help"]).unwrap(), RetrievalMode::Help);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(p(&["--k", "0"]).is_err());
        assert!(p(&["--k", "101"]).is_err());
        assert!(p(&["--k", "x"]).is_err());
        assert!(p(&["--configs", "bogus"]).unwrap_err().contains("bogus"));
        assert!(p(&["--nope"]).unwrap_err().contains("--nope"));
        assert!(p(&["--corpus"]).unwrap_err().contains("requires a value"));
        assert!(p(&["--replay-cassette", "a", "--record-cassette", "b"]).is_err());
        assert!(p(&["--embedder-http", "a", "--embedder-uds", "b"]).is_err());
        let e = p(&["--embedding-profile", "bogus"]).unwrap_err();
        assert!(e.contains("bogus") && e.contains("bge-v1.5"), "{e}");
        assert!(p(&["--embedding-profile", ","]).is_err());
        assert!(p(&["--bootstrap-resamples", "5"]).is_err());
        assert!(p(&["--bootstrap-seed", "-1"]).is_err());
    }

    #[test]
    fn embedding_profile_parses_a_deduplicated_list() {
        match p(&["--embedding-profile", "none, bge-v1.5,none"]).unwrap() {
            RetrievalMode::Run(a) => assert_eq!(a.embedding_profiles, vec!["none", "bge-v1.5"]),
            _ => panic!(),
        }
        match p(&[]).unwrap() {
            RetrievalMode::Run(a) => assert!(a.embedding_profiles.is_empty()),
            _ => panic!(),
        }
    }

    #[test]
    fn bootstrap_flags_parse_and_default() {
        let RetrievalMode::Run(a) = p(&[]).unwrap() else {
            panic!()
        };
        assert_eq!((a.bootstrap_seed, a.bootstrap_resamples), (788, 10_000));
        let RetrievalMode::Run(a) =
            p(&["--bootstrap-seed", "5", "--bootstrap-resamples", "500"]).unwrap()
        else {
            panic!()
        };
        assert_eq!((a.bootstrap_seed, a.bootstrap_resamples), (5, 500));
    }

    #[test]
    fn private_requires_explicit_outputs() {
        let e = p(&["--private"]).unwrap_err();
        assert!(e.contains("--out"), "{e}");
        let e = p(&["--private", "--out", "/x"]).unwrap_err();
        assert!(e.contains("--judge-cache"), "{e}");
        assert!(p(&["--private", "--out", "/x", "--judge-cache", "/y"]).is_ok());
        assert!(p(&["--private", "--out", "/x", "--no-grounded"]).is_ok());
    }

    #[test]
    fn private_guard_refuses_a_cassette_recorded_inside_a_git_tree() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::create_dir(tree.path().join(".git")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let args = |cassette: &Path| {
            let RetrievalMode::Run(a) = p(&[
                "--private",
                "--out",
                outside.path().join("out").to_str().unwrap(),
                "--no-grounded",
                "--record-cassette",
                cassette.to_str().unwrap(),
            ])
            .unwrap() else {
                panic!()
            };
            *a
        };
        let e = private_guard(&args(&tree.path().join("cassette.jsonl"))).unwrap_err();
        assert!(
            e.contains("--record-cassette") && e.contains("git working tree"),
            "{e}"
        );
        assert!(private_guard(&args(&outside.path().join("cassette.jsonl"))).is_ok());
    }

    #[test]
    fn private_guard_refuses_outputs_inside_a_git_tree() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::create_dir(tree.path().join(".git")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mk = |out: &Path, cache: &Path, no_grounded: bool| {
            let RetrievalMode::Run(a) = p(&[
                "--private",
                "--out",
                out.to_str().unwrap(),
                "--judge-cache",
                cache.to_str().unwrap(),
                if no_grounded {
                    "--no-grounded"
                } else {
                    "--dry-run"
                },
            ])
            .unwrap() else {
                panic!()
            };
            *a
        };
        // A not-yet-existing output below a git tree is still inside it.
        let in_tree = tree.path().join("reports/run1");
        let e = private_guard(&mk(&in_tree, &outside.path().join("c.jsonl"), false)).unwrap_err();
        assert!(e.contains("--out") && e.contains("git working tree"), "{e}");
        let e = private_guard(&mk(
            &outside.path().join("out"),
            &tree.path().join("cache.jsonl"),
            false,
        ))
        .unwrap_err();
        assert!(e.contains("--judge-cache"), "{e}");
        // The judge cache is irrelevant (and not checked) under --no-grounded.
        assert!(private_guard(&mk(
            &outside.path().join("out"),
            &tree.path().join("cache.jsonl"),
            true
        ))
        .is_ok());
        assert!(private_guard(&mk(
            &outside.path().join("out"),
            &outside.path().join("c.jsonl"),
            false
        ))
        .is_ok());
    }

    #[test]
    fn stage_configs_are_accepted_but_not_default() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match parse_args(&a(&[])).unwrap() {
            RetrievalMode::Run(r) => {
                assert_eq!(r.configs.len(), DEFAULT_CONFIGS.len());
                assert!(r
                    .configs
                    .iter()
                    .all(|c| !STAGE_CONFIGS.contains(&c.as_str())));
            }
            m => panic!("{m:?}"),
        }
        match parse_args(&a(&[
            "--configs",
            "lcg-passages,lcg-passages-rerank,lcg-passages-expand,lcg-passages-expand-rerank",
            "--reranker-uds",
            "/r",
            "--expander-http",
            "http://e",
        ]))
        .unwrap()
        {
            RetrievalMode::Run(r) => {
                assert_eq!(r.configs.len(), 4);
                assert_eq!(r.reranker_uds.as_deref(), Some("/r"));
                assert_eq!(r.expander_http.as_deref(), Some("http://e"));
            }
            m => panic!("{m:?}"),
        }
        assert!(parse_args(&a(&["--reranker-uds", "/r", "--reranker-http", "http://r"])).is_err());
        assert!(parse_args(&a(&["--expander-uds", "/r", "--expander-http", "http://r"])).is_err());
    }
}
