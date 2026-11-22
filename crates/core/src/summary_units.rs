//! Summary units (issue #850, ADR-0850): one short LLM-written description per ingested episode,
//! stored beside the passages in the LCG-only `SummaryUnit` table and used **only** as an extra
//! retrieval candidate by `knowledge_search_passages` when a call sets `summaries: true`.
//!
//! - **Ingest** (opt-in, `LCG_SUMMARY_UNITS`, default off): `episode::add_episode_with_opts` calls
//!   [`crate::extractor::Extractor::summarize`] once per episode, cleans the reply with
//!   [`clean_summary`], embeds it with the passage embedder and writes one `SummaryUnit` row in
//!   the same WAL flush as the episode. With the switch off nothing here runs.
//! - **Retrieval**: a summary candidate travels through the first stage and the rerank head as a
//!   [`PassageResult`] whose `content` is the summary text and whose [`SummarySource`] names its
//!   episode. Every belief lookup keys on the episode ([`PassageResult::belief_key`]), so a
//!   superseded note's summary is demoted exactly like its passage. [`resolve_results`] then
//!   replaces each summary candidate with its own episode's passage, marked `via: "summary"`,
//!   removes duplicates and applies the `max_summary_slots` cap. Summary text never reaches a
//!   response.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::types::PassageResult;

/// Workspace switch that turns summary-unit generation on at ingest.
pub const SUMMARY_UNITS_ENV: &str = "LCG_SUMMARY_UNITS";

/// Summary candidates probed per summaries-on search (the primary query only). E14 (d) added the
/// top 10 summaries to the passage candidates; this mirrors it. A constant, not a request
/// parameter, to keep the API surface small.
pub const SUMMARY_CANDIDATES: usize = 10;

/// Default `max_summary_slots`: at most this many returned results may come in via a summary.
/// Protects multi-hop questions, which lost slots to summary hits in E14.
pub const DEFAULT_MAX_SUMMARY_SLOTS: usize = 3;

/// A body with fewer non-whitespace characters than this gets no summarize call: there is
/// nothing to describe, and an empty or degenerate note must not cost an LLM call.
pub const MIN_SUMMARY_BODY_CHARS: usize = 20;

/// The summarize prompt sees at most this many characters of the episode body. Chunks are far
/// shorter; this only bounds a very long `knowledge_add_episode` body.
pub const MAX_SUMMARY_INPUT_CHARS: usize = 24_000;

/// Upper bound on a stored summary's length, in characters. 3–6 sentences plus one is well under
/// this; it only guards against a runaway reply.
pub const MAX_SUMMARY_CHARS: usize = 2_000;

/// The provider's `max_tokens` for one summarize call.
pub const SUMMARIZE_MAX_TOKENS: u32 = 512;

/// One `SummaryUnit` row as written at ingest.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryUnitRow {
    pub uuid: String,
    pub group_id: String,
    /// The episode this summary describes.
    pub episode_uuid: String,
    /// The cleaned summary text: what is embedded, and what the reranker scores.
    pub content: String,
    pub content_embedding: Vec<f32>,
    pub created_at: String,
}

/// One nearest-neighbour hit from the `SummaryUnit` vector index.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryHit {
    pub uuid: String,
    pub episode_uuid: String,
    pub content: String,
    /// Cosine *distance* (`1 - similarity`), as the index returns it.
    pub distance: f64,
}

/// Where a summary candidate came from: its source episode. Never serialized, so a summary row
/// that escaped resolution could not even leak this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummarySource {
    /// The source episode's uuid: the key every belief lookup uses for this candidate.
    pub episode_uuid: String,
    /// The source episode's own passage text. A resolved result carries it, and its
    /// `[[wikilinks]]` are what the derived-below-cited rule reads (not the summary's).
    pub episode_content: String,
}

/// Parses an `LCG_SUMMARY_UNITS` value: trimmed, case-insensitive. `on`/`1`/`true` enable,
/// `off`/`0`/`false` disable, anything else is `None` (unrecognised).
pub fn parse_summary_units_switch(raw: &str) -> Option<bool> {
    crate::search::parse_belief_aware_switch(raw)
}

/// Whether summary-unit generation is on for this ingest: `LCG_SUMMARY_UNITS` holds an enabling
/// value. Unset, empty or a disabling value is off; an unrecognised value warns on stderr once per
/// process and is off. Read per ingest, so it needs no `AppState` field.
pub fn summary_units_enabled_from_env() -> bool {
    static WARNED: AtomicBool = AtomicBool::new(false);
    match std::env::var(SUMMARY_UNITS_ENV) {
        Ok(v) if !v.trim().is_empty() => match parse_summary_units_switch(&v) {
            Some(b) => b,
            None => {
                if !WARNED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "liminis-context-graph: {SUMMARY_UNITS_ENV}={v:?} is not one of \
                         off/0/false/on/1/true; summary units stay off"
                    );
                }
                false
            }
        },
        _ => false,
    }
}

