//! The explicit, paid render step for retrieval-v2 (#788 scope change, ADR-0788):
//! `lcg-eval retrieval-render` has an LLM write each skeleton note in the voice its origin implies
//! (journal, decision record, standup recap, clipped article, agent topic page with citations, filed
//! answer) and each question's wording, paraphrases included. The output is checked in; nothing in
//! CI or in a normal eval run calls a model, and there is no template fallback.
//!
//! The prompts ask for verbatim entity names and values; [`super::check`] then verifies the result
//! against the skeleton. A failing note or question can be re-rendered alone with `--only`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::{self, StreamExt};
use serde_json::{json, Value};

use super::check::{self, Rendered};
use super::questions::content_words;
use super::skeleton::*;
use super::{check_dir, write_fixture, RenderMeta};

/// Bump when the prompts below change; recorded in `render.json`.
pub const PROMPT_VERSION: &str = "retrieval-v2-render-1";

/// A text-generation model.
pub trait TextModel: Send + Sync {
    fn model(&self) -> &str;
    fn complete<'a>(
        &'a self,
        system: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> BoxFuture<'a, Result<String, String>>;
}

/// Direct Anthropic Messages API client (the same wire format the judge uses).
pub struct AnthropicText {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicText {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(600))
                .build()
                .expect("http client"),
        }
    }
}

impl TextModel for AnthropicText {
    fn model(&self) -> &str {
        &self.model
    }

    fn complete<'a>(
        &'a self,
        system: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let body = json!({
                "model": self.model,
                "max_tokens": max_tokens,
                "system": system,
                "messages": [{"role": "user", "content": user}],
                "thinking": {"type": "disabled"},
            });
            let mut attempt = 0u32;
            loop {
                let sent = self
                    .client
                    .post("https://api.anthropic.com/v1/messages")
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await;
                let resp = match sent {
                    Ok(r) => r,
                    Err(_) if attempt < 4 => {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    Err(e) => return Err(format!("render request failed after retries: {e}")),
                };
                let status = resp.status();
                if matches!(status.as_u16(), 429 | 500 | 502 | 503 | 529) && attempt < 4 {
                    tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
                    attempt += 1;
                    continue;
                }
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    let detail = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                        .unwrap_or_else(|| text.chars().take(300).collect());
                    return Err(format!("render request failed ({status}): {detail}"));
                }
                let v: Value =
                    serde_json::from_str(&text).map_err(|e| format!("bad response: {e}"))?;
                let out: String = v["content"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter(|b| b["type"] == "text")
                            .filter_map(|b| b["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default();
                if out.trim().is_empty() {
                    return Err("the model returned no text".into());
                }
                return Ok(out);
            }
        })
    }
}

// ── prompts ──────────────────────────────────────────────────────────────────────

pub const NOTE_SYSTEM: &str = "You write synthetic workplace notes for a retrieval-evaluation corpus. The notes must read as if a real person (or a tool, where stated) wrote them, not as a template: vary sentence length, tone, order and detail, and include the ordinary context a real notebook has.

Hard rules:
1. Every fact you are given must appear. The entity name and the value of each fact must appear VERBATIM: same spelling, same digits, same currency, percent and date formatting. You may reword the rest of the sentence freely.
2. Name no project, supplier, colleague or policy other than the ones you are given. Do not invent people or organisations; say 'the team', 'a vendor', 'someone in finance' instead.
3. Add no facts about the given entities beyond those provided: no extra figures, dates, prices, names or counts for them.
4. Output only the note as markdown. No preamble, no explanation, no code fences.";

pub const QUESTION_SYSTEM: &str = "You write the wording of evaluation questions for a note-search benchmark. Write each question the way a person would type it into a search box or ask an assistant: one sentence, natural, varied (do not start every question the same way). Answer with ONLY a JSON object mapping each question id to its wording. No commentary.";

