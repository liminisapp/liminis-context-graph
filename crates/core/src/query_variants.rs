//! Caller-supplied query variants for `knowledge_search_passages` (issue #802, ADR-0802).
//!
//! The caller (typically an LLM agent) may pass up to [`MAX_QUERY_VARIANTS`] extra phrasings of the
//! primary `query` in a `queries` array. This module owns the validation of that parameter;
//! retrieval and fusion live in [`crate::search`].
//!
//! Rules (nothing is ever silently truncated):
//! - absent, `null` and `[]` all mean "no variants" (the request is then byte-identical to a
//!   request without `queries`);
//! - a non-array, or an array holding a non-string, is rejected;
//! - more than [`MAX_QUERY_VARIANTS`] entries are rejected;
//! - an entry that is empty after trimming is rejected;
//! - the total number of Unicode scalar values (`chars()`) across the *trimmed* entries must not
//!   exceed [`MAX_VARIANTS_TOTAL_CHARS`];
//! - variants are trimmed; the primary `query` is not (so existing behaviour is unchanged);
//! - a trimmed variant equal (exact, case-sensitive) to the primary query *as sent* (the primary is
//!   not trimmed, so a `query` with surrounding whitespace is not matched by its trimmed twin) or
//!   to an earlier variant is dropped, so repetition cannot boost a document. Surviving variants keep their
//!   original position in `[query, ...queries]`, so evidence indices stay stable for the caller.

use serde_json::Value;

use crate::error::Error;

/// Maximum number of extra query variants a caller may supply.
pub const MAX_QUERY_VARIANTS: usize = 5;

/// Maximum combined length of all variants, in Unicode scalar values (`chars().count()`) of the
/// trimmed strings. Generous enough for a short sentence per variant.
pub const MAX_VARIANTS_TOTAL_CHARS: usize = 2000;

/// A validated variant: its index in the caller's `[query, ...queries]` (1-based for variants;
/// 0 is the primary) and its trimmed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub index: usize,
    pub text: String,
}

/// Validates the `queries` request parameter against `primary`.
///
/// Returns the variants that should actually be searched (duplicates of the primary or of an
/// earlier variant removed), each tagged with its original index. An empty result means "no
/// variants": the caller must take the unchanged single-query path.
pub fn parse_query_variants(value: &Value, primary: &str) -> Result<Vec<Variant>, Error> {
    let arr = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(a) => a,
        _ => {
            return Err(Error::Ipc(
                "queries must be an array of strings".to_string(),
            ))
        }
    };
    if arr.len() > MAX_QUERY_VARIANTS {
        return Err(Error::Ipc(format!(
            "queries has {} entries; at most {MAX_QUERY_VARIANTS} are allowed",
            arr.len()
        )));
    }

    let mut trimmed: Vec<&str> = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        let s = v
            .as_str()
            .ok_or_else(|| Error::Ipc(format!("queries[{i}] must be a string")))?
            .trim();
        if s.is_empty() {
            return Err(Error::Ipc(format!(
                "queries[{i}] must be a non-empty string"
            )));
        }
        trimmed.push(s);
    }

    let total: usize = trimmed.iter().map(|s| s.chars().count()).sum();
    if total > MAX_VARIANTS_TOTAL_CHARS {
        return Err(Error::Ipc(format!(
            "queries total length is {total} characters; at most {MAX_VARIANTS_TOTAL_CHARS} are allowed"
        )));
    }

    let mut out: Vec<Variant> = Vec::with_capacity(trimmed.len());
    for (i, s) in trimmed.into_iter().enumerate() {
        if s == primary || out.iter().any(|v| v.text == s) {
            continue;
        }
        out.push(Variant {
            index: i + 1,
            text: s.to_string(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn err_msg(v: Value) -> String {
        parse_query_variants(&v, "primary").unwrap_err().to_string()
    }

    #[test]
    fn absent_null_and_empty_array_mean_no_variants() {
        assert!(parse_query_variants(&Value::Null, "q").unwrap().is_empty());
        assert!(parse_query_variants(&json!([]), "q").unwrap().is_empty());
    }

    #[test]
    fn valid_variants_keep_original_indices_and_are_trimmed() {
        let v = parse_query_variants(&json!(["  a  ", "b"]), "q").unwrap();
        assert_eq!(
            v,
            vec![
                Variant {
                    index: 1,
                    text: "a".into()
                },
                Variant {
                    index: 2,
                    text: "b".into()
                }
            ]
        );
    }

    #[test]
    fn five_variants_are_accepted_six_rejected() {
        assert_eq!(
            parse_query_variants(&json!(["a", "b", "c", "d", "e"]), "q")
                .unwrap()
                .len(),
            5
        );
        let m = err_msg(json!(["a", "b", "c", "d", "e", "f"]));
        assert!(m.contains("at most 5"), "{m}");
    }

    #[test]
    fn empty_and_whitespace_entries_are_rejected() {
        assert!(err_msg(json!(["ok", ""])).contains("queries[1]"));
        assert!(err_msg(json!(["  \t\n"])).contains("queries[0]"));
    }

    #[test]
    fn wrong_types_are_rejected() {
        assert!(err_msg(json!("a string")).contains("array of strings"));
        assert!(err_msg(json!({"a": 1})).contains("array of strings"));
        assert!(err_msg(json!(3)).contains("array of strings"));
        assert!(err_msg(json!(["ok", 7])).contains("queries[1] must be a string"));
        assert!(err_msg(json!([null])).contains("queries[0] must be a string"));
    }

    #[test]
    fn total_length_bound_counts_chars_not_bytes() {
        // 2000 two-byte chars (4000 bytes) is exactly at the bound and must pass.
        let at_bound = "é".repeat(MAX_VARIANTS_TOTAL_CHARS);
        assert!(parse_query_variants(&json!([at_bound]), "q").is_ok());
        // One more char is over: rejected, never truncated.
        let over = "é".repeat(MAX_VARIANTS_TOTAL_CHARS + 1);
        assert!(err_msg(json!([over])).contains("at most 2000"));
        // The bound is on the total across entries.
        let half = "x".repeat(1001);
        assert!(err_msg(json!([half.clone(), half])).contains("total length"));
    }

    #[test]
    fn length_bound_ignores_surrounding_whitespace() {
        let s = format!("  {}  ", "x".repeat(MAX_VARIANTS_TOTAL_CHARS));
        assert!(parse_query_variants(&json!([s]), "q").is_ok());
    }

    #[test]
    fn duplicates_of_primary_or_earlier_variant_are_dropped_with_stable_indices() {
        let v = parse_query_variants(&json!(["q", "a", " a ", "b", "q "]), "q").unwrap();
        let got: Vec<(usize, &str)> = v.iter().map(|x| (x.index, x.text.as_str())).collect();
        assert_eq!(got, vec![(2, "a"), (4, "b")]);
    }

    #[test]
    fn dedup_is_case_sensitive() {
        let v = parse_query_variants(&json!(["Q"]), "q").unwrap();
        assert_eq!(v.len(), 1);
    }
}
