//! `knowledge_status` per-group episode breakdown (issue #884, ADR-0884).
//!
//! Opt-in (`include_episode_breakdown: true`) measurement surface for the reopening triggers of
//! ADR-0828: per group, how many episodes are **live**, **superseded** and **pending**, plus how
//! many entities and edges exist *only* because of superseded episodes.
//!
//! Liveness is never re-implemented here. Each episode is classified by
//! [`EpisodeSupersession::is_pending`] and [`EpisodeSupersession::live_at`], and each edge by
//! [`belief::evaluate`], so the counts agree with what belief and search treat as live. The scans
//! only project narrow columns ([`Conn::breakdown_scan_episodes`] and friends); aggregation is in
//! Rust. Read-only: nothing is written to the graph or the WAL.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Value};

use crate::belief::{self, BeliefState, EpisodeSupersession};
use crate::db::Conn;
use crate::error::Error;

/// Rows per keyset page of the episode and edge scans.
const SCAN_PAGE: usize = 5000;
/// Episode uuids per `MENTIONS` lookup.
const MENTION_CHUNK: usize = 1000;

/// Where an episode stands at the evaluated instant. The three states partition every episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeState {
    Live,
    Superseded,
    /// Document-mode revision that has not committed (ADR-0785). Tested first: a pending episode
    /// is never live, and is not counted as superseded even if `superseded_at` is set.
    Pending,
}

/// Classifies one episode at `now_us` with the shared liveness predicate.
pub fn classify_episode(ep: &EpisodeSupersession, now_us: i64) -> EpisodeState {
    if ep.is_pending() {
        EpisodeState::Pending
    } else if ep.live_at(now_us) {
        EpisodeState::Live
    } else {
        EpisodeState::Superseded
    }
}

/// One group's counts. Every field is always reported, 0 included.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupBreakdown {
    pub live: u64,
    pub superseded: u64,
    pub pending: u64,
    pub only_superseded_entities: u64,
    pub only_superseded_edges: u64,
}

/// The whole breakdown, keyed (and so ordered) by `group_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Breakdown {
    pub as_of_us: i64,
    pub groups: BTreeMap<String, GroupBreakdown>,
}

impl Breakdown {
    fn group(&mut self, group_id: &str) -> &mut GroupBreakdown {
        self.groups.entry(group_id.to_string()).or_default()
    }

    /// `{as_of_us, groups: [{group_id, live, superseded, pending, only_superseded_entities,
    /// only_superseded_edges}]}`, groups sorted by `group_id`.
    pub fn to_json(&self) -> Value {
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|(gid, g)| {
                json!({
                    "group_id": gid,
                    "live": g.live,
                    "superseded": g.superseded,
                    "pending": g.pending,
                    "only_superseded_entities": g.only_superseded_entities,
                    "only_superseded_edges": g.only_superseded_edges,
                })
            })
            .collect();
        json!({ "as_of_us": self.as_of_us, "groups": groups })
    }
}

/// Per-entity support seen so far across `MENTIONS` edges. Pending mentions are neutral.
#[derive(Debug, Default)]
struct EntitySupport {
    has_live: bool,
    has_superseded: bool,
}

/// Accumulates `MENTIONS` support per entity.
#[derive(Debug, Default)]
struct EntityTracker {
    support: HashMap<String, EntitySupport>,
}

impl EntityTracker {
    fn note(&mut self, entity_uuid: &str, state: EpisodeState) {
        match state {
            EpisodeState::Pending => {}
            EpisodeState::Live => {
                self.support
                    .entry(entity_uuid.to_string())
                    .or_default()
                    .has_live = true;
            }
            EpisodeState::Superseded => {
                self.support
                    .entry(entity_uuid.to_string())
                    .or_default()
                    .has_superseded = true;
            }
        }
    }

    /// Entities with at least one superseded mention and no live mention.
    fn only_superseded(&self) -> Vec<String> {
        self.support
            .iter()
            .filter(|(_, s)| s.has_superseded && !s.has_live)
            .map(|(u, _)| u.clone())
            .collect()
    }
}

