//! Episode source identity (issue #853, ADR-0853): which source file an episode belongs to.
//!
//! Every chunk-derived episode carries a composed display value,
//! `source_description = "{source_file}:{chunk_id}"`. A URL source contains colons (the scheme,
//! and sometimes a port), so that string cannot be split back into its two parts by looking at
//! colons. Since #853 the chunk and document ingest paths store the two halves separately in
//! `Episodic.source_file` / `Episodic.chunk_id`; this module is the single rule that
//! `remove_episodes_by_source` (`knowledge_delete_by_source`) and `get_entities_by_source`
//! (`knowledge_get_entities_by_source`) apply to decide whether an episode belongs to a requested
//! source.
//!
//! The rule maps every episode to **at most one** source file, and an episode matches a request
//! only when that source file is byte-for-byte the requested one. Because a row has at most one
//! source, it can never match two different requests — which is what makes over-matching (the
//! #829/#830 defect, where `https://a.com` also matched `https://a.com:8443/...`) impossible by
//! construction. For a **legacy** row (no stored `source_file`) the rule may fail to attribute the
//! row (under-match); it never attributes it to a different source.
//!
//! | Row | Source file |
//! |---|---|
//! | `source_file` set (non-empty) | `source_file`, authoritative |
//! | legacy, `write_path` `chunk` / `document` | `source_description` minus the suffix `":" + name`; `None` if it does not end that way |
//! | legacy, `write_path` NULL / empty (pre-0.19.0) | `source_description` minus `":" + name` when it ends that way, otherwise `source_description` whole |
//! | legacy, `write_path` `episode` (raw `knowledge_add_episode`) | `source_description` whole (exact equality only) |
//! | legacy, any other `write_path` | `None` (fail safe) |
//!
//! The split point always comes from the row's own `name` — the stored chunk ID for the chunk and
//! document paths — never from the position of a colon. That is exact for URLs with ports, IPv6
//! literals and document-mode chunk IDs (`{doc_id}::{hex}`), all of which defeat "split on the
//! last colon".

use crate::types::{WRITE_PATH_CHUNK, WRITE_PATH_DOCUMENT, WRITE_PATH_EPISODE};

/// The source file an episode belongs to, or `None` when it cannot be determined safely.
///
/// `source_file` is the stored `Episodic.source_file` column (`None` / empty on a legacy row);
/// `source_description`, `name` and `write_path` are the stored columns of the same names. See
/// the module docs for the rule.
pub fn episode_source_file<'a>(
    source_file: Option<&'a str>,
    source_description: &'a str,
    name: &'a str,
    write_path: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(sf) = source_file.filter(|s| !s.is_empty()) {
        return Some(sf);
    }
    let stripped = strip_chunk_suffix(source_description, name);
    let whole = Some(source_description).filter(|s| !s.is_empty());
    match write_path {
        Some(WRITE_PATH_CHUNK) | Some(WRITE_PATH_DOCUMENT) => stripped,
        // Pre-0.19.0 rows carry no write-path marker and may be either a chunk episode or a raw
        // `add_episode` one. Treat a composed-form description as a chunk (the ADR-0292 /
        // ADR-0824 precedent), and anything else as an exact whole-string source. The fallback
        // must not apply when the composed form matched: `https://a.com:8443` with name `8443`
        // belongs to `https://a.com` and must not also match `https://a.com:8443`.
        None | Some("") => stripped.or(whole),
        // A raw `add_episode` name is free-form, so stripping it could attribute the row to a
        // shorter source it never named. Exact equality only.
        Some(WRITE_PATH_EPISODE) => whole,
        Some(_) => None,
    }
}

/// Whether an episode belongs to `requested`, by [`episode_source_file`]. Byte-exact: no URL
/// normalisation, case folding or trailing-slash handling. An empty `requested` never matches.
pub fn episode_matches_source(
    source_file: Option<&str>,
    source_description: &str,
    name: &str,
    write_path: Option<&str>,
    requested: &str,
) -> bool {
    !requested.is_empty()
        && episode_source_file(source_file, source_description, name, write_path) == Some(requested)
}

