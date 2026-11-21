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
//! Two things name an entity as a note-ID junk node:
//!
//! 1. the episode's **own** source ID ([`own_source_ids`]), and
//! 2. a **citation target**: the ID-shaped target of a `[[wikilink]]` in the episode body
//!    ([`citation_targets`]). Wiki pages and answer notes end with `Sources: [[project-quill-core-1]]`
//!    or `(from [[project-quill-core-1]])`, and the model extracts the cited note's ID as an entity.
//!    On the v2 corpus this — not the own ID — is the origin of every observed note-ID node.
//!
//! An entity that merely shares a name with some other note's ID, without being cited as a link in
//! this episode's body, is never matched. Non-ID-shaped link targets (`[[Quill Core]]`) name real
//! entities and are never matched either.

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
/// Only **ID-shaped** identifiers are included (see [`is_id_shaped`]): a note filed under its
/// subject's name (`people/Alice.md`, `Postgres.md`, `Quill Core.md`) has no distinguishable ID, and
/// treating that stem as one would drop the real `Alice` entity the note is about (and, via the
/// `#`/`/` normalisation, short names like `C#` or `AC/DC` could collide with an ID of `c`/`dc`).
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
        .filter(|c| is_id_shaped(c))
        .collect()
}

/// Whether a normalised identifier looks like a slug/ID rather than a subject name: no whitespace,
/// at least one letter, and at least one `-`, `_` or ASCII digit (`project-quill-core-1`, `note_7`,
/// `doc3`). Bare words such as `alice` or `postgres` are not ID-shaped. Identifiers with no letter at
/// all (a daily note `2024-01-15.md`, a bare number `42`) are not ID-shaped either: they are what a
/// real Date or number entity extracted from the note would be named. Empty input is not ID-shaped.
fn is_id_shaped(s: &str) -> bool {
    !s.is_empty()
        && !s.chars().any(char::is_whitespace)
        && s.chars().any(char::is_alphabetic)
        && s.chars()
            .any(|c| c == '-' || c == '_' || c.is_ascii_digit())
}

/// The normalised, ID-shaped targets of `[[wikilink]]`s in `body` (citation targets).
///
/// Handles `[[target]]`, `[[target|alias]]`, `[[target#heading]]` and `[[folder/target.md]]`. A
/// target that is not ID-shaped (see [`is_id_shaped`]) — `[[Quill Core]]`, `[[Alice]]` — names a
/// real entity and is not returned.
pub fn citation_targets(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };
        let inner = &after[..end];
        // A link never spans lines; an unclosed `[[` followed by a later `]]` is not a link.
        if !inner.contains('\n') {
            let target = inner.split('|').next().unwrap_or("");
            let norm = normalize_source_id(target);
            if is_id_shaped(&norm) {
                out.insert(norm);
            }
        }
        rest = &after[end + 2..];
    }
    out
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
        let got = ids("chunk-a#1", "something else entirely", None, true);
        assert_eq!(got.len(), 1);
        assert!(got.contains("chunk-a"));
    }

    #[test]
    fn subject_named_files_are_not_own_ids() {
        // A note titled after its subject has no ID to match; the real entity must survive.
        for (name, sd, doc) in [
            (
                "people/Alice.md#0",
                "people/Alice.md:people/Alice.md#0",
                None,
            ),
            ("Postgres#1", "Postgres.md:Postgres#1", Some("Postgres")),
            (
                "Quill Core#0",
                "Quill Core.md:Quill Core#0",
                Some("Quill Core"),
            ),
            ("notes/C#.md#0", "notes/C#.md:notes/C#.md#0", None),
            // Daily / dated and bare-number notes: a real Date entity must survive.
            (
                "notes/2024-01-15.md#0",
                "notes/2024-01-15.md:notes/2024-01-15.md#0",
                Some("2024-01-15"),
            ),
            ("42#0", "42.md:42#0", Some("42")),
        ] {
            let got = ids(name, sd, doc, true);
            assert!(got.is_empty(), "{name}: {got:?}");
        }
        let own = ids(
            "people/Alice.md#0",
            "people/Alice.md:people/Alice.md#0",
            None,
            true,
        );
        assert!(!matches_source_id("Alice", &own));
        let dated = ids(
            "2024-01-15#0",
            "2024-01-15.md:2024-01-15#0",
            Some("2024-01-15"),
            true,
        );
        assert!(!matches_source_id("2024-01-15", &dated));
        // Mixed: the slug-shaped candidate is kept, the bare-word one is not.
        let got = ids("alice#0", "alice:alice#0", Some("note-9"), true);
        assert_eq!(got.len(), 1);
        assert!(got.contains("note-9"));
    }

    #[test]
    fn citation_targets_from_v2_excerpts() {
        let wiki = "Supplier: the supplier relationship is with Quill Supply. Sources: [[project-quill-core-1]], [[project-quill-core-2]]";
        let got = citation_targets(wiki);
        assert_eq!(got.len(), 2);
        assert!(got.contains("project-quill-core-1") && got.contains("project-quill-core-2"));
        let answer =
            "Ownership of Quill Core sits with Callum Brennan. (from [[project-quill-core-1]])";
        assert!(citation_targets(answer).contains("project-quill-core-1"));
        let region =
            "Current cloud region for Tern Core: us-east. Sources: [[decision-region-tern-core-4]]";
        assert!(citation_targets(region).contains("decision-region-tern-core-4"));
    }

    #[test]
    fn citation_targets_skip_non_id_shaped_and_handle_variants() {
        let got = citation_targets(
            "See [[Quill Core]] and [[Alice]]; also [[Note-7|the note]], [[notes/doc-3.md#intro]] and [[2024-01-15]].",
        );
        assert!(!got.contains("quill core") && !got.contains("alice"));
        assert!(!got.contains("2024-01-15"));
        assert!(got.contains("note-7"), "{got:?}");
        assert!(got.contains("doc-3"), "{got:?}");
        assert_eq!(got.len(), 2);
    }

    // Documents a known, accepted limit (ADR-0824): the ID-shape gate cannot tell a hyphenated or
    // versioned *concept* link from a note-ID link, so these are citation targets too and an entity
    // extracted under exactly that name is dropped. Pinned so a change is deliberate, not silent.
    #[test]
    fn hyphenated_and_versioned_concept_links_are_citation_targets() {
        let got = citation_targets(
            "We migrated to [[GPT-4]] and [[machine-learning]]; see [[covid-19]], [[Node-RED]], [[S3]], [[e-commerce]].",
        );
        for t in [
            "gpt-4",
            "machine-learning",
            "covid-19",
            "node-red",
            "s3",
            "e-commerce",
        ] {
            assert!(got.contains(t), "{t}: {got:?}");
        }
        // Bare words and spaced names are still real entities.
        assert!(citation_targets("[[Postgres]] [[Quill Core]]").is_empty());
    }

    #[test]
    fn citation_targets_ignore_unclosed_and_multiline_brackets() {
        assert!(citation_targets("an [[unclosed-id-1 link").is_empty());
        assert!(citation_targets("[[note-1\nnote-2]]").is_empty());
        assert!(citation_targets("").is_empty());
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