/// Whether an edge counts as only-superseded: exactly [`belief::evaluate`] `== Superseded`, so
/// edge semantics match belief reads (hard-deleted producers ignored; superseded plus pending
/// producers with none live count; pending-only, legacy and producer-less edges do not).
pub fn edge_is_only_superseded(
    edge: &belief::EdgeBeliefRow,
    episodes: &HashMap<String, EpisodeSupersession>,
    now_us: i64,
) -> bool {
    belief::evaluate(edge, episodes, now_us) == BeliefState::Superseded
}

/// Computes the breakdown at `now_us`. Three read-only passes: episodes, `MENTIONS`, edges.
pub fn compute(conn: &Conn, now_us: i64) -> Result<Breakdown, Error> {
    let mut out = Breakdown {
        as_of_us: now_us,
        groups: BTreeMap::new(),
    };

    // Pass 1: every episode, classified and tallied by its own group.
    let mut episodes: HashMap<String, EpisodeSupersession> = HashMap::new();
    let mut after: Option<String> = None;
    loop {
        let page = conn.breakdown_scan_episodes(after.as_deref(), SCAN_PAGE)?;
        let Some((last, _)) = page.last() else { break };
        after = Some(last.clone());
        let n = page.len();
        for (uuid, ep) in page {
            let g = out.group(&ep.group_id);
            match classify_episode(&ep, now_us) {
                EpisodeState::Live => g.live += 1,
                EpisodeState::Superseded => g.superseded += 1,
                EpisodeState::Pending => g.pending += 1,
            }
            episodes.insert(uuid, ep);
        }
        if n < SCAN_PAGE {
            break;
        }
    }

    // Pass 2: MENTIONS support per entity (pending episodes are neutral, so never scanned).
    let non_pending: Vec<String> = {
        let mut v: Vec<String> = episodes
            .iter()
            .filter(|(_, ep)| !ep.is_pending())
            .map(|(u, _)| u.clone())
            .collect();
        v.sort();
        v
    };
    let mut tracker = EntityTracker::default();
    for chunk in non_pending.chunks(MENTION_CHUNK) {
        for (ep_uuid, entity_uuid) in conn.lint_scan_mentions(chunk)? {
            if let Some(ep) = episodes.get(&ep_uuid) {
                tracker.note(&entity_uuid, classify_episode(ep, now_us));
            }
        }
    }
    let candidates = tracker.only_superseded();
    if !candidates.is_empty() {
        let groups = conn.entity_groups(&candidates)?;
        let mut seen: HashSet<&str> = HashSet::new();
        for uuid in &candidates {
            // An entity deleted since the MENTIONS read has no group and is skipped.
            if let Some(gid) = groups.get(uuid) {
                if seen.insert(uuid.as_str()) {
                    out.group(gid).only_superseded_entities += 1;
                }
            }
        }
    }

    // Pass 3: edges, judged by their producing episodes and tallied by the edge's own group.
    let mut after: Option<String> = None;
    loop {
        let page = conn.breakdown_scan_edges(after.as_deref(), SCAN_PAGE)?;
        let Some((last, _)) = page.last() else { break };
        after = Some(last.uuid.clone());
        let n = page.len();
        for (edge, gid) in page {
            let g = out.group(&gid);
            if edge_is_only_superseded(&edge, &episodes, now_us) {
                g.only_superseded_edges += 1;
            }
        }
        if n < SCAN_PAGE {
            break;
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::belief::EdgeBeliefRow;

    const NOW: i64 = 1_000_000;

    fn live() -> EpisodeSupersession {
        EpisodeSupersession::default()
    }

    fn superseded() -> EpisodeSupersession {
        EpisodeSupersession {
            superseded_at_us: Some(NOW - 1),
            ..Default::default()
        }
    }

    fn pending() -> EpisodeSupersession {
        EpisodeSupersession {
            revision_id_present: true,
            revision_committed_at_us: None,
            ..Default::default()
        }
    }

    fn edge(producers: &[&str]) -> EdgeBeliefRow {
        EdgeBeliefRow {
            uuid: "e".into(),
            episodes: producers.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn classify_partitions() {
        assert_eq!(classify_episode(&live(), NOW), EpisodeState::Live);
        assert_eq!(
            classify_episode(&superseded(), NOW),
            EpisodeState::Superseded
        );
        assert_eq!(classify_episode(&pending(), NOW), EpisodeState::Pending);
    }

    #[test]
    fn pending_with_superseded_at_is_pending_only() {
        let ep = EpisodeSupersession {
            superseded_at_us: Some(NOW - 1),
            ..pending()
        };
        assert_eq!(classify_episode(&ep, NOW), EpisodeState::Pending);
    }

    #[test]
    fn future_dated_supersession_counts_as_live() {
        let ep = EpisodeSupersession {
            superseded_at_us: Some(NOW + 10),
            ..Default::default()
        };
        assert_eq!(classify_episode(&ep, NOW), EpisodeState::Live);
        assert_eq!(classify_episode(&ep, NOW + 10), EpisodeState::Superseded);
    }

    #[test]
    fn committed_revision_is_live_unless_superseded() {
        let ep = EpisodeSupersession {
            revision_id_present: true,
            revision_committed_at_us: Some(NOW - 5),
            ..Default::default()
        };
        assert_eq!(classify_episode(&ep, NOW), EpisodeState::Live);
    }

    #[test]
    fn entity_rules() {
        let mut t = EntityTracker::default();
        // Globex: only superseded mentions. Acme: superseded and live. Pending-only: neutral.
        t.note("globex", EpisodeState::Superseded);
        t.note("acme", EpisodeState::Superseded);
        t.note("acme", EpisodeState::Live);
        t.note("pend", EpisodeState::Pending);
        // Superseded plus pending, no live: counts.
        t.note("mixed", EpisodeState::Superseded);
        t.note("mixed", EpisodeState::Pending);
        let mut got = t.only_superseded();
        got.sort();
        assert_eq!(got, vec!["globex".to_string(), "mixed".to_string()]);
    }

    #[test]
    fn edge_rules() {
        let eps: HashMap<String, EpisodeSupersession> = [
            ("l".to_string(), live()),
            ("s".to_string(), superseded()),
            ("s2".to_string(), superseded()),
            ("p".to_string(), pending()),
        ]
        .into_iter()
        .collect();
        // All superseded: counted. Any live producer: not. Mixed superseded + pending: counted.
        assert!(edge_is_only_superseded(&edge(&["s", "s2"]), &eps, NOW));
        assert!(!edge_is_only_superseded(&edge(&["s", "l"]), &eps, NOW));
        assert!(edge_is_only_superseded(&edge(&["s", "p"]), &eps, NOW));
        // Pending-only, producer-less and all-deleted edges: not counted.
        assert!(!edge_is_only_superseded(&edge(&["p"]), &eps, NOW));
        assert!(!edge_is_only_superseded(&edge(&[]), &eps, NOW));
        assert!(!edge_is_only_superseded(&edge(&["gone"]), &eps, NOW));
        // A hard-deleted producer is ignored next to a superseded one.
        assert!(edge_is_only_superseded(&edge(&["s", "gone"]), &eps, NOW));
    }

    #[test]
    fn to_json_zero_fills_and_sorts() {
        let mut b = Breakdown {
            as_of_us: 7,
            groups: BTreeMap::new(),
        };
        b.group("b").live = 3;
        b.group("a");
        let v = b.to_json();
        assert_eq!(v["as_of_us"], 7);
        let groups = v["groups"].as_array().unwrap();
        assert_eq!(groups[0]["group_id"], "a");
        assert_eq!(groups[0]["superseded"], 0);
        assert_eq!(groups[0]["pending"], 0);
        assert_eq!(groups[0]["only_superseded_entities"], 0);
        assert_eq!(groups[0]["only_superseded_edges"], 0);
        assert_eq!(groups[1]["live"], 3);
    }
}
