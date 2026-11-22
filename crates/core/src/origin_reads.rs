//! Read-side support breakdown and `origins` filtering (issue #752, ADR-0752).
//!
//! Origin lives on episodes only (FR-016). An edge's origins come from its producing episodes
//! (`RelatesToNode_.episodes`, never the either-endpoint `episode_uuids` inference of ADR-0012);
//! an entity's from the episodes that `MENTIONS` it. Both are computed here at read time from
//! batched uuid-keyed lookups, so nothing can drift and a WAL rebuild reproduces them exactly.
//!
//! **Same-fact grouping.** Ingest never deduplicates edges (ADR-0705), so a derived page that
//! restates a primary note's fact produces a second, separate edge. For `support` an edge is
//! therefore aggregated over its *siblings*: edges sharing `(group, source entity, target entity,
//! name)`. The grouping is deliberately coarse and read-only — it moves a count between origins
//! for facts a reader would already consider the same, and never creates or merges anything.
//! The `origins` filter, by contrast, looks only at the edge's **own** episodes.
//!
//! Only episodes live at the read instant count (the predicate `belief::evaluate` uses), so a
//! corrected episode stops contributing: `support` is "support now".

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::db::{Conn, MentionRow};
use crate::error::Error;
use crate::origin::{EpisodeOriginInfo, Origin, SupportTally};

/// What an edge read needs to know about one returned edge.
#[derive(Debug, Clone)]
pub struct EdgeKey {
    pub uuid: String,
    pub group_id: String,
    pub source_uuid: String,
    pub target_uuid: String,
    pub name: String,
}

/// Maximum number of mentioning episodes `find_entities` reports per entity when
/// `include_episodes` is set (issue #857). A fixed service default, not a request parameter.
pub const ENTITY_EPISODE_CAP: usize = 50;

/// One live episode mentioning an entity, as reported by `include_episodes`.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveEpisode {
    pub uuid: String,
    pub source_description: String,
    /// The stored `source_file`; `None` on legacy rows.
    pub source_file: Option<String>,
    pub created_at_us: i64,
}

/// The capped, ordered live mentioning episodes of one entity (issue #857).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EpisodeProvenance {
    /// Newest first (`created_at DESC, uuid DESC`), at most [`ENTITY_EPISODE_CAP`].
    pub episodes: Vec<LiveEpisode>,
    /// `true` when the cap removed at least one live episode.
    pub truncated: bool,
}

impl EpisodeProvenance {
    /// Sorts `live` newest-first with a total order, then applies `cap`.
    pub fn from_live(mut live: Vec<LiveEpisode>, cap: usize) -> Self {
        live.sort_by(|a, b| {
            b.created_at_us
                .cmp(&a.created_at_us)
                .then_with(|| b.uuid.cmp(&a.uuid))
        });
        let truncated = live.len() > cap;
        live.truncate(cap);
        Self {
            episodes: live,
            truncated,
        }
    }
}

/// The `support` annotation of one returned edge or entity, and whether it passes the filter.
#[derive(Debug, Clone)]
pub struct Support {
    pub support: Value,
    pub corroboration: usize,
    /// Always `true` when no `origins` filter was given.
    pub matches: bool,
    /// Live mentioning episodes of an entity; `Some` only when requested (issue #857).
    pub episodes: Option<EpisodeProvenance>,
}

impl Support {
    /// Writes `support` and `corroboration` onto a serialized result object.
    pub fn annotate(&self, v: &mut Value) {
        v["support"] = self.support.clone();
        v["corroboration"] = json!(self.corroboration);
    }

