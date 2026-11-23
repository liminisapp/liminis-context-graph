//! Episode origin (issue #752, ADR-0752): a closed-vocabulary, caller-declared label saying what
//! kind of content an episode is, so an agent's own derived pages never corroborate themselves.
//!
//! This module is pure (no I/O): the vocabulary, the corroborating / non-corroborating
//! classification, parameter parsing and the per-document support tally. Storage lives in
//! `Episodic.origin` (`schema`, `db`); a NULL or empty value reads as [`Origin::Primary`], so
//! graphs and WALs written before this feature need no backfill.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::error::Error;

/// Prefix of every refusal `Error::Ipc` message raised by an origin guard (FR-011/012/013).
pub const ORIGIN_CONFLICT_PREFIX: &str = "origin_conflict:";

/// What kind of content an episode is. A caller's claim, never authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Origin {
    /// A human-written note. The default.
    Primary,
    /// Clipped or imported third-party material.
    External,
    /// An agent-written page or a consolidation.
    Derived,
    /// A filed Q&A response.
    Answer,
}

/// The closed vocabulary, in the fixed order used for `support` objects and error messages.
pub const ALL: [Origin; 4] = [
    Origin::Primary,
    Origin::External,
    Origin::Derived,
    Origin::Answer,
];

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Primary => "primary",
            Origin::External => "external",
            Origin::Derived => "derived",
            Origin::Answer => "answer",
        }
    }

    /// `primary` and `external` are sources in their own right; `derived` and `answer` only cite.
    pub fn is_corroborating(self) -> bool {
        matches!(self, Origin::Primary | Origin::External)
    }

    fn index(self) -> usize {
        match self {
            Origin::Primary => 0,
            Origin::External => 1,
            Origin::Derived => 2,
            Origin::Answer => 3,
        }
    }

    /// Strict parse of a caller-supplied value; the error names the allowed values.
    pub fn parse(s: &str) -> Result<Origin, Error> {
        match s {
            "primary" => Ok(Origin::Primary),
            "external" => Ok(Origin::External),
            "derived" => Ok(Origin::Derived),
            "answer" => Ok(Origin::Answer),
            other => Err(Error::Ipc(format!(
                "invalid origin {other:?}: must be one of {}",
                vocabulary()
            ))),
        }
    }

    /// Lenient read of a stored value: NULL, empty (a dumped NULL) and anything unrecognised read
    /// as `primary` (FR-002: no origin means primary).
    pub fn from_stored(s: Option<&str>) -> Origin {
        match s {
            Some(v) => Origin::parse(v).unwrap_or(Origin::Primary),
            None => Origin::Primary,
        }
    }
}

/// `"primary, external, derived, answer"` — for error messages.
pub fn vocabulary() -> String {
    ALL.iter()
        .map(|o| o.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolves the optional write-side `origin` param: absent → `primary`, otherwise validated.
pub fn resolve(raw: Option<&str>) -> Result<Origin, Error> {
    match raw {
        None => Ok(Origin::Primary),
        Some(s) => Origin::parse(s),
    }
}

/// Parses the optional `origin` request param (a JSON string, or absent / null).
pub fn parse_origin_param(p: &Value) -> Result<Option<Origin>, Error> {
    match p.get("origin") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(Origin::parse(s)?)),
        Some(_) => Err(Error::Ipc(format!(
            "origin must be a string, one of {}",
            vocabulary()
        ))),
    }
}

/// Parses the optional `origins` read filter: absent / null → `None`; otherwise a non-empty array
/// of vocabulary values (duplicates collapse). An unknown value or an empty list is an error.
pub fn parse_origins_param(p: &Value) -> Result<Option<Vec<Origin>>, Error> {
    let arr = match p.get("origins") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(a)) => a,
        Some(_) => {
            return Err(Error::Ipc(format!(
                "origins must be a non-empty array of: {}",
                vocabulary()
            )))
        }
    };
    if arr.is_empty() {
        return Err(Error::Ipc(format!(
            "origins must be a non-empty array of: {}",
            vocabulary()
        )));
    }
    let mut out: Vec<Origin> = Vec::new();
    for v in arr {
        let s = v.as_str().ok_or_else(|| {
            Error::Ipc(format!(
                "origins entries must be strings from: {}",
                vocabulary()
            ))
        })?;
        let o = Origin::parse(s)?;
        if !out.contains(&o) {
            out.push(o);
        }
    }
    Ok(Some(out))
}

