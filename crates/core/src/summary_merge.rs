//! Pure helpers for entity-merge summary consolidation (issue #651, ADR-0651).
//!
//! When extraction merges an extracted entity into an existing one, the two summaries used to be
//! space-joined, so a frequently mentioned entity accreted one sentence per mention, contradictions
//! included. The merge now produces one bounded summary:
//!
//! 1. [`decide_consolidation`] decides whether any work is needed at all (an empty or
//!    already-contained incoming summary changes nothing and costs no LLM call).
//! 2. When it is, the extractor consolidates the two descriptions (prompt built by
//!    [`consolidation_prompts`]), and the result is bounded by [`cap_summary`].
//! 3. If the extractor is unavailable, fails, or returns nothing, [`fallback_merge`] produces a
//!    deterministic bounded merge that favours the most recent text.
//!
//! Nothing in here does I/O; the call sites live in `episode.rs` (Phase B, lock-free).

/// Hard upper bound, in `char`s, on any merged entity summary — extractor output and fallback
/// alike. Unbounded concatenation must not be reachable on any path (issue #651, FR-006/FR-007).
pub const MERGED_SUMMARY_CAP: usize = 600;

/// What a merge must do about the summary, decided before any extractor call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsolidationPlan {
    /// The incoming summary adds nothing (empty, or contained in the current one): keep the
    /// current summary unchanged — no extractor call, no re-embed.
    Keep,
    /// The current summary is empty: the incoming one is used directly (already capped).
    UseIncoming(String),
    /// Both sides carry text: ask the extractor to consolidate them.
    Consolidate,
}

pub fn decide_consolidation(current: &str, incoming: &str) -> ConsolidationPlan {
    let incoming = incoming.trim();
    if incoming.is_empty() || current.contains(incoming) {
        return ConsolidationPlan::Keep;
    }
    if current.trim().is_empty() {
        return ConsolidationPlan::UseIncoming(cap_summary(incoming));
    }
    ConsolidationPlan::Consolidate
}

/// Byte offsets just past each sentence terminator in `text`: a `.`, `!` or `?` followed by
/// whitespace or the end of the string. Requiring the following whitespace leaves decimals
/// (`3.5`) and dotted identifiers (`v1.2.3`) intact; abbreviations such as "e.g. " are not
/// special-cased, which only affects where a cut falls, never correctness.
fn sentence_ends(text: &str) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '.' | '!' | '?') && chars.peek().is_none_or(|&(_, n)| n.is_whitespace()) {
            ends.push(i + c.len_utf8());
        }
    }
    ends
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Bounds `text` to [`MERGED_SUMMARY_CAP`] chars, keeping its *lead* — the shape of a coherent
/// description, such as the extractor's consolidation. Cuts at the last sentence boundary that
/// fits; a first sentence longer than the cap is hard-truncated on a char boundary.
pub fn cap_summary(text: &str) -> String {
    cap_summary_to(text, MERGED_SUMMARY_CAP)
}

fn cap_summary_to(text: &str, cap: usize) -> String {
    let text = text.trim();
    if char_len(text) <= cap {
        return text.to_string();
    }
    let best = sentence_ends(text)
        .into_iter()
        .rev()
        .find(|&end| char_len(&text[..end]) <= cap);
    match best {
        Some(end) => text[..end].trim_end().to_string(),
        None => text
            .chars()
            .take(cap)
            .collect::<String>()
            .trim_end()
            .to_string(),
    }
}

/// Deterministic bounded merge used when no extractor result is available: `existing` followed
/// by `incoming`, then — if over the cap — whole *leading* sentences dropped until it fits, so
/// the most recent text survives. If even the final sentence alone exceeds the cap, its last
/// `cap` chars are kept (cut on a char boundary).
pub fn fallback_merge(existing: &str, incoming: &str) -> String {
    fallback_merge_to(existing, incoming, MERGED_SUMMARY_CAP)
}

