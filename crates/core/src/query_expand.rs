//! Opt-in LLM query expansion for passage search (issue #796).
//!
//! A local LLM rewrites the query into up to three variants — **lexical** (keywords),
//! **semantic** (a paraphrase) and **hyde** (a hypothetical answer passage). Each variant is
//! searched on its own and the ranked lists are fused with RRF alongside the original query's
//! list ([`crate::search`]). Expansion costs about a second per query, so it is never a default
//! and never runs unless a call asks for it.
//!
//! The LLM is reached over the same sidecar transport as the reranker
//! ([`crate::sidecar_client`]), via the OpenAI-compatible `/v1/chat/completions` route. Any
//! failure — endpoint down, timeout, empty or malformed output — surfaces as an `Err` or an empty
//! variant list and the caller falls back to the original query alone.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::{error::Error, sidecar_client::SidecarClient};

/// Route served by the sidecar (Apple Foundation Models) or any OpenAI-compatible endpoint.
pub const CHAT_ROUTE: &str = "/v1/chat/completions";

/// Maximum variants of each kind kept from the model's output.
const MAX_PER_KIND: usize = 1;
/// Variants longer than this are dropped (a runaway generation is not a query).
const MAX_VARIANT_CHARS: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantKind {
    Lexical,
    Semantic,
    Hyde,
}

impl VariantKind {
    pub fn as_str(self) -> &'static str {
        match self {
            VariantKind::Lexical => "lexical",
            VariantKind::Semantic => "semantic",
            VariantKind::Hyde => "hyde",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryVariant {
    pub kind: VariantKind,
    pub text: String,
}

/// Rewrites a query into variants. An empty `Ok` means "no usable variants" — the caller treats
/// it the same as an error and searches the original query alone.
pub trait QueryExpander: Send + Sync {
    fn expand<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Result<Vec<QueryVariant>, Error>>;
}

const SYSTEM_PROMPT: &str = "You rewrite search queries for a personal-notes search engine. \
Given a query, output exactly three lines and nothing else:\n\
lex: <3-6 distinctive keywords that would literally appear in a matching note>\n\
vec: <one sentence restating the query in different words, naming the likely entity or topic>\n\
hyde: <two sentences written as if they were the matching note's own text answering the query>\n\
Do not add commentary, quotes, numbering or markdown.";

/// Strips list markers, numbering and surrounding whitespace from one output line.
///
/// Numbering is removed only in the list form `\d+[.)]` followed by whitespace (`2. `, `3) `). A
/// leading number that is part of the text (`2021 budget`, `3.5 release notes`) is kept: it is
/// often the most discriminative token of an unlabelled line.
fn clean_line(raw: &str) -> &str {
    let s = raw.trim().trim_start_matches(['-', '*', '•', ' ', '\t']);
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let rest = &s[digits..];
        if let Some(after) = rest.strip_prefix(['.', ')']) {
            if after.starts_with(char::is_whitespace) {
                return after.trim_start();
            }
        }
    }
    s
}

fn clean_text(text: &str) -> &str {
    text.trim()
        .trim_matches('*')
        .trim()
        .trim_matches('"')
        .trim()
}

/// Accumulates variants under the shared rules: empty, overly long, or case-insensitively
/// duplicate (of the original query or an earlier variant) texts are dropped, and each kind is
/// capped at [`MAX_PER_KIND`].
struct VariantSet {
    out: Vec<QueryVariant>,
    seen: Vec<String>,
    counts: [usize; 3],
}

impl VariantSet {
    fn new(original_query: &str) -> Self {
        Self {
            out: Vec::new(),
            seen: vec![original_query.trim().to_lowercase()],
            counts: [0; 3],
        }
    }