fn kind_guidance(n: &SkNote) -> String {
    match n.kind.as_str() {
        "fact" => "Voice: a short journal-style working note by a team member about the entity, with ordinary surrounding chatter. In one natural sentence say what the entity is, using the given kind and domain words.".into(),
        "decision" => {
            let mut s = "Voice: a short decision record or update by a team member.".to_string();
            if n.explicit == Some(true) {
                s.push_str(" This revises an earlier choice: say plainly that it replaces the earlier choice, and state that the change takes effect on the given effective date, written exactly as given. Do not name the earlier choice.");
            } else if n.earlier_values.is_empty() {
                s.push_str(" This is the first choice recorded for the topic.");
            } else {
                s.push_str(" State the new setting as simply how things are now. Do NOT say it replaces, supersedes or changes an earlier choice, and do not mention any earlier value.");
            }
            s
        }
        "recap" => "Voice: a brief standup recap (bullets or short prose) that mentions the fact in passing among routine updates.".into(),
        "clip" if n.facts.is_empty() => "Voice: an excerpt from a clipped web article by an external publication, journalistic and impersonal. Begin with an italic line `_Clipped from the web. Source: example.org/<slug>._`. Name no company or product.".into(),
        "clip" => "Voice: an excerpt from a clipped web article by an external publication, journalistic and impersonal, which reports the given fact about the named supplier in passing. Begin with an italic line `_Clipped from the web. Source: example.org/<slug>._`.".into(),
        "wiki" if n.topic.is_some() => "Voice: an agent-maintained summary page: neutral, tidy, a short heading, the current setting stated plainly, and a final line `Sources: [[id]]` citing exactly the given source id.".into(),
        "wiki" => "Voice: an agent-maintained summary page: neutral, tidy, a heading, one sentence saying what the entity is, a bullet list with one bullet per given fact (the value must appear), and a final line `Sources: ` followed by the given source ids each written `[[id]]`, comma-separated.".into(),
        "answer" => "Voice: a filed question-and-answer note saved by an assistant: a heading, `**Question:**` (a user's request to be reminded of the fact, naming the entity), `**Answer:**` (states the fact), and a closing citation `(from [[id]])` of the given source id.".into(),
        _ => "Voice: a long-running team log of weekly notes: meetings, chores, plans and small decisions, written by different people over time.".into(),
    }
}

fn facts_block(n: &SkNote, only_section: Option<usize>) -> String {
    let mut s = String::new();
    for f in &n.facts {
        if only_section.is_some() && f.section != only_section {
            continue;
        }
        s.push_str(&format!(
            "- entity: {} | attribute: {} | value (verbatim): {} | meaning: {}\n",
            f.entity, f.attribute, f.value, f.statement
        ));
    }
    if s.is_empty() {
        s.push_str("(none)\n");
    }
    s
}

fn entities_block(n: &SkNote) -> String {
    let mut s = String::new();
    for e in &n.entities {
        s.push_str(&format!(
            "- {} — the {} responsible for {}\n",
            e.name, e.kind_note, e.domain_note
        ));
    }
    if s.is_empty() {
        s.push_str("(none: name no project, supplier, colleague or policy at all)\n");
    }
    s
}

/// Prompt for a note rendered in one call (or one section of a long note).
pub fn note_prompt(n: &SkNote, section: Option<usize>) -> (String, u32) {
    let target = match section {
        Some(_) => n.target_words / n.sections.max(1),
        None => n.target_words,
    };
    let mut u = format!(
        "NOTE ID: {}\nKind: {}\nOrigin: {}\n",
        n.id, n.kind, n.origin
    );
    u.push_str(&format!("{}\n\n", kind_guidance(n)));
    u.push_str(&format!("Title: {}\n", n.title_hint));
    match section {
        Some(0) => u.push_str(&format!(
            "This is section 1 of {}. Begin with the heading line `# {}` then the section body.\n",
            n.sections, n.title_hint
        )),
        Some(s) => u.push_str(&format!(
            "This is section {} of {}. Begin with a `## ` heading line (a short, plain heading) then the section body. Do not repeat the document title.\n",
            s + 1,
            n.sections
        )),
        None => u.push_str("Begin with the title as a `# ` heading line.\n"),
    }
    u.push_str(&format!(
        "Target length: about {target} words (stay within 25%).\n\n"
    ));
    u.push_str("Entities this note may name (and no others):\n");
    u.push_str(&entities_block(n));
    u.push_str("\nFacts that must appear (entity name and value verbatim):\n");
    u.push_str(&facts_block(n, section));
    if let Some(t) = &n.topic {
        u.push_str(&format!("\nDecision topic: {t}\n"));
    }
    if let Some(e) = &n.effective_text {
        u.push_str(&format!("Effective date (write exactly): {e}\n"));
    }
    if !n.cites.is_empty() {
        u.push_str(&format!(
            "Source ids to cite, each exactly as `[[id]]`: {}\n",
            n.cites.join(", ")
        ));
    }
    if n.sections > 1 {
        u.push_str("\nAround the facts write ordinary workplace notes (meetings, chores, plans, small decisions). Embed each fact inside a normal paragraph; do not highlight it. Name no one else.\n");
    }
    let max = ((target as f64) * 1.9) as u32 + 400;
    (u, max.min(8000))
}

