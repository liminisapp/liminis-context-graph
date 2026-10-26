//! Belief time (issue #705, ADR-0705): *when the graph believed* an edge, derived rather than
//! stored.
//!
//! Belief is distinct from validity (`valid_at` / `invalid_at`, the world's clock). An edge is
//! believed at time `t` iff it was ingested at or before `t` and, for extracted edges, at least
//! one episode in its `episodes` column was not superseded at or before `t` (FR-007). Nothing is
//! written onto an edge when its episode is superseded — [`evaluate`] reads the supersession
//! state of the producing episodes at query time.

use std::collections::HashMap;

use serde::Serialize;

/// The belief-relevant columns of one `RelatesToNode_` row. Timestamps are microseconds since the
/// Unix epoch: the whole-second `value_as_timestamp_str` read-back would collapse a correction and
/// the edges it ingested onto one instant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeBeliefRow {
    pub uuid: String,
    pub ingested_at_us: Option<i64>,
    pub episodes: Vec<String>,
    pub warrant: Option<String>,
    pub valid_at_source: Option<String>,
}

/// Supersession state of one `Episodic` row. An episode absent from the lookup map was
/// hard-deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EpisodeSupersession {
    pub superseded_at_us: Option<i64>,
}

/// How an edge stands at the evaluated instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BeliefState {
    /// Extracted, with at least one producing episode live at `t`; or warranted (no episode).
    Believed,
    /// Every producing episode that still exists had been superseded at or before `t`.
    Superseded,
    /// No producing episode and no warrant: a pre-#705 row. Treated as believed, reported as legacy.
    Legacy,
    /// Every producing episode was hard-deleted (`knowledge_delete_episode` and friends). The
    /// evidence is gone, so the fact is unbelieved at every `t`.
    SupportDeleted,
    /// The edge was first written after `t`.
    NotYetIngested,
}

impl BeliefState {
    /// Whether an edge in this state counts as believed (legacy rows do, FR-007).
    pub fn is_believed(self) -> bool {
        matches!(self, BeliefState::Believed | BeliefState::Legacy)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BeliefState::Believed => "believed",
            BeliefState::Superseded => "superseded",
            BeliefState::Legacy => "legacy",
            BeliefState::SupportDeleted => "support_deleted",
            BeliefState::NotYetIngested => "not_yet_ingested",
        }
    }
}

/// FR-007. `episodes` maps a producing-episode uuid to its supersession state; a uuid missing from
/// the map means the episode no longer exists.
pub fn evaluate(
    edge: &EdgeBeliefRow,
    episodes: &HashMap<String, EpisodeSupersession>,
    t_us: i64,
) -> BeliefState {
    if edge.ingested_at_us.is_some_and(|i| i > t_us) {
        return BeliefState::NotYetIngested;
    }
    if edge.episodes.is_empty() {
        return if edge.warrant.is_some() {
            BeliefState::Believed
        } else {
            BeliefState::Legacy
        };
    }
    let mut any_present = false;
    for uuid in &edge.episodes {
        if let Some(ep) = episodes.get(uuid) {
            any_present = true;
            if ep.superseded_at_us.is_none_or(|s| s > t_us) {
                return BeliefState::Believed;
            }
        }
    }
    if any_present {
        BeliefState::Superseded
    } else {
        BeliefState::SupportDeleted
    }
}

/// The `belief` annotation attached to each edge by an opted-in read (FR-009).
#[derive(Debug, Clone, Serialize)]
pub struct BeliefAnnotation {
    pub state: BeliefState,
    pub believed: bool,
    pub producing_episode_uuids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warrant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_at_source: Option<String>,
}

impl BeliefAnnotation {
    pub fn new(edge: &EdgeBeliefRow, state: BeliefState) -> Self {
        Self {
            state,
            believed: state.is_believed(),
            producing_episode_uuids: edge.episodes.clone(),
            warrant: edge.warrant.clone(),
            valid_at_source: edge.valid_at_source.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(ingested: i64, episodes: &[&str], warrant: Option<&str>) -> EdgeBeliefRow {
        EdgeBeliefRow {
            uuid: "e".into(),
            ingested_at_us: Some(ingested),
            episodes: episodes.iter().map(|s| s.to_string()).collect(),
            warrant: warrant.map(str::to_string),
            valid_at_source: None,
        }
    }

    fn eps(entries: &[(&str, Option<i64>)]) -> HashMap<String, EpisodeSupersession> {
        entries
            .iter()
            .map(|(u, s)| {
                (
                    u.to_string(),
                    EpisodeSupersession {
                        superseded_at_us: *s,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn believed_on_half_open_interval_t1_t2() {
        // Ingested at 100, episode superseded at 200: believed on [100, 200).
        let e = edge(100, &["a"], None);
        let m = eps(&[("a", Some(200))]);
        assert_eq!(evaluate(&e, &m, 99), BeliefState::NotYetIngested);
        assert_eq!(evaluate(&e, &m, 100), BeliefState::Believed);
        assert_eq!(evaluate(&e, &m, 199), BeliefState::Believed);
        assert_eq!(evaluate(&e, &m, 200), BeliefState::Superseded);
        assert_eq!(evaluate(&e, &m, 10_000), BeliefState::Superseded);
    }

    #[test]
    fn live_episode_is_believed_forever() {
        let e = edge(100, &["a"], None);
        let m = eps(&[("a", None)]);
        assert_eq!(evaluate(&e, &m, i64::MAX), BeliefState::Believed);
    }

    #[test]
    fn believed_until_every_source_is_superseded() {
        let e = edge(100, &["a", "b"], None);
        let m = eps(&[("a", Some(200)), ("b", Some(300))]);
        assert_eq!(evaluate(&e, &m, 250), BeliefState::Believed);
        assert_eq!(evaluate(&e, &m, 300), BeliefState::Superseded);
    }

    #[test]
    fn warranted_edge_without_episode_is_believed() {
        let e = edge(100, &[], Some("assertion"));
        assert_eq!(evaluate(&e, &eps(&[]), 500), BeliefState::Believed);
        assert_eq!(evaluate(&e, &eps(&[]), 50), BeliefState::NotYetIngested);
    }

    #[test]
    fn legacy_edge_is_believed_and_reported_legacy() {
        let e = edge(100, &[], None);
        let s = evaluate(&e, &eps(&[]), 500);
        assert_eq!(s, BeliefState::Legacy);
        assert!(s.is_believed());
    }

    #[test]
    fn hard_deleted_producing_episode_is_unbelieved_at_every_t() {
        let e = edge(100, &["gone"], None);
        let m = eps(&[]);
        assert_eq!(evaluate(&e, &m, 150), BeliefState::SupportDeleted);
        assert_eq!(evaluate(&e, &m, i64::MAX), BeliefState::SupportDeleted);
        assert!(!BeliefState::SupportDeleted.is_believed());
    }

    #[test]
    fn missing_ingested_at_does_not_gate() {
        let mut e = edge(0, &["a"], None);
        e.ingested_at_us = None;
        assert_eq!(evaluate(&e, &eps(&[("a", None)]), 1), BeliefState::Believed);
    }
}