    fn push(&mut self, kind: VariantKind, text: &str) {
        let text = clean_text(text);
        if text.is_empty() || text.chars().count() > MAX_VARIANT_CHARS {
            return;
        }
        let slot = match kind {
            VariantKind::Lexical => 0,
            VariantKind::Semantic => 1,
            VariantKind::Hyde => 2,
        };
        let key = text.to_lowercase();
        if self.counts[slot] >= MAX_PER_KIND || self.seen.contains(&key) {
            return;
        }
        self.counts[slot] += 1;
        self.seen.push(key);
        self.out.push(QueryVariant {
            kind,
            text: text.to_string(),
        });
    }
}

/// Parses the model's `lex:` / `vec:` / `hyde:` lines. Tolerant of case, list markers, bold
/// markers and surrounding whitespace; ignores every other line. Variants that are empty, overly
/// long, or identical (case-insensitively) to the original query or to an earlier variant are
/// dropped.
///
/// Apple Foundation Models often ignores the labels. When the reply holds **no** labelled line at
/// all and at least two non-empty lines, it is read positionally: line 1 is the lexical variant,
/// line 2 the semantic one, and the remaining lines (joined with spaces) the hypothetical answer,
/// under the same dedupe and length rules; a leading line that ends in `:` (a preamble such as
/// "Here are the variants:") is skipped first. A reply with some labels is never re-read positionally,
/// and a single unlabelled line is not treated as the three-part structure.
pub fn parse_variants(output: &str, original_query: &str) -> Vec<QueryVariant> {
    let mut set = VariantSet::new(original_query);
    let mut labelled = false;
    for raw in output.lines() {
        let Some((label, rest)) = clean_line(raw).split_once(':') else {
            continue;
        };
        let kind = match label.trim().trim_matches('*').to_ascii_lowercase().as_str() {
            "lex" | "lexical" => VariantKind::Lexical,
            "vec" | "semantic" | "sem" => VariantKind::Semantic,
            "hyde" => VariantKind::Hyde,
            _ => continue,
        };
        labelled = true;
        set.push(kind, rest);
    }
    if !labelled {
        let mut lines: Vec<&str> = output
            .lines()
            .map(clean_line)
            .filter(|l| !l.is_empty())
            .collect();
        // A chatty preamble ("Here are the variants:") is not the lexical variant.
        if lines.first().is_some_and(|l| l.ends_with(':')) {
            lines.remove(0);
        }
        if lines.len() >= 2 {
            set.push(VariantKind::Lexical, lines[0]);
            set.push(VariantKind::Semantic, lines[1]);
            if lines.len() > 2 {
                set.push(VariantKind::Hyde, &lines[2..].join(" "));
            }
        }
    }
    set.out
}

/// Running counts of expansion replies, so a model that ignores the output format shows up in the
/// service log instead of silently degrading every `expand: true` call to the first stage.
static EXPANSIONS: AtomicU64 = AtomicU64::new(0);
static EXPANSIONS_WITHOUT_VARIANTS: AtomicU64 = AtomicU64::new(0);

/// `(replies parsed, replies that yielded no usable variant)` since process start.
pub fn expansion_counts() -> (u64, u64) {
    (
        EXPANSIONS.load(Ordering::Relaxed),
        EXPANSIONS_WITHOUT_VARIANTS.load(Ordering::Relaxed),
    )
}

/// Parses a reply and records the outcome in the counters; logs every reply that yields nothing,
/// with the running rate.
fn parse_and_count(content: &str, original_query: &str) -> Vec<QueryVariant> {
    let variants = parse_variants(content, original_query);
    let total = EXPANSIONS.fetch_add(1, Ordering::Relaxed) + 1;
    if variants.is_empty() {
        let empty = EXPANSIONS_WITHOUT_VARIANTS.fetch_add(1, Ordering::Relaxed) + 1;
        eprintln!(
            "[liminis-context-graph] query expansion: reply had no usable variants \
             ({empty} of {total} replies so far); the call falls back to the original query"
        );
    }
    variants
}

/// [`QueryExpander`] over an OpenAI-compatible `/v1/chat/completions` endpoint.
pub struct OaiQueryExpander {
    client: Arc<SidecarClient>,
    model: String,
}

impl OaiQueryExpander {
    pub fn new(client: Arc<SidecarClient>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }
}

impl QueryExpander for OaiQueryExpander {
    fn expand<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Result<Vec<QueryVariant>, Error>> {
        Box::pin(async move {
            let body = json!({
                "model": self.model,
                "messages": [
                    {"role": "system", "content": SYSTEM_PROMPT},
                    {"role": "user", "content": query},
                ],
                "temperature": 0,
                "max_tokens": 256,
                "stream": false,
            });
            let resp: Value = self.client.post_json(CHAT_ROUTE, &body).await?;
            let content = resp["choices"][0]["message"]["content"]
                .as_str()
                .ok_or_else(|| {
                    Error::Ipc("expansion response missing choices[0].message.content".into())
                })?;
            Ok(parse_and_count(content, query))
        })
    }
}

/// Test double returning canned variants, or failing on demand.
pub struct MockQueryExpander {
    result: Result<Vec<QueryVariant>, String>,
    calls: std::sync::atomic::AtomicUsize,
}

impl MockQueryExpander {
    pub fn with_variants(variants: Vec<QueryVariant>) -> Self {
        Self {
            result: Ok(variants),
            calls: Default::default(),
        }
    }

