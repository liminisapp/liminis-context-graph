//! Stable finding identifiers (issue #755, ADR-0755).
//!
//! An id is the SHA-256 of the category, the group and the identities of the subject items —
//! stored uuids, `doc_key`s and designation strings only. Time, page position and row order never
//! enter it, so an unchanged finding keeps its id across unrelated ingests, a WAL rebuild and a
//! checkpoint restore, and a finding that disappears and later reappears carries the same id.

use crate::sha256_hex::sha256_hex;

/// Field separator (ASCII unit separator): cannot occur in a uuid, a `doc_key` prefix or a
/// normalised relation designation, so `["a", "bc"]` and `["ab", "c"]` hash differently.
const SEP: char = '\u{1f}';

/// Version tag baked into every id. Bump only if the id basis of a category changes.
const ID_VERSION: &str = "lint:v1";

/// `sha256_hex("lint:v1 ␟ <category> ␟ <group> ␟ <part> ␟ …")`. `parts` are the subject keys in
/// the fixed order each category documents.
pub fn finding_id(category: &str, group_id: &str, parts: &[&str]) -> String {
    let mut s = String::new();
    s.push_str(ID_VERSION);
    s.push(SEP);
    s.push_str(category);
    s.push(SEP);
    s.push_str(group_id);
    for p in parts {
        s.push(SEP);
        s.push_str(p);
    }
    sha256_hex(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_stable_and_hex() {
        let a = finding_id("weak_support", "g", &["s", "t", "n"]);
        assert_eq!(a, finding_id("weak_support", "g", &["s", "t", "n"]));
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn id_is_sensitive_to_part_order_and_boundaries() {
        assert_ne!(
            finding_id("c", "g", &["a", "b"]),
            finding_id("c", "g", &["b", "a"])
        );
        assert_ne!(
            finding_id("c", "g", &["a", "bc"]),
            finding_id("c", "g", &["ab", "c"])
        );
    }

    #[test]
    fn id_separates_category_and_group() {
        assert_ne!(
            finding_id("weak_support", "g", &["x"]),
            finding_id("orphan_entity", "g", &["x"])
        );
        assert_ne!(
            finding_id("weak_support", "g1", &["x"]),
            finding_id("weak_support", "g2", &["x"])
        );
    }
}
