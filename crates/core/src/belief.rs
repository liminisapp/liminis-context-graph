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
    /// `episode` or `document` (issue #707); `None` for `chunk_removed` or a live episode.
    pub kind: Option<String>,
    /// Superseding episode uuid, or the superseding document's `doc_id`.
    pub superseded_by: Option<String>,
    pub reason: Option<String>,
    pub group_id: String,
    /// The logical document the episode came from (issue #707).
    pub doc_id: Option<String>,
}

impl EpisodeSupersession {
    fn is_document_superseded(&self) -> bool {
        self.kind.as_deref() == Some("document")
    }
}

/// One durable `DocSupersession` record (issue #707): document `superseding_doc_id` replaced
/// `superseded_doc_id` within a group. `effective_at_us` is where "in force" ends;
/// `recorded_at_us` is when the graph learned of it. Never an edge field: validity is untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocSupersessionRecord {
    pub group_id: String,
    pub superseded_doc_id: String,
    pub superseding_doc_id: String,
    pub effective_at_us: i64,
    pub recorded_at_us: i64,
}

/// Document supersession records keyed by `(group_id, superseded_doc_id)`.
pub type DocRecords = HashMap<(String, String), DocSupersessionRecord>;

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

/// Why an edge has no recorded belief interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoRecordCause {
    /// A pre-#705 row: no producing episode and no warrant.
    Legacy,
    /// Every producing episode was hard-deleted, so the evidence (and its end) is gone.
    SupportDeleted,
}

impl NoRecordCause {
    pub fn as_str(self) -> &'static str {
        match self {
            NoRecordCause::Legacy => "legacy",
            NoRecordCause::SupportDeleted => "support_deleted",
        }
    }
}

/// The belief interval `[T1, T2)` of an edge (issue #706): derived from the same records as
/// [`evaluate`], never stored. T2 is not exposed by [`evaluate`], which only answers "believed at
/// `t`?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeliefInterval {
    /// At least one surviving producing episode is live: no end yet.
    Open,
    /// The interval was never recorded (legacy row, or all support hard-deleted).
    NoRecord(NoRecordCause),
    /// A warranted (direct-assertion) edge: believed until a retraction primitive exists, so the
    /// interval never closes. Distinct from `Open` because it has no producing episode at all.
    NotDefined,
    /// Every surviving producing episode was superseded. `t1_us` is the edge's `ingested_at`
    /// (absent only on rows predating ADR-0673's stamp, which are then treated as `i64::MIN`);
    /// `t2_us` is the last supersession instant among the surviving producers.
    /// `derived_from_surviving_support` is true when some producer no longer exists, so `t2_us`
    /// ignores it.
    Closed {
        t1_us: i64,
        t2_us: i64,
        derived_from_surviving_support: bool,
    },
}

/// Classifies the belief interval of `edge` (see [`BeliefInterval`]). `episodes` has the same
/// meaning as for [`evaluate`]: a producing-episode uuid missing from the map was hard-deleted.
pub fn belief_interval(
    edge: &EdgeBeliefRow,
    episodes: &HashMap<String, EpisodeSupersession>,
) -> BeliefInterval {
    if edge.episodes.is_empty() {
        return if edge.warrant.is_some() {
            BeliefInterval::NotDefined
        } else {
            BeliefInterval::NoRecord(NoRecordCause::Legacy)
        };
    }
    let mut present = 0usize;
    let mut t2: Option<i64> = None;
    for uuid in &edge.episodes {
        let Some(ep) = episodes.get(uuid) else {
            continue;
        };
        present += 1;
        match ep.superseded_at_us {
            None => return BeliefInterval::Open,
            Some(s) => t2 = Some(t2.map_or(s, |m| m.max(s))),
        }
    }
    match t2 {
        None => BeliefInterval::NoRecord(NoRecordCause::SupportDeleted),
        Some(t2_us) => BeliefInterval::Closed {
            t1_us: edge.ingested_at_us.unwrap_or(i64::MIN),
            t2_us,
            derived_from_surviving_support: present < edge.episodes.len(),
        },
    }
}