/// Whether `body` is worth a summarize call (at least [`MIN_SUMMARY_BODY_CHARS`] non-whitespace
/// characters).
pub fn body_is_summarizable(body: &str) -> bool {
    body.chars().filter(|c| !c.is_whitespace()).count() >= MIN_SUMMARY_BODY_CHARS
}

/// Truncates `s` to at most `max` characters, on a character boundary.
fn truncate_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// The rendered summarize prompts `(system, user)` for `body`. Shared by every provider and by
/// the cassette key, so a wording change invalidates stale cassette records loudly.
///
/// The instruction is E14's: 3–6 plain sentences plus one key-concepts sentence, and no headings
/// or labels, because fixed labels ("Topics:", "Entities:") inflate similarity between unrelated
/// items. The reply is a JSON object because the OpenAI-compatible provider always requests JSON
/// output; a bare-prose reply is tolerated too ([`parse_summary_reply`]).
pub fn summarize_prompts(body: &str) -> (String, String) {
    let system = "You write short descriptions of notes for a search index. Given one note, \
         describe in your own words what it is about: its subject, purpose and main points, as \
         you would describe it to a colleague who has not read it.\n\
         Rules:\n\
         - Write 3 to 6 plain sentences, then one final sentence naming the note's key concepts.\n\
         - Plain prose only: no headings, no labels such as \"Summary:\", \"Topics:\" or \
         \"Entities:\", no bullet points, no markdown.\n\
         - Respond with ONLY a single JSON object of the form {\"summary\": \"...\"}. No other \
         text, no markdown code fences."
        .to_string();
    let user = format!(
        "Note:\n{}\n\nRespond with the JSON object.",
        truncate_chars(body, MAX_SUMMARY_INPUT_CHARS)
    );
    (system, user)
}

/// Labels a model sometimes puts in front of a summary line despite the instruction. Removed by
/// [`clean_summary`] so they never reach the embedded text (FR-003).
const SUMMARY_LABELS: &[&str] = &[
    "summary",
    "note summary",
    "description",
    "overview",
    "topics",
    "topic",
    "entities",
    "key concepts",
    "concepts",
    "keywords",
    "key words",
    "key points",
    "key terms",
    "tags",
    "themes",
];

/// Strips a leading known label (`Topics:`, `**Summary**:`) from `line`, case-insensitively.
fn strip_label(line: &str) -> &str {
    let unbolded = line.trim_start_matches(['*', '_']).trim_start();
    let Some(colon) = unbolded.find(':') else {
        return line;
    };
    let label = unbolded[..colon]
        .trim()
        .trim_end_matches(['*', '_'])
        .trim()
        .to_ascii_lowercase();
    if SUMMARY_LABELS.contains(&label.as_str()) {
        unbolded[colon + 1..]
            .trim_start_matches(['*', '_'])
            .trim_start()
    } else {
        line
    }
}

/// Cleans a model's summary text into the plain prose that is embedded and stored: drops code
/// fences, markdown heading markers, bullet markers and known labels, joins the lines with
/// single spaces and bounds the length to [`MAX_SUMMARY_CHARS`]. `None` when nothing is left.
pub fn clean_summary(raw: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        let line = line.trim_start_matches('#').trim_start();
        let line = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("• "))
            .unwrap_or(line)
            .trim();
        let line = strip_label(line).trim();
        // A line that was nothing but bold markup or a label carries no text.
        let line = line.trim_matches(['*', '_']).trim();
        let bare = line.trim_end_matches(':').trim().to_ascii_lowercase();
        if line.is_empty() || SUMMARY_LABELS.contains(&bare.as_str()) {
            continue;
        }
        parts.push(line.to_string());
    }
    let joined = parts.join(" ");
    let joined = joined.trim();
    let mut trimmed = joined;
    while let Some(inner) = ['"', '\''].iter().find_map(|&q| {
        trimmed
            .strip_prefix(q)
            .and_then(|r| r.strip_suffix(q))
            .filter(|inner| !inner.contains(q))
    }) {
        trimmed = inner.trim();
    }
    let bounded = truncate_chars(trimmed, MAX_SUMMARY_CHARS).trim_end();
    (!bounded.is_empty()).then(|| bounded.to_string())
}