fn fallback_merge_to(existing: &str, incoming: &str, cap: usize) -> String {
    let (existing, incoming) = (existing.trim(), incoming.trim());
    let joined = match (existing.is_empty(), incoming.is_empty()) {
        (true, _) => incoming.to_string(),
        (_, true) => existing.to_string(),
        _ => format!("{existing} {incoming}"),
    };
    if char_len(&joined) <= cap {
        return joined;
    }
    // Candidate start offsets: the beginning of each sentence after the first.
    for end in sentence_ends(&joined) {
        let rest = joined[end..].trim_start();
        if rest.is_empty() {
            break;
        }
        if char_len(rest) <= cap {
            return rest.to_string();
        }
    }
    let skip = char_len(&joined) - cap;
    joined
        .chars()
        .skip(skip)
        .collect::<String>()
        .trim_start()
        .to_string()
}

/// `(system, user)` prompts for the consolidation call (issue #651, FR-001).
pub fn consolidation_prompts(
    entity_name: &str,
    existing: &str,
    incoming: &str,
) -> (String, String) {
    let system = format!(
        "You maintain entity summaries in a knowledge graph. You are given the current summary \
         of an entity and a newly extracted description of the same entity. Merge them into ONE \
         concise, current description of the entity.\n\
         Rules:\n\
         - On conflict, prefer the most recent or most specific statement and drop the \
         superseded claim; never list both sides of a contradiction.\n\
         - Keep durable facts from both descriptions; drop repetition and per-source phrasing.\n\
         - Plain prose, no bullet points, at most {} characters.\n\
         - Respond with ONLY a single JSON object of the form {{\"summary\": \"...\"}}. No other \
         text, no markdown code fences.",
        MERGED_SUMMARY_CAP - 100
    );
    let user = format!(
        "Entity: {entity_name}\n\nCurrent summary:\n{existing}\n\nNew description:\n{incoming}\n\n\
         Respond with the JSON object."
    );
    (system, user)
}