/// `source_description` with the suffix `":" + name` removed, or `None` when `name` is empty or
/// the description does not end that way. An empty remainder is `None` too.
fn strip_chunk_suffix<'a>(source_description: &'a str, name: &str) -> Option<&'a str> {
    if name.is_empty() {
        return None;
    }
    source_description
        .strip_suffix(name)
        .and_then(|s| s.strip_suffix(':'))
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: Option<&str> = Some("chunk");
    const DOCUMENT: Option<&str> = Some("document");
    const EPISODE: Option<&str> = Some("episode");

    #[test]
    fn stored_source_file_is_authoritative() {
        assert_eq!(
            episode_source_file(
                Some("https://a.com:8443/x"),
                "https://a.com:8443/x:c1",
                "c1",
                CHUNK
            ),
            Some("https://a.com:8443/x")
        );
        // Even when the display value disagrees, the stored identity wins.
        assert_eq!(
            episode_source_file(Some("a.md"), "something else", "c1", EPISODE),
            Some("a.md")
        );
    }

    #[test]
    fn empty_stored_source_file_is_treated_as_absent() {
        assert_eq!(
            episode_source_file(Some(""), "notes/a.md:c1", "c1", CHUNK),
            Some("notes/a.md")
        );
    }

    #[test]
    fn legacy_chunk_row_strips_its_own_name() {
        assert_eq!(
            episode_source_file(None, "https://a.com:8443/x:c1", "c1", CHUNK),
            Some("https://a.com:8443/x")
        );
        assert!(!episode_matches_source(
            None,
            "https://a.com:8443/x:c1",
            "c1",
            CHUNK,
            "https://a.com"
        ));
        assert!(episode_matches_source(
            None,
            "https://a.com:8443/x:c1",
            "c1",
            CHUNK,
            "https://a.com:8443/x"
        ));
    }

    #[test]
    fn legacy_chunk_row_that_is_not_composed_has_no_source() {
        assert_eq!(episode_source_file(None, "notes/a.md", "c1", CHUNK), None);
        assert_eq!(episode_source_file(None, "notes/a.md:c1", "", CHUNK), None);
        assert_eq!(episode_source_file(None, ":c1", "c1", CHUNK), None);
    }

    #[test]
    fn chunk_id_containing_colons_is_split_by_name_not_by_colon() {
        assert_eq!(
            episode_source_file(None, "notes/a.md:sec:1", "sec:1", CHUNK),
            Some("notes/a.md")
        );
        assert!(!episode_matches_source(
            None,
            "notes/a.md:sec:1",
            "sec:1",
            CHUNK,
            "notes/a.md:sec"
        ));
    }

    #[test]
    fn document_mode_names_split_exactly() {
        let name = "doc-1::0123456789ab";
        let sd = format!("https://a.com:8443/doc:{name}");
        assert_eq!(
            episode_source_file(None, &sd, name, DOCUMENT),
            Some("https://a.com:8443/doc")
        );
        // Splitting on the last colon would have produced `https://a.com:8443/doc:doc-1:`.
        assert!(!episode_matches_source(
            None,
            &sd,
            name,
            DOCUMENT,
            "https://a.com:8443/doc:doc-1:"
        ));
    }

    #[test]
    fn prefix_related_sources_never_match_each_other() {
        assert!(!episode_matches_source(
            None,
            "notes/ab:c1",
            "c1",
            CHUNK,
            "notes/a"
        ));
        assert!(!episode_matches_source(
            Some("notes/ab"),
            "notes/ab:c1",
            "c1",
            CHUNK,
            "notes/a"
        ));
        assert!(!episode_matches_source(
            Some("https://a.com/page"),
            "https://a.com/page:c1",
            "c1",
            CHUNK,
            "https://a.com/p"
        ));
        assert!(!episode_matches_source(
            None,
            "https://a.com/p?x=1:c1",
            "c1",
            None,
            "https://a.com/p"
        ));
    }

    #[test]
    fn legacy_null_write_path_row_composed_form_strips() {
        assert_eq!(
            episode_source_file(None, "https://a.com:8443/x:c1", "c1", None),
            Some("https://a.com:8443/x")
        );
        assert_eq!(
            episode_source_file(None, "https://a.com:8443/x:c1", "c1", Some("")),
            Some("https://a.com:8443/x")
        );
    }

    #[test]
    fn legacy_null_write_path_row_not_composed_is_whole_string() {
        assert_eq!(
            episode_source_file(None, "doc.md", "free-form", None),
            Some("doc.md")
        );
        assert!(episode_matches_source(
            None,
            "doc.md",
            "free-form",
            None,
            "doc.md"
        ));
    }

    /// The NULL-row trap: a description in composed form belongs to its stripped prefix only, so
    /// the whole-string fallback must not also make it match the longer source.
    #[test]
    fn legacy_null_row_composed_form_does_not_also_match_whole_string() {
        assert_eq!(
            episode_source_file(None, "https://a.com:8443", "8443", None),
            Some("https://a.com")
        );
        assert!(!episode_matches_source(
            None,
            "https://a.com:8443",
            "8443",
            None,
            "https://a.com:8443"
        ));
    }

    #[test]
    fn episode_row_matches_exact_description_only() {
        assert!(!episode_matches_source(
            None,
            "doc.md:section1",
            "ep-a2",
            EPISODE,
            "doc.md"
        ));
        // Even when the name happens to equal the suffix: a raw add_episode name is free-form.
        assert!(!episode_matches_source(
            None,
            "https://a.com:8443",
            "8443",
            EPISODE,
            "https://a.com"
        ));
        assert!(episode_matches_source(
            None,
            "doc.md:section1",
            "ep-a2",
            EPISODE,
            "doc.md:section1"
        ));
    }

    #[test]
    fn unknown_write_path_fails_safe() {
        assert_eq!(
            episode_source_file(None, "notes/a.md:c1", "c1", Some("future")),
            None
        );
        assert_eq!(
            episode_source_file(None, "notes/a.md", "c1", Some("future")),
            None
        );
    }

    #[test]
    fn empty_request_never_matches() {
        assert!(!episode_matches_source(None, "", "", None, ""));
        assert!(!episode_matches_source(Some(""), "", "c1", EPISODE, ""));
    }

    /// At most one source per row: across a grid of rows and requests, no row matches two
    /// different requests.
    #[test]
    fn no_row_matches_two_different_sources() {
        let rows: &[(Option<&str>, &str, &str, Option<&str>)] = &[
            (None, "https://a.com:8443/x:c1", "c1", CHUNK),
            (None, "https://a.com:8443/x:c1", "c1", None),
            (None, "https://a.com:8443/x:c1", "c1", EPISODE),
            (None, "https://a.com:8443", "8443", None),
            (None, "doc.md:section1", "ep", EPISODE),
            (None, "a::b:c", "b:c", CHUNK),
            (Some("https://a.com"), "https://a.com:c1", "c1", CHUNK),
        ];
        let requests = [
            "https://a.com",
            "https://a.com:8443",
            "https://a.com:8443/x",
            "https://a.com:8443/x:c1",
            "doc.md",
            "doc.md:section1",
            "a",
            "a:",
            "a::b",
        ];
        for (sf, sd, name, wp) in rows {
            let hits: Vec<_> = requests
                .iter()
                .filter(|r| episode_matches_source(*sf, sd, name, *wp, r))
                .collect();
            assert!(hits.len() <= 1, "{sd:?}/{name:?}/{wp:?} matched {hits:?}");
        }
    }
}
