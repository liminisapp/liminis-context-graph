//! The query-variants fixture for `lcg-passages-variants` (issue #802, ADR-0802).
//!
//! `query-variants.jsonl` holds, per question id, up to [`MAX_VARIANTS`] caller-style rewrites of
//! the question. They simulate an LLM agent supplying `queries` to `knowledge_search_passages`.
//! They are written once, out of band, by an LLM that sees **only the question text** (see
//! `lcg-eval retrieval-variants` and `query-variants.prompt.md`), and stamped with the hash of the
//! `questions.jsonl` they were written for.
//!
//! One JSON object per line: `{"id": "...", "questions_hash": "...", "variants": ["...", ...]}`.
//!
//! Loading fails clearly — never silently scores — on a stale hash, an unknown or duplicate
//! question id, more than [`MAX_VARIANTS`] variants, an empty variant, or an answer leak.
//!
//! **Leak check.** A variant leaks the answer if it contains, verbatim, the title or the key-fact
//! string of any note that is relevant to its question. Matching is case-insensitive over
//! whitespace-normalised text. A note's *title* is the first `# ` heading of its markdown, checked
//! both whole and with a leading `Notes on ` / `Q&A: ` stripped (the subject name). Needles
//! shorter than [`MIN_NEEDLE_CHARS`] are skipped (a one-word heading like "Update" is not a leak).
//! A needle that already appears in the question text is skipped too: the variant writer was
//! given that text, so repeating it is not leaked information (direct questions name their
//! subject). The check is a safety net for verbatim leaks only; it cannot see paraphrased ones.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Value};

use super::fixture::Fixture;

/// File name of the variants fixture, next to `questions.jsonl`.
pub const FILE: &str = "query-variants.jsonl";

/// Most variants a question may carry (matches `knowledge_search_passages`' limit).
pub const MAX_VARIANTS: usize = 5;

/// Titles and key facts shorter than this (in chars, after normalisation) are not leak needles.
pub const MIN_NEEDLE_CHARS: usize = 8;

/// Rejects a variant set whose combined length (Unicode scalar values of the trimmed entries)
/// exceeds the service's `queries` cap, [`lcg_core::query_variants::MAX_VARIANTS_TOTAL_CHARS`].
/// The service would refuse such a request at run time, so the offline check must too.
pub fn check_total_len(variants: &[String]) -> Result<(), String> {
    let total: usize = variants.iter().map(|s| s.chars().count()).sum();
    let max = lcg_core::query_variants::MAX_VARIANTS_TOTAL_CHARS;
    if total > max {
        return Err(format!(
            "variants total {total} characters; at most {max} are allowed (the service rejects more)"
        ));
    }
    Ok(())
}

/// Variants by question id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VariantsFixture {
    pub by_id: HashMap<String, Vec<String>>,
}

impl VariantsFixture {
    /// The variants for a question; empty when the fixture has none for it (valid).
    pub fn for_question(&self, id: &str) -> &[String] {
        self.by_id.get(id).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// One verbatim leak: which question, which variant, and what it contained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    pub question: String,
    pub variant: String,
    pub note: String,
    pub what: &'static str,
    pub needle: String,
}

impl std::fmt::Display for Leak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "question '{}': variant {:?} contains the {} of relevant note '{}' ({:?})",
            self.question, self.variant, self.what, self.note, self.needle
        )
    }
}

/// Lower-cases and collapses runs of whitespace to single spaces.
fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The title needles of a note body: its first `# ` heading, whole, and with a leading
/// `Notes on ` / `Q&A: ` stripped. Normalised and de-duplicated; too-short ones are dropped.
pub fn title_needles(body: &str) -> Vec<String> {
    let heading = body
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(str::trim)
        .unwrap_or("");
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let n = normalize(s);
        if n.chars().count() >= MIN_NEEDLE_CHARS && !out.contains(&n) {
            out.push(n);
        }
    };
    push(heading);
    for prefix in ["Notes on ", "Q&A: "] {
        if let Some(rest) = heading.strip_prefix(prefix) {
            push(rest);
        }
    }
    out
}