fn forbidden_words(q: &SkQuestion) -> Vec<String> {
    let mut set = BTreeSet::new();
    for b in &q.basis {
        set.extend(content_words(b));
    }
    set.into_iter().collect()
}

/// Prompt for a batch of questions of one category.
pub fn questions_prompt(batch: &[&SkQuestion]) -> (String, u32) {
    let mut u = String::from("Write the wording for each question below.\n\n");
    for q in batch {
        u.push_str(&format!(
            "id: {}\ncategory: {}\nasks: {}\n",
            q.id, q.category, q.intent
        ));
        u.push_str(&format!("plain wording to improve on: {}\n", q.text_hint));
        match q.category.as_str() {
            "paraphrase" => {
                u.push_str("RULES: do NOT name the entity. Refer to it by role instead");
                if let Some(d) = &q.descriptor {
                    u.push_str(&format!(" (e.g. \"{d}\")"));
                }
                u.push_str(
                    ", and ask for the same thing in different words than the plain wording.",
                );
                let fw = forbidden_words(q);
                if !fw.is_empty() {
                    u.push_str(&format!(
                        " Do not use any of these words: {}.",
                        fw.join(", ")
                    ));
                }
                u.push('\n');
            }
            _ => {
                if !q.names.is_empty() {
                    u.push_str(&format!(
                        "RULES: the question must contain {} written exactly as shown.\n",
                        q.names
                            .iter()
                            .map(|n| format!("\"{n}\""))
                            .collect::<Vec<_>>()
                            .join(" and ")
                    ));
                }
                if q.negative {
                    u.push_str(
                        "The thing asked about does not exist in the notes; do not hint at that.\n",
                    );
                }
            }
        }
        u.push('\n');
    }
    u.push_str("Answer with a single JSON object: {\"<id>\": \"<question wording>\", ...}");
    (u, 400 + 120 * batch.len() as u32)
}

/// Extracts the `{id: wording}` object from a model reply.
pub fn parse_questions_reply(reply: &str) -> Result<BTreeMap<String, String>, String> {
    let start = reply.find('{').ok_or("reply has no JSON object")?;
    let end = reply.rfind('}').ok_or("reply has no JSON object")?;
    let v: Value =
        serde_json::from_str(&reply[start..=end]).map_err(|e| format!("reply is not JSON: {e}"))?;
    let obj = v.as_object().ok_or("reply is not a JSON object")?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            out.insert(k.clone(), s.trim().to_string());
        }
    }
    Ok(out)
}

/// Removes a surrounding code fence and any text before the first heading.
pub fn clean_note(reply: &str) -> String {
    let mut t = reply.trim().to_string();
    if t.starts_with("```") {
        t = t
            .trim_start_matches("```markdown")
            .trim_start_matches("```md")
            .trim_start_matches("```")
            .trim()
            .to_string();
        if let Some(i) = t.rfind("```") {
            t.truncate(i);
        }
    }
    if let Some(i) = t.find('#') {
        if i > 0 && !t[..i].contains("_Clipped") {
            t = t[i..].to_string();
        }
    }
    t.trim().to_string()
}

// ── the render run ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Options {
    pub concurrency: usize,
    /// Keep notes already present in `<dir>/notes` (resume a crashed run).
    pub resume: bool,
    /// Re-render only these note or question ids; everything else is kept as it is.
    pub only: Option<BTreeSet<String>>,
}