/// Cleans a consolidation reply's summary text: trims whitespace, unwraps a *matching* pair of
/// surrounding quotes or backticks (a lone edge quote belongs to the prose, as in
/// `"Acme" is a company.`), and bounds it with [`cap_summary`]. Returns `None` for an empty reply
/// so the caller falls back deterministically.
pub fn finalize_consolidation(raw: &str) -> Option<String> {
    let mut trimmed = raw.trim();
    while let Some(inner) = ['"', '\'', '`'].iter().find_map(|&q| {
        trimmed
            .strip_prefix(q)
            .and_then(|r| r.strip_suffix(q))
            .filter(|inner| !inner.contains(q))
    }) {
        trimmed = inner.trim();
    }
    let capped = cap_summary(trimmed);
    (!capped.is_empty()).then_some(capped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_incoming_keeps_current() {
        assert_eq!(
            decide_consolidation("A thing.", ""),
            ConsolidationPlan::Keep
        );
        assert_eq!(
            decide_consolidation("A thing.", "  \n"),
            ConsolidationPlan::Keep
        );
    }

    #[test]
    fn contained_incoming_keeps_current() {
        assert_eq!(
            decide_consolidation("A thing. It is blue.", "It is blue."),
            ConsolidationPlan::Keep
        );
        assert_eq!(
            decide_consolidation("same", "same"),
            ConsolidationPlan::Keep
        );
    }

    #[test]
    fn empty_current_uses_incoming() {
        assert_eq!(
            decide_consolidation("", "Fresh text."),
            ConsolidationPlan::UseIncoming("Fresh text.".to_string())
        );
    }

    #[test]
    fn empty_current_incoming_is_capped() {
        let long = "word ".repeat(300);
        match decide_consolidation("", &long) {
            ConsolidationPlan::UseIncoming(s) => assert!(char_len(&s) <= MERGED_SUMMARY_CAP),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn differing_text_consolidates() {
        assert_eq!(
            decide_consolidation("It is blue.", "It is red."),
            ConsolidationPlan::Consolidate
        );
    }

    #[test]
    fn cap_summary_passes_short_text_through() {
        assert_eq!(cap_summary("  Short one.  "), "Short one.");
    }

    #[test]
    fn cap_summary_cuts_at_sentence_boundary() {
        let text = format!("{} {}", "First sentence.", "x".repeat(700));
        // The only boundary that fits is after "First sentence."
        assert_eq!(cap_summary(&text), "First sentence.");
    }

    #[test]
    fn cap_summary_hard_truncates_unbroken_text() {
        let text = "y".repeat(1000);
        assert_eq!(char_len(&cap_summary(&text)), MERGED_SUMMARY_CAP);
    }

    #[test]
    fn cap_summary_is_char_boundary_safe() {
        let text = "é".repeat(1000);
        let out = cap_summary(&text);
        assert_eq!(char_len(&out), MERGED_SUMMARY_CAP);
        let text = "日本語。".repeat(400);
        assert!(char_len(&cap_summary(&text)) <= MERGED_SUMMARY_CAP);
    }

    #[test]
    fn decimals_and_abbreviations_do_not_split_or_panic() {
        assert_eq!(sentence_ends("Version 3.5 is out"), Vec::<usize>::new());
        assert_eq!(sentence_ends("See e.g. this. Done"), vec![8, 14]);
        assert_eq!(sentence_ends("Ends here."), vec![10]);
        let _ = fallback_merge_to("v1.2.3 shipped. e.g. foo", "bar 1.5x. baz?", 12);
    }

    #[test]
    fn fallback_joins_when_within_cap() {
        assert_eq!(fallback_merge("One.", "Two."), "One. Two.");
        assert_eq!(fallback_merge("", "Two."), "Two.");
        assert_eq!(fallback_merge("One.", ""), "One.");
    }

    #[test]
    fn fallback_drops_leading_sentences_keeping_most_recent() {
        let out = fallback_merge_to("Alpha one. Beta two.", "Gamma three.", 25);
        assert_eq!(out, "Beta two. Gamma three.");
        let out = fallback_merge_to("Alpha one. Beta two.", "Gamma three.", 15);
        assert_eq!(out, "Gamma three.");
    }

    #[test]
    fn fallback_never_exceeds_cap() {
        let existing = "Some earlier statement about the thing. ".repeat(40);
        let out = fallback_merge(&existing, "The newest statement.");
        assert!(char_len(&out) <= MERGED_SUMMARY_CAP);
        assert!(out.ends_with("The newest statement."));
    }

    #[test]
    fn fallback_hard_truncates_single_overlong_sentence_from_the_tail() {
        let incoming = format!("{}END", "z".repeat(900));
        let out = fallback_merge("Old.", &incoming);
        assert_eq!(char_len(&out), MERGED_SUMMARY_CAP);
        assert!(out.ends_with("END"));
        let multibyte = "é".repeat(1000);
        assert_eq!(
            char_len(&fallback_merge("Old.", &multibyte)),
            MERGED_SUMMARY_CAP
        );
    }

    #[test]
    fn prompts_instruct_preference_for_recent_and_specific() {
        let (system, user) = consolidation_prompts("Widget", "Old text.", "New text.");
        assert!(system.contains("most recent or most specific"));
        assert!(system.contains("superseded"));
        assert!(
            user.contains("Widget") && user.contains("Old text.") && user.contains("New text.")
        );
    }

    #[test]
    fn finalize_trims_quotes_and_rejects_empty() {
        assert_eq!(
            finalize_consolidation("  \"Merged.\"  ").as_deref(),
            Some("Merged.")
        );
        assert_eq!(finalize_consolidation("   "), None);
        assert_eq!(finalize_consolidation("\"\""), None);
        // A lone or non-wrapping edge quote is prose, not a wrapper.
        assert_eq!(
            finalize_consolidation("\"Acme\" is a company.").as_deref(),
            Some("\"Acme\" is a company.")
        );
        assert_eq!(
            finalize_consolidation("\"a\" and \"b\"").as_deref(),
            Some("\"a\" and \"b\"")
        );
        let long = format!("Keep. {}", "w".repeat(900));
        assert_eq!(finalize_consolidation(&long).as_deref(), Some("Keep."));
    }
}
