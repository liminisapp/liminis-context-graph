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
//! 2. a **citation target**: the target of a `[[wikilink]]` in the episode body that names a note
//!    already ingested in the same group ([`citation_candidates`] + [`known_source_ids_of_rows`]).
//!    Wiki pages and answer notes end with `Sources: [[project-quill-core-1]]` or
//!    `(from [[project-quill-core-1]])`, and the model extracts the cited note's ID as an entity.
//!    On the v2 corpus this — not the own ID — is the origin of every observed note-ID node.
//!
//! The citation rule is deliberately narrow, because no name *shape* separates `project-quill-core-1`
//! from `GPT-4` or `covid-19`. An entity is dropped only when **all** hold: (a) its name, before any
//! normalisation, is verbatim the link target, which is itself all-lowercase, space-free and
//! ID-shaped; and (b) that target is the source ID (doc ID, or source-file stem) of an episode
//! already ingested in the same group — i.e. it cites a note LCG knows. Everything else survives,
//! including `[[GPT-4]]` (not lowercase), `[[machine-learning]]` with no such note ingested, and
//! `[[Quill Core]]` (not ID-shaped). A note ingested *after* the one citing it is missed; the
//! extraction prompt is what covers that case.

use std::collections::{BTreeMap, BTreeSet};

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

/// Candidate citation targets in `body`: `[[wikilink]]` targets that could be a note ID, as a map
/// from the **verbatim** link target (before any normalisation, `|alias` removed, trimmed) to its
/// [normalised](normalize_source_id) form.
///
/// A target is a candidate only if it is verbatim all-lowercase, contains no whitespace and is
/// ID-shaped (see [`is_id_shaped`]); `[[GPT-4]]`, `[[Node-RED]]`, `[[Quill Core]]` and `[[Alice]]`
/// are not. Handles `[[target]]`, `[[target|alias]]`, `[[target#heading]]` and
/// `[[folder/target.md]]` (the normalised form strips the fragment, path and extension).
pub fn citation_candidates(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };
        let inner = &after[..end];
        // A link never spans lines; an unclosed `[[` followed by a later `]]` is not a link.
        if !inner.contains('\n') {
            let raw = inner.split('|').next().unwrap_or("").trim();
            if !raw.is_empty() && raw == raw.to_lowercase() && !raw.chars().any(char::is_whitespace)
            {
                let norm = normalize_source_id(raw);
                if is_id_shaped(&norm) {
                    out.insert(raw.to_string(), norm);
                }
            }
        }
        rest = &after[end + 2..];
    }
    out
}

/// One already-ingested episode row, as read for the known-source-ID check.
#[derive(Debug, Clone, Default)]
pub struct EpisodeIdRow {
    pub name: String,
    pub source_description: String,
    pub doc_id: Option<String>,
    /// `true` when the row was written by `knowledge_process_chunk` (its `name` is a chunk id and
    /// its `source_description` is `{source_file}:{chunk_id}`).
    pub is_chunk_path: bool,
}

/// The normalised source IDs of the given already-ingested episodes (their `doc_id`, and on the
/// chunk path the chunk-id prefix and source-file stem) — the same derivation as
/// [`own_source_ids`], so a note is "known" exactly when it would have named itself.
pub fn known_source_ids_of_rows(rows: &[EpisodeIdRow]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for r in rows {
        out.extend(own_source_ids(
            &r.name,
            &r.source_description,
            r.doc_id.as_deref(),
            r.is_chunk_path,
        ));
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
    fn citation_candidates_from_v2_excerpts() {
        let wiki = "Supplier: the supplier relationship is with Quill Supply. Sources: [[project-quill-core-1]], [[project-quill-core-2]]";
        let got = citation_candidates(wiki);
        assert_eq!(got.len(), 2);
        assert_eq!(
            got.get("project-quill-core-1").map(String::as_str),
            Some("project-quill-core-1")
        );
        assert!(got.contains_key("project-quill-core-2"));
        let answer =
            "Ownership of Quill Core sits with Callum Brennan. (from [[project-quill-core-1]])";
        assert!(citation_candidates(answer).contains_key("project-quill-core-1"));
        let region =
            "Current cloud region for Tern Core: us-east. Sources: [[decision-region-tern-core-4]]";
        assert!(citation_candidates(region).contains_key("decision-region-tern-core-4"));
    }

    // (a): the verbatim target must be all-lowercase, space-free and ID-shaped, so concept links
    // with capitals (`GPT-4`, `Node-RED`, `S3`) are never candidates.
    #[test]
    fn citation_candidates_require_verbatim_lowercase_id_shape() {
        let got = citation_candidates(
            "See [[Quill Core]], [[Alice]], [[GPT-4]], [[Node-RED]], [[S3]], [[Note-7|the note]] and [[2024-01-15]]; also [[machine-learning]], [[covid-19]] and [[notes/doc-3.md#intro]].",
        );
        for no in [
            "Quill Core",
            "Alice",
            "GPT-4",
            "Node-RED",
            "S3",
            "Note-7",
            "2024-01-15",
        ] {
            assert!(!got.contains_key(no), "{no}: {got:?}");
        }
        // Lowercase concept links are still *candidates*; rule (b) (the note must be ingested in
        // the group) is what spares them.
        assert!(got.contains_key("machine-learning"), "{got:?}");
        assert!(got.contains_key("covid-19"), "{got:?}");
        assert_eq!(
            got.get("notes/doc-3.md#intro").map(String::as_str),
            Some("doc-3")
        );
    }

    #[test]
    fn citation_candidates_ignore_alias_unclosed_and_multiline_brackets() {
        let got = citation_candidates("[[note-7|the note]]");
        assert!(got.contains_key("note-7"));
        assert!(citation_candidates("an [[unclosed-id-1 link").is_empty());
        assert!(citation_candidates("[[note-1\nnote-2]]").is_empty());
        assert!(citation_candidates("").is_empty());
    }

    #[test]
    fn known_source_ids_come_from_doc_id_chunk_prefix_and_source_file() {
        let rows = vec![
            EpisodeIdRow {
                name: "project-quill-core-1#0".into(),
                source_description: "notes/Project-Quill-Core-1.md:project-quill-core-1#0".into(),
                doc_id: Some("project-quill-core-1".into()),
                is_chunk_path: true,
            },
            // A free-form episode contributes only its doc_id, never its free-form name.
            EpisodeIdRow {
                name: "machine-learning".into(),
                source_description: "x".into(),
                doc_id: None,
                is_chunk_path: false,
            },
            EpisodeIdRow {
                name: "n#0".into(),
                source_description: "wiki-quill-core.md:n#0".into(),
                doc_id: None,
                is_chunk_path: true,
            },
        ];
        let got = known_source_ids_of_rows(&rows);
        assert!(got.contains("project-quill-core-1"));
        assert!(got.contains("wiki-quill-core"));
        assert!(!got.contains("machine-learning"));
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