/// The document supersession record that applies to a document-superseded episode, if any.
fn doc_record<'a>(
    ep: &EpisodeSupersession,
    docs: &'a DocRecords,
) -> Option<&'a DocSupersessionRecord> {
    if !ep.is_document_superseded() {
        return None;
    }
    let doc = ep.doc_id.as_ref()?;
    docs.get(&(ep.group_id.clone(), doc.clone()))
}

/// Whether the fact was *in force* at `t` (FR-004, FR-007), independent of whether it was
/// believed at `t`. A producing episode is in force unless its document was superseded with an
/// effective date at or before `t`; an episode superseded any other way (revision, removal) was
/// corrected, so it makes no validity claim. The edge is in force iff any producing episode is
/// (mirroring belief's "any live source" rule); `None` when every present producing episode was
/// corrected or none remain. Edges with no producing episode (warranted, legacy) are in force.
pub fn in_force(
    edge: &EdgeBeliefRow,
    episodes: &HashMap<String, EpisodeSupersession>,
    docs: &DocRecords,
    t_us: i64,
) -> Option<bool> {
    if edge.episodes.is_empty() {
        return Some(true);
    }
    let mut any_ended = false;
    for uuid in &edge.episodes {
        let Some(ep) = episodes.get(uuid) else {
            continue;
        };
        if ep.is_document_superseded() {
            match doc_record(ep, docs) {
                Some(rec) if rec.effective_at_us <= t_us => any_ended = true,
                _ => return Some(true),
            }
        } else if ep.superseded_at_us.is_none() {
            return Some(true);
        }
    }
    if any_ended {
        Some(false)
    } else {
        None
    }
}

/// Why belief in a fact ended (FR-007): `corrected` (revision or removal — the fact was never
/// true) versus `document_superseded` (true until the effective date).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupersessionCause {
    Corrected,
    DocumentSuperseded,
}

/// Stored supersession state of a fact, reported independently of the evaluated instant so the
/// "corrected" / "in force until" classification is readable from any read (SC-005).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SupersessionInfo {
    pub cause: SupersessionCause,
    pub superseded_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by_document: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_force_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
}

pub(crate) fn format_us(us: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(us as i128 * 1_000)
        .map(crate::db::format_datetime_rfc3339_subsecond)
        .unwrap_or_default()
}

/// Present only when every producing episode that still exists has a recorded supersession; taken
/// from the one that ended belief last.
pub fn supersession_info(
    edge: &EdgeBeliefRow,
    episodes: &HashMap<String, EpisodeSupersession>,
    docs: &DocRecords,
) -> Option<SupersessionInfo> {
    let mut last: Option<(&EpisodeSupersession, i64)> = None;
    for uuid in &edge.episodes {
        let Some(ep) = episodes.get(uuid) else {
            continue;
        };
        let at = ep.superseded_at_us?;
        if last.is_none_or(|(_, l)| at >= l) {
            last = Some((ep, at));
        }
    }
    let (ep, at) = last?;
    let rec = doc_record(ep, docs);
    let cause = if ep.is_document_superseded() {
        SupersessionCause::DocumentSuperseded
    } else {
        SupersessionCause::Corrected
    };
    Some(SupersessionInfo {
        cause,
        superseded_at: format_us(at),
        superseded_by_document: if ep.is_document_superseded() {
            ep.superseded_by.clone()
        } else {
            None
        },
        in_force_until: rec.map(|r| format_us(r.effective_at_us)),
        recorded_at: rec.map(|r| format_us(r.recorded_at_us)),
    })
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
    /// Whether the fact was in force at the evaluated instant (issue #707), independent of
    /// `believed`. `null` when it was corrected and so never true.
    pub in_force: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersession: Option<SupersessionInfo>,
}

