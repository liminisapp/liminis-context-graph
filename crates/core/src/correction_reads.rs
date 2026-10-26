//! Correction reads (issue #706, ADR-0706): what a correction changed, and what was written while
//! the old belief held. Both are derived from the records #705 already keeps (ADR-0705) and write
//! nothing.
//!
//! This module holds the pure decision logic — fact identity and diffing, recovering a revision's
//! units, windowing candidates and shaping the responses. The storage lookups live in `db.rs` and
//! the IPC wiring in `handlers.rs`.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::belief::BeliefInterval;
use crate::types::RelatesToEdge;

/// The note attached to every affected-work response, including the empty and non-defined ones.
pub const AFFECTED_WORK_NOTE: &str = "Candidates are episodes ingested inside the fact's belief \
interval that mention one of its endpoint entities. They are matched by entity overlap, not by \
recorded dependence: review them, do not treat them as proven to have used the fact.";

/// The note attached to every changed-facts response.
pub const CHANGES_NOTE: &str = "Facts are matched by (source entity, target entity, relation \
name, fact text). A reworded fact therefore appears as one removal plus one addition. A heading \
rename supersedes one chunk and creates another, so a per-episode diff shows only removals for \
the old chunk.";

/// Basis label carried by every affected-work response (FR-003).
pub const AFFECTED_WORK_BASIS: &str = "entity_overlap";

/// The belief-relevant columns of one `Episodic` row. Timestamps are microseconds since the Unix
/// epoch (see [`crate::belief`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EpisodeRecord {
    pub uuid: String,
    pub name: String,
    pub group_id: String,
    pub ingested_at_us: Option<i64>,
    pub superseded_at_us: Option<i64>,
    pub superseded_by: Option<String>,
    pub supersession_reason: Option<String>,
}

/// An episode that MENTIONS at least one of a set of entities.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MentioningEpisode {
    pub uuid: String,
    pub name: String,
    pub ingested_at_us: Option<i64>,
    pub source_description: String,
    pub matched_entity_uuids: Vec<String>,
}

/// Two edges are the same fact when they share source entity, target entity, relation name and
/// fact text (spec Assumptions). `name` is `RelatesToNode_.name`, not the reclassifiable
/// `relation_type` (ADR-0033/0037).
type FactKey<'a> = (&'a str, &'a str, &'a str, &'a str);

fn fact_key(e: &RelatesToEdge) -> FactKey<'_> {
    (
        e.source_node_uuid.as_str(),
        e.target_node_uuid.as_str(),
        e.name.as_str(),
        e.fact.as_str(),
    )
}

/// FR-001: facts the old side supports that the new side does not (`removed`), and the reverse
/// (`added`). A fact present on both sides appears on neither. Each side is ordered by uuid.
pub fn diff_facts(
    old: Vec<RelatesToEdge>,
    new: Vec<RelatesToEdge>,
) -> (Vec<RelatesToEdge>, Vec<RelatesToEdge>) {
    let old_keys: HashSet<(String, String, String, String)> = old.iter().map(owned_key).collect();
    let new_keys: HashSet<(String, String, String, String)> = new.iter().map(owned_key).collect();
    let mut removed: Vec<RelatesToEdge> = old
        .into_iter()
        .filter(|e| !new_keys.contains(&owned_key(e)))
        .collect();
    let mut added: Vec<RelatesToEdge> = new
        .into_iter()
        .filter(|e| !old_keys.contains(&owned_key(e)))
        .collect();
    removed.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    added.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    (removed, added)
}

fn owned_key(e: &RelatesToEdge) -> (String, String, String, String) {
    let (s, t, n, f) = fact_key(e);
    (s.to_string(), t.to_string(), n.to_string(), f.to_string())
}

