//! `lcg-eval retrieval-variants` (issue #802): the offline exchange that produces
//! `query-variants.jsonl`, mirroring `retrieval-render`'s `--export-prompts` / `--replies`.
//!
//! The variants simulate an LLM agent supplying `queries` to `knowledge_search_passages`, so they
//! are written by an LLM that sees **only the question text**: no corpus, notes or relevance
//! labels. Each prompt is the fixed instruction text committed next to the fixture
//! (`query-variants.prompt.md`, the *system* prompt) plus the question text alone (the *user*
//! prompt). Editing the prompt file changes every call key, so stale replies are never reused.
//!
//! ```text
//! lcg-eval retrieval-variants --dir F --export-prompts prompts.jsonl   # maintainer
//! # ... an LLM writes <replies>/<key>.md for each line (a JSON array of up to 5 strings) ...
//! lcg-eval retrieval-variants --dir F --replies <replies>              # writes the fixture
//! lcg-eval retrieval-variants --dir F --check                          # validate + leak check
//! ```
//!
//! No API key is needed anywhere; this is a one-time maintainer step, never run in CI or by a
//! Fabrik worker.

use std::path::Path;

use serde_json::{json, Value};

use crate::retrieval::fixture::{self, Fixture};
use crate::retrieval::generator::render::{call_key, FileText, TextModel};
use crate::retrieval::variants::{self, MAX_VARIANTS};

/// The fixed generation prompt, committed next to the fixture. It is the system prompt verbatim.
pub const PROMPT_FILE: &str = "query-variants.prompt.md";

pub const USAGE: &str = "lcg-eval retrieval-variants — produce query-variants.jsonl (a one-time maintainer step; no API key)

USAGE:
    lcg-eval retrieval-variants --dir <FIXTURE DIR> --export-prompts <FILE>
    lcg-eval retrieval-variants --dir <FIXTURE DIR> --replies <DIR>
    lcg-eval retrieval-variants --dir <FIXTURE DIR> --check

--export-prompts writes one JSON line per question (key, id, system, user). The system text is
<DIR>/query-variants.prompt.md verbatim; the user text is the question and nothing else — no corpus,
no notes, no relevance labels.

--replies reads <DIR>/<key>.md for every question (a JSON array of at most 5 strings; a markdown
code fence around it is tolerated), checks that no variant leaks a relevant note's title or key fact,
and writes <FIXTURE DIR>/query-variants.jsonl stamped with the questions hash. Nothing is written if
any reply is missing or invalid, or any variant leaks.

--check validates an existing query-variants.jsonl against the current questions (hash, ids,
counts) and runs the leak check.
";

/// The system prompt: the committed prompt file, verbatim.
pub fn load_prompt(dir: &Path) -> Result<String, String> {
    let p = dir.join(PROMPT_FILE);
    let t = std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    if t.trim().is_empty() {
        return Err(format!("{} is empty", p.display()));
    }
    Ok(t)
}

/// One exported call per question, in question order. The user prompt is the question text only.
pub fn export_calls(fx: &Fixture, system: &str) -> Vec<Value> {
    fx.questions
        .iter()
        .map(|q| {
            json!({"key": call_key(system, &q.text), "id": q.id,
                   "system": system, "user": q.text})
        })
        .collect()
}

/// Parses one reply: a JSON array of strings, possibly inside a code fence or after a sentence.
/// More than [`MAX_VARIANTS`] is an error, never truncated. Entries are trimmed; an empty entry is
/// an error.
pub fn parse_reply(reply: &str) -> Result<Vec<String>, String> {
    let start = reply.find('[').ok_or("reply has no JSON array")?;
    let end = reply.rfind(']').ok_or("reply has no JSON array")?;
    if end < start {
        return Err("reply has no JSON array".into());
    }
    let v: Value =
        serde_json::from_str(&reply[start..=end]).map_err(|e| format!("reply is not JSON: {e}"))?;
    let arr = v.as_array().ok_or("reply is not a JSON array")?;
    if arr.len() > MAX_VARIANTS {
        return Err(format!(
            "reply has {} variants; at most {MAX_VARIANTS} are allowed",
            arr.len()
        ));
    }
    let mut out = Vec::with_capacity(arr.len());
    for (i, s) in arr.iter().enumerate() {
        let s = s
            .as_str()
            .ok_or_else(|| format!("variants[{i}] is not a string"))?
            .trim();
        if s.is_empty() {
            return Err(format!("variants[{i}] is empty"));
        }
        out.push(s.to_string());
    }
    Ok(out)
}

