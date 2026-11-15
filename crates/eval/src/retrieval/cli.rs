//! Argument parsing for `lcg-eval retrieval …`. Hand-rolled to match the extraction CLI's style.

use std::path::PathBuf;

use crate::cli::DEFAULT_JUDGE_MODEL;

pub const DEFAULT_K: usize = 10;
pub const DEFAULT_FIXTURE_DIR: &str = "crates/eval/fixtures/retrieval";
pub const DEFAULT_EVIDENCE_CACHE: &str = "retrieval_evidence_cache.jsonl";

/// The four headline configurations (FR-004), then the labelled extra belief-aware variant.
pub const HEADLINE_CONFIGS: [&str; 4] = ["lcg-passages", "lcg-entities-facts", "qmd", "qmd-rerank"];
pub const ALL_CONFIGS: [&str; 5] = [
    "lcg-passages",
    "lcg-entities-facts",
    "qmd",
    "qmd-rerank",
    "lcg-entities-facts-belief",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalArgs {
    pub corpus: PathBuf,
    pub k: usize,
    pub configs: Vec<String>,
    pub service_bin: Option<PathBuf>,
    pub embedder_http: Option<String>,
    pub embedder_uds: Option<String>,
    pub extractor_http: Option<String>,
    pub extractor_uds: Option<String>,
    pub replay_cassette: Option<String>,
    pub record_cassette: Option<String>,
    pub qmd_bin: PathBuf,
    pub qmd_index: Option<String>,
    pub judge_model: String,
    pub judge_cache: PathBuf,
    pub no_grounded: bool,
    pub out: Option<PathBuf>,
    pub dry_run: bool,
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
                               qmd-rerank, lcg-entities-facts-belief. Default: all five.
    --service-bin <PATH>       liminis-context-graph binary to spawn (default: alongside lcg-eval,
                               else `liminis-context-graph` on PATH).
    --embedder-http <URL>      Real embedder for the spawned service (required for LCG configs
    --embedder-uds <SOCKET>    unless LCG_EMBEDDING_URL is set; mock/hash embedders are refused).
    --extractor-http <URL>     Extraction endpoint for the spawned service (else the service's
    --extractor-uds <SOCKET>   own default, e.g. ANTHROPIC_API_KEY).
    --replay-cassette <PATH>   Serve ingest extraction from a recorded cassette (LCG_REPLAY_LLM).
    --record-cassette <PATH>   Record ingest extraction to a cassette (LCG_RECORD_LLM).
    --qmd-bin <PATH>           qmd executable (default: qmd). Installed out of band; if it cannot
                               run, the qmd configurations are reported as skipped.
    --qmd-index <NAME>         qmd named index to create (default: lcg-eval-<corpus_version>).
    --judge-model <MODEL>      Grounded-evidence judge model (default: claude-sonnet-4-6).
    --judge-cache <PATH>       JSONL verdict cache (default: retrieval_evidence_cache.jsonl).
    --no-grounded              Skip grounded-evidence judging (reported as skipped).
    --out <DIR>                Write report.json and report.md here.
    --dry-run                  Print the resolved plan (configs, k, hashes) and exit.
    -h, --help                 Print this help.
"
}

pub fn parse_args(args: &[String]) -> Result<RetrievalMode, String> {
    let mut a = RetrievalArgs {
        corpus: PathBuf::from(DEFAULT_FIXTURE_DIR),
        k: DEFAULT_K,
        configs: ALL_CONFIGS.iter().map(|s| s.to_string()).collect(),
        service_bin: None,
        embedder_http: None,
        embedder_uds: None,
        extractor_http: None,
        extractor_uds: None,
        replay_cassette: None,
        record_cassette: None,
        qmd_bin: PathBuf::from("qmd"),
        qmd_index: None,
        judge_model: DEFAULT_JUDGE_MODEL.to_string(),
        judge_cache: PathBuf::from(DEFAULT_EVIDENCE_CACHE),
        no_grounded: false,
        out: None,
        dry_run: false,
    };
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
                    if !ALL_CONFIGS.contains(&c) {
                        return Err(format!(
                            "unknown configuration '{c}' (expected one of: {})",
                            ALL_CONFIGS.join(", ")
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
            "--extractor-http" => a.extractor_http = Some(val("--extractor-http")?),
            "--extractor-uds" => a.extractor_uds = Some(val("--extractor-uds")?),
            "--replay-cassette" => a.replay_cassette = Some(val("--replay-cassette")?),
            "--record-cassette" => a.record_cassette = Some(val("--record-cassette")?),
            "--qmd-bin" => a.qmd_bin = PathBuf::from(val("--qmd-bin")?),
            "--qmd-index" => a.qmd_index = Some(val("--qmd-index")?),
            "--judge-model" => a.judge_model = val("--judge-model")?,
            "--judge-cache" => a.judge_cache = PathBuf::from(val("--judge-cache")?),
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
    Ok(RetrievalMode::Run(Box::new(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<RetrievalMode, String> {
        parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn defaults() {
        match p(&[]).unwrap() {
            RetrievalMode::Run(a) => {
                assert_eq!(a.k, DEFAULT_K);
                assert_eq!(a.configs.len(), 5);
                assert_eq!(a.corpus, PathBuf::from(DEFAULT_FIXTURE_DIR));
                assert!(!a.dry_run);
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
    }
}
