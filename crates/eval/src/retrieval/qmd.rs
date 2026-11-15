//! qmd (tobi/qmd) baseline adapter (FR-004, FR-006, FR-007, FR-016).
//!
//! qmd is installed and indexed **out of band** and driven only through its CLI as a subprocess;
//! nothing here links against it. Commands used (qmd README, verified 2026-11-15):
//!
//! ```text
//! qmd --index NAME collection add DIR --name NAME
//! qmd --index NAME embed
//! qmd --index NAME query --json -n K [--no-rerank] "text"
//! ```
//!
//! `--no-rerank` disables the LLM reranker; qmd documents no switch for query expansion, so both
//! qmd configurations include expansion. qmd does not expose model-call counts, so they are
//! reported as "n/a". A missing binary, failed indexing or unparsable output yields
//! [`Skipped`] — never a zero score.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use futures::future::BoxFuture;
use serde_json::Value;

use super::{RankedResult, Retriever};

#[derive(Debug, Clone)]
pub struct QmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Why a qmd configuration was not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped(pub String);

pub trait QmdRunner: Send + Sync {
    /// `Err(Skipped)` only when the binary cannot be started at all.
    fn run<'a>(&'a self, args: &'a [String]) -> BoxFuture<'a, Result<QmdOutput, Skipped>>;
}

/// Spawns the real `qmd` binary.
pub struct ProcessRunner {
    pub bin: PathBuf,
}