/// The units of the revision that superseded `old`: its direct superseder (the final unit, the
/// only one `superseded_by` records) plus the earlier units of the same revision, which stay live
/// with no stored link. `pool` is every episode of `old`'s group and name.
///
/// A pool member is an earlier unit when it was ingested at or before the supersession instant
/// `T`, was not superseded at or before `T` (that excludes the old generation, superseded at
/// exactly `T` by the same commit, and every older generation) and is not `old` itself. A later
/// generation was ingested after `T`, so chains need no special handling.
///
/// Known limit (ADR-0706): a free-form `knowledge_add_episode` episode that reuses the chunk id as
/// its name and sits in that window is swept in.
///
/// Returns an empty list when `old` has no superseder (`chunk_removed`). Ordered by ingest time,
/// then uuid, with the final unit included whether or not it is in `pool`.
pub fn revision_unit_uuids(old: &EpisodeRecord, pool: &[EpisodeRecord]) -> Vec<String> {
    let (Some(by), Some(t)) = (old.superseded_by.as_deref(), old.superseded_at_us) else {
        return Vec::new();
    };
    let mut units: Vec<(Option<i64>, String)> = pool
        .iter()
        .filter(|p| {
            p.uuid != old.uuid
                && p.group_id == old.group_id
                && p.name == old.name
                && p.ingested_at_us.is_some_and(|i| i <= t)
                && p.superseded_at_us.is_none_or(|s| s > t)
        })
        .map(|p| (p.ingested_at_us, p.uuid.clone()))
        .collect();
    if !units.iter().any(|(_, u)| u == by) {
        let ingested = pool
            .iter()
            .find(|p| p.uuid == by)
            .and_then(|p| p.ingested_at_us);
        units.push((ingested, by.to_string()));
    }
    units.sort();
    units.into_iter().map(|(_, u)| u).collect()
}

/// Keeps the candidates ingested inside `[t1_us, t2_us)` that are not among the edge's own
/// producing episodes, ordered by ingest time then uuid (FR-002). The window is half-open: an
/// episode ingested at exactly `t2_us` — the correction itself — is outside. A candidate with no
/// `ingested_at` cannot be placed in the window and is left out.
pub fn filter_window(
    candidates: Vec<MentioningEpisode>,
    t1_us: i64,
    t2_us: i64,
    own_producers: &[String],
) -> Vec<MentioningEpisode> {
    let mut out: Vec<MentioningEpisode> = candidates
        .into_iter()
        .filter(|c| {
            c.ingested_at_us.is_some_and(|i| i >= t1_us && i < t2_us)
                && !own_producers.contains(&c.uuid)
        })
        .collect();
    out.sort_by(|a, b| (a.ingested_at_us, &a.uuid).cmp(&(b.ingested_at_us, &b.uuid)));
    out
}

/// RFC 3339 with microseconds, e.g. `2026-10-26T10:29:00.123456Z`.
pub fn format_micros(us: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_micros(us)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
}

fn opt_micros(us: Option<i64>) -> Value {
    us.and_then(format_micros)
        .map_or(Value::Null, Value::String)
}

fn fact_json(e: &RelatesToEdge) -> Value {
    json!({
        "uuid": e.uuid,
        "source_node_uuid": e.source_node_uuid,
        "target_node_uuid": e.target_node_uuid,
        "name": e.name,
        "fact": e.fact,
        "episodes": e.episodes,
    })
}

fn episode_json(e: &EpisodeRecord) -> Value {
    json!({
        "uuid": e.uuid,
        "name": e.name,
        "group_id": e.group_id,
        "ingested_at": opt_micros(e.ingested_at_us),
        "superseded_at": opt_micros(e.superseded_at_us),
        "reason": e.supersession_reason,
    })
}

/// Response for `knowledge_get_correction_changes` on an episode that was never superseded
/// (US1 AC4): nothing to compare.
pub fn changes_not_superseded(ep: &EpisodeRecord) -> Value {
    json!({
        "status": "not_superseded",
        "episode": episode_json(ep),
        "replacement": Value::Null,
        "removed": [],
        "removed_count": 0,
        "added": [],
        "added_count": 0,
        "note": "The episode has not been superseded, so there is nothing to compare.",
    })
}