/// The refusal error shape shared by every origin guard.
pub fn conflict(msg: impl std::fmt::Display) -> Error {
    Error::Ipc(format!("{ORIGIN_CONFLICT_PREFIX} {msg}"))
}

/// The identity a source document is counted under: its `doc_id` where present, otherwise the
/// episode itself (the best available identity, FR-007).
pub fn doc_key(doc_id: Option<&str>, episode_uuid: &str) -> String {
    match doc_id {
        Some(d) if !d.is_empty() => format!("doc:{d}"),
        _ => format!("ep:{episode_uuid}"),
    }
}

/// Origin, document and liveness of one stored episode (the read side of [`Origin`]).
#[derive(Debug, Clone)]
pub struct EpisodeOriginInfo {
    pub origin: Origin,
    pub doc_id: Option<String>,
    /// Microseconds since the epoch; `None` while no supersession has touched the episode.
    pub superseded_at_us: Option<i64>,
    pub group_id: String,
    /// Whether the episode carries a document-mode `revision_id` (ADR-0785).
    pub revision_id_present: bool,
    /// Microseconds since the epoch of the revision's commit; `None` while it is pending.
    pub revision_committed_at_us: Option<i64>,
}

impl EpisodeOriginInfo {
    /// Whether the episode is live at `t_us` — exactly `EpisodeSupersession::live_at`, the
    /// belief/search predicate: visible (not part of an uncommitted document revision) and not
    /// superseded by `t_us`. A document supersession still pending at a future date counts as
    /// live; an episode of a pending revision does not (issue #883).
    pub fn is_live(&self, t_us: i64) -> bool {
        crate::belief::revision_visible_at(
            self.revision_id_present,
            self.revision_committed_at_us,
            t_us,
        ) && self.is_unsuperseded(t_us)
    }

    /// Not superseded at `t_us`, regardless of revision state. Used only by the ingest veto
    /// (`entities_with_corroborating_mention`), where a pending primary revision must still
    /// shield its entity from derived rewrites.
    pub fn is_unsuperseded(&self, t_us: i64) -> bool {
        self.superseded_at_us.is_none_or(|s| s > t_us)
    }
}

/// Per-origin count of distinct source documents.
#[derive(Debug, Default, Clone)]
pub struct SupportTally {
    docs: [HashSet<String>; 4],
}

impl SupportTally {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one episode. Many chunks sharing a `doc_id` count once.
    pub fn add(&mut self, origin: Origin, doc_id: Option<&str>, episode_uuid: &str) {
        self.docs[origin.index()].insert(doc_key(doc_id, episode_uuid));
    }

    pub fn count(&self, origin: Origin) -> usize {
        self.docs[origin.index()].len()
    }

    /// Distinct corroborating documents (primary + external).
    pub fn corroboration(&self) -> usize {
        ALL.iter()
            .filter(|o| o.is_corroborating())
            .map(|o| self.count(*o))
            .sum()
    }

    /// Whether any primary or external document is present.
    pub fn has_corroborating(&self) -> bool {
        self.corroboration() > 0
    }

    /// `{primary, external, derived, answer}` — all four keys, always (zero-valued included).
    pub fn to_json(&self) -> Value {
        json!({
            "primary": self.count(Origin::Primary),
            "external": self.count(Origin::External),
            "derived": self.count(Origin::Derived),
            "answer": self.count(Origin::Answer),
        })
    }
}

/// One stamp rule: episodes matching **all** of its selectors get `origin`.
#[derive(Debug, Clone)]
pub struct StampRule {
    pub origin: Origin,
    /// `source_description STARTS WITH` this (the source file precedes the `:chunk_id`).
    pub source_path_prefix: Option<String>,
    /// The episode's `attributes.kind` equals this string.
    pub attributes_kind: Option<String>,
}

