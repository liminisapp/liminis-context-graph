//! The skeleton → graph mapping (#819, FR-005): data kept next to the fixture, never in code.
//!
//! Format (`graph-mapping.json`): see `docs/graph-integrity-evaluation.md`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// Orientation of an edge relative to the skeleton fact's subject (the `entity` field).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Edge source is the fact's entity, edge target is the value entity.
    SubjectToObject,
    /// Edge source is the value entity, edge target is the fact's entity.
    ObjectToSubject,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct KindMap {
    /// Node type labels accepted for entities of this skeleton kind.
    pub node_types: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RelationExpect {
    pub name: String,
    pub direction: Direction,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LinkMap {
    pub relation_types: Vec<RelationExpect>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct JunkMap {
    #[serde(default)]
    pub prefixes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawMapping {
    mapping_version: u32,
    kinds: BTreeMap<String, KindMap>,
    links: BTreeMap<String, LinkMap>,
    #[serde(default)]
    junk: JunkMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub mapping_version: u32,
    pub kinds: BTreeMap<String, KindMap>,
    pub links: BTreeMap<String, LinkMap>,
    pub junk: JunkMap,
    /// Full sha256 of the raw file bytes; identifies the mapping in reports and `--compare`.
    pub sha256: String,
}

impl Mapping {
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: RawMapping =
            serde_json::from_str(text).map_err(|e| format!("invalid mapping JSON: {e}"))?;
        if raw.mapping_version != 1 {
            return Err(format!(
                "unsupported mapping_version {} (expected 1)",
                raw.mapping_version
            ));
        }
        for (k, m) in &raw.kinds {
            if m.node_types.is_empty() || m.node_types.iter().any(|t| t.trim().is_empty()) {
                return Err(format!("kind {k:?}: node_types must be non-empty"));
            }
        }
        for (a, m) in &raw.links {
            if m.relation_types.is_empty()
                || m.relation_types.iter().any(|t| t.name.trim().is_empty())
            {
                return Err(format!("link {a:?}: relation_types must be non-empty"));
            }
        }
        Ok(Self {
            mapping_version: raw.mapping_version,
            kinds: raw.kinds,
            links: raw.links,
            junk: raw.junk,
            sha256: lcg_core::sha256_hex::sha256_hex(text.as_bytes()),
        })
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read mapping {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"{"mapping_version":1,
      "kinds":{"project":{"node_types":["Project"]}},
      "links":{"Owner":{"relation_types":[{"name":"OWNED_BY","direction":"subject_to_object"},{"name":"OWNS","direction":"object_to_subject"}]}},
      "junk":{"prefixes":["recap-"]}}"#;

    #[test]
    fn parses_valid_mapping() {
        let m = Mapping::parse(OK).unwrap();
        assert_eq!(
            m.links["Owner"].relation_types[1].direction,
            Direction::ObjectToSubject
        );
        assert_eq!(m.junk.prefixes, vec!["recap-"]);
        assert_eq!(m.sha256.len(), 64);
    }

    #[test]
    fn rejects_unknown_direction_empty_lists_and_bad_version() {
        assert!(Mapping::parse(&OK.replace("subject_to_object", "sideways")).is_err());
        assert!(Mapping::parse(&OK.replace(r#"["Project"]"#, "[]")).is_err());
        assert!(Mapping::parse(
            r#"{"mapping_version":1,"kinds":{},"links":{"A":{"relation_types":[]}}}"#
        )
        .is_err());
        assert!(
            Mapping::parse(&OK.replace(r#""mapping_version":1"#, r#""mapping_version":2"#))
                .is_err()
        );
    }
}