    /// Fills the provenance fields of a serialized entity (issue #857); a no-op unless episodes
    /// were requested. `episode_uuids` and `source_descriptions` are index-aligned and not
    /// de-duplicated (as on `get_entity_neighbors`); `source_files` is a de-duplicated list in
    /// first-appearance order that omits episodes with no stored file, so it is *not* aligned.
    /// `episodes_truncated` is written only when the cap removed something.
    pub fn annotate_provenance(&self, v: &mut Value) {
        let Some(p) = &self.episodes else { return };
        let mut files: Vec<&str> = Vec::new();
        for f in p.episodes.iter().filter_map(|e| e.source_file.as_deref()) {
            if !f.is_empty() && !files.contains(&f) {
                files.push(f);
            }
        }
        v["episode_uuids"] = json!(p.episodes.iter().map(|e| &e.uuid).collect::<Vec<_>>());
        v["source_descriptions"] = json!(p
            .episodes
            .iter()
            .map(|e| &e.source_description)
            .collect::<Vec<_>>());
        v["source_files"] = json!(files);
        if p.truncated {
            v["episodes_truncated"] = json!(true);
        }
    }
}

fn passes(filter: Option<&[Origin]>, origin: Origin) -> bool {
    filter.is_none_or(|f| f.contains(&origin))
}

/// Support and filter verdict for each of `edges`, keyed by edge uuid.
pub fn edge_support(
    conn: &Conn<'_>,
    edges: &[EdgeKey],
    filter: Option<&[Origin]>,
    as_of_us: i64,
) -> Result<HashMap<String, Support>, Error> {
    let uuids: Vec<String> = edges.iter().map(|e| e.uuid.clone()).collect();
    let own = conn.get_edge_belief_rows(&uuids)?;

    // Siblings: one batched endpoint query, narrowed back to exact key matches in Rust.
    let mut groups: Vec<String> = edges.iter().map(|e| e.group_id.clone()).collect();
    let mut srcs: Vec<String> = edges.iter().map(|e| e.source_uuid.clone()).collect();
    let mut dsts: Vec<String> = edges.iter().map(|e| e.target_uuid.clone()).collect();
    for v in [&mut groups, &mut srcs, &mut dsts] {
        v.retain(|s| !s.is_empty());
        v.sort();
        v.dedup();
    }
    let sibling_rows = conn.sibling_edge_rows(&groups, &srcs, &dsts)?;
    type Key = (String, String, String, String);
    let mut by_key: HashMap<Key, Vec<&crate::db::SiblingEdgeRow>> = HashMap::new();
    for r in &sibling_rows {
        by_key
            .entry((
                r.group_id.clone(),
                r.source_uuid.clone(),
                r.target_uuid.clone(),
                r.name.clone(),
            ))
            .or_default()
            .push(r);
    }

    let mut ep_uuids: Vec<String> = own
        .values()
        .flat_map(|r| r.episodes.iter().cloned())
        .chain(sibling_rows.iter().flat_map(|r| r.episodes.iter().cloned()))
        .collect();
    ep_uuids.sort();
    ep_uuids.dedup();
    let info: HashMap<String, EpisodeOriginInfo> = conn.get_episode_origin_info(&ep_uuids)?;

    let mut out = HashMap::with_capacity(edges.len());
    for e in edges {
        let own_eps: &[String] = own
            .get(&e.uuid)
            .map(|r| r.episodes.as_slice())
            .unwrap_or(&[]);
        // Filter verdict: the edge's own live episodes. An edge with no producing episode
        // (asserted, cross-group warrant, legacy) is `primary`, as the spec treats legacy edges.
        let matches = if own_eps.is_empty() {
            passes(filter, Origin::Primary)
        } else {
            own_eps.iter().any(|u| {
                info.get(u)
                    .is_some_and(|i| i.is_live(as_of_us) && passes(filter, i.origin))
            })
        };
        // Support: every live episode of the same-fact class, counted per distinct document.
        let key = (
            e.group_id.clone(),
            e.source_uuid.clone(),
            e.target_uuid.clone(),
            e.name.clone(),
        );
        let mut class_eps: HashSet<&str> = own_eps.iter().map(String::as_str).collect();
        if let Some(sibs) = by_key.get(&key) {
            for s in sibs {
                class_eps.extend(s.episodes.iter().map(String::as_str));
            }
        }
        let mut tally = SupportTally::new();
        for u in class_eps {
            if let Some(i) = info.get(u).filter(|i| i.is_live(as_of_us)) {
                tally.add(i.origin, i.doc_id.as_deref(), u);
            }
        }
        out.insert(
            e.uuid.clone(),
            Support {
                support: tally.to_json(),
                corroboration: tally.corroboration(),
                matches,
                episodes: None,
            },
        );
    }
    Ok(out)
}