/// Work to do: how many model calls a run would make.
pub fn plan_calls(sk: &Skeleton, existing: &Rendered, o: &Options) -> (usize, usize) {
    let wanted = |id: &str, have: bool| match &o.only {
        Some(s) => s.contains(id),
        None => !(o.resume && have),
    };
    let mut note_calls = 0;
    for n in &sk.notes {
        if wanted(&n.id, existing.notes.contains_key(&n.id)) {
            note_calls += n.sections.max(1);
        }
    }
    let mut by_cat: BTreeMap<&str, usize> = BTreeMap::new();
    for q in &sk.questions {
        if wanted(&q.id, existing.questions.contains_key(&q.id)) {
            *by_cat.entry(q.category.as_str()).or_default() += 1;
        }
    }
    let q_calls = by_cat.values().map(|n| n.div_ceil(QUESTION_BATCH)).sum();
    (note_calls, q_calls)
}

pub const QUESTION_BATCH: usize = 8;

async fn render_note(model: &dyn TextModel, n: &SkNote) -> Result<String, String> {
    let mut parts = Vec::new();
    for s in 0..n.sections.max(1) {
        let (user, max) = note_prompt(n, (n.sections > 1).then_some(s));
        let reply = model
            .complete(NOTE_SYSTEM, &user, max)
            .await
            .map_err(|e| format!("note '{}' section {}: {e}", n.id, s + 1))?;
        parts.push(clean_note(&reply));
    }
    Ok(parts.join("\n\n"))
}