/// Every verbatim leak in `vf` against `fx`. Empty means clean.
pub fn find_leaks(fx: &Fixture, vf: &VariantsFixture) -> Vec<Leak> {
    let mut leaks = Vec::new();
    for q in &fx.questions {
        let variants = vf.for_question(&q.id);
        if variants.is_empty() {
            continue;
        }
        let question_text = normalize(&q.text);
        // (note, kind, needle) for every relevant note of this question.
        let mut needles: Vec<(String, &'static str, String)> = Vec::new();
        for rel in &q.relevant {
            if let Some(body) = fx.notes.get(&rel.note) {
                for n in title_needles(body) {
                    needles.push((rel.note.clone(), "title", n));
                }
            }
        }
        for ev in &q.evidence {
            let n = normalize(&ev.key_fact);
            if n.chars().count() >= MIN_NEEDLE_CHARS {
                needles.push((ev.note.clone(), "key fact", n));
            }
        }
        for variant in variants {
            let v = normalize(variant);
            for (note, what, needle) in &needles {
                if v.contains(needle.as_str()) && !question_text.contains(needle.as_str()) {
                    leaks.push(Leak {
                        question: q.id.clone(),
                        variant: variant.clone(),
                        note: note.clone(),
                        what,
                        needle: needle.clone(),
                    });
                    break;
                }
            }
        }
    }
    leaks
}

/// `Err` with every leak listed when any variant leaks.
pub fn check_leaks(fx: &Fixture, vf: &VariantsFixture) -> Result<(), String> {
    let leaks = find_leaks(fx, vf);
    if leaks.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{FILE} leaks answers ({} variant(s)); regenerate those questions' variants:\n  {}",
        leaks.len(),
        leaks
            .iter()
            .map(Leak::to_string)
            .collect::<Vec<_>>()
            .join("\n  ")
    ))
}

/// Parses and validates `text` against `fx` (hash, ids, counts, emptiness) — no leak check.
pub fn parse_variants(text: &str, fx: &Fixture) -> Result<VariantsFixture, String> {
    let known: HashSet<&str> = fx.questions.iter().map(|q| q.id.as_str()).collect();
    let mut by_id: HashMap<String, Vec<String>> = HashMap::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let at = format!("{FILE} line {}", i + 1);
        let v: Value = serde_json::from_str(line).map_err(|e| format!("{at}: bad JSON: {e}"))?;
        let id = v["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{at}: missing string field 'id'"))?;
        let hash = v["questions_hash"]
            .as_str()
            .ok_or_else(|| format!("{at} ('{id}'): missing string field 'questions_hash'"))?;
        if hash != fx.questions_hash {
            return Err(format!(
                "{at} ('{id}'): stamped with questions hash {hash} but the current questions.jsonl hashes to {}; \
                 the variants are stale — regenerate them with `lcg-eval retrieval-variants`",
                fx.questions_hash
            ));
        }
        if !known.contains(id) {
            return Err(format!(
                "{at}: question id '{id}' is not in questions.jsonl"
            ));
        }
        let arr = v["variants"]
            .as_array()
            .ok_or_else(|| format!("{at} ('{id}'): 'variants' must be an array of strings"))?;
        if arr.len() > MAX_VARIANTS {
            return Err(format!(
                "{at} ('{id}'): {} variants; at most {MAX_VARIANTS} are allowed",
                arr.len()
            ));
        }
        let mut variants = Vec::with_capacity(arr.len());
        for (j, s) in arr.iter().enumerate() {
            let s = s
                .as_str()
                .ok_or_else(|| format!("{at} ('{id}'): variants[{j}] must be a string"))?
                .trim();
            if s.is_empty() {
                return Err(format!("{at} ('{id}'): variants[{j}] is empty"));
            }
            variants.push(s.to_string());
        }
        check_total_len(&variants).map_err(|e| format!("{at} ('{id}'): {e}"))?;
        if by_id.insert(id.to_string(), variants).is_some() {
            return Err(format!("{at}: duplicate question id '{id}'"));
        }
    }
    Ok(VariantsFixture { by_id })
}

/// Loads `<dir>/query-variants.jsonl`. `Ok(None)` when the file does not exist (the variants
/// config is then a skipped slot, not a failure); otherwise it is validated and leak-checked.
pub fn load_variants(dir: &Path, fx: &Fixture) -> Result<Option<VariantsFixture>, String> {
    let path = dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let vf = parse_variants(&text, fx)?;
    check_leaks(fx, &vf)?;
    Ok(Some(vf))
}