impl StampRule {
    fn matches(&self, source_description: &str, attributes: &str) -> bool {
        if let Some(prefix) = &self.source_path_prefix {
            if !source_description.starts_with(prefix.as_str()) {
                return false;
            }
        }
        if let Some(kind) = &self.attributes_kind {
            let have = serde_json::from_str::<Value>(attributes)
                .ok()
                .and_then(|v| v.get("kind").and_then(|k| k.as_str().map(str::to_string)));
            if have.as_deref() != Some(kind.as_str()) {
                return false;
            }
        }
        true
    }
}

/// Parses the `rules` param of `knowledge_stamp_origin`: a non-empty array of
/// `{origin, source_path_prefix?, attributes_kind?}`, each with at least one selector.
pub fn parse_stamp_rules(p: &Value) -> Result<Vec<StampRule>, Error> {
    let arr = p
        .get("rules")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| Error::Ipc("rules is required and must be a non-empty array".to_string()))?;
    let mut rules = Vec::with_capacity(arr.len());
    for (i, r) in arr.iter().enumerate() {
        let origin = Origin::parse(r.get("origin").and_then(Value::as_str).ok_or_else(|| {
            Error::Ipc(format!(
                "rules[{i}].origin is required, one of {}",
                vocabulary()
            ))
        })?)?;
        let opt = |key: &str| -> Result<Option<String>, Error> {
            match r.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
                _ => Err(Error::Ipc(format!(
                    "rules[{i}].{key} must be a non-empty string when provided"
                ))),
            }
        };
        let source_path_prefix = opt("source_path_prefix")?;
        let attributes_kind = opt("attributes_kind")?;
        if source_path_prefix.is_none() && attributes_kind.is_none() {
            return Err(Error::Ipc(format!(
                "rules[{i}] needs at least one selector: source_path_prefix or attributes_kind"
            )));
        }
        rules.push(StampRule {
            origin,
            source_path_prefix,
            attributes_kind,
        });
    }
    Ok(rules)
}

/// One episode as the stamp planner sees it.
#[derive(Debug, Clone)]
pub struct StampInput {
    pub uuid: String,
    pub doc_id: Option<String>,
    pub current: Origin,
    pub source_description: String,
    pub attributes: String,
}

/// An episode whose origin the stamp would change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StampChange {
    pub uuid: String,
    pub doc_id: Option<String>,
    pub from: Origin,
    pub to: Origin,
}

/// The outcome of planning a stamp: nothing is written by planning.
#[derive(Debug, Clone, Default)]
pub struct StampPlan {
    /// Episodes selected (after expanding whole documents).
    pub matched: usize,
    pub changes: Vec<StampChange>,
}