/// Renders everything the options select. `on_note` is called with each finished note so a crash
/// loses nothing (`--resume`).
pub async fn render_all(
    sk: &Skeleton,
    model: &dyn TextModel,
    existing: Rendered,
    o: &Options,
    on_note: &(dyn Fn(&str, &str) + Sync),
) -> Result<(Rendered, usize), String> {
    let mut out = existing;
    let wanted = |id: &str, have: bool| match &o.only {
        Some(s) => s.contains(id),
        None => !(o.resume && have),
    };
    let todo: Vec<&SkNote> = sk
        .notes
        .iter()
        .filter(|n| wanted(&n.id, out.notes.contains_key(&n.id)))
        .collect();
    let mut calls = 0usize;
    let results: Vec<Result<(String, String, usize), String>> = stream::iter(todo)
        .map(|n| async move {
            let body = render_note(model, n).await?;
            on_note(&n.id, &body);
            Ok::<_, String>((n.id.clone(), body, n.sections.max(1)))
        })
        .buffer_unordered(o.concurrency.max(1))
        .collect()
        .await;
    let mut failures = Vec::new();
    for r in results {
        match r {
            Ok((id, body, c)) => {
                out.notes.insert(id, body);
                calls += c;
            }
            Err(e) => failures.push(e),
        }
    }
    if !failures.is_empty() {
        return Err(format!(
            "{} note(s) failed (rerun with --resume to continue): {}",
            failures.len(),
            failures
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    // Questions, batched per category.
    let mut by_cat: BTreeMap<&str, Vec<&SkQuestion>> = BTreeMap::new();
    for q in &sk.questions {
        if wanted(&q.id, out.questions.contains_key(&q.id)) {
            by_cat.entry(q.category.as_str()).or_default().push(q);
        }
    }
    let batches: Vec<Vec<&SkQuestion>> = by_cat
        .into_values()
        .flat_map(|v| {
            v.chunks(QUESTION_BATCH)
                .map(|c| c.to_vec())
                .collect::<Vec<_>>()
        })
        .collect();
    let replies: Vec<Result<BTreeMap<String, String>, String>> = stream::iter(batches)
        .map(|b| async move {
            let (user, max) = questions_prompt(&b);
            let reply = model.complete(QUESTION_SYSTEM, &user, max).await?;
            let got = parse_questions_reply(&reply)?;
            if let Some(q) = b.iter().find(|q| !got.contains_key(&q.id)) {
                return Err(format!("the reply has no wording for question '{}'", q.id));
            }
            Ok(got)
        })
        .buffer_unordered(o.concurrency.max(1))
        .collect()
        .await;
    for r in replies {
        out.questions.extend(r?);
        calls += 1;
    }
    // Questions not selected keep their previous wording (an `--only` run).
    for q in &sk.questions {
        if !out.questions.contains_key(&q.id) {
            return Err(format!(
                "question '{}' has no wording; render it (no --only/--resume skip)",
                q.id
            ));
        }
    }
    Ok((out, calls))
}

// ── offline exchange: prompts out, replies in ────────────────────────────────────

/// Stable key for one model call: the reply file for this prompt is `<replies>/<key>.md`.
pub fn call_key(system: &str, user: &str) -> String {
    lcg_core::sha256_hex::sha256_hex(format!("{system}\u{0}{user}").as_bytes())[..16].to_string()
}

/// Every call a full render makes, in the order `render_all` makes them, as JSON lines
/// (`key`, `kind`, `ids`, `section`, `system`, `user`, `max_tokens`). Lets the replies be written
/// outside this process (for example by an agent with no API key) and read back with `--replies`.
pub fn export_calls(sk: &Skeleton) -> Vec<Value> {
    let mut out = Vec::new();
    for n in &sk.notes {
        for s in 0..n.sections.max(1) {
            let (user, max) = note_prompt(n, (n.sections > 1).then_some(s));
            out.push(
                json!({"key": call_key(NOTE_SYSTEM, &user), "kind": "note", "ids": [n.id],
                "section": s + 1, "sections": n.sections.max(1), "system": NOTE_SYSTEM,
                "user": user, "max_tokens": max}),
            );
        }
    }
    let mut by_cat: BTreeMap<&str, Vec<&SkQuestion>> = BTreeMap::new();
    for q in &sk.questions {
        by_cat.entry(q.category.as_str()).or_default().push(q);
    }
    for v in by_cat.into_values() {
        for b in v.chunks(QUESTION_BATCH) {
            let (user, max) = questions_prompt(b);
            let ids: Vec<&str> = b.iter().map(|q| q.id.as_str()).collect();
            out.push(
                json!({"key": call_key(QUESTION_SYSTEM, &user), "kind": "questions",
                "ids": ids, "system": QUESTION_SYSTEM, "user": user, "max_tokens": max}),
            );
        }
    }
    out
}

/// A [`TextModel`] that answers from reply files written ahead of time (`<dir>/<key>.md`).
pub struct FileText {
    dir: std::path::PathBuf,
    model: String,
}

impl FileText {
    pub fn new(dir: &Path, model: String) -> Self {
        Self {
            dir: dir.to_path_buf(),
            model,
        }
    }
}

impl TextModel for FileText {
    fn model(&self) -> &str {
        &self.model
    }

    fn complete<'a>(
        &'a self,
        system: &'a str,
        user: &'a str,
        _max_tokens: u32,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let p = self.dir.join(format!("{}.md", call_key(system, user)));
            let t = std::fs::read_to_string(&p)
                .map_err(|e| format!("no reply file {}: {e}", p.display()))?;
            if t.trim().is_empty() {
                return Err(format!("reply file {} is empty", p.display()));
            }
            Ok(t)
        })
    }
}

// ── CLI: retrieval-render ────────────────────────────────────────────────────────

pub const USAGE: &str = "lcg-eval retrieval-render — render the retrieval-v2 prose with an LLM (a deliberate, paid, maintainer step)

USAGE:
    lcg-eval retrieval-render --dir <FIXTURE DIR> [--model <MODEL>] [--concurrency <N>]
                              [--resume] [--only <ID,ID,...>] [--dry-run]

Reads <DIR>/skeleton.json (produced by `lcg-eval retrieval-gen`), has the model write every note and
every question's wording, then writes manifest.json, notes/*.md, questions.jsonl,
questions.meta.json and render.json (render model, prompt version, skeleton hash) into <DIR> and
runs the checks. The rendered output is meant to be checked in; CI and ordinary eval runs never call
a model. Needs ANTHROPIC_API_KEY. Roughly 600 model calls for the full corpus.

