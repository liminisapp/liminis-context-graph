//! Shared read-time derivations over a [`LintSnapshot`]: belief per edge and the ADR-0752
//! "same fact" classes. Built once per run and borrowed by every analyser.

use std::collections::{BTreeMap, HashMap};

use crate::belief::{self, EpisodeSupersession};
use crate::origin::SupportTally;

use super::snapshot::{LintEdge, LintEpisode, LintSnapshot};

/// `(group, source entity, target entity, relation name)` — the ADR-0752 same-fact key.
pub type ClassKey = (String, String, String, String);

pub struct Ctx<'a> {
    pub snap: &'a LintSnapshot,
    pub as_of_us: i64,
    pub ep_by_uuid: HashMap<&'a str, &'a LintEpisode>,
    /// `believed[i]` is whether `snap.edges[i]` is believed at `as_of_us` (ADR-0705; legacy rows
    /// count as believed).
    pub believed: Vec<bool>,
    /// Same-fact classes over same-group edges (cross-group edges never take part), believed or
    /// not, so a class keeps its superseded siblings.
    pub classes: BTreeMap<ClassKey, Vec<usize>>,
}

impl<'a> Ctx<'a> {
    pub fn new(snap: &'a LintSnapshot, as_of_us: i64) -> Ctx<'a> {
        let ep_by_uuid: HashMap<&str, &LintEpisode> =
            snap.episodes.iter().map(|e| (e.uuid.as_str(), e)).collect();

        let mut believed = Vec::with_capacity(snap.edges.len());
        if !snap.edges.is_empty() {
            let supers: HashMap<String, EpisodeSupersession> = snap
                .episodes
                .iter()
                .map(|e| {
                    (
                        e.uuid.clone(),
                        EpisodeSupersession {
                            superseded_at_us: e.superseded_at_us,
                            group_id: e.group_id.clone(),
                            ..Default::default()
                        },
                    )
                })
                .collect();
            for e in &snap.edges {
                believed.push(belief::evaluate(&e.belief, &supers, as_of_us).is_believed());
            }
        }

        let mut classes: BTreeMap<ClassKey, Vec<usize>> = BTreeMap::new();
        for (i, e) in snap.edges.iter().enumerate() {
            if e.same_group() {
                classes
                    .entry((
                        e.group_id.clone(),
                        e.source.clone(),
                        e.target.clone(),
                        e.name.clone(),
                    ))
                    .or_default()
                    .push(i);
            }
        }
        Ctx {
            snap,
            as_of_us,
            ep_by_uuid,
            believed,
            classes,
        }
    }

    pub fn edge(&self, i: usize) -> &'a LintEdge {
        &self.snap.edges[i]
    }

    /// The episode if it exists and is live at the evaluated instant.
    pub fn live_episode(&self, uuid: &str) -> Option<&'a LintEpisode> {
        self.ep_by_uuid
            .get(uuid)
            .copied()
            .filter(|e| e.is_live(self.as_of_us))
    }

    /// Per-origin distinct-document tally over the live producing episodes of `edges`
    /// (ADR-0752 `support`).
    pub fn tally(&self, edges: &[usize]) -> SupportTally {
        let mut t = SupportTally::new();
        for &i in edges {
            for u in &self.edge(i).belief.episodes {
                if let Some(ep) = self.live_episode(u) {
                    t.add(ep.origin, ep.doc_id.as_deref(), &ep.uuid);
                }
            }
        }
        t
    }
}