    pub fn failing() -> Self {
        Self {
            result: Err("mock expander unavailable".to_string()),
            calls: Default::default(),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl QueryExpander for MockQueryExpander {
    fn expand<'a>(&'a self, _query: &'a str) -> BoxFuture<'a, Result<Vec<QueryVariant>, Error>> {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.result.clone().map_err(Error::Ipc)
        })
    }
}

#[cfg(test)]
mod tests {

    /// A leading year or version in an unlabelled line is text, not list numbering.
    #[test]
    fn positional_fallback_keeps_a_leading_number_that_is_part_of_the_text() {
        let v = parse_variants(
            "2021 budget approval, owner, deadline\nWhat was approved for the 2021 budget?\n1999 was the year it was approved.",
            "budget",
        );
        let text = |k| v.iter().find(|x| x.kind == k).map(|x| x.text.as_str());
        assert_eq!(
            text(VariantKind::Lexical),
            Some("2021 budget approval, owner, deadline")
        );
        assert_eq!(
            text(VariantKind::Hyde),
            Some("1999 was the year it was approved.")
        );
        let v = parse_variants("3.5 release notes\nWhat changed in version 3.5?", "changes");
        assert_eq!(v[0].text, "3.5 release notes");
    }

    /// A leading preamble line is not the lexical variant.
    #[test]
    fn positional_fallback_skips_a_leading_preamble_line() {
        let v = parse_variants(
            "Here are the variants:\nkw1, kw2\nWhat is X?\nX is a thing.",
            "q",
        );
        let text = |k| v.iter().find(|x| x.kind == k).map(|x| x.text.as_str());
        assert_eq!(text(VariantKind::Lexical), Some("kw1, kw2"));
        assert_eq!(text(VariantKind::Semantic), Some("What is X?"));
        assert_eq!(text(VariantKind::Hyde), Some("X is a thing."));
    }

    /// Real list numbering is still stripped.
    #[test]
    fn list_numbering_is_still_stripped() {
        let v = parse_variants(
            "1. budget approval\n2) What was approved?\n3. It was approved.",
            "q",
        );
        assert_eq!(v[0].text, "budget approval");
        assert_eq!(v[1].text, "What was approved?");
        assert_eq!(v[2].text, "It was approved.");
    }

    #[test]
    fn unlabelled_reply_is_read_positionally() {
        // Real Apple Foundation Models output (no `lex:` / `vec:` / `hyde:` labels).
        let out = "ownership, marlin core, entity, control, corporation, rights  \nWho owns the Marlin Core entity? The ownership details involve a corporation holding rights to the core.";
        let v = parse_variants(out, "Who owns Marlin Core?");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].kind, VariantKind::Lexical);
        assert_eq!(
            v[0].text,
            "ownership, marlin core, entity, control, corporation, rights"
        );
        assert_eq!(v[1].kind, VariantKind::Semantic);
        assert!(v[1].text.starts_with("Who owns the Marlin Core entity?"));
    }

    #[test]
    fn unlabelled_reply_with_three_parts_fills_hyde_from_the_rest() {
        let out = "sign-off, bypass, induction\nWho can override the new-hire induction rule?\nThe manager's sign-off allows bypassing the induction requirement.\nIt is recorded in the exceptions log.";
        let v = parse_variants(out, "q");
        assert_eq!(v.len(), 3);
        assert_eq!(v[2].kind, VariantKind::Hyde);
        assert_eq!(
            v[2].text,
            "The manager's sign-off allows bypassing the induction requirement. It is recorded in the exceptions log."
        );
    }

    #[test]
    fn labelled_reply_is_never_read_positionally() {
        let out = "lex: sign-off, bypass, induction, new hire, exception  \nvec: Who can override the new-hire induction rule?  \nhyde: The manager's sign-off allows bypassing the induction requirement.";
        let v = parse_variants(out, "q");
        assert_eq!(v.len(), 3);
        assert_eq!(
            v[0].text,
            "sign-off, bypass, induction, new hire, exception"
        );
        // A partly labelled reply keeps only its labelled lines.
        let partial = parse_variants("some preamble\nlex: alpha beta\nand a stray line", "q");
        assert_eq!(partial.len(), 1);
        assert_eq!(partial[0].kind, VariantKind::Lexical);
    }