/// Plans a stamp over `episodes`. Rules are evaluated in order and the first match wins. An
/// episode selected by a rule that carries a `doc_id` pulls in every episode of that document, so
/// one `doc_id` never ends up with two origins (FR-013); a rule set that would give a document two
/// different origins is refused. Idempotent: episodes already at their target are counted as
/// matched but not as changes.
pub fn plan_stamp(episodes: &[StampInput], rules: &[StampRule]) -> Result<StampPlan, Error> {
    let mut doc_target: std::collections::HashMap<&str, Origin> = std::collections::HashMap::new();
    let mut direct: std::collections::HashMap<&str, Origin> = std::collections::HashMap::new();
    for e in episodes {
        let Some(rule) = rules
            .iter()
            .find(|r| r.matches(&e.source_description, &e.attributes))
        else {
            continue;
        };
        direct.insert(e.uuid.as_str(), rule.origin);
        if let Some(d) = e.doc_id.as_deref().filter(|d| !d.is_empty()) {
            match doc_target.get(d) {
                Some(prev) if *prev != rule.origin => {
                    return Err(conflict(format!(
                        "the rules would give document {d:?} two origins ({:?} and {:?}); one \
                         document has one origin",
                        prev.as_str(),
                        rule.origin.as_str()
                    )));
                }
                _ => {
                    doc_target.insert(d, rule.origin);
                }
            }
        }
    }
    let mut plan = StampPlan::default();
    for e in episodes {
        let target = match e.doc_id.as_deref().and_then(|d| doc_target.get(d)) {
            Some(t) => Some(*t),
            None => direct.get(e.uuid.as_str()).copied(),
        };
        let Some(to) = target else { continue };
        plan.matched += 1;
        if e.current != to {
            plan.changes.push(StampChange {
                uuid: e.uuid.clone(),
                doc_id: e.doc_id.clone(),
                from: e.current,
                to,
            });
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_vocabulary_and_names_it_on_error() {
        for o in ALL {
            assert_eq!(Origin::parse(o.as_str()).unwrap(), o);
        }
        let err = Origin::parse("wiki").unwrap_err().to_string();
        assert!(err.contains("primary, external, derived, answer"), "{err}");
    }

    #[test]
    fn missing_or_empty_stored_origin_is_primary() {
        assert_eq!(resolve(None).unwrap(), Origin::Primary);
        assert_eq!(Origin::from_stored(None), Origin::Primary);
        assert_eq!(Origin::from_stored(Some("")), Origin::Primary);
        assert_eq!(Origin::from_stored(Some("derived")), Origin::Derived);
    }

    #[test]
    fn classification() {
        assert!(Origin::Primary.is_corroborating());
        assert!(Origin::External.is_corroborating());
        assert!(!Origin::Derived.is_corroborating());
        assert!(!Origin::Answer.is_corroborating());
    }

    #[test]
    fn origin_param_parsing() {
        assert_eq!(parse_origin_param(&json!({})).unwrap(), None);
        assert_eq!(parse_origin_param(&json!({"origin": null})).unwrap(), None);
        assert_eq!(
            parse_origin_param(&json!({"origin": "derived"})).unwrap(),
            Some(Origin::Derived)
        );
        assert!(parse_origin_param(&json!({"origin": "x"})).is_err());
        assert!(parse_origin_param(&json!({"origin": 3})).is_err());
    }

    #[test]
    fn origins_filter_parsing() {
        assert_eq!(parse_origins_param(&json!({})).unwrap(), None);
        assert_eq!(
            parse_origins_param(&json!({"origins": ["primary", "external", "primary"]})).unwrap(),
            Some(vec![Origin::Primary, Origin::External])
        );
        assert!(parse_origins_param(&json!({"origins": []})).is_err());
        assert!(parse_origins_param(&json!({"origins": ["nope"]})).is_err());
        assert!(parse_origins_param(&json!({"origins": "primary"})).is_err());
        assert!(parse_origins_param(&json!({"origins": [1]})).is_err());
    }

    #[test]
    fn tally_counts_distinct_documents_not_episodes() {
        let mut t = SupportTally::new();
        // One long note split into three chunks, all under one doc_id (SC-008).
        t.add(Origin::Primary, Some("note-a"), "e1");
        t.add(Origin::Primary, Some("note-a"), "e2");
        t.add(Origin::Primary, Some("note-a"), "e3");
        assert_eq!(t.count(Origin::Primary), 1);
        // Episodes without a doc_id each count as their own document.
        t.add(Origin::Primary, None, "e4");
        t.add(Origin::Primary, None, "e5");
        assert_eq!(t.count(Origin::Primary), 3);
        t.add(Origin::Derived, Some("wiki-page"), "e6");
        assert_eq!(t.count(Origin::Derived), 1);
        assert_eq!(t.corroboration(), 3);
        assert_eq!(
            t.to_json(),
            json!({"primary": 3, "external": 0, "derived": 1, "answer": 0})
        );
    }

    #[test]
    fn derived_never_adds_to_corroboration() {
        let mut t = SupportTally::new();
        t.add(Origin::Primary, None, "e1");
        t.add(Origin::Derived, None, "e2");
        t.add(Origin::Answer, None, "e3");
        assert_eq!(t.corroboration(), 1);
        assert!(t.has_corroborating());
    }

    fn input(uuid: &str, doc: Option<&str>, cur: Origin, src: &str, attrs: &str) -> StampInput {
        StampInput {
            uuid: uuid.into(),
            doc_id: doc.map(Into::into),
            current: cur,
            source_description: src.into(),
            attributes: attrs.into(),
        }
    }

    #[test]
    fn plan_first_matching_rule_wins_and_is_idempotent() {
        let rules = parse_stamp_rules(&json!({"rules": [
            {"origin": "derived", "source_path_prefix": "wiki/"},
            {"origin": "answer", "source_path_prefix": "wiki/qa"},
            {"origin": "external", "attributes_kind": "clip"},
        ]}))
        .unwrap();
        let eps = vec![
            input("a", None, Origin::Primary, "wiki/qa/1.md:c", "{}"),
            input(
                "b",
                None,
                Origin::Primary,
                "notes/x.md:c",
                r#"{"kind":"clip"}"#,
            ),
            input("c", None, Origin::Primary, "notes/y.md:c", "{}"),
            input("d", None, Origin::Derived, "wiki/z.md:c", "{}"),
        ];
        let plan = plan_stamp(&eps, &rules).unwrap();
        assert_eq!(plan.matched, 3);
        let to: Vec<(&str, Origin)> = plan
            .changes
            .iter()
            .map(|c| (c.uuid.as_str(), c.to))
            .collect();
        assert_eq!(to, vec![("a", Origin::Derived), ("b", Origin::External)]);
        // Re-planning over the stamped state changes nothing.
        let stamped: Vec<StampInput> = eps
            .iter()
            .map(|e| {
                let mut e = e.clone();
                if let Some(c) = plan.changes.iter().find(|c| c.uuid == e.uuid) {
                    e.current = c.to;
                }
                e
            })
            .collect();
        assert!(plan_stamp(&stamped, &rules).unwrap().changes.is_empty());
    }

    #[test]
    fn plan_expands_documents_and_refuses_split_origins() {
        let rules = parse_stamp_rules(&json!({"rules": [
            {"origin": "derived", "source_path_prefix": "wiki/"},
            {"origin": "external", "source_path_prefix": "clips/"},
        ]}))
        .unwrap();
        let ok = vec![
            input("a", Some("d"), Origin::Primary, "wiki/p.md:1", "{}"),
            input("b", Some("d"), Origin::Primary, "other/p.md:2", "{}"),
        ];
        assert_eq!(plan_stamp(&ok, &rules).unwrap().changes.len(), 2);
        let split = vec![
            input("a", Some("d"), Origin::Primary, "wiki/p.md:1", "{}"),
            input("b", Some("d"), Origin::Primary, "clips/p.md:2", "{}"),
        ];
        assert!(plan_stamp(&split, &rules).is_err());
    }

    #[test]
    fn stamp_rules_need_a_selector_and_a_valid_origin() {
        assert!(parse_stamp_rules(&json!({})).is_err());
        assert!(parse_stamp_rules(&json!({"rules": []})).is_err());
        assert!(parse_stamp_rules(&json!({"rules": [{"origin": "derived"}]})).is_err());
        assert!(
            parse_stamp_rules(&json!({"rules": [{"origin": "x", "attributes_kind": "k"}]}))
                .is_err()
        );
    }

    #[test]
    fn conflict_error_carries_prefix() {
        assert!(conflict("x").to_string().contains("origin_conflict: x"));
    }
    /// SC-001 (issue #883): the entity read and the belief/search predicate classify every
    /// episode identically, over the whole revision × supersession grid.
    #[test]
    fn is_live_matches_episode_supersession_live_at() {
        use crate::belief::EpisodeSupersession;
        let t = 100;
        let revisions = [
            (false, None),
            (true, None),
            (true, Some(50)),
            (true, Some(500)),
        ];
        let superseded = [None, Some(50), Some(500)];
        for (present, committed) in revisions {
            for s in superseded {
                let info = EpisodeOriginInfo {
                    origin: Origin::Primary,
                    doc_id: None,
                    superseded_at_us: s,
                    group_id: "g".into(),
                    revision_id_present: present,
                    revision_committed_at_us: committed,
                };
                let sup = EpisodeSupersession {
                    superseded_at_us: s,
                    revision_id_present: present,
                    revision_committed_at_us: committed,
                    ..Default::default()
                };
                assert_eq!(
                    info.is_live(t),
                    sup.live_at(t),
                    "revision=({present},{committed:?}) superseded={s:?}"
                );
                // The veto predicate ignores revision state.
                assert_eq!(info.is_unsuperseded(t), s.is_none_or(|x| x > t));
            }
        }
    }
}
