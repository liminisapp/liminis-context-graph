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
//!   position of a multi-token name when it is the article `a` (`A Tale of Two Cities`); other
//!   leading letters (`C Programming`, `X Corp`) remain designators.
//!
//! On top of the set-inequality rule, a separate **short all-caps code** rule (issue #666)
//! covers digit-free distinct codes (`ACDS` vs `ACDM`) that the token sets above cannot see.
//! A *code token* is, in the **original-case** name (so it is read before lowercasing), a
//! whitespace-separated token that after trimming punctuation is 2–6 alphabetic characters, all
//! uppercase, and not a Roman numeral (`II`/`III` stay the documented ADR-0650 gap). Because an
//! acronym legitimately aliases its expansion (`IBM` / `International Business Machines`) and a
//! shout-case name its normal-case form, the rule is a **mutual-exclusion** test: it vetoes only
//! when *each* side has a code token the other lacks. `ACDS`/`ACDM`, `UK`/`USA` and
//! `US Army`/`UK Army` are vetoed; `IBM`/`IBM Corp`, `NASA`/`nasa` and `PROJECT AURORA DOCS`/
//! `Project Aurora` are not. A code against a name with no all-caps token (`Acme` vs `ACDM`)
//! is a known gap left to the embedding and the opt-in LLM check (ADR-0652).

use std::collections::BTreeSet;

use crate::prompts::{normalize_name, strip_control_chars};

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
        let leading_article = i == 0 && multi && trimmed == "a";
        if !initial && !leading_article {
            out.insert(trimmed.to_string());
        }
    }
    out
}

/// True for a well-formed Roman numeral (upper-case), e.g. `II`, `VIII`, `XL`.
fn is_roman_numeral(s: &str) -> bool {
    let mut rest = s;
    // Strict grammar: M{0,3}(CM|CD|D?C{0,3})(XC|XL|L?X{0,3})(IX|IV|V?I{0,3})
    for _ in 0..3 {
        rest = rest.strip_prefix('M').unwrap_or(rest);
    }
    rest = if let Some(r) = rest.strip_prefix("CM").or_else(|| rest.strip_prefix("CD")) {
        r
    } else {
        let r = rest.strip_prefix('D').unwrap_or(rest);
        strip_up_to(r, 'C', 3)
    };
    rest = if let Some(r) = rest.strip_prefix("XC").or_else(|| rest.strip_prefix("XL")) {
        r
    } else {
        let r = rest.strip_prefix('L').unwrap_or(rest);
        strip_up_to(r, 'X', 3)
    };
    rest = if let Some(r) = rest.strip_prefix("IX").or_else(|| rest.strip_prefix("IV")) {
        r
    } else {
        let r = rest.strip_prefix('V').unwrap_or(rest);
        strip_up_to(r, 'I', 3)
    };
    rest.is_empty()
}

fn strip_up_to(s: &str, c: char, max: usize) -> &str {
    let mut rest = s;
    for _ in 0..max {
        match rest.strip_prefix(c) {
            Some(r) => rest = r,
            None => break,
        }
    }
    rest
}

/// The short all-caps code tokens of `name` (see the module docs), lowercased.
fn code_tokens(name: &str) -> BTreeSet<String> {
    strip_control_chars(name)
        .split_whitespace()
        .map(trim_punct)
        .filter(|t| {
            let n = t.chars().count();
            (2..=6).contains(&n)
                && t.chars().all(|c| c.is_alphabetic() && c.is_uppercase())
                && !is_roman_numeral(t)
        })
        .map(str::to_lowercase)
        .collect()
}

/// True when each of `a` and `b` has a code token the other lacks.
fn code_mismatch(a: &str, b: &str) -> bool {
    let (ca, cb) = (code_tokens(a), code_tokens(b));
    !ca.is_subset(&cb) && !cb.is_subset(&ca)
}

/// True when `a` and `b` carry different distinguishing-token sets — including when only one
/// side has any — or distinct short all-caps codes, i.e. when they must not be merged on
/// embedding similarity alone.
pub fn identifier_mismatch(a: &str, b: &str) -> bool {
    distinguishing_tokens(a) != distinguishing_tokens(b) || code_mismatch(a, b)
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
    fn leading_letter_other_than_article_is_a_designator() {
        assert!(vetoed("C Programming", "D Programming"));
        assert!(vetoed("X Corp", "Y Corp"));
    }

    #[test]
    fn roman_numerals_beyond_i_are_a_known_gap() {
        // Documented in ADR-0650: multi-letter Roman numerals are not tokens.
        assert!(!vetoed("World War II", "World War III"));
        assert!(!vetoed("Henry VII", "Henry VIII"));
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

    #[test]
    fn distinct_short_codes_are_vetoed() {
        for (a, b) in [
            ("ACDS", "ACDM"),
            ("UK", "USA"),
            ("US Army", "UK Army"),
            ("ACDS Platform", "ACDM Platform"),
        ] {
            assert!(vetoed(a, b), "{a:?} / {b:?} should be vetoed");
        }
    }

    #[test]
    fn acronym_and_case_aliases_are_not_vetoed_by_the_code_rule() {
        for (a, b) in [
            ("IBM", "International Business Machines"),
            ("AWS", "Amazon Web Services"),
            ("IBM", "IBM Corp"),
            ("NASA", "nasa"),
            ("NASA", "Nasa"),
            ("PROJECT AURORA DOCS", "Project Aurora"),
            ("REST API", "RESTful API"),
            ("PostgreSQL", "Postgres"),
            ("New York", "New York City"),
            ("ACDS", "ACDS"),
        ] {
            assert!(!vetoed(a, b), "{a:?} / {b:?} should not be vetoed");
        }
    }

    #[test]
    fn code_rule_known_gaps_fall_back_to_the_embedding() {
        // A code against a name with no all-caps token, and over-long shout-case words.
        assert!(!vetoed("Acme", "ACDM"));
        assert!(!vetoed("ACDMXYZ", "ACDSXYZ"));
    }

    #[test]
    fn code_tokens_exclude_roman_numerals_and_caseless_scripts() {
        assert!(code_tokens("World War II").is_empty());
        assert!(code_tokens("Henry VIII").is_empty());
        assert!(code_tokens("東京").is_empty());
        let want: BTreeSet<String> = ["acds", "uk"].map(String::from).into();
        assert_eq!(code_tokens("(ACDS) and UK, a Co."), want);
    }
}