    #[test]
    fn a_single_unlabelled_line_yields_nothing() {
        assert!(parse_variants("I cannot help with that.", "q").is_empty());
        assert!(parse_variants("", "q").is_empty());
    }

    #[test]
    fn positional_fallback_applies_the_same_dedupe_rule() {
        let v = parse_variants(
            "who owns marlin core?\nWho owns Marlin Core?",
            "Who owns Marlin Core?",
        );
        assert!(v.is_empty());
    }

    #[test]
    fn counters_record_replies_without_variants() {
        let (t0, e0) = expansion_counts();
        assert!(parse_and_count("nothing useful", "q").is_empty());
        assert_eq!(
            parse_and_count("a b\nsome different words here", "q").len(),
            2
        );
        let (t1, e1) = expansion_counts();
        // Other tests may bump the counters concurrently, so assert lower bounds.
        assert!(t1 >= t0 + 2);
        assert!(e1 > e0);
    }
    use super::*;

    #[test]
    fn parses_the_three_labelled_lines() {
        let out = "lex: shift rostering initiative\nvec: the project about scheduling staff shifts\nhyde: The rostering project automates shift assignment. It launched in Q3.";
        let v = parse_variants(out, "initiative covering shift rostering");
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].kind, VariantKind::Lexical);
        assert_eq!(v[0].text, "shift rostering initiative");
        assert_eq!(v[1].kind, VariantKind::Semantic);
        assert_eq!(v[2].kind, VariantKind::Hyde);
    }

    #[test]
    fn tolerates_case_markers_and_noise() {
        let out = "Sure! Here you go:\n- **LEX:** alpha beta\n2. Vec: \"gamma delta\"\n  HyDE:   epsilon  \nthanks";
        let v = parse_variants(out, "q");
        assert_eq!(
            v.iter()
                .map(|x| (x.kind, x.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (VariantKind::Lexical, "alpha beta"),
                (VariantKind::Semantic, "gamma delta"),
                (VariantKind::Hyde, "epsilon"),
            ]
        );
    }

    #[test]
    fn malformed_or_empty_output_yields_no_variants() {
        assert!(parse_variants("", "q").is_empty());
        assert!(parse_variants("I cannot help with that.", "q").is_empty());
        assert!(parse_variants("lex:\nvec:   \nhyde:", "q").is_empty());
        assert!(parse_variants("unknown: thing", "q").is_empty());
    }

    #[test]
    fn drops_duplicates_of_the_original_and_of_each_other() {
        let v = parse_variants(
            "lex: Hello World\nvec: hello world\nhyde: other text",
            "hello world",
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].kind, VariantKind::Hyde);
    }

    #[test]
    fn keeps_only_the_first_variant_per_kind() {
        let v = parse_variants("lex: one\nlex: two\nvec: three", "q");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].text, "one");
    }

    #[test]
    fn drops_runaway_variants() {
        let long = "x".repeat(MAX_VARIANT_CHARS + 1);
        assert!(parse_variants(&format!("hyde: {long}"), "q").is_empty());
    }

    #[tokio::test]
    async fn mock_expander_returns_canned_or_fails() {
        let m = MockQueryExpander::with_variants(vec![QueryVariant {
            kind: VariantKind::Lexical,
            text: "k".into(),
        }]);
        assert_eq!(m.expand("q").await.unwrap().len(), 1);
        assert_eq!(m.calls(), 1);
        assert!(MockQueryExpander::failing().expand("q").await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn oai_expander_round_trips_over_uds() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let path = std::env::temp_dir()
            .join(format!("lcg-qe-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16384];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    assert!(req.starts_with("POST /v1/chat/completions"), "{req}");
                    let content = "lex: alpha\\nvec: beta gamma\\nhyde: delta epsilon";
                    let body = format!(
                        r#"{{"choices":[{{"message":{{"role":"assistant","content":"{content}"}}}}]}}"#
                    );
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        let client = Arc::new(SidecarClient::new_uds(
            path.clone(),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(2),
        ));
        let e = OaiQueryExpander::new(client, "apple-foundation");
        let v = e.expand("some query").await.unwrap();
        assert_eq!(v.len(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn oai_expander_errors_when_endpoint_is_down() {
        let client = Arc::new(SidecarClient::new_uds(
            "/nonexistent/lcg-llm.sock",
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        ));
        let e = OaiQueryExpander::new(client, "m");
        assert!(e.expand("q").await.is_err());
    }
}