/// One fixture line, in question order, stamped with `questions_hash`.
pub fn fixture_line(id: &str, questions_hash: &str, variants: &[String]) -> String {
    json!({"id": id, "questions_hash": questions_hash, "variants": variants}).to_string()
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::retrieval::fixture::{
        Category, EvidenceSpec, Fixture, Manifest, Question, Relevant,
    };
    use std::collections::BTreeMap;

    /// A tiny fixture: `para-1` is relevant to a note titled "Notes on Marlin Policy" whose key
    /// fact is "exception approver is Callum Brennan"; `direct-1` names its subject itself.
    pub fn tiny_fixture() -> Fixture {
        let mut notes = BTreeMap::new();
        notes.insert(
            "policy-marlin".to_string(),
            "# Notes on Marlin Policy\n\nMarlin Policy is the policy responsible for onboarding. The Marlin Policy exception approver is Callum Brennan.\n".to_string(),
        );
        notes.insert(
            "person-aiko".to_string(),
            "# Notes on Aiko Osei\n\nAiko Osei reports to Dana.\n".to_string(),
        );
        let q = |id: &str, text: &str, note: &str, key_fact: &str| Question {
            id: id.to_string(),
            category: Category::Paraphrase,
            text: text.to_string(),
            relevant: vec![Relevant {
                note: note.to_string(),
                gain: 2,
            }],
            evidence: vec![EvidenceSpec {
                note: note.to_string(),
                key_fact: key_fact.to_string(),
            }],
            current: vec![],
            stale: vec![],
            negative: false,
        };
        Fixture {
            dir: std::path::PathBuf::from("."),
            manifest: Manifest {
                corpus_version: "t".into(),
                max_chunk_tokens: 900,
                notes: vec![],
                supersedes: vec![],
            },
            questions_version: "t".into(),
            questions: vec![
                q(
                    "para-1",
                    "Whose sign-off lets someone get around the rule covering new-hire induction?",
                    "policy-marlin",
                    "exception approver is Callum Brennan",
                ),
                q(
                    "direct-1",
                    "Aiko Osei reports to whom?",
                    "person-aiko",
                    "reports to Dana",
                ),
            ],
            notes,
            corpus_hash: "c".into(),
            questions_hash: "abcd1234abcd1234".into(),
        }
    }

    pub fn line(id: &str, hash: &str, variants: &[&str]) -> String {
        fixture_line(
            id,
            hash,
            &variants.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    const H: &str = "abcd1234abcd1234";

    #[test]
    fn a_clean_fixture_loads_and_passes_the_leak_check() {
        let fx = tiny_fixture();
        let text = format!(
            "{}\n{}\n",
            line(
                "para-1",
                H,
                &[
                    "onboarding policy exception approver",
                    "who can waive induction rules"
                ]
            ),
            line(
                "direct-1",
                H,
                &["Aiko Osei manager", "Aiko Osei reporting line"]
            ),
        );
        let vf = parse_variants(&text, &fx).unwrap();
        assert_eq!(vf.for_question("para-1").len(), 2);
        assert!(find_leaks(&fx, &vf).is_empty());
        check_leaks(&fx, &vf).unwrap();
    }

    #[test]
    fn a_question_with_no_variants_is_valid() {
        let fx = tiny_fixture();
        let text = format!("{}\n", line("para-1", H, &[]));
        let vf = parse_variants(&text, &fx).unwrap();
        assert!(vf.for_question("para-1").is_empty());
        assert!(vf.for_question("direct-1").is_empty());
        assert!(check_leaks(&fx, &vf).is_ok());
    }

    #[test]
    fn a_title_leak_names_the_question_and_the_variant() {
        let fx = tiny_fixture();
        let vf = parse_variants(
            &line("para-1", H, &["what is in the marlin policy note"]),
            &fx,
        )
        .unwrap();
        let leaks = find_leaks(&fx, &vf);
        assert_eq!(leaks.len(), 1, "{leaks:?}");
        assert_eq!(leaks[0].what, "title");
        let msg = check_leaks(&fx, &vf).unwrap_err();
        assert!(msg.contains("para-1"), "{msg}");
        assert!(msg.contains("what is in the marlin policy note"), "{msg}");
    }

    #[test]
    fn the_whole_heading_is_a_needle_too() {
        let fx = tiny_fixture();
        let vf = parse_variants(&line("para-1", H, &["Notes   on MARLIN policy"]), &fx).unwrap();
        assert_eq!(find_leaks(&fx, &vf).len(), 1);
    }

    #[test]
    fn a_key_fact_leak_is_caught_case_insensitively() {
        let fx = tiny_fixture();
        let vf = parse_variants(
            &line("para-1", H, &["The Exception Approver is callum brennan"]),
            &fx,
        )
        .unwrap();
        let leaks = find_leaks(&fx, &vf);
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].what, "key fact");
    }

    #[test]
    fn repeating_what_the_question_already_says_is_not_a_leak() {
        // `direct-1` names Aiko Osei; a variant naming them again leaks nothing.
        let fx = tiny_fixture();
        let vf = parse_variants(&line("direct-1", H, &["Aiko Osei reports to"]), &fx).unwrap();
        assert!(find_leaks(&fx, &vf).is_empty());
    }

    #[test]
    fn a_stale_hash_is_refused() {
        let fx = tiny_fixture();
        let err = parse_variants(&line("para-1", "ffffffffffffffff", &["x y"]), &fx).unwrap_err();
        assert!(err.contains("stale"), "{err}");
        assert!(err.contains("ffffffffffffffff"), "{err}");
    }

    #[test]
    fn an_unknown_id_is_refused() {
        let fx = tiny_fixture();
        let err = parse_variants(&line("gone-9", H, &["x y"]), &fx).unwrap_err();
        assert!(err.contains("gone-9"), "{err}");
    }

    #[test]
    fn a_fixture_over_the_services_total_length_cap_is_refused() {
        // The service rejects `queries` over MAX_VARIANTS_TOTAL_CHARS at run time, so the offline
        // check must catch it first. 5 x 401 chars = 2005 > 2000; counted in chars, not bytes.
        let fx = tiny_fixture();
        let long = "é".repeat(401);
        let five: Vec<&str> = vec![long.as_str(); 5];
        let err = parse_variants(&line("para-1", H, &five), &fx).unwrap_err();
        assert!(err.contains("2005"), "{err}");
        assert!(err.contains("para-1"), "{err}");
        let ok: Vec<&str> = vec![&long[..2 * 400]; 5];
        assert!(parse_variants(&line("para-1", H, &ok), &fx).is_ok());
    }

    #[test]
    fn too_many_empty_and_duplicate_entries_are_refused() {
        let fx = tiny_fixture();
        let six = line("para-1", H, &["a1", "b2", "c3", "d4", "e5", "f6"]);
        assert!(parse_variants(&six, &fx).unwrap_err().contains("at most 5"));
        let empty = line("para-1", H, &["ok ok", "  "]);
        assert!(parse_variants(&empty, &fx).unwrap_err().contains("empty"));
        let dup = format!(
            "{}\n{}",
            line("para-1", H, &["a b"]),
            line("para-1", H, &["c d"])
        );
        assert!(parse_variants(&dup, &fx).unwrap_err().contains("duplicate"));
        let not_str = format!(r#"{{"id":"para-1","questions_hash":"{H}","variants":[3]}}"#);
        assert!(parse_variants(&not_str, &fx)
            .unwrap_err()
            .contains("must be a string"));
    }

    #[test]
    fn short_headings_are_not_needles() {
        assert!(title_needles("# Update\n\nbody").is_empty());
        assert_eq!(title_needles("# Q&A: Marlin Core — launch\n"), {
            vec![
                "q&a: marlin core — launch".to_string(),
                "marlin core — launch".to_string(),
            ]
        });
    }

    #[test]
    fn load_variants_returns_none_when_the_file_is_absent() {
        let fx = tiny_fixture();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_variants(dir.path(), &fx).unwrap(), None);
    }

    #[test]
    fn load_variants_validates_and_leak_checks_the_file() {
        let fx = tiny_fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE),
            line("para-1", H, &["onboarding policy exception approver"]),
        )
        .unwrap();
        assert!(load_variants(dir.path(), &fx).unwrap().is_some());
        std::fs::write(
            dir.path().join(FILE),
            line("para-1", H, &["marlin policy approver"]),
        )
        .unwrap();
        assert!(load_variants(dir.path(), &fx)
            .unwrap_err()
            .contains("leaks answers"));
    }
}
