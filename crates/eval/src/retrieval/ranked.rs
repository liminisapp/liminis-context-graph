//! `ranked.jsonl` (`--record-ranked`, issue #872): per-question, passage-level ranked hits for
//! every config that exposes per-hit fields, written beside the report.
//!
//! One line per config and scored question:
//! `{"config","question","category","hits":[{"rank","note","uuid","score","similarity","legs"}]}`
//! with each hit field omitted when the service did not supply it. Lines of different configs of
//! one run join on `question`. Evidence text is never written. Failed queries are absent; a
//! question that returned zero passages is present with an empty `hits` array, so a join has no
//! gaps.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::report::{ConfigOutcome, Status};

pub const RANKED_FILE: &str = "ranked.jsonl";

/// JSONL lines (no trailing newline per line), configs in run order, questions in scoring order.
pub fn ranked_lines(outcomes: &[ConfigOutcome]) -> Vec<String> {
    let mut lines = Vec::new();
    for o in outcomes {
        let Status::Ran(run) = &o.status else {
            continue;
        };
        for q in &run.ranked {
            lines.push(
                json!({
                    "config": o.id,
                    "question": q.question,
                    "category": q.category.label(),
                    "hits": q.hits.iter().map(|h| h.to_json()).collect::<Vec<_>>(),
                })
                .to_string(),
            );
        }
    }
    lines
}

/// Writes `<out>/ranked.jsonl` and returns its path.
pub fn write_ranked(out: &Path, outcomes: &[ConfigOutcome]) -> Result<PathBuf, String> {
    let path = out.join(RANKED_FILE);
    let mut body = ranked_lines(outcomes).join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    std::fs::write(&path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::fixture::Category;
    use crate::retrieval::mapping::RankedHit;
    use crate::retrieval::metrics::summarize_by_category;
    use crate::retrieval::report::{ConfigRun, Grounded, RankedQuestion};
    use std::collections::BTreeSet;

    fn outcome(id: &str, qs: &[(&str, Vec<RankedHit>)]) -> ConfigOutcome {
        let (overall, by_category) = summarize_by_category(&[]);
        ConfigOutcome {
            id: id.into(),
            description: String::new(),
            status: Status::Ran(Box::new(ConfigRun {
                scores: vec![],
                overall,
                by_category,
                latency_p50_ms: None,
                latency_p95_ms: None,
                model_calls_per_query: None,
                lookup_calls: 0,
                evidence_gaps: 0,
                legs_tally: None,
                query_errors: vec![],
                grounded: Grounded::Skipped("x".into()),
                origin: None,
                ranked: qs
                    .iter()
                    .map(|(q, h)| RankedQuestion {
                        question: (*q).into(),
                        category: Category::Direct,
                        hits: h.clone(),
                    })
                    .collect(),
            })),
        }
    }

    fn hit(rank: usize, legs: Option<&str>) -> RankedHit {
        RankedHit {
            rank,
            note: Some("n".into()),
            uuid: None,
            score: Some(0.5),
            similarity: legs.map(|_| 0.7),
            legs: legs.map(str::to_string),
        }
    }

    #[test]
    fn two_configs_join_on_question_id_with_no_missing_ids() {
        let raw = outcome(
            "lcg-passages-raw",
            &[("q1", vec![hit(1, None)]), ("q2", vec![])],
        );
        let hybrid = outcome(
            "lcg-passages-hybrid",
            &[
                ("q1", vec![hit(1, Some("both"))]),
                ("q2", vec![hit(1, Some("lexical"))]),
            ],
        );
        let lines = ranked_lines(&[raw, hybrid]);
        assert_eq!(lines.len(), 4);
        let ids = |cfg: &str| -> BTreeSet<String> {
            lines
                .iter()
                .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
                .filter(|v| v["config"] == cfg)
                .map(|v| v["question"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(ids("lcg-passages-raw"), ids("lcg-passages-hybrid"));
        // A zero-passage question is present with an empty list.
        let q2: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(q2["hits"], json!([]));
        // legs only where the service supplied it.
        let h: serde_json::Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(h["hits"][0]["legs"], "both");
        let r: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(r["hits"][0].get("legs").is_none());
    }

    #[test]
    fn skipped_configs_and_configs_without_hits_write_nothing() {
        let skipped = ConfigOutcome {
            id: "qmd".into(),
            description: String::new(),
            status: Status::Skipped("absent".into()),
        };
        assert!(ranked_lines(&[skipped, outcome("lcg-entities-facts", &[])]).is_empty());
    }

    #[test]
    fn write_ranked_puts_the_file_beside_the_report() {
        let d = tempfile::tempdir().unwrap();
        let p = write_ranked(d.path(), &[outcome("c", &[("q1", vec![hit(1, None)])])]).unwrap();
        assert_eq!(p, d.path().join("ranked.jsonl"));
        let body = std::fs::read_to_string(p).unwrap();
        assert_eq!(body.lines().count(), 1);
        assert!(body.ends_with('\n'));
    }
}
