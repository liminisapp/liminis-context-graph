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

use crate::db::Conn;
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

/// The `support` annotation of one returned edge or entity, and whether it passes the filter.
#[derive(Debug, Clone)]
pub struct Support {
    pub support: Value,
    pub corroboration: usize,
    /// Always `true` when no `origins` filter was given.
    pub matches: bool,
}

impl Support {
    /// Writes `support` and `corroboration` onto a serialized result object.
    pub fn annotate(&self, v: &mut Value) {
        v["support"] = self.support.clone();
        v["corroboration"] = json!(self.corroboration);
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
    let rows = conn.entity_mention_origins(entity_uuids)?;
    let mut tallies: HashMap<&str, SupportTally> = HashMap::new();
    let mut matched: HashSet<&str> = HashSet::new();
    let mut mentioned: HashSet<&str> = HashSet::new();
    for (entity, ep, info) in &rows {
        mentioned.insert(entity.as_str());
        if !info.is_live(as_of_us) {
            continue;
        }
        tallies
            .entry(entity.as_str())
            .or_default()
            .add(info.origin, info.doc_id.as_deref(), ep);
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
        out.insert(
            u.clone(),
            Support {
                support: tally.to_json(),
                corroboration: tally.corroboration(),
                matches,
            },
        );
    }
    Ok(out)
}