/// Reads every question's reply through `model` and returns the fixture text (one line per
/// question, question order) — validated and leak-checked, but not written.
pub async fn build_fixture_text(
    fx: &Fixture,
    system: &str,
    model: &dyn TextModel,
) -> Result<String, String> {
    let mut lines = Vec::with_capacity(fx.questions.len());
    let mut problems = Vec::new();
    let mut by_id = variants::VariantsFixture::default();
    for q in &fx.questions {
        match model.complete(system, &q.text, 0).await {
            Err(e) => problems.push(format!("{}: {e}", q.id)),
            Ok(reply) => match parse_reply(&reply) {
                Err(e) => problems.push(format!("{}: {e}", q.id)),
                Ok(vs) => {
                    lines.push(variants::fixture_line(&q.id, &fx.questions_hash, &vs));
                    by_id.by_id.insert(q.id.clone(), vs);
                }
            },
        }
    }
    if !problems.is_empty() {
        return Err(format!(
            "{} question(s) have no usable reply; nothing written:\n  {}",
            problems.len(),
            problems.join("\n  ")
        ));
    }
    variants::check_leaks(fx, &by_id)?;
    Ok(lines.join("\n") + "\n")
}

pub async fn run_cli(args: &[String]) -> i32 {
    match run(args).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lcg-eval retrieval-variants: {e}");
            if e.starts_with("unknown") || e.contains("requires a value") {
                2
            } else {
                1
            }
        }
    }
}