OPTIONS:
    --dir <DIR>          Fixture directory containing skeleton.json
    --model <MODEL>      Render model (default: claude-sonnet-4-6). Recorded in render.json.
    --concurrency <N>    Parallel requests (default 6)
    --resume             Keep notes already in <DIR>/notes (continue a crashed run)
    --only <IDS>         Re-render just these note or question ids (e.g. ones the checks flagged)
    --dry-run            Print how many model calls the run would make and exit (no key needed)
    --export-prompts <F> Write every call's prompt to F as JSON lines (key, system, user, ...) and exit
    --replies <DIR>      Read each reply from DIR/<key>.md instead of calling the API (no key needed);
                         --model then names the model that wrote the replies
    -h, --help           Print this help

Re-rendering changes the corpus: bump corpus_version and questions_version in scenario.json,
regenerate the skeleton, then render. review.json is only ever updated while it is still pending.
";

/// Fills `questions_hash`, `skeleton_hash`, `questions_version` and the sample's question text in a
/// **pending** review record, so a pending template always names the question set it was written
/// for. Verdicts are never touched. A reviewed record is a human's and is never modified; if the
/// questions changed under it, say so.
pub fn update_review(
    dir: &Path,
    questions_text: &str,
    sk_hash: &str,
    q_version: &str,
) -> Result<(), String> {
    let p = dir.join("review.json");
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Ok(());
    };
    let mut v: Value = serde_json::from_str(&text).map_err(|e| format!("review.json: {e}"))?;
    let hash = crate::retrieval::fixture::questions_hash_of(questions_text);
    if v["status"] == "reviewed" {
        if v["questions_hash"] != json!(hash) {
            return Err("review.json is marked reviewed but the questions changed: a human must re-review; the record was left untouched".into());
        }
        return Ok(());
    }
    v["questions_hash"] = json!(hash);
    v["skeleton_hash"] = json!(sk_hash);
    v["questions_version"] = json!(q_version);
    let texts: BTreeMap<String, String> = questions_text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|q| {
            Some((
                q["id"].as_str()?.to_string(),
                q["text"].as_str()?.to_string(),
            ))
        })
        .collect();
    if let Some(sample) = v["sample"].as_array_mut() {
        for s in sample {
            if let Some(t) = s["id"].as_str().and_then(|id| texts.get(id)) {
                s["text"] = json!(t);
            }
        }
    }
    let mut out = serde_json::to_string_pretty(&v).expect("serializable");
    out.push('\n');
    std::fs::write(&p, out).map_err(|e| format!("review.json: {e}"))
}

/// A partial run (`--resume` / `--only`) keeps most of the previously rendered prose, so it must use
/// the model and prompt version `render.json` already records; otherwise the untouched notes would
/// be mislabelled.
pub fn check_partial_run_provenance(
    prev: &RenderMeta,
    model: &str,
    skeleton_hash: &str,
) -> Result<(), String> {
    if prev.skeleton_hash != skeleton_hash {
        return Err(format!(
            "render.json was rendered from skeleton {} but skeleton.json is now {skeleton_hash}; the notes this partial run (--resume/--only) keeps would come from the old skeleton. Render the whole corpus again without --resume/--only",
            prev.skeleton_hash
        ));
    }
    if prev.render_model == model && prev.prompt_version == PROMPT_VERSION {
        return Ok(());
    }
    Err(format!(
        "render.json records model '{}' with prompt '{}', but this partial run (--resume/--only) would use '{model}' with '{PROMPT_VERSION}'; the untouched notes would be mislabelled. Re-run with --model {} (and the matching prompt version), or render the whole corpus again without --resume/--only",
        prev.render_model, prev.prompt_version, prev.render_model
    ))
}

/// Every `--only` id must be a note or question of the skeleton; a typo would otherwise make a
/// zero-call run that still rewrites `render.json`.
pub fn check_only_ids(sk: &Skeleton, only: &BTreeSet<String>) -> Result<(), String> {
    let known: BTreeSet<&str> = sk
        .notes
        .iter()
        .map(|n| n.id.as_str())
        .chain(sk.questions.iter().map(|q| q.id.as_str()))
        .collect();
    let unknown: Vec<&str> = only
        .iter()
        .map(String::as_str)
        .filter(|id| !known.contains(id))
        .collect();
    if only.is_empty() {
        return Err("--only needs at least one note or question id".into());
    }
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "--only names ids that are not in the skeleton: {}",
            unknown.join(", ")
        ))
    }
}

