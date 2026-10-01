//! Deterministic identifier-mismatch veto for extraction-time entity dedup (issue #650,
//! ADR-0650).
//!
//! Name embeddings treat numbers and identifiers as near-noise, so `ADR 2018` / `ADR 2019`
//! score a cosine well above `DEDUP_THRESHOLD`. This module is the pure predicate that stops
//! such look-alikes merging: two names whose *distinguishing-token* sets differ are never
//! duplicates. It performs no I/O and is called only from extraction (`episode.rs` and the
//! name-aware dedup candidate selection in `db.rs`) — never from the exact-name path or
//! `knowledge_merge_entities`.
//!
//! Distinguishing tokens, per whitespace-separated token of the [`normalize_name`]d name:
//!
//! - **Digit-bearing**: a leading `#` and surrounding punctuation are stripped, the token is
//!   split on `-`/`–`/`—`, and each piece containing a numeric character is kept (`0.15.0`,
//!   `#611` → `611`, `COVID-19` → `19`, `GPT-4` and `GPT 4` agree).
//! - **Standalone single-letter designators** (`Phase A`, `Plan B`): a token that is exactly one
//!   letter, except trailing-period initials (`Brett A.`) and a single-letter token in first
//!   position of a multi-token name (the article in `A Tale of Two Cities`).

use std::collections::BTreeSet;

use crate::prompts::normalize_name;

fn is_dash(c: char) -> bool {
    matches!(c, '-' | '–' | '—')
}

fn trim_punct(s: &str) -> &str {
    s.trim_matches(|c: char| !c.is_alphanumeric())
}

/// The set of distinguishing tokens of `name` (see the module docs).
pub fn distinguishing_tokens(name: &str) -> BTreeSet<String> {
    let normalized = normalize_name(name);
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    let multi = tokens.len() > 1;
    let mut out = BTreeSet::new();
    for (i, raw) in tokens.iter().enumerate() {
        let stripped = raw.strip_prefix('#').unwrap_or(raw);
        let trimmed = trim_punct(stripped);
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().any(char::is_numeric) {
            for piece in trimmed.split(is_dash) {
                let piece = trim_punct(piece);
                if piece.chars().any(char::is_numeric) {
                    out.insert(piece.to_string());
                }
            }
            continue;
        }
        let mut chars = trimmed.chars();
        let single_letter =
            matches!((chars.next(), chars.next()), (Some(c), None) if c.is_alphabetic());
        if !single_letter {
            continue;
        }
        let initial = raw.ends_with('.');
        let leading_article = i == 0 && multi;
        if !initial && !leading_article {
            out.insert(trimmed.to_string());
        }
    }
    out
}

/// True when `a` and `b` carry different distinguishing-token sets — including when only one
/// side has any — i.e. when they must not be merged on embedding similarity alone.
pub fn identifier_mismatch(a: &str, b: &str) -> bool {
    distinguishing_tokens(a) != distinguishing_tokens(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vetoed(a: &str, b: &str) -> bool {
        let r = identifier_mismatch(a, b);
        assert_eq!(r, identifier_mismatch(b, a), "veto must be symmetric");
        r
    }

    #[test]
    fn distinct_pairs_from_issue_are_vetoed() {
        for (a, b) in [
            ("ADR 2018", "ADR 2019"),
            ("lcg 0.15.0", "lcg 0.16.2"),
            ("RFC 9110", "RFC 9111"),
            ("Q3 2025 roadmap", "Q4 2025 roadmap"),
            ("issue #611", "issue #612"),
            ("Project Aurora", "Project Aurora v2"),
            ("Phase A", "Phase B"),
            ("Python 3", "Python"),
            ("Plan B", "Plan"),
            ("World War I", "World War II"),
            ("Vitamin C", "Vitamin D"),
        ] {
            assert!(vetoed(a, b), "{a:?} / {b:?} should be vetoed");
        }
    }

    #[test]
    fn aliases_without_identifier_differences_are_not_vetoed() {
        for (a, b) in [
            ("PostgreSQL", "Postgres"),
            ("New York", "New York City"),
            ("2019 ADR", "ADR 2019"),
            ("3M", "3M Company"),
            ("GPT-4", "GPT 4"),
            ("ADR-2018", "ADR 2018"),
            ("#611", "611"),
            ("ADR 2018", "adr 2018"),
        ] {
            assert!(!vetoed(a, b), "{a:?} / {b:?} should not be vetoed");
        }
    }

    #[test]
    fn trailing_period_initials_and_leading_article_are_exempt() {
        assert!(!vetoed("Brett Adamson", "Brett A."));
        assert!(!vetoed("Alice B. Smith", "Alice Smith"));
        assert!(!vetoed("A Tale of Two Cities", "Tale of Two Cities"));
        // Known gap (ADR-0650): distinct initials are both exempt, so they are not vetoed.
        assert!(!vetoed("Brett A.", "Brett B."));
    }

    #[test]
    fn hyphenated_words_are_not_single_letters() {
        assert!(distinguishing_tokens("E-mail").is_empty());
        assert!(distinguishing_tokens("T-shirt").is_empty());
    }

    #[test]
    fn token_extraction() {
        let t = distinguishing_tokens("COVID-19 and lcg 0.15.0 (#611)");
        let want: BTreeSet<String> = ["19", "0.15.0", "611"].map(String::from).into();
        assert_eq!(t, want);
        let t = distinguishing_tokens("Phase A");
        assert_eq!(t, BTreeSet::from(["a".to_string()]));
    }

    #[test]
    fn conservative_known_gaps_are_vetoed() {
        assert!(vetoed("Web 2.0", "Web2.0"));
        assert!(vetoed("K8s", "K8s cluster 2"));
    }

    #[test]
    fn cjk_digit_difference_is_vetoed() {
        assert!(vetoed("第2章", "第3章"));
        assert!(!vetoed("東京", "東京都"));
    }
}
