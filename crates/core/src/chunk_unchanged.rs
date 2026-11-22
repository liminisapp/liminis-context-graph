//! The `knowledge_process_chunk` unchanged-resubmission check (issue #858, ADR-0858).
//!
//! A pure comparison between the live episode(s) a `(group_id, chunk_id)` already has and an
//! incoming request. It fails closed: anything that is not a clear match — more than one live
//! episode, a foreign `write_path`, an unparsable stored value — is "changed", so the caller takes
//! the ordinary supersede path and a false `unchanged` (stale facts silently believed) is avoided.

use serde_json::Value;

use crate::db::ChunkLiveSnapshot;
use crate::origin::Origin;
use crate::sha256_hex::sha256_hex;

/// The request fields a normal ingest would store, in their stored forms.
#[derive(Debug, Clone)]
pub struct ChunkRequestView<'a> {
    pub chunk_text: &'a str,
    pub chunk_id: &'a str,
    /// The attributes JSON-object string the ingest would store (`attributes_param_to_string`).
    pub attributes: &'a str,
    pub origin: Origin,
    pub source_file: &'a str,
    pub doc_id: Option<&'a str>,
    /// Microseconds of an *explicit* request `reference_time`. `None` — omitted, so "now" — is
    /// "no opinion" and never counts as a difference (FR-009).
    pub explicit_reference_time_us: Option<i64>,
}

/// The uuid of the live episode when `live` is exactly one chunk-attributable episode that is
/// identical to `req`; `None` otherwise.
pub fn unchanged_episode_uuid<'a>(
    live: &'a [ChunkLiveSnapshot],
    req: &ChunkRequestView<'_>,
) -> Option<&'a str> {
    let [row] = live else {
        return None;
    };
    chunk_is_unchanged(row, req).then_some(row.uuid.as_str())
}

