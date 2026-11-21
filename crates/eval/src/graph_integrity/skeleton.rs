//! Minimal view of a fixture `skeleton.json` (#819): the ground-truth entities, note IDs and facts.

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SkeletonEntity {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SkeletonFact {
    pub entity: String,
    pub attribute: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
struct RawNote {
    id: String,
    #[serde(default)]
    facts: Vec<SkeletonFact>,
}

#[derive(Debug, Deserialize)]
struct RawSkeleton {
    entities: Vec<SkeletonEntity>,
    #[serde(default)]
    notes: Vec<RawNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skeleton {
    pub entities: Vec<SkeletonEntity>,
    pub note_ids: Vec<String>,
    /// Every fact of every note, in file order (duplicates across notes are kept; scoring
    /// de-duplicates).
    pub facts: Vec<SkeletonFact>,
    /// Full sha256 of the raw file bytes; identifies the skeleton in reports and `--compare`.
    pub sha256: String,
}

impl Skeleton {
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: RawSkeleton =
            serde_json::from_str(text).map_err(|e| format!("invalid skeleton JSON: {e}"))?;
        if raw.entities.is_empty() {
            return Err("skeleton has no entities".to_string());
        }
        let note_ids = raw.notes.iter().map(|n| n.id.clone()).collect();
        let facts = raw.notes.into_iter().flat_map(|n| n.facts).collect();
        Ok(Self {
            entities: raw.entities,
            note_ids,
            facts,
            sha256: lcg_core::sha256_hex::sha256_hex(text.as_bytes()),
        })
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read skeleton {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entities_notes_and_facts_tolerating_notes_without_facts() {
        let t = r#"{"entities":[{"name":"A","kind":"project","extra":1}],
            "notes":[{"id":"n1","facts":[{"entity":"A","attribute":"Budget","value":"$1","statement":"s"}]},{"id":"n2"}]}"#;
        let s = Skeleton::parse(t).unwrap();
        assert_eq!(s.entities.len(), 1);
        assert_eq!(s.note_ids, vec!["n1", "n2"]);
        assert_eq!(s.facts.len(), 1);
        assert_eq!(s.sha256.len(), 64);
    }

    #[test]
    fn rejects_empty_and_malformed() {
        assert!(Skeleton::parse("{}").is_err());
        assert!(Skeleton::parse(r#"{"entities":[]}"#).is_err());
        assert!(Skeleton::parse("nope").is_err());
    }
}
