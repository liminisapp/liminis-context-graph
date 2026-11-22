//! Server self-description for the socket (`knowledge_capabilities`, issue #854, ADR-0854).
//!
//! A raw-socket client can ask the running service what it is and what it supports: the crate
//! version, a `protocol_version` compatibility integer, a list of stable feature strings and the
//! list of IPC methods the dispatcher accepts. Everything here is static build information; it
//! needs no database and no lock, so it answers in degraded mode and while a write is held.
//!
//! # Adding a feature (read this when you ship a capability)
//!
//! **Later issues add their own feature string to [`FEATURES`] when they ship.** Nothing enforces
//! this, so it is a contributor duty (see CONTRIBUTING.md, "Release runbook").
//!
//! - Strings are `lower_snake_case`, carry no version suffix, and are kept **sorted** in the array
//!   (a unit test enforces sorting, uniqueness and the format).
//! - Strings are **append-only**: once released, a feature string is never renamed or removed.
//!   Removing a capability is a breaking change and bumps [`PROTOCOL_VERSION`].
//!
//! # Protocol version
//!
//! [`PROTOCOL_VERSION`] is an integer starting at 1. It is bumped only on a breaking change to
//! the socket protocol (a removed or renamed method, a changed default scope, changed response
//! semantics). Purely additive changes (a new method, a new response field, a new feature
//! string) do not bump it; clients detect those through `features` and `methods`.
//!
//! # `methods`
//!
//! The dispatcher is a plain `match`, so it cannot be iterated. [`methods`] is built from
//! [`crate::write_policy::CLASSIFICATION`], which a test already requires to name exactly the
//! dispatched methods; `tests/capabilities.rs` additionally compares the advertised list with
//! the dispatch arms directly.

use serde_json::{json, Value};

/// The crate version, exactly as `--version` and MCP `initialize` report it.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Socket-protocol compatibility integer. Bumped only on a breaking protocol change.
pub const PROTOCOL_VERSION: u32 = 1;

/// Stable feature strings. Sorted, `lower_snake_case`, append-only. Later issues add their own
/// string here when they ship (see the module docs).
pub const FEATURES: &[&str] = &[
    "chunk_supersede",
    "entity_provenance",
    "hybrid_passage_search",
    "lint",
    "origin",
    "process_document",
    "walk",
    "writable_groups",
];

/// Sorted, de-duplicated names of every IPC method the dispatcher accepts.
pub fn methods() -> Vec<&'static str> {
    let mut m: Vec<&'static str> = crate::write_policy::CLASSIFICATION
        .iter()
        .map(|(name, _)| *name)
        .collect();
    m.sort_unstable();
    m.dedup();
    m
}

/// Sorted, de-duplicated feature strings.
pub fn features() -> Vec<&'static str> {
    let mut f: Vec<&'static str> = FEATURES.to_vec();
    f.sort_unstable();
    f.dedup();
    f
}

/// The `knowledge_capabilities` result.
pub fn payload() -> Value {
    json!({
        "server_version": SERVER_VERSION,
        "protocol_version": PROTOCOL_VERSION,
        "features": features(),
        "methods": methods(),
    })
}

/// Merges `server_version` and `protocol_version` into a JSON object (used by `health_check`).
pub fn with_version_fields(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut() {
        obj.insert("server_version".to_string(), json!(SERVER_VERSION));
        obj.insert("protocol_version".to_string(), json!(PROTOCOL_VERSION));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_are_sorted_unique_lower_snake_case() {
        let mut sorted = FEATURES.to_vec();
        sorted.sort_unstable();
        assert_eq!(FEATURES, sorted.as_slice(), "FEATURES must stay sorted");
        let mut dedup = sorted.clone();
        dedup.dedup();
        assert_eq!(dedup, sorted, "FEATURES must be duplicate-free");
        for f in FEATURES {
            assert!(
                !f.is_empty()
                    && f.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "feature {f:?} is not lower_snake_case"
            );
        }
    }

    #[test]
    fn methods_are_sorted_unique_and_include_self_and_health_check() {
        let m = methods();
        let mut sorted = m.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(m, sorted);
        assert!(m.contains(&"health_check"));
        assert!(m.contains(&"knowledge_capabilities"));
    }

    #[test]
    fn payload_has_the_four_fields() {
        let p = payload();
        assert_eq!(p["server_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(p["protocol_version"], 1);
        assert!(p["features"].is_array());
        assert!(p["methods"].is_array());
    }

    #[test]
    fn with_version_fields_is_additive() {
        let v = with_version_fields(json!({"ok": true}));
        assert_eq!(v["ok"], true);
        assert_eq!(v["server_version"], SERVER_VERSION);
        assert_eq!(v["protocol_version"], 1);
    }
}