/// Removes the previously rendered notes and question wording ahead of a full render.
pub fn reset_for_full_run(dir: &Path) -> Result<(), String> {
    if let Ok(rd) = std::fs::read_dir(dir.join("notes")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "md") {
                std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            }
        }
    }
    match std::fs::remove_file(dir.join("questions.jsonl")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("questions.jsonl: {e}")),
    }
}

/// Written next to the notes while a render is unfinished; see `run`.
pub const PROGRESS_FILE: &str = "render.progress.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct Progress {
    render_model: String,
    prompt_version: String,
    skeleton_hash: String,
}

pub async fn run_cli(args: &[String]) -> i32 {
    match run(args).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lcg-eval retrieval-render: {e}");
            if e.starts_with("unknown")
                || e.contains("requires a value")
                || e.contains("ANTHROPIC_API_KEY")
            {
                2
            } else {
                1
            }
        }
    }
}

async fn run(args: &[String]) -> Result<i32, String> {
    let mut dir = None;
    let mut model = crate::cli::DEFAULT_JUDGE_MODEL.to_string();
    let mut o = Options {
        concurrency: 6,
        resume: false,
        only: None,
    };
    let mut dry = false;
    let mut export: Option<String> = None;
    let mut replies: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            "--resume" => o.resume = true,
            "--dry-run" => dry = true,
            flag @ ("--dir" | "--model" | "--concurrency" | "--only" | "--export-prompts"
            | "--replies") => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                match flag {
                    "--dir" => dir = Some(v.clone()),
                    "--model" => model = v.clone(),
                    "--export-prompts" => export = Some(v.clone()),
                    "--replies" => replies = Some(v.clone()),
                    "--concurrency" => {
                        o.concurrency = v
                            .parse()
                            .map_err(|_| "--concurrency must be an integer".to_string())?
                    }
                    _ => {
                        o.only = Some(
                            v.split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect(),
                        )
                    }
                }
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
        i += 1;
    }
    let dir = dir.ok_or_else(|| format!("--dir is required\n\n{USAGE}"))?;
    let dir = Path::new(&dir);
    let skeleton_text = std::fs::read_to_string(dir.join("skeleton.json")).map_err(|e| {
        format!(
            "cannot read {}: {e}; run `lcg-eval retrieval-gen` first",
            dir.join("skeleton.json").display()
        )
    })?;
    let sk = Skeleton::from_json(&skeleton_text)?;
    if let Some(only) = &o.only {
        check_only_ids(&sk, only)?;
    }
    if let Some(f) = &export {
        let lines: Vec<String> = export_calls(&sk).iter().map(|v| v.to_string()).collect();
        std::fs::write(f, lines.join("\n") + "\n").map_err(|e| format!("{f}: {e}"))?;
        println!("retrieval-render: wrote {} prompts to {f}", lines.len());
        return Ok(0);
    }
    let existing = if o.resume || o.only.is_some() {
        check::load_rendered_partial(dir)
    } else {
        Rendered::default()
    };
    // A partial run (--resume / --only) keeps most of the previously rendered prose, so what
    // describes that prose must be checked: render.progress.json while a run is unfinished (it is
    // written before the first model call, so a crash leaves it behind), else render.json. With
    // neither, kept notes have no recorded origin and the partial run is refused.
    let progress_path = dir.join(PROGRESS_FILE);
    let partial = o.resume || o.only.is_some();
    let new_hash = skeleton_hash(&skeleton_text);
    let previous: Option<RenderMeta> = if partial {
        if let Ok(t) = std::fs::read_to_string(&progress_path) {
            let p: Progress = serde_json::from_str(&t)
                .map_err(|e| format!("{PROGRESS_FILE} is unreadable: {e}"))?;
            Some(RenderMeta {
                render_model: p.render_model,
                prompt_version: p.prompt_version,
                rendered_on: String::new(),
                skeleton_hash: p.skeleton_hash,
                corpus_version: sk.corpus_version.clone(),
                questions_version: sk.questions_version.clone(),
                model_calls: 0,
            })
        } else {
            match std::fs::read_to_string(dir.join("render.json")) {
                Ok(t) => Some(
                    serde_json::from_str(&t)
                        .map_err(|e| format!("render.json is unreadable: {e}"))?,
                ),
                Err(_) if !existing.notes.is_empty() => {
                    return Err(format!(
                        "{} note(s) are already in notes/ but neither render.json nor {PROGRESS_FILE} records the model and skeleton that produced them; a partial run (--resume/--only) cannot vouch for them. Delete notes/ and render the whole corpus without --resume/--only",
                        existing.notes.len()
                    ));
                }
                Err(_) => None,
            }
        }
    } else {
        None
    };
    if let Some(prev) = &previous {
        check_partial_run_provenance(prev, &model, &new_hash)?;
    }
    // Calls already recorded by an earlier finished run; a crashed run's calls are not counted.
    let prior_calls = if partial {
        std::fs::read_to_string(dir.join("render.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<RenderMeta>(&t).ok())
            .map_or(0, |m| m.model_calls)
    } else {
        0
    };
    let (nc, qc) = plan_calls(&sk, &existing, &o);
    println!(
        "retrieval-render: {} notes and {} questions; this run makes {nc} note calls + {qc} question calls with {model}",
        sk.notes.len(),
        sk.questions.len()
    );
    if dry {
        return Ok(0);
    }
    let client: Box<dyn TextModel> = match &replies {
        Some(r) => Box::new(FileText::new(Path::new(r), model.clone())),
        None => {
            let key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
                "ANTHROPIC_API_KEY is not set: rendering calls a model and is a deliberate maintainer step".to_string()
            })?;
            Box::new(AnthropicText::new(key, model.clone()))
        }
    };
    std::fs::create_dir_all(dir.join("notes")).map_err(|e| e.to_string())?;
    if !partial {
        // A full run replaces everything, so nothing from an earlier skeleton may survive in
        // notes/ or questions.jsonl to be "resumed" under this run's provenance after a crash.
        reset_for_full_run(dir)?;
    }
    // Recorded before the first model call so a crashed run can be resumed only by the same
    // model, prompt and skeleton. Removed once render.json takes over.
    let progress = Progress {
        render_model: model.clone(),
        prompt_version: PROMPT_VERSION.to_string(),
        skeleton_hash: new_hash.clone(),
    };
    std::fs::write(
        &progress_path,
        serde_json::to_string_pretty(&progress).expect("serializable") + "\n",
    )
    .map_err(|e| format!("{PROGRESS_FILE}: {e}"))?;
    let notes_dir = dir.join("notes");
    let on_note = move |id: &str, body: &str| {
        let _ = std::fs::write(
            notes_dir.join(format!("{id}.md")),
            format!("{}\n", body.trim_end()),
        );
    };
    let (rendered, calls) = render_all(&sk, client.as_ref(), existing, &o, &on_note).await?;
    let meta = RenderMeta {
        render_model: model,
        prompt_version: PROMPT_VERSION.to_string(),
        rendered_on: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        skeleton_hash: skeleton_hash(&skeleton_text),
        corpus_version: sk.corpus_version.clone(),
        questions_version: sk.questions_version.clone(),
        model_calls: prior_calls + calls,
    };
    write_fixture(dir, &sk, &rendered, &meta)?;
    let _ = std::fs::remove_file(&progress_path);
    let questions_text =
        std::fs::read_to_string(dir.join("questions.jsonl")).map_err(|e| e.to_string())?;
    if let Err(e) = update_review(
        dir,
        &questions_text,
        &meta.skeleton_hash,
        &meta.questions_version,
    ) {
        eprintln!("lcg-eval retrieval-render: WARNING {e}");
    }
    let findings = check_dir(dir, &sk, &skeleton_text)?;
    eprint!("{}", check::describe(&findings));
    println!(
        "retrieval-render: wrote the fixture to {} ({calls} model calls this run, {} in total, {} error(s), {} warning(s))",
        dir.display(),
        meta.model_calls,
        findings.errors.len(),
        findings.warnings.len()
    );
    if findings.errors.is_empty() {
        println!("Next: read review.json and fill every verdict yourself; record the cassette (docs/retrieval-quality-evaluation.md).");
        Ok(0)
    } else {
        println!("Fix the errors above by re-rendering the named ids with --only (or adjust the prompts and bump the versions).");
        Ok(1)
    }
}
