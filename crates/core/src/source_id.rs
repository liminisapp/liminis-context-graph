//! Deterministic "own source ID" predicate for extraction output (issue #824, ADR-0824).
//!
//! Under an ontology that declares a `Document` entity type the model can extract the note it is
//! reading as an entity named after the note's identifier (e.g. `project-quill-core-1`). Such a
//! node absorbs edges that belong to the real entities and pollutes walks and entity search. The
//! sanctioned representation of the source note is the *asserted* `Document`/`Source` entity the
//! host supplies, never an extracted one.
//!
//! This module is the pure, I/O-free half of the fix: it derives the set of normalised
//! identifiers that name the episode being extracted ([`own_source_ids`]) and tests an entity name
//! against it ([`matches_source_id`]). `episode.rs` calls it after extraction and drops matches.
//!
//! The rule is **own-ID only**: an entity named after some *other* note's ID is never matched
//! here (FR-007).

use std::collections::BTreeSet;

use crate::prompts::strip_control_chars;

/// File extensions stripped (one, trailing) from an identifier before comparison.
const NOTE_EXTENSIONS: &[&str] = &[".md", ".markdown", ".mdx", ".txt"];

/// Normalises a note identifier or entity name for own-ID comparison.
///
/// Strips control characters, trims, lowercases, drops a path prefix (up to the last `/` or `\`),
/// a trailing `#fragment`, wrapping punctuation (`[[ ]]`, backticks, quotes, brackets) and one
/// trailing file extension. Both sides of a comparison go through this same function. The result
/// may be empty, in which case it is never a candidate ID.
pub fn normalize_source_id(raw: &str) -> String {
    let mut s = strip_control_chars(raw).trim().to_lowercase();
    // Wrapping punctuation first, so `[[note-id]]` and `` `note-id.md` `` unwrap to the bare ID.
    s = trim_wrapping(&s).to_string();
    // Fragment (`#0` chunk suffix, `#heading`) before the path/extension handling.
    if let Some(idx) = s.find('#') {
        s.truncate(idx);
    }
    if let Some(idx) = s.rfind(['/', '\\']) {
        s = s[idx + 1..].to_string();
    }
    s = trim_wrapping(&s).to_string();
    for ext in NOTE_EXTENSIONS {
        if let Some(stripped) = s.strip_suffix(ext) {
            s = stripped.to_string();
            break;
        }
    }
    trim_wrapping(&s).to_string()
}

fn trim_wrapping(s: &str) -> &str {
    s.trim_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '[' | ']'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '`'
                    | '"'
                    | '\''
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
                    | '.'
                    | ','
                    | ';'
                    | ':'
            )
    })
}

/// The normalised identifiers that name the episode being extracted.
///
/// - `doc_id` applies on every path.
/// - `name` (the chunk id, with any `#<n>` suffix removed by normalisation) and the `source_file`
///   part of `source_description` (recovered by stripping the trailing `:{name}`) apply only when
///   `is_chunk_path` is set — i.e. `knowledge_process_chunk`, whose `name` is a chunk id. A
///   free-form `knowledge_add_episode` name such as `"Alice"` is not a source ID, and matching on
///   it would drop real entities (FR-009).
///
/// Empty identifiers are never included.
pub fn own_source_ids(
    name: &str,
    source_description: &str,
    doc_id: Option<&str>,
    is_chunk_path: bool,
) -> BTreeSet<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(d) = doc_id {
        candidates.push(d.to_string());
    }
    if is_chunk_path {
        candidates.push(name.to_string());
        let source_file = source_description
            .strip_suffix(name)
            .and_then(|s| s.strip_suffix(':'))
            .unwrap_or("");
        candidates.push(source_file.to_string());
    }
    candidates
        .iter()
        .map(|c| normalize_source_id(c))
        .filter(|c| !c.is_empty())
        .collect()
}

/// Whether `entity_name` normalises to one of the episode's own source IDs.
pub fn matches_source_id(entity_name: &str, own_ids: &BTreeSet<String>) -> bool {
    if own_ids.is_empty() {
        return false;
    }
    let n = normalize_source_id(entity_name);
    !n.is_empty() && own_ids.contains(&n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(name: &str, sd: &str, doc: Option<&str>, chunk: bool) -> BTreeSet<String> {
        own_source_ids(name, sd, doc, chunk)
    }

    #[test]
    fn normalizes_case_whitespace_and_control_chars() {
        assert_eq!(
            normalize_source_id("  Project-Quill-Core-1\u{1} "),
            "project-quill-core-1"
        );
    }

    #[test]
    fn normalizes_extension_path_fragment_and_wrapping() {
        for raw in [
            "project-quill-core-1.md",
            "notes/derived/project-quill-core-1.md",
            "C:\\notes\\Project-Quill-Core-1.MD",
            "project-quill-core-1#3",
            "[[project-quill-core-1]]",
            "`project-quill-core-1`",
            "\"project-quill-core-1\".",
            "project-quill-core-1.txt",
        ] {
            assert_eq!(normalize_source_id(raw), "project-quill-core-1", "{raw}");
        }
    }

    #[test]
    fn empty_and_punctuation_only_normalize_to_empty() {
        assert_eq!(normalize_source_id(""), "");
        assert_eq!(normalize_source_id("  "), "");
        assert_eq!(normalize_source_id("[[ ]]"), "");
    }

    #[test]
    fn chunk_path_collects_doc_id_chunk_prefix_and_source_file() {
        let got = ids(
            "project-quill-core-1#0",
            "project-quill-core-1:project-quill-core-1#0",
            Some("project-quill-core-1"),
            true,
        );
        assert_eq!(got.len(), 1);
        assert!(got.contains("project-quill-core-1"));

        // Source file differing from the chunk prefix and doc id: all three are collected.
        let got = ids(
            "chunk-a#1",
            "notes/File-B.md:chunk-a#1",
            Some("doc-c"),
            true,
        );
        let want: BTreeSet<String> = ["chunk-a", "file-b", "doc-c"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn free_form_path_uses_only_doc_id() {
        let got = ids("Alice", "Alice:Alice", None, false);
        assert!(got.is_empty());
        let got = ids("Alice", "whatever", Some("doc-1"), false);
        assert_eq!(got.len(), 1);
        assert!(got.contains("doc-1"));
    }

    #[test]
    fn source_description_without_name_suffix_yields_no_source_file() {
        let got = ids("chunk#1", "something else entirely", None, true);
        assert_eq!(got.len(), 1);
        assert!(got.contains("chunk"));
    }

    #[test]
    fn empty_ids_are_never_candidates() {
        let got = ids("", ":", Some("  "), true);
        assert!(got.is_empty());
        assert!(!matches_source_id("", &got));
        assert!(!matches_source_id("anything", &got));
    }

    #[test]
    fn matches_variants_of_own_id_but_not_other_ids() {
        let own = ids("note-1#0", "note-1:note-1#0", Some("note-1"), true);
        assert!(matches_source_id("note-1", &own));
        assert!(matches_source_id("Note-1.md", &own));
        assert!(matches_source_id("[[note-1]]", &own));
        assert!(!matches_source_id("note-2", &own));
        assert!(!matches_source_id("note-10", &own));
        assert!(!matches_source_id("Quill Core", &own));
    }
}