/// Parses a summarize reply — `{"summary": "..."}` per the prompt, tolerating code fences and,
/// when the model ignored the JSON instruction, bare prose — into cleaned summary text. `None`
/// for an empty or unusable reply.
pub fn parse_summary_reply(content: &str) -> Option<String> {
    let json_str = crate::extractor::extract_json_block(content);
    let text = match serde_json::from_str::<serde_json::Value>(json_str) {
        Ok(v) => v["summary"].as_str().map(str::to_string),
        Err(_) if !content.contains('{') => Some(content.to_string()),
        Err(_) => None,
    };
    text.and_then(|t| clean_summary(&t))
}

/// Builds the final result list from the fully ordered candidates of a summaries-on search
/// (FR-012–FR-014):
///
/// - a summary candidate becomes **its own episode's passage** (`uuid` and `content` from the
///   episode), keeps the candidate's `score`, `similarity`, `rerank_score`, `belief` and
///   `matched_queries` so `score` stays in rank order, and is marked `via: "summary"`;
/// - a note (episode) appears at most once, at its best position; a later duplicate is skipped
///   and the next candidate fills the slot;
/// - at most `max_slots` results come in via a summary; further summary candidates are skipped;
/// - the walk stops at `num_results`.
///
/// Returns the results and how many came in via a summary.
pub fn resolve_results(
    candidates: Vec<PassageResult>,
    num_results: usize,
    max_slots: usize,
) -> (Vec<PassageResult>, usize) {
    let mut out: Vec<PassageResult> = Vec::with_capacity(num_results.min(candidates.len()));
    let mut seen: HashSet<String> = HashSet::new();
    let mut via_summary = 0usize;
    for mut c in candidates {
        if out.len() >= num_results {
            break;
        }
        match c.summary_of.take() {
            Some(src) => {
                if via_summary >= max_slots || seen.contains(&src.episode_uuid) {
                    continue;
                }
                seen.insert(src.episode_uuid.clone());
                c.uuid = src.episode_uuid;
                c.content = src.episode_content;
                c.via = Some("summary");
                via_summary += 1;
                out.push(c);
            }
            None => {
                if !seen.insert(c.uuid.clone()) {
                    continue;
                }
                out.push(c);
            }
        }
    }
    (out, via_summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passage(uuid: &str, score: f64) -> PassageResult {
        PassageResult {
            uuid: uuid.into(),
            name: uuid.into(),
            content: format!("passage {uuid}"),
            source_description: String::new(),
            group_id: "g".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            valid_at: None,
            score,
            attributes: "{}".into(),
            origin: "primary".into(),
            similarity: None,
            matched_queries: None,
            rerank_score: None,
            belief: None,
            source_file: None,
            chunk_id: None,
            via: None,
            summary_of: None,
        }
    }

    fn summary(uuid: &str, episode: &str, score: f64) -> PassageResult {
        let mut p = passage(uuid, score);
        p.content = format!("summary text of {episode}");
        p.summary_of = Some(SummarySource {
            episode_uuid: episode.into(),
            episode_content: format!("passage {episode}"),
        });
        p
    }

    fn ids(r: &[PassageResult]) -> Vec<&str> {
        r.iter().map(|p| p.uuid.as_str()).collect()
    }

    #[test]
    fn summary_hit_resolves_to_its_own_episode_passage_marked_via_summary() {
        let (out, n) = resolve_results(vec![summary("s1", "e1", 0.9), passage("e2", 0.8)], 10, 3);
        assert_eq!(ids(&out), vec!["e1", "e2"]);
        assert_eq!(n, 1);
        assert_eq!(out[0].via, Some("summary"));
        assert_eq!(out[0].content, "passage e1");
        assert_eq!(out[0].score, 0.9, "the summary candidate's score is kept");
        assert!(out[0].summary_of.is_none());
        assert_eq!(out[1].via, None);
        assert!(out.iter().all(|p| !p.content.contains("summary text")));
    }

    #[test]
    fn a_note_appears_once_at_its_better_position() {
        // Summary ranks first: the note holds slot 1 via the summary, its direct passage is skipped.
        let (out, _) = resolve_results(
            vec![
                summary("s1", "e1", 0.9),
                passage("e2", 0.8),
                passage("e1", 0.7),
                passage("e3", 0.6),
            ],
            3,
            3,
        );
        assert_eq!(ids(&out), vec!["e1", "e2", "e3"]);
        assert_eq!(out[0].via, Some("summary"));
        // Passage ranks first: the summary duplicate is skipped and does not count against the cap.
        let (out, n) = resolve_results(
            vec![
                passage("e1", 0.9),
                summary("s1", "e1", 0.8),
                passage("e2", 0.7),
            ],
            3,
            3,
        );
        assert_eq!(ids(&out), vec!["e1", "e2"]);
        assert_eq!(out[0].via, None);
        assert_eq!(n, 0);
    }

    #[test]
    fn slot_cap_holds_and_next_passages_fill_the_slots() {
        let cands = vec![
            summary("s1", "e1", 0.99),
            summary("s2", "e2", 0.98),
            summary("s3", "e3", 0.97),
            summary("s4", "e4", 0.96),
            summary("s5", "e5", 0.95),
            passage("p1", 0.5),
            passage("p2", 0.4),
            passage("p3", 0.3),
        ];
        for (cap, want_ids) in [
            (3usize, vec!["e1", "e2", "e3", "p1", "p2"]),
            (1, vec!["e1", "p1", "p2", "p3"]),
            (0, vec!["p1", "p2", "p3"]),
        ] {
            let (out, n) = resolve_results(cands.clone(), 5, cap);
            assert_eq!(ids(&out), want_ids, "cap {cap}");
            assert_eq!(n, cap.min(5));
            assert_eq!(out.iter().filter(|p| p.via.is_some()).count(), cap);
        }
    }

    #[test]
    fn stops_at_num_results() {
        let (out, _) = resolve_results(
            vec![passage("a", 0.9), passage("b", 0.8), summary("s", "c", 0.7)],
            2,
            3,
        );
        assert_eq!(ids(&out), vec!["a", "b"]);
    }

    #[test]
    fn without_summaries_it_is_plain_truncation() {
        let cands = vec![passage("a", 0.9), passage("b", 0.8), passage("c", 0.7)];
        let (out, n) = resolve_results(cands, 2, 3);
        assert_eq!(ids(&out), vec!["a", "b"]);
        assert_eq!(n, 0);
    }

    #[test]
    fn clean_summary_strips_headings_labels_and_bullets() {
        let raw = "## Summary\nSummary: The note describes a solar kiln.\n\
                   - It dries timber slowly.\n**Topics:** kilns, timber\nKey concepts: drying.";
        let out = clean_summary(raw).unwrap();
        assert_eq!(
            out,
            "The note describes a solar kiln. It dries timber slowly. kilns, timber drying."
        );
        for label in ["Summary:", "Topics:", "Key concepts:", "#", "**"] {
            assert!(!out.contains(label), "{label} survived in {out:?}");
        }
    }

    #[test]
    fn clean_summary_keeps_ordinary_colons_and_drops_fences() {
        let out = clean_summary("```\nThe plan: ship it in May.\n```").unwrap();
        assert_eq!(out, "The plan: ship it in May.");
    }

    #[test]
    fn clean_summary_empty_is_none() {
        assert_eq!(clean_summary(""), None);
        assert_eq!(clean_summary("   \n  "), None);
        assert_eq!(clean_summary("Summary:\n**Topics:**"), None);
        assert_eq!(clean_summary("\"\""), None);
    }

    #[test]
    fn clean_summary_bounds_length() {
        let out = clean_summary(&"word ".repeat(2_000)).unwrap();
        assert!(out.chars().count() <= MAX_SUMMARY_CHARS);
    }

    #[test]
    fn parse_summary_reply_accepts_json_fenced_and_prose() {
        assert_eq!(
            parse_summary_reply(r#"{"summary": "A note about kilns."}"#).as_deref(),
            Some("A note about kilns.")
        );
        assert_eq!(
            parse_summary_reply("```json\n{\"summary\": \"Fenced.\"}\n```").as_deref(),
            Some("Fenced.")
        );
        assert_eq!(
            parse_summary_reply("Just prose.").as_deref(),
            Some("Just prose.")
        );
        assert_eq!(parse_summary_reply(r#"{"summary": ""}"#), None);
        assert_eq!(parse_summary_reply(r#"{"other": "x"}"#), None);
        assert_eq!(parse_summary_reply(r#"{"summary": "unterminated"#), None);
    }

    #[test]
    fn switch_parsing() {
        for on in ["on", "ON", " 1 ", "true"] {
            assert_eq!(parse_summary_units_switch(on), Some(true));
        }
        for off in ["off", "0", "False"] {
            assert_eq!(parse_summary_units_switch(off), Some(false));
        }
        assert_eq!(parse_summary_units_switch("yes"), None);
    }

    #[test]
    fn summarizable_body_threshold() {
        assert!(!body_is_summarizable("   "));
        assert!(!body_is_summarizable("tiny note"));
        assert!(body_is_summarizable(
            "A note long enough to be worth describing."
        ));
    }

    #[test]
    fn prompts_carry_the_e14_instruction_and_bound_the_body() {
        let (system, user) = summarize_prompts("The body.");
        assert!(system.contains("3 to 6 plain sentences"));
        assert!(system.contains("key concepts"));
        assert!(user.contains("The body."));
        let long = "x".repeat(MAX_SUMMARY_INPUT_CHARS + 100);
        let (_, user) = summarize_prompts(&long);
        assert!(user.len() < long.len());
    }
}