async fn run(args: &[String]) -> Result<i32, String> {
    let (mut dir, mut export, mut replies, mut check) = (None, None, None, false);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            "--check" => check = true,
            flag @ ("--dir" | "--export-prompts" | "--replies") => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| format!("{flag} requires a value"))?
                    .clone();
                match flag {
                    "--dir" => dir = Some(v),
                    "--export-prompts" => export = Some(v),
                    _ => replies = Some(v),
                }
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
        i += 1;
    }
    let dir = dir.ok_or_else(|| format!("--dir is required\n\n{USAGE}"))?;
    let dir = Path::new(&dir);
    let modes = [export.is_some(), replies.is_some(), check]
        .iter()
        .filter(|m| **m)
        .count();
    if modes != 1 {
        return Err(format!(
            "give exactly one of --export-prompts, --replies, --check\n\n{USAGE}"
        ));
    }
    let fx = fixture::load(dir)?;

    if let Some(f) = export {
        let system = load_prompt(dir)?;
        let lines: Vec<String> = export_calls(&fx, &system)
            .iter()
            .map(|v| v.to_string())
            .collect();
        std::fs::write(&f, lines.join("\n") + "\n").map_err(|e| format!("{f}: {e}"))?;
        println!("retrieval-variants: wrote {} prompts to {f}", lines.len());
        return Ok(0);
    }
    if let Some(r) = replies {
        let system = load_prompt(dir)?;
        let model = FileText::new(Path::new(&r), "offline replies".to_string());
        let text = build_fixture_text(&fx, &system, &model).await?;
        let out = dir.join(variants::FILE);
        std::fs::write(&out, &text).map_err(|e| format!("{}: {e}", out.display()))?;
        println!(
            "retrieval-variants: wrote {} (questions hash {})",
            out.display(),
            fx.questions_hash
        );
        return Ok(0);
    }
    // --check
    match variants::load_variants(dir, &fx)? {
        None => Err(format!(
            "{} does not exist",
            dir.join(variants::FILE).display()
        )),
        Some(vf) => {
            let total: usize = vf.by_id.values().map(Vec::len).sum();
            println!(
                "retrieval-variants: ok — {} question(s) with variants, {total} variant(s), no leaks (questions hash {})",
                vf.by_id.len(),
                fx.questions_hash
            );
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::variants::testing::tiny_fixture;

    const SYSTEM: &str = "Write up to 5 search-query rewrites as a JSON array.";

    #[test]
    fn prompts_hold_only_the_question_text_and_the_fixed_system_text() {
        let fx = tiny_fixture();
        let calls = export_calls(&fx, SYSTEM);
        assert_eq!(calls.len(), fx.questions.len());
        for (c, q) in calls.iter().zip(&fx.questions) {
            assert_eq!(c["system"], SYSTEM);
            assert_eq!(c["user"], q.text.as_str());
            assert_eq!(c["id"], q.id.as_str());
            assert_eq!(c["key"], call_key(SYSTEM, &q.text).as_str());
            // Nothing from the corpus or the relevance labels reaches the prompt.
            let blob = c.to_string();
            for rel in &q.relevant {
                assert!(!blob.contains(&rel.note), "note id leaked: {blob}");
            }
            for ev in &q.evidence {
                assert!(!blob.contains(&ev.key_fact), "key fact leaked: {blob}");
            }
            assert!(!blob.contains("Callum Brennan"), "{blob}");
        }
    }

    #[test]
    fn replies_parse_plain_fenced_and_chatty_arrays() {
        assert_eq!(
            parse_reply(r#"["a b", " c d "]"#).unwrap(),
            vec!["a b", "c d"]
        );
        assert_eq!(parse_reply("```json\n[\"x y\"]\n```").unwrap(), vec!["x y"]);
        assert_eq!(
            parse_reply("Here you go: [\"p q\"] hope that helps").unwrap(),
            vec!["p q"]
        );
        assert!(parse_reply("[]").unwrap().is_empty());
    }

    #[test]
    fn bad_replies_are_rejected_not_truncated() {
        assert!(parse_reply("no array here").is_err());
        assert!(parse_reply("[1, 2]").unwrap_err().contains("not a string"));
        assert!(parse_reply(r#"["a", " "]"#).unwrap_err().contains("empty"));
        let six = r#"["a","b","c","d","e","f"]"#;
        assert!(parse_reply(six).unwrap_err().contains("at most 5"));
    }

    fn write_replies(dir: &Path, fx: &Fixture, per_question: &[&str]) {
        for (q, body) in fx.questions.iter().zip(per_question) {
            std::fs::write(dir.join(format!("{}.md", call_key(SYSTEM, &q.text))), body).unwrap();
        }
    }

    #[tokio::test]
    async fn export_replies_write_round_trip_stamps_the_hash() {
        let fx = tiny_fixture();
        let dir = tempfile::tempdir().unwrap();
        write_replies(
            dir.path(),
            &fx,
            &[
                r#"["onboarding policy exception approver", "who waives induction rules"]"#,
                r#"["Aiko Osei manager"]"#,
            ],
        );
        let model = FileText::new(dir.path(), "m".into());
        let text = build_fixture_text(&fx, SYSTEM, &model).await.unwrap();
        // Written text loads back through the strict loader.
        let vf = variants::parse_variants(&text, &fx).unwrap();
        assert_eq!(vf.for_question("para-1").len(), 2);
        assert_eq!(vf.for_question("direct-1"), ["Aiko Osei manager"]);
        for line in text.lines() {
            let v: Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["questions_hash"], fx.questions_hash.as_str());
        }
    }

    #[tokio::test]
    async fn a_leaking_reply_blocks_the_write() {
        let fx = tiny_fixture();
        let dir = tempfile::tempdir().unwrap();
        write_replies(
            dir.path(),
            &fx,
            &[r#"["what does the marlin policy say"]"#, r#"[]"#],
        );
        let model = FileText::new(dir.path(), "m".into());
        let err = build_fixture_text(&fx, SYSTEM, &model).await.unwrap_err();
        assert!(err.contains("para-1"), "{err}");
        assert!(err.contains("marlin policy"), "{err}");
    }

    #[tokio::test]
    async fn a_missing_or_oversized_reply_blocks_the_write() {
        let fx = tiny_fixture();
        let dir = tempfile::tempdir().unwrap();
        write_replies(
            dir.path(),
            &fx,
            &[r#"["a1","b2","c3","d4","e5","f6"]"#, r#"["x y"]"#],
        );
        std::fs::remove_file(
            dir.path()
                .join(format!("{}.md", call_key(SYSTEM, &fx.questions[1].text))),
        )
        .unwrap();
        let model = FileText::new(dir.path(), "m".into());
        let err = build_fixture_text(&fx, SYSTEM, &model).await.unwrap_err();
        assert!(err.contains("2 question(s)"), "{err}");
        assert!(err.contains("at most 5"), "{err}");
        assert!(err.contains("direct-1"), "{err}");
    }

    #[test]
    fn the_committed_prompt_names_the_three_variant_kinds() {
        let p = include_str!("../../../fixtures/retrieval-v2/query-variants.prompt.md");
        for needle in ["keywords", "name", "hypothetical answer", "JSON array", "5"] {
            assert!(p.contains(needle), "prompt must mention {needle:?}");
        }
    }
}