/// Whether `row` — the chunk's single live episode — is identical to `req`.
pub fn chunk_is_unchanged(row: &ChunkLiveSnapshot, req: &ChunkRequestView<'_>) -> bool {
    if !crate::types::write_path_is_chunk_eligible(row.write_path.as_deref()) {
        return false;
    }
    // Content. Document mode hashes the exact source bytes (`sha256_hex(text.as_bytes())`, no
    // trim / line-ending / Unicode folding), so byte equality is the same normalisation. A stored
    // hash is honoured when present; chunk-mode rows store none, so they compare directly.
    let content_same = match row.chunk_hash.as_deref() {
        Some(h) => h == sha256_hex(req.chunk_text.as_bytes()),
        None => row.content == req.chunk_text,
    };
    if !content_same {
        return false;
    }
    // Attributes as parsed JSON, so key order and whitespace do not matter (as `same_metadata`).
    let json =
        |s: &str| serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.into()));
    if json(&row.attributes) != json(req.attributes) {
        return false;
    }
    // A NULL stored origin is `primary`, consistent with every other reader.
    if Origin::from_stored(row.origin.as_deref()) != req.origin {
        return false;
    }
    // `source_file`: the #853 column; a legacy row without it must carry the exact
    // `"{source_file}:{chunk_id}"` description, otherwise it is treated as different.
    let source_same = match row.source_file.as_deref() {
        Some(sf) => sf == req.source_file,
        None => row.source_description == format!("{}:{}", req.source_file, req.chunk_id),
    };
    if !source_same {
        return false;
    }
    if row.doc_id.as_deref() != req.doc_id {
        return false;
    }
    if let Some(us) = req.explicit_reference_time_us {
        if row.valid_at_us != us {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> ChunkLiveSnapshot {
        ChunkLiveSnapshot {
            uuid: "u1".into(),
            content: "hello world".into(),
            attributes: r#"{"a":1,"b":2}"#.into(),
            origin: Some("primary".into()),
            source_file: Some("f.md".into()),
            source_description: "f.md:c1".into(),
            valid_at_us: 1_700_000_000_123_456,
            doc_id: None,
            write_path: Some("chunk".into()),
            chunk_hash: None,
        }
    }

    fn req() -> ChunkRequestView<'static> {
        ChunkRequestView {
            chunk_text: "hello world",
            chunk_id: "c1",
            attributes: r#"{"a":1,"b":2}"#,
            origin: Origin::Primary,
            source_file: "f.md",
            doc_id: None,
            explicit_reference_time_us: None,
        }
    }

    #[test]
    fn identical_is_unchanged() {
        assert!(chunk_is_unchanged(&row(), &req()));
        assert_eq!(unchanged_episode_uuid(&[row()], &req()), Some("u1"));
    }

    #[test]
    fn one_character_text_difference() {
        let mut r = req();
        r.chunk_text = "hello worle";
        assert!(!chunk_is_unchanged(&row(), &r));
    }

    #[test]
    fn crlf_is_a_difference() {
        let mut stored = row();
        stored.content = "a\nb".into();
        let mut r = req();
        r.chunk_text = "a\r\nb";
        assert!(!chunk_is_unchanged(&stored, &r));
    }

    #[test]
    fn attributes_key_order_is_equal() {
        let mut r = req();
        r.attributes = r#"{ "b": 2, "a": 1 }"#;
        assert!(chunk_is_unchanged(&row(), &r));
    }

    #[test]
    fn attributes_difference() {
        let mut r = req();
        r.attributes = r#"{"a":1,"b":3}"#;
        assert!(!chunk_is_unchanged(&row(), &r));
        r.attributes = "{}";
        assert!(!chunk_is_unchanged(&row(), &r));
    }

    #[test]
    fn empty_attributes_equal() {
        let mut stored = row();
        stored.attributes = "{}".into();
        let mut r = req();
        r.attributes = "{}";
        assert!(chunk_is_unchanged(&stored, &r));
    }

    #[test]
    fn null_origin_is_primary() {
        let mut stored = row();
        stored.origin = None;
        assert!(chunk_is_unchanged(&stored, &req()));
        let mut r = req();
        r.origin = Origin::External;
        assert!(!chunk_is_unchanged(&stored, &r));
    }

    #[test]
    fn origin_difference() {
        let mut r = req();
        r.origin = Origin::Derived;
        assert!(!chunk_is_unchanged(&row(), &r));
    }

    #[test]
    fn source_file_difference_and_legacy_fallback() {
        let mut r = req();
        r.source_file = "g.md";
        assert!(!chunk_is_unchanged(&row(), &r));

        let mut legacy = row();
        legacy.source_file = None;
        assert!(chunk_is_unchanged(&legacy, &req()));
        legacy.source_description = "free form".into();
        assert!(!chunk_is_unchanged(&legacy, &req()));
    }

    #[test]
    fn doc_id_difference() {
        let mut r = req();
        r.doc_id = Some("d1");
        assert!(!chunk_is_unchanged(&row(), &r));
        let mut stored = row();
        stored.doc_id = Some("d1".into());
        assert!(!chunk_is_unchanged(&stored, &req()));
        assert!(chunk_is_unchanged(&stored, &r));
    }

    #[test]
    fn explicit_reference_time() {
        let mut r = req();
        r.explicit_reference_time_us = Some(1_700_000_000_123_456);
        assert!(chunk_is_unchanged(&row(), &r));
        r.explicit_reference_time_us = Some(1_700_000_000_123_457);
        assert!(!chunk_is_unchanged(&row(), &r));
        // Omitted is not a difference (the default `req()`).
        assert!(chunk_is_unchanged(&row(), &req()));
    }

    #[test]
    fn multiple_live_rows_never_unchanged() {
        assert_eq!(unchanged_episode_uuid(&[row(), row()], &req()), None);
        assert_eq!(unchanged_episode_uuid(&[], &req()), None);
    }

    #[test]
    fn foreign_write_path_never_unchanged() {
        let mut stored = row();
        stored.write_path = Some("add_episode".into());
        assert!(!chunk_is_unchanged(&stored, &req()));
        stored.write_path = Some("document".into());
        assert!(!chunk_is_unchanged(&stored, &req()));
        stored.write_path = None;
        assert!(chunk_is_unchanged(&stored, &req()));
    }

    #[test]
    fn stored_hash_is_used_when_present() {
        let mut stored = row();
        stored.content = "not the text".into();
        stored.chunk_hash = Some(sha256_hex("hello world".as_bytes()));
        assert!(chunk_is_unchanged(&stored, &req()));
        stored.chunk_hash = Some(sha256_hex("other".as_bytes()));
        assert!(!chunk_is_unchanged(&stored, &req()));
    }
}