impl QmdRunner for ProcessRunner {
    fn run<'a>(&'a self, args: &'a [String]) -> BoxFuture<'a, Result<QmdOutput, Skipped>> {
        Box::pin(async move {
            let out = tokio::process::Command::new(&self.bin)
                .args(args)
                .stdin(std::process::Stdio::null())
                .output()
                .await
                .map_err(|e| {
                    Skipped(format!(
                        "qmd binary '{}' could not be started: {e} (install it out of band, see docs/retrieval-quality-evaluation.md)",
                        self.bin.display()
                    ))
                })?;
            Ok(QmdOutput {
                success: out.status.success(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        })
    }
}

#[derive(Debug, Clone)]
pub struct QmdInfo {
    pub version: String,
    pub index_name: String,
    pub index_seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QmdHit {
    pub file: String,
    pub score: Option<f64>,
    pub snippet: String,
}

/// Normalises a qmd result path (`qmd://collection/dir/Some Note.md`) to a note slug.
pub fn note_of_path(path: &str) -> Option<String> {
    let last = path.rsplit('/').next()?;
    let stem = last.strip_suffix(".md").unwrap_or(last);
    let mut slug = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    (!slug.is_empty()).then_some(slug)
}

/// Parses `qmd query --json` stdout. Tolerates progress noise before/after the JSON and either a
/// bare array or an object holding the array under `results`.
pub fn parse_results(stdout: &str) -> Result<Vec<QmdHit>, String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        // A successful `query --json` prints at least `[]`; nothing at all means the JSON went
        // elsewhere or qmd misbehaved. Erroring (not `Ok(vec![])`) keeps that from being scored
        // as an all-miss run (FR-007: skipped, never zero).
        return Err("qmd produced no output (expected a JSON array)".into());
    }
    let v: Value = serde_json::from_str(trimmed)
        .or_else(|_| {
            let s = trimmed
                .find('[')
                .ok_or_else(|| "no JSON array in qmd output".to_string())?;
            let e = trimmed
                .rfind(']')
                .ok_or_else(|| "no JSON array in qmd output".to_string())?;
            if s >= e {
                return Err("no JSON array in qmd output".to_string());
            }
            serde_json::from_str::<Value>(&trimmed[s..=e]).map_err(|e| e.to_string())
        })
        .map_err(|e: String| format!("qmd output is not parseable JSON: {e}"))?;
    let arr = match &v {
        Value::Array(a) => a,
        Value::Object(_) => v["results"]
            .as_array()
            .ok_or("qmd JSON object has no \"results\" array")?,
        _ => return Err("qmd JSON is neither an array nor an object".into()),
    };
    let mut out = Vec::new();
    for item in arr {
        let file = item["file"]
            .as_str()
            .or_else(|| item["path"].as_str())
            .ok_or("qmd result has no \"file\"")?
            .to_string();
        let snippet = ["snippet", "text", "content", "body"]
            .iter()
            .find_map(|k| item[*k].as_str())
            .unwrap_or("")
            .to_string();
        out.push(QmdHit {
            file,
            score: item["score"].as_f64(),
            snippet,
        });
    }
    Ok(out)
}

fn q(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// Detects qmd, indexes `notes_dir` under a named index, and returns run metadata.
/// Any failure is a [`Skipped`] with the reason.
pub async fn prepare(
    runner: &dyn QmdRunner,
    index_name: &str,
    notes_dir: &Path,
) -> Result<QmdInfo, Skipped> {
    let ver = runner.run(&q(&["--version"])).await?;
    let version = if ver.success {
        ver.stdout
            .trim()
            .lines()
            .next()
            .unwrap_or("unknown")
            .to_string()
    } else {
        "unknown".to_string()
    };

    let started = Instant::now();
    let dir = notes_dir.to_string_lossy().into_owned();
    let add = runner
        .run(&q(&[
            "--index",
            index_name,
            "collection",
            "add",
            &dir,
            "--name",
            index_name,
        ]))
        .await?;
    if !add.success && !add.stderr.to_lowercase().contains("exist") {
        return Err(Skipped(format!(
            "qmd collection add failed: {}",
            first_line(&add.stderr, &add.stdout)
        )));
    }
    let upd = runner.run(&q(&["--index", index_name, "update"])).await?;
    if !upd.success {
        return Err(Skipped(format!(
            "qmd update failed: {}",
            first_line(&upd.stderr, &upd.stdout)
        )));
    }
    let emb = runner.run(&q(&["--index", index_name, "embed"])).await?;
    if !emb.success {
        return Err(Skipped(format!(
            "qmd embed failed (model download unavailable?): {}",
            first_line(&emb.stderr, &emb.stdout)
        )));
    }
    Ok(QmdInfo {
        version,
        index_name: index_name.to_string(),
        index_seconds: started.elapsed().as_secs_f64(),
    })
}

fn first_line(a: &str, b: &str) -> String {
    let s = if a.trim().is_empty() { b } else { a };
    s.trim().lines().next().unwrap_or("no output").to_string()
}

pub struct QmdRetriever {
    pub runner: Arc<dyn QmdRunner>,
    pub index_name: String,
    pub rerank: bool,
    pub id: &'static str,
    /// note id -> body; used as evidence when qmd returns no snippet.
    pub notes: Arc<BTreeMap<String, String>>,
}

impl Retriever for QmdRetriever {
    fn id(&self) -> &str {
        self.id
    }

    fn description(&self) -> String {
        format!(
            "qmd query {} (query expansion on; per-process spawn, latency includes model load)",
            if self.rerank {
                "with reranker"
            } else {
                "--no-rerank"
            }
        )
    }

    fn retrieve<'a>(
        &'a self,
        question: &'a str,
        k: usize,
    ) -> BoxFuture<'a, Result<RankedResult, String>> {
        Box::pin(async move {
            let mut args = q(&["--index", &self.index_name, "query", "--json", "-n"]);
            args.push(k.to_string());
            if !self.rerank {
                args.push("--no-rerank".into());
            }
            args.push(question.to_string());
            let t = Instant::now();
            let out = self.runner.run(&args).await.map_err(|s| s.0)?;
            let latency = t.elapsed();
            if !out.success {
                return Err(format!(
                    "qmd query failed: {}",
                    first_line(&out.stderr, &out.stdout)
                ));
            }
            let hits = parse_results(&out.stdout)?;
            let mut notes = Vec::new();
            let mut evidence = Vec::new();
            for h in hits.into_iter().take(k) {
                let note = note_of_path(&h.file);
                evidence.push(if h.snippet.trim().is_empty() {
                    note.as_ref()
                        .and_then(|n| self.notes.get(n))
                        .cloned()
                        .unwrap_or_default()
                } else {
                    h.snippet
                });
                if let Some(n) = note {
                    notes.push(n);
                }
            }
            Ok(RankedResult {
                notes,
                evidence,
                model_calls: None,
                lookup_calls: 0,
                latency,
            })
        })
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::sync::Mutex;

    /// Scripted runner: `handler(args)` returns the output.
    type Handler = Box<dyn Fn(&[String]) -> Result<QmdOutput, Skipped> + Send + Sync>;

    pub struct FakeRunner {
        pub handler: Handler,
        pub log: Mutex<Vec<Vec<String>>>,
    }
    impl FakeRunner {
        pub fn new(
            f: impl Fn(&[String]) -> Result<QmdOutput, Skipped> + Send + Sync + 'static,
        ) -> Self {
            Self {
                handler: Box::new(f),
                log: Mutex::new(Vec::new()),
            }
        }
    }
    impl QmdRunner for FakeRunner {
        fn run<'a>(&'a self, args: &'a [String]) -> BoxFuture<'a, Result<QmdOutput, Skipped>> {
            self.log.lock().unwrap().push(args.to_vec());
            let r = (self.handler)(args);
            Box::pin(async move { r })
        }
    }
    pub fn ok(stdout: &str) -> Result<QmdOutput, Skipped> {
        Ok(QmdOutput {
            success: true,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    const SAMPLE: &str = r##"[
  {"docid":"#a1b2c3","score":0.91,"file":"qmd://nb/pricing-decision.md","title":"Pricing","snippet":"We chose usage-based pricing."},
  {"docid":"#d4e5f6","score":0.40,"file":"qmd://nb/Old Pricing Note.md"}
]"##;

    #[test]
    fn parses_array_with_and_without_snippet() {
        let h = parse_results(SAMPLE).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].score, Some(0.91));
        assert_eq!(h[0].snippet, "We chose usage-based pricing.");
        assert_eq!(h[1].snippet, "");
    }

    #[test]
    fn parses_object_form_and_noise() {
        let h = parse_results("loading...\n[{\"file\":\"a.md\"}]\n").unwrap();
        assert_eq!(h.len(), 1);
        let h = parse_results("{\"results\":[{\"file\":\"a.md\"}]}").unwrap();
        assert_eq!(h.len(), 1);
        // A valid empty array is a real "no hits"; blank output is an error, not an all-miss.
        assert!(parse_results("[]").unwrap().is_empty());
        assert!(parse_results("")
            .unwrap_err()
            .contains("produced no output"));
        assert!(parse_results("  \n").is_err());
        assert!(parse_results("garbage").is_err());
    }

    #[test]
    fn path_normalisation() {
        assert_eq!(
            note_of_path("qmd://nb/pricing-decision.md").as_deref(),
            Some("pricing-decision")
        );
        assert_eq!(
            note_of_path("qmd://nb/Old Pricing Note.md").as_deref(),
            Some("old-pricing-note")
        );
        assert_eq!(
            note_of_path("docs/Guide_v2.md").as_deref(),
            Some("guide-v2")
        );
        assert_eq!(note_of_path("///").as_deref(), None);
    }

    #[tokio::test]
    async fn missing_binary_is_skipped_with_reason() {
        let r = ProcessRunner {
            bin: PathBuf::from("/nonexistent/qmd-binary"),
        };
        let e = prepare(&r, "idx", Path::new("/x")).await.unwrap_err();
        assert!(e.0.contains("could not be started"), "{}", e.0);
    }

    #[tokio::test]
    async fn embed_failure_is_skipped_not_zero() {
        let r = FakeRunner::new(|a| {
            if a.contains(&"embed".to_string()) {
                Ok(QmdOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: "model download failed\nmore".into(),
                })
            } else {
                ok("qmd 1.2.3\n")
            }
        });
        let e = prepare(&r, "idx", Path::new("/x")).await.unwrap_err();
        assert!(e.0.contains("embed failed") && e.0.contains("model download failed"));
    }

    #[tokio::test]
    async fn prepare_records_version_and_runs_setup_in_order() {
        let r = FakeRunner::new(|_| ok("qmd 1.2.3\n"));
        let info = prepare(&r, "idx", Path::new("/notes")).await.unwrap();
        assert_eq!(info.version, "qmd 1.2.3");
        let log = r.log.lock().unwrap();
        assert_eq!(log[1][..3], ["--index", "idx", "collection"]);
        assert!(log[2].contains(&"update".to_string()));
        assert!(log[3].contains(&"embed".to_string()));
    }

    #[tokio::test]
    async fn retriever_passes_k_and_rerank_flag_and_maps_notes() {
        let r = Arc::new(FakeRunner::new(|_| ok(SAMPLE)));
        let mut notes = BTreeMap::new();
        notes.insert("old-pricing-note".to_string(), "OLD BODY".to_string());
        let mk = |rerank| QmdRetriever {
            runner: r.clone(),
            index_name: "idx".into(),
            rerank,
            id: "qmd",
            notes: Arc::new(notes.clone()),
        };
        let res = mk(false).retrieve("price?", 7).await.unwrap();
        assert_eq!(res.notes, vec!["pricing-decision", "old-pricing-note"]);
        assert_eq!(
            res.evidence,
            vec!["We chose usage-based pricing.", "OLD BODY"]
        );
        assert_eq!(res.model_calls, None);
        mk(true).retrieve("price?", 7).await.unwrap();
        let log = r.log.lock().unwrap();
        assert!(log[0].contains(&"--no-rerank".to_string()));
        assert!(!log[1].contains(&"--no-rerank".to_string()));
        let n = log[0].iter().position(|a| a == "-n").unwrap();
        assert_eq!(log[0][n + 1], "7");
    }

    #[tokio::test]
    async fn failed_query_is_an_error_not_an_empty_result() {
        let r = Arc::new(FakeRunner::new(|_| {
            Ok(QmdOutput {
                success: false,
                stdout: String::new(),
                stderr: "boom".into(),
            })
        }));
        let rt = QmdRetriever {
            runner: r,
            index_name: "i".into(),
            rerank: true,
            id: "qmd-rerank",
            notes: Arc::new(BTreeMap::new()),
        };
        assert!(rt.retrieve("x", 3).await.unwrap_err().contains("boom"));
    }

    #[tokio::test]
    async fn silent_success_is_an_error_not_an_empty_result() {
        let r = Arc::new(FakeRunner::new(|_| ok("")));
        let rt = QmdRetriever {
            runner: r,
            index_name: "idx".into(),
            rerank: false,
            id: "qmd",
            notes: Arc::new(BTreeMap::new()),
        };
        let e = rt.retrieve("q", 3).await.unwrap_err();
        assert!(e.contains("produced no output"), "{e}");
    }
}
