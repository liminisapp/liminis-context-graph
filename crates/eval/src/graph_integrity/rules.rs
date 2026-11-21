//! The matching rules of `lcg-eval graph-integrity` (#819), each defined once (FR-016).
//!
//! Everything that decides "does this string match that string" lives here and is documented in
//! `docs/graph-integrity-evaluation.md`. No fuzzy matching, no aliases, no substring name matching.

use std::collections::HashSet;

/// Normalisation used for every comparison: lowercase, `_` and `-` read as spaces, runs of
/// whitespace collapsed to one space, ends trimmed. So `Cedar_Core`, `cedar  core` and
/// `CEDAR-CORE` are the same name, and `OWNED_BY` equals `owned by`.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for ch in s.chars() {
        if ch == '_' || ch == '-' || ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        for lc in ch.to_lowercase() {
            out.push(lc);
        }
    }
    out
}

/// Name match: equality of normalised names. Never substring.
pub fn names_match(a: &str, b: &str) -> bool {
    normalize(a) == normalize(b)
}

/// True when `needle` (already normalised, non-empty) occurs in `haystack` (already normalised) at
/// alphanumeric word boundaries, so `6` does not match inside `96` but `$4,200` matches in
/// `monthly budget of $4,200.`. Boundary is judged only on the side(s) where the needle itself
/// starts/ends with an alphanumeric character.
pub fn contains_bounded(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let first_alnum = needle.chars().next().is_some_and(|c| c.is_alphanumeric());
    let last_alnum = needle
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric());
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let at = start + pos;
        let end = at + needle.len();
        let before_ok = !first_alnum
            || haystack[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = !last_alnum
            || haystack[end..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        // advance by one char to keep slicing on a char boundary
        start = at + haystack[at..].chars().next().map_or(1, |c| c.len_utf8());
    }
    false
}

/// Type-label match: some label (other than the implicit `Entity` root) equals, case-insensitively,
/// one of the expected node types. `labels` carries the type plus its ancestors.
pub fn type_matches(labels: &[String], expected: &[String]) -> bool {
    labels.iter().any(|l| {
        !l.eq_ignore_ascii_case("Entity") && expected.iter().any(|e| e.eq_ignore_ascii_case(l))
    })
}

/// Effective edge type: the stored `relation_type` when non-empty, else the edge `name`. Compared
/// through [`normalize`].
pub fn effective_edge_type<'a>(relation_type: Option<&'a str>, name: &'a str) -> &'a str {
    match relation_type {
        Some(t) if !t.trim().is_empty() => t,
        _ => name,
    }
}

/// The "named like a note ID" rule. A node is junk when it is not a skeleton entity and either
/// (a) its normalised name equals a normalised note ID, or (b) its raw name is lowercase
/// slug-shaped (`[a-z0-9]+(-[a-z0-9]+)+`), starts with one of `prefixes` (`prefix` here is the
/// first slug segment, e.g. `recap`) and contains an all-digit segment (`recap-01-aiko-osei`,
/// `digest-047`).
pub struct JunkRule {
    note_ids: HashSet<String>,
    entity_names: HashSet<String>,
    prefixes: HashSet<String>,
}

impl JunkRule {
    pub fn new<'a, I, E, P>(note_ids: I, entity_names: E, prefixes: P) -> Self
    where
        I: IntoIterator<Item = &'a str>,
        E: IntoIterator<Item = &'a str>,
        P: IntoIterator<Item = &'a str>,
    {
        Self {
            note_ids: note_ids.into_iter().map(normalize).collect(),
            entity_names: entity_names.into_iter().map(normalize).collect(),
            prefixes: prefixes
                .into_iter()
                .map(|p| normalize(p.trim_end_matches('-')))
                .collect(),
        }
    }

    pub fn is_junk(&self, name: &str) -> bool {
        let norm = normalize(name);
        if self.entity_names.contains(&norm) {
            return false;
        }
        if self.note_ids.contains(&norm) {
            return true;
        }
        let raw = name.trim();
        let segs: Vec<&str> = raw.split('-').collect();
        if segs.len() < 2
            || !segs.iter().all(|s| {
                !s.is_empty()
                    && s.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            })
        {
            return false;
        }
        self.prefixes.contains(segs[0])
            && segs.iter().any(|s| s.chars().all(|c| c.is_ascii_digit()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_folds_case_separators_and_whitespace() {
        assert_eq!(normalize("  Cedar_Core "), "cedar core");
        assert_eq!(normalize("CEDAR-CORE"), "cedar core");
        assert_eq!(normalize("cedar \t core"), "cedar core");
        assert_eq!(normalize("OWNED_BY"), "owned by");
        assert!(names_match("Cedar Core", "cedar  core"));
        assert!(!names_match("Cedar Core", "Cedar Supply"));
        assert!(!names_match("Cedar", "Cedar Core"));
    }

    #[test]
    fn contains_bounded_respects_alphanumeric_boundaries() {
        assert!(contains_bounded("9 engineers work on helix core", "9"));
        assert!(!contains_bounded("96 engineers", "9"));
        assert!(!contains_bounded("team of 69", "9"));
        assert!(contains_bounded("monthly budget of $4,200.", "$4,200"));
        assert!(!contains_bounded("budget of $4,2001", "$4,200"));
        assert!(contains_bounded("lena moreau owns it", "lena moreau"));
        assert!(!contains_bounded("lena moreauville", "lena moreau"));
        assert!(!contains_bounded("anything", ""));
        // multibyte haystack does not panic
        assert!(contains_bounded("café 9 é", "9"));
    }

    #[test]
    fn type_matches_ignores_entity_root_and_case() {
        let labels = vec!["Entity".to_string(), "Project".to_string()];
        assert!(type_matches(&labels, &["project".to_string()]));
        assert!(!type_matches(&labels, &["Person".to_string()]));
        assert!(!type_matches(&labels, &["Entity".to_string()]));
    }

    #[test]
    fn effective_type_falls_back_to_name() {
        assert_eq!(effective_edge_type(Some("OWNS"), "x"), "OWNS");
        assert_eq!(effective_edge_type(Some(" "), "x"), "x");
        assert_eq!(effective_edge_type(None, "x"), "x");
    }

    #[test]
    fn junk_rule_exact_ids_and_slug_shapes() {
        let ids = ["project-cedar-core-1", "wiki-cedar-core"];
        let ents = ["Cedar Core"];
        let pre = ["recap-", "digest-", "wiki-"];
        let r = JunkRule::new(ids, ents, pre);
        assert!(r.is_junk("project-cedar-core-1"));
        assert!(r.is_junk("wiki-cedar-core")); // exact id
        assert!(r.is_junk("recap-01-aiko-osei")); // prefix + digit segment
        assert!(r.is_junk("digest-047"));
        assert!(!r.is_junk("wiki-something-else")); // prefix but no digit, not an id
        assert!(!r.is_junk("Cedar Core"));
        assert!(!r.is_junk("Aiko Osei"));
        assert!(!r.is_junk("budget-2027")); // digit but prefix not listed
    }
}