/// Response for a superseded episode. `unit_uuids` is empty for `chunk_removed` (FR-005), which
/// reports `replacement: null` and an explicit statement that no replacement exists.
pub fn changes_superseded(
    ep: &EpisodeRecord,
    unit_uuids: &[String],
    removed: &[RelatesToEdge],
    added: &[RelatesToEdge],
) -> Value {
    let (replacement, note) = match ep.superseded_by.as_deref() {
        Some(by) => (
            json!({"superseded_by": by, "unit_episode_uuids": unit_uuids}),
            CHANGES_NOTE.to_string(),
        ),
        None => (
            Value::Null,
            format!(
                "No replacement exists: the episode was superseded without a superseding \
                 episode (reason {}), so the added side is empty. {CHANGES_NOTE}",
                ep.supersession_reason.as_deref().unwrap_or("unknown")
            ),
        ),
    };
    json!({
        "status": "superseded",
        "episode": episode_json(ep),
        "replacement": replacement,
        "removed": removed.iter().map(fact_json).collect::<Vec<_>>(),
        "removed_count": removed.len(),
        "added": added.iter().map(fact_json).collect::<Vec<_>>(),
        "added_count": added.len(),
        "note": note,
    })
}

/// Response for `knowledge_get_affected_work`. `candidates` is only meaningful for a closed
/// interval; every other state returns an empty list alongside an explicit `interval.state`
/// (FR-006), and every response carries `basis: "entity_overlap"` (FR-003).
pub fn affected_work_response(
    edge_uuid: &str,
    group_id: &str,
    interval: &BeliefInterval,
    candidates: &[MentioningEpisode],
) -> Value {
    let interval_json = match interval {
        BeliefInterval::Open => json!({
            "state": "open",
            "cause": "still_believed",
            "detail": "A producing episode is still live, so the belief interval has no end and \
                       affected work is not defined.",
        }),
        BeliefInterval::NoRecord(cause) => json!({
            "state": "no_record",
            "cause": cause.as_str(),
            "detail": "No belief interval was recorded for this fact.",
        }),
        BeliefInterval::NotDefined => json!({
            "state": "not_defined",
            "cause": "warranted",
            "detail": "A warranted (direct assertion) fact is believed until a retraction \
                       primitive exists, so its interval never closes.",
        }),
        BeliefInterval::Closed {
            t1_us,
            t2_us,
            derived_from_surviving_support,
        } => json!({
            "state": "closed",
            "start": format_micros(*t1_us),
            "end": format_micros(*t2_us),
            "derived_from_surviving_support": derived_from_surviving_support,
        }),
    };
    json!({
        "edge_uuid": edge_uuid,
        "group_id": group_id,
        "interval": interval_json,
        "basis": AFFECTED_WORK_BASIS,
        "candidates": candidates.iter().map(|c| json!({
            "uuid": c.uuid,
            "name": c.name,
            "ingested_at": opt_micros(c.ingested_at_us),
            "source_description": c.source_description,
            "matched_entity_uuids": c.matched_entity_uuids,
        })).collect::<Vec<_>>(),
        "count": candidates.len(),
        "note": AFFECTED_WORK_NOTE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::belief::NoRecordCause;

    fn edge(uuid: &str, src: &str, dst: &str, name: &str, fact: &str) -> RelatesToEdge {
        RelatesToEdge {
            uuid: uuid.into(),
            source_node_uuid: src.into(),
            target_node_uuid: dst.into(),
            name: name.into(),
            fact: fact.into(),
            ..Default::default()
        }
    }

    fn ep(uuid: &str, ingested: i64, superseded: Option<i64>, by: Option<&str>) -> EpisodeRecord {
        EpisodeRecord {
            uuid: uuid.into(),
            name: "chunk".into(),
            group_id: "g".into(),
            ingested_at_us: Some(ingested),
            superseded_at_us: superseded,
            superseded_by: by.map(str::to_string),
            supersession_reason: superseded.map(|_| "revision".to_string()),
        }
    }

    fn cand(uuid: &str, ingested: Option<i64>) -> MentioningEpisode {
        MentioningEpisode {
            uuid: uuid.into(),
            name: uuid.into(),
            ingested_at_us: ingested,
            source_description: String::new(),
            matched_entity_uuids: vec!["a".into()],
        }
    }

    #[test]
    fn diff_excludes_a_fact_present_on_both_sides() {
        let old = vec![
            edge("o1", "a", "b", "A → B", "same"),
            edge("o2", "a", "c", "A → C", "dropped"),
        ];
        let new = vec![
            edge("n1", "a", "b", "A → B", "same"),
            edge("n2", "a", "d", "A → D", "new"),
        ];
        let (removed, added) = diff_facts(old, new);
        assert_eq!(
            removed.iter().map(|e| &e.uuid[..]).collect::<Vec<_>>(),
            ["o2"]
        );
        assert_eq!(
            added.iter().map(|e| &e.uuid[..]).collect::<Vec<_>>(),
            ["n2"]
        );
    }

    #[test]
    fn diff_reworded_fact_is_one_removal_plus_one_addition() {
        let (removed, added) = diff_facts(
            vec![edge("o", "a", "b", "A → B", "likes")],
            vec![edge("n", "a", "b", "A → B", "adores")],
        );
        assert_eq!((removed.len(), added.len()), (1, 1));
    }

    #[test]
    fn diff_matches_on_all_four_identity_fields() {
        for (src, dst, name) in [("x", "b", "N"), ("a", "x", "N"), ("a", "b", "M")] {
            let (removed, added) = diff_facts(
                vec![edge("o", "a", "b", "N", "f")],
                vec![edge("n", src, dst, name, "f")],
            );
            assert_eq!((removed.len(), added.len()), (1, 1), "{src}{dst}{name}");
        }
    }

    #[test]
    fn diff_with_no_new_side_reports_everything_removed() {
        let (removed, added) = diff_facts(vec![edge("o", "a", "b", "N", "f")], vec![]);
        assert_eq!((removed.len(), added.len()), (1, 0));
    }

    #[test]
    fn units_for_chunk_removed_are_empty() {
        let old = ep("e1", 10, Some(20), None);
        assert!(revision_unit_uuids(&old, std::slice::from_ref(&old)).is_empty());
    }

    #[test]
    fn units_for_a_simple_revision_are_the_superseder() {
        let old = ep("e1", 10, Some(20), Some("e2"));
        let pool = vec![old.clone(), ep("e2", 20, None, None)];
        assert_eq!(revision_unit_uuids(&old, &pool), ["e2"]);
    }

    #[test]
    fn units_include_earlier_live_units_of_a_multi_unit_revision() {
        // e1 is the old generation; u1 is an earlier unit (still live), e2 the final unit.
        let old = ep("e1", 10, Some(30), Some("e2"));
        let pool = vec![
            old.clone(),
            ep("u1", 25, None, None),
            ep("e2", 30, None, None),
        ];
        assert_eq!(revision_unit_uuids(&old, &pool), ["u1", "e2"]);
    }

    #[test]
    fn units_exclude_the_whole_old_generation_and_older_ones() {
        // Old generation = e1a + e1b, both superseded at 30. An even older e0 was superseded at 5.
        let old = ep("e1a", 10, Some(30), Some("e2"));
        let pool = vec![
            ep("e0", 1, Some(5), Some("e1a")),
            old.clone(),
            ep("e1b", 11, Some(30), Some("e2")),
            ep("e2", 30, None, None),
        ];
        assert_eq!(revision_unit_uuids(&old, &pool), ["e2"]);
    }

    #[test]
    fn units_in_a_chain_compare_with_the_direct_superseder_only() {
        // e1 -> e2 (T=20), later e2 -> e3 (T=40). Asking about e1 sees e2, not e3.
        let old = ep("e1", 10, Some(20), Some("e2"));
        let pool = vec![
            old.clone(),
            ep("e2", 20, Some(40), Some("e3")),
            ep("e3", 40, None, None),
        ];
        assert_eq!(revision_unit_uuids(&old, &pool), ["e2"]);
    }

    #[test]
    fn units_keep_the_superseder_even_when_absent_from_the_pool() {
        let old = ep("e1", 10, Some(20), Some("gone"));
        assert_eq!(
            revision_unit_uuids(&old, std::slice::from_ref(&old)),
            ["gone"]
        );
    }

    #[test]
    fn units_ignore_other_groups() {
        let old = ep("e1", 10, Some(20), Some("e2"));
        let mut other = ep("x", 15, None, None);
        other.group_id = "other".into();
        let pool = vec![old.clone(), other, ep("e2", 20, None, None)];
        assert_eq!(revision_unit_uuids(&old, &pool), ["e2"]);
    }

    #[test]
    fn window_is_half_open() {
        let got = filter_window(
            vec![
                cand("before", Some(99)),
                cand("at_t1", Some(100)),
                cand("inside", Some(150)),
                cand("at_t2", Some(200)),
                cand("after", Some(201)),
            ],
            100,
            200,
            &[],
        );
        assert_eq!(
            got.iter().map(|c| &c.uuid[..]).collect::<Vec<_>>(),
            ["at_t1", "inside"]
        );
    }

    #[test]
    fn window_excludes_own_producers_and_unplaceable_candidates() {
        let got = filter_window(
            vec![
                cand("own", Some(150)),
                cand("none", None),
                cand("ok", Some(160)),
            ],
            100,
            200,
            &["own".to_string()],
        );
        assert_eq!(got.iter().map(|c| &c.uuid[..]).collect::<Vec<_>>(), ["ok"]);
    }

    #[test]
    fn window_orders_by_ingest_time_then_uuid() {
        let got = filter_window(
            vec![
                cand("b", Some(150)),
                cand("a", Some(150)),
                cand("z", Some(120)),
            ],
            100,
            200,
            &[],
        );
        assert_eq!(
            got.iter().map(|c| &c.uuid[..]).collect::<Vec<_>>(),
            ["z", "a", "b"]
        );
    }

    #[test]
    fn every_affected_work_response_carries_the_entity_overlap_basis() {
        for interval in [
            BeliefInterval::Open,
            BeliefInterval::NotDefined,
            BeliefInterval::NoRecord(NoRecordCause::Legacy),
            BeliefInterval::NoRecord(NoRecordCause::SupportDeleted),
            BeliefInterval::Closed {
                t1_us: 1,
                t2_us: 2,
                derived_from_surviving_support: false,
            },
        ] {
            let v = affected_work_response("e", "g", &interval, &[]);
            assert_eq!(v["basis"], "entity_overlap");
            assert!(v["note"]
                .as_str()
                .unwrap()
                .contains("not by recorded dependence"));
            assert_eq!(v["count"], 0);
            assert!(v["candidates"].is_array());
            assert!(v.get("episode_uuids").is_none());
        }
    }

    #[test]
    fn non_closed_intervals_name_their_case() {
        let state = |i: BeliefInterval| {
            affected_work_response("e", "g", &i, &[])["interval"]["state"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(state(BeliefInterval::Open), "open");
        assert_eq!(state(BeliefInterval::NotDefined), "not_defined");
        assert_eq!(
            state(BeliefInterval::NoRecord(NoRecordCause::Legacy)),
            "no_record"
        );
    }

    #[test]
    fn chunk_removed_response_states_no_replacement() {
        let old = ep("e1", 10, Some(20), None);
        let v = changes_superseded(&old, &[], &[edge("o", "a", "b", "N", "f")], &[]);
        assert_eq!(v["status"], "superseded");
        assert!(v["replacement"].is_null());
        assert_eq!(v["added_count"], 0);
        assert_eq!(v["removed_count"], 1);
        assert!(v["note"]
            .as_str()
            .unwrap()
            .contains("No replacement exists"));
    }

    #[test]
    fn not_superseded_response_has_nothing_to_compare() {
        let v = changes_not_superseded(&ep("e1", 10, None, None));
        assert_eq!(v["status"], "not_superseded");
        assert_eq!(v["removed_count"], 0);
        assert_eq!(v["added_count"], 0);
    }
}