impl BeliefAnnotation {
    pub fn new(
        edge: &EdgeBeliefRow,
        state: BeliefState,
        in_force: Option<bool>,
        supersession: Option<SupersessionInfo>,
    ) -> Self {
        Self {
            state,
            believed: state.is_believed(),
            producing_episode_uuids: edge.episodes.clone(),
            warrant: edge.warrant.clone(),
            valid_at_source: edge.valid_at_source.clone(),
            in_force,
            supersession,
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
                        ..Default::default()
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
    fn interval_warranted_is_not_defined() {
        let e = edge(100, &[], Some("assertion"));
        assert_eq!(belief_interval(&e, &eps(&[])), BeliefInterval::NotDefined);
    }

    #[test]
    fn interval_legacy_has_no_record() {
        let e = edge(100, &[], None);
        assert_eq!(
            belief_interval(&e, &eps(&[])),
            BeliefInterval::NoRecord(NoRecordCause::Legacy)
        );
    }

    #[test]
    fn interval_support_deleted_has_no_record() {
        let e = edge(100, &["gone"], None);
        assert_eq!(
            belief_interval(&e, &eps(&[])),
            BeliefInterval::NoRecord(NoRecordCause::SupportDeleted)
        );
    }

    #[test]
    fn interval_open_while_any_producer_is_live() {
        let e = edge(100, &["a", "b"], None);
        let m = eps(&[("a", Some(200)), ("b", None)]);
        assert_eq!(belief_interval(&e, &m), BeliefInterval::Open);
        // A live producer wins over a hard-deleted one too.
        let e = edge(100, &["gone", "b"], None);
        assert_eq!(
            belief_interval(&e, &eps(&[("b", None)])),
            BeliefInterval::Open
        );
    }

    #[test]
    fn interval_closed_single_producer() {
        let e = edge(100, &["a"], None);
        assert_eq!(
            belief_interval(&e, &eps(&[("a", Some(200))])),
            BeliefInterval::Closed {
                t1_us: 100,
                t2_us: 200,
                derived_from_surviving_support: false
            }
        );
    }

    #[test]
    fn interval_closed_ends_at_last_producer() {
        let e = edge(100, &["a", "b"], None);
        let m = eps(&[("a", Some(300)), ("b", Some(200))]);
        assert_eq!(
            belief_interval(&e, &m),
            BeliefInterval::Closed {
                t1_us: 100,
                t2_us: 300,
                derived_from_surviving_support: false
            }
        );
    }

    #[test]
    fn interval_closed_flags_partially_deleted_support() {
        let e = edge(100, &["gone", "b"], None);
        assert_eq!(
            belief_interval(&e, &eps(&[("b", Some(250))])),
            BeliefInterval::Closed {
                t1_us: 100,
                t2_us: 250,
                derived_from_surviving_support: true
            }
        );
    }

    #[test]
    fn interval_closed_without_ingested_at_starts_at_min() {
        let mut e = edge(0, &["a"], None);
        e.ingested_at_us = None;
        assert_eq!(
            belief_interval(&e, &eps(&[("a", Some(9))])),
            BeliefInterval::Closed {
                t1_us: i64::MIN,
                t2_us: 9,
                derived_from_surviving_support: false
            }
        );
    }

    #[test]
    fn missing_ingested_at_does_not_gate() {
        let mut e = edge(0, &["a"], None);
        e.ingested_at_us = None;
        assert_eq!(evaluate(&e, &eps(&[("a", None)]), 1), BeliefState::Believed);
    }

    fn doc_ep(superseded: i64, doc: &str, by: &str) -> EpisodeSupersession {
        EpisodeSupersession {
            superseded_at_us: Some(superseded),
            kind: Some("document".into()),
            superseded_by: Some(by.into()),
            reason: Some("document_superseded".into()),
            group_id: "g".into(),
            doc_id: Some(doc.into()),
        }
    }

    fn corrected_ep(superseded: i64) -> EpisodeSupersession {
        EpisodeSupersession {
            superseded_at_us: Some(superseded),
            kind: Some("episode".into()),
            superseded_by: Some("new".into()),
            reason: Some("revision".into()),
            group_id: "g".into(),
            doc_id: Some("B".into()),
        }
    }

    fn rec(effective: i64, recorded: i64) -> DocRecords {
        let mut m = DocRecords::new();
        m.insert(
            ("g".into(), "B".into()),
            DocSupersessionRecord {
                group_id: "g".into(),
                superseded_doc_id: "B".into(),
                superseding_doc_id: "A".into(),
                effective_at_us: effective,
                recorded_at_us: recorded,
            },
        );
        m
    }

    #[test]
    fn retroactive_supersession_is_believed_but_not_in_force() {
        // Effective at 200, recorded at 300: believed on [100, 300), in force on [100, 200).
        let e = edge(100, &["b"], None);
        let m: HashMap<_, _> = [("b".to_string(), doc_ep(300, "B", "A"))].into();
        let d = rec(200, 300);
        assert_eq!(in_force(&e, &m, &d, 150), Some(true));
        assert_eq!(in_force(&e, &m, &d, 250), Some(false));
        assert_eq!(evaluate(&e, &m, 250), BeliefState::Believed);
        assert_eq!(evaluate(&e, &m, 300), BeliefState::Superseded);
        assert_eq!(in_force(&e, &m, &d, 300), Some(false));
    }

    #[test]
    fn future_effective_date_is_believed_and_in_force_until_then() {
        // Recorded at 200, effective at 500: superseded_at = 500.
        let e = edge(100, &["b"], None);
        let m: HashMap<_, _> = [("b".to_string(), doc_ep(500, "B", "A"))].into();
        let d = rec(500, 200);
        assert_eq!(evaluate(&e, &m, 499), BeliefState::Believed);
        assert_eq!(in_force(&e, &m, &d, 499), Some(true));
        assert_eq!(evaluate(&e, &m, 500), BeliefState::Superseded);
        assert_eq!(in_force(&e, &m, &d, 500), Some(false));
    }

    #[test]
    fn corrected_fact_has_no_in_force_claim() {
        let e = edge(100, &["b"], None);
        let m: HashMap<_, _> = [("b".to_string(), corrected_ep(200))].into();
        assert_eq!(in_force(&e, &m, &DocRecords::new(), 150), None);
        assert_eq!(in_force(&e, &m, &DocRecords::new(), 250), None);
    }

    #[test]
    fn edge_in_force_while_any_source_is() {
        let e = edge(100, &["b", "a"], None);
        let m: HashMap<_, _> = [
            ("b".to_string(), doc_ep(200, "B", "A")),
            ("a".to_string(), EpisodeSupersession::default()),
        ]
        .into();
        assert_eq!(in_force(&e, &m, &rec(200, 200), 900), Some(true));
        let w = edge(100, &[], Some("assertion"));
        assert_eq!(in_force(&w, &m, &rec(200, 200), 900), Some(true));
        let mixed = edge(100, &["b", "c"], None);
        let m2: HashMap<_, _> = [
            ("b".to_string(), doc_ep(200, "B", "A")),
            ("c".to_string(), corrected_ep(250)),
        ]
        .into();
        assert_eq!(in_force(&mixed, &m2, &rec(200, 200), 900), Some(false));
    }

    #[test]
    fn supersession_info_classifies_cause() {
        let e = edge(100, &["b"], None);
        let m: HashMap<_, _> = [("b".to_string(), doc_ep(300, "B", "A"))].into();
        let info = supersession_info(&e, &m, &rec(200, 300)).unwrap();
        assert_eq!(info.cause, SupersessionCause::DocumentSuperseded);
        assert_eq!(info.superseded_by_document.as_deref(), Some("A"));
        assert!(info.in_force_until.is_some() && info.recorded_at.is_some());
        let c: HashMap<_, _> = [("b".to_string(), corrected_ep(300))].into();
        let info = supersession_info(&e, &c, &DocRecords::new()).unwrap();
        assert_eq!(info.cause, SupersessionCause::Corrected);
        assert!(info.in_force_until.is_none());
        let live: HashMap<_, _> = [("b".to_string(), EpisodeSupersession::default())].into();
        assert!(supersession_info(&e, &live, &DocRecords::new()).is_none());
    }
}