/// Support and filter verdict for each of `entity_uuids`, keyed by entity uuid. An entity with no
/// mentioning episode at all (asserted) is `primary` for the filter.
pub fn entity_support(
    conn: &Conn<'_>,
    entity_uuids: &[String],
    filter: Option<&[Origin]>,
    as_of_us: i64,
) -> Result<HashMap<String, Support>, Error> {
    entity_support_with_episodes(conn, entity_uuids, filter, as_of_us, false)
}

/// [`entity_support`], optionally also collecting each entity's live mentioning episodes
/// (`collect_episodes`, issue #857). Both come from the same single batched mention query, so
/// the flag adds no per-entity lookup; `support` is always computed over the full uncapped set.
pub fn entity_support_with_episodes(
    conn: &Conn<'_>,
    entity_uuids: &[String],
    filter: Option<&[Origin]>,
    as_of_us: i64,
    collect_episodes: bool,
) -> Result<HashMap<String, Support>, Error> {
    let rows = conn.entity_mention_rows(entity_uuids)?;
    Ok(support_from_mention_rows(
        &rows,
        entity_uuids,
        filter,
        as_of_us,
        collect_episodes,
    ))
}

fn support_from_mention_rows(
    rows: &[MentionRow],
    entity_uuids: &[String],
    filter: Option<&[Origin]>,
    as_of_us: i64,
    collect_episodes: bool,
) -> HashMap<String, Support> {
    let mut tallies: HashMap<&str, SupportTally> = HashMap::new();
    let mut live_eps: HashMap<&str, Vec<LiveEpisode>> = HashMap::new();
    let mut matched: HashSet<&str> = HashSet::new();
    let mut mentioned: HashSet<&str> = HashSet::new();
    for r in rows {
        let (entity, ep, info) = (&r.entity_uuid, &r.episode_uuid, &r.info);
        mentioned.insert(entity.as_str());
        if !info.is_live(as_of_us) {
            continue;
        }
        tallies
            .entry(entity.as_str())
            .or_default()
            .add(info.origin, info.doc_id.as_deref(), ep);
        if collect_episodes {
            live_eps
                .entry(entity.as_str())
                .or_default()
                .push(LiveEpisode {
                    uuid: ep.clone(),
                    source_description: r.source_description.clone(),
                    source_file: r.source_file.clone(),
                    created_at_us: r.created_at_us,
                });
        }
        if passes(filter, info.origin) {
            matched.insert(entity.as_str());
        }
    }
    let mut out = HashMap::with_capacity(entity_uuids.len());
    for u in entity_uuids {
        let tally = tallies.remove(u.as_str()).unwrap_or_default();
        let matches = if mentioned.contains(u.as_str()) {
            matched.contains(u.as_str())
        } else {
            passes(filter, Origin::Primary)
        };
        let episodes = collect_episodes.then(|| {
            EpisodeProvenance::from_live(
                live_eps.remove(u.as_str()).unwrap_or_default(),
                ENTITY_EPISODE_CAP,
            )
        });
        out.insert(
            u.clone(),
            Support {
                support: tally.to_json(),
                corroboration: tally.corroboration(),
                matches,
                episodes,
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        entity: &str,
        ep: &str,
        created: i64,
        superseded: Option<i64>,
        file: Option<&str>,
    ) -> MentionRow {
        MentionRow {
            entity_uuid: entity.to_string(),
            episode_uuid: ep.to_string(),
            info: EpisodeOriginInfo {
                origin: Origin::Primary,
                doc_id: None,
                superseded_at_us: superseded,
                group_id: "g".to_string(),
            },
            source_description: format!("desc-{ep}"),
            source_file: file.map(str::to_string),
            created_at_us: created,
        }
    }

    fn uuids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flag_off_collects_no_episodes() {
        let rows = vec![row("e", "a", 1, None, None)];
        let out = support_from_mention_rows(&rows, &uuids(&["e"]), None, 100, false);
        assert!(out["e"].episodes.is_none());
        let mut v = json!({"episode_uuids": [], "source_descriptions": []});
        out["e"].annotate_provenance(&mut v);
        assert_eq!(v, json!({"episode_uuids": [], "source_descriptions": []}));
    }

    #[test]
    fn live_only_newest_first_aligned_descriptions() {
        let rows = vec![
            row("e", "a", 10, None, Some("a.md")),
            row("e", "b", 20, None, Some("b.md")),
            row("e", "c", 30, Some(50), Some("c.md")), // superseded at 50 <= now 100
        ];
        let out = support_from_mention_rows(&rows, &uuids(&["e"]), None, 100, true);
        let mut v = json!({});
        out["e"].annotate_provenance(&mut v);
        assert_eq!(v["episode_uuids"], json!(["b", "a"]));
        assert_eq!(v["source_descriptions"], json!(["desc-b", "desc-a"]));
        assert_eq!(v["source_files"], json!(["b.md", "a.md"]));
        assert!(v.get("episodes_truncated").is_none());
    }

    #[test]
    fn source_files_dedup_and_omit_missing() {
        let rows = vec![
            row("e", "a", 10, None, Some("x.md")),
            row("e", "b", 20, None, None),
            row("e", "c", 30, None, Some("x.md")),
            row("e", "d", 40, None, Some("")),
        ];
        let out = support_from_mention_rows(&rows, &uuids(&["e"]), None, 100, true);
        let mut v = json!({});
        out["e"].annotate_provenance(&mut v);
        assert_eq!(v["episode_uuids"].as_array().unwrap().len(), 4);
        assert_eq!(v["source_files"], json!(["x.md"]));
    }

    #[test]
    fn cap_truncates_deterministically_and_support_is_uncapped() {
        let n = ENTITY_EPISODE_CAP + 7;
        // Identical created_at: ties break on uuid DESC.
        let rows: Vec<MentionRow> = (0..n)
            .map(|i| row("e", &format!("ep{i:03}"), 5, None, None))
            .collect();
        let out = support_from_mention_rows(&rows, &uuids(&["e"]), None, 100, true);
        let p = out["e"].episodes.as_ref().unwrap();
        assert_eq!(p.episodes.len(), ENTITY_EPISODE_CAP);
        assert!(p.truncated);
        assert_eq!(p.episodes[0].uuid, format!("ep{:03}", n - 1));
        assert_eq!(out["e"].support["primary"], json!(n));
        let mut v = json!({});
        out["e"].annotate_provenance(&mut v);
        assert_eq!(v["episodes_truncated"], json!(true));
    }

    #[test]
    fn exactly_at_cap_is_not_truncated() {
        let rows: Vec<MentionRow> = (0..ENTITY_EPISODE_CAP)
            .map(|i| row("e", &format!("ep{i:03}"), i as i64, None, None))
            .collect();
        let out = support_from_mention_rows(&rows, &uuids(&["e"]), None, 100, true);
        assert!(!out["e"].episodes.as_ref().unwrap().truncated);
    }

    #[test]
    fn entity_without_live_episodes_gets_empty_lists() {
        let rows = vec![row("e", "a", 1, Some(2), None)];
        let out = support_from_mention_rows(&rows, &uuids(&["e", "other"]), None, 100, true);
        for k in ["e", "other"] {
            let mut v = json!({});
            out[k].annotate_provenance(&mut v);
            assert_eq!(v["episode_uuids"], json!([]));
            assert!(v.get("episodes_truncated").is_none());
        }
    }
}
