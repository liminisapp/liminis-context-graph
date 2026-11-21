//! `knowledge_walk` (issue #820, ADR-0820): a caller-planned walk over typed relations.
//!
//! The *caller* (an agent) plans the walk — start entity, per-hop relation types, phrase and
//! direction, optional answer type — and this module executes it. LCG makes **no LLM call**; the
//! only model use is embedding the hop phrases and the query, which the handler does up front and
//! passes in as [`WalkEmbeddings`].
//!
//! The algorithm is E13's v5, kept generic (no relation names, entity names or paths are
//! hard-coded). Per hop:
//!
//! 1. candidates = edges incident to the frontier whose far end has not been visited;
//! 2. relation-type filter (empty list = no filter; no match = fall back to all, recorded);
//! 3. on the last hop, when `answer_type` is an entity type and the mode is not `history`:
//!    keep far ends of that type (none = fall back to all, recorded);
//! 4. direction: `soft` adds [`DIRECTION_BONUS`] to matching edges; `hard` keeps only matching
//!    edges (none = fall back to all, recorded); `any` applies nothing;
//! 5. rank by cosine(hop phrase, fact embedding) + bonus, ties broken by edge uuid, keep the top
//!    B ([`BEAM_SINGLE`] / [`BEAM_LIST`]); their far ends become the next frontier.
//!
//! If the planned hops end on no entity of `answer_type`, one *completion hop* runs: edges from
//! the final frontier to entities of that type, ranked by cosine(query, fact embedding).
//!
//! The graph is reached through the small [`WalkGraph`] trait so the algorithm is unit-testable
//! on an in-memory graph with exact vectors; `handlers.rs` supplies the `Db`-backed
//! implementation.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::db::cosine_similarity;
use crate::error::Error;
use crate::ontology::Ontology;
use crate::types::{EntityRow, RelatesToEdge};

/// Fixed soft-direction bonus (cosine units) for an edge whose direction matches the hop's.
pub const DIRECTION_BONUS: f32 = 0.05;
/// Beam width in `single` mode.
pub const BEAM_SINGLE: usize = 3;
/// Beam width in `list` and `history` modes.
pub const BEAM_LIST: usize = 10;
/// The `answer_type` sentinel for a plain value (not an entity type): no type filter, no
/// completion hop.
pub const ANSWER_TYPE_VALUE: &str = "value";
/// Attribute key #816 writes on an edge whose endpoint types violate its relation's signature.
pub const SIGNATURE_MISMATCH_KEY: &str = "signature_mismatch";

// ── Request ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WalkMode {
    Single,
    List,
    History,
}

impl WalkMode {
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "single" => Ok(Self::Single),
            "list" => Ok(Self::List),
            "history" => Ok(Self::History),
            other => Err(Error::Ipc(format!(
                "invalid mode {other:?}: expected \"single\", \"list\" or \"history\""
            ))),
        }
    }

    fn beam(self) -> usize {
        match self {
            Self::Single => BEAM_SINGLE,
            Self::List | Self::History => BEAM_LIST,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DirectionMode {
    Soft,
    Hard,
}

impl DirectionMode {
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "soft" => Ok(Self::Soft),
            "hard" => Ok(Self::Hard),
            other => Err(Error::Ipc(format!(
                "invalid direction_mode {other:?}: expected \"soft\" or \"hard\""
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Out,
    In,
    Any,
}

impl Direction {
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "out" => Ok(Self::Out),
            "in" => Ok(Self::In),
            "any" => Ok(Self::Any),
            other => Err(Error::Ipc(format!(
                "invalid direction {other:?}: expected \"out\", \"in\" or \"any\""
            ))),
        }
    }
}

/// One planned hop.
#[derive(Debug, Clone)]
pub struct Hop {
    /// Allowed relation types; empty means "no relation filter".
    pub relation_types: Vec<String>,
    /// Short description of the hop, embedded and compared with each edge's fact embedding.
    pub phrase: String,
    pub direction: Direction,
}

/// A validated walk plan (start entities already resolved by the handler).
#[derive(Debug, Clone)]
pub struct WalkPlan {
    pub start: Vec<String>,
    pub hops: Vec<Hop>,
    /// Entity type name; `None` when absent or [`ANSWER_TYPE_VALUE`].
    pub answer_type: Option<String>,
    pub query: Option<String>,
    pub mode: WalkMode,
    pub direction_mode: DirectionMode,
}

/// Parses the `answer_type` param: absent/empty → `None`; `"value"` → `None` (no entity-type
/// semantics). Anything else is an entity type name — an unknown one is **not** an error (it
/// simply matches nothing, so the type filter falls back and says so), which keeps free-form
/// groups, which have no declared entity types, walkable.
pub fn parse_answer_type(raw: Option<&str>) -> Option<String> {
    let t = raw?.trim();
    if t.is_empty() || t.eq_ignore_ascii_case(ANSWER_TYPE_VALUE) {
        None
    } else {
        Some(t.to_string())
    }
}

impl WalkPlan {
    /// Validates the shape of the plan. An empty `hops` list is allowed only when it can still
    /// do something: an entity-type `answer_type` and a non-empty `query` (only the completion
    /// hop runs, from the start entities).
    pub fn validate(&self) -> Result<(), Error> {
        if self.start.is_empty() {
            return Err(Error::Ipc("walk has no start entities".to_string()));
        }
        if self.hops.is_empty() && !(self.answer_type.is_some() && self.has_query()) {
            return Err(Error::Ipc(
                "hops is empty: provide at least one hop, or an entity-type answer_type together \
                 with a query (then only the completion hop runs)"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn has_query(&self) -> bool {
        self.query.as_deref().is_some_and(|q| !q.trim().is_empty())
    }
}

/// Pre-computed embeddings (query-side instruction) for the plan's text.
#[derive(Debug, Clone, Default)]
pub struct WalkEmbeddings {
    /// One per hop, same order as [`WalkPlan::hops`].
    pub hops: Vec<Vec<f32>>,
    /// Embedding of `query`, when one was supplied.
    pub query: Option<Vec<f32>>,
}

// ── Graph access ──────────────────────────────────────────────────────────────

pub trait WalkGraph {
    /// Edges incident to any entity in `frontier`, in either direction, each carrying its
    /// `fact_embedding`. Invalidated edges are included only when `include_invalidated`.
    fn incident_edges(
        &self,
        frontier: &[String],
        include_invalidated: bool,
    ) -> Result<Vec<RelatesToEdge>, Error>;

    /// Entity rows (name, labels) for the given uuids; unknown uuids are simply absent.
    fn entities(&self, uuids: &[String]) -> Result<Vec<EntityRow>, Error>;
}

// ── Response ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WalkEndpoint {
    pub uuid: String,
    pub name: String,
    /// Primary entity type; empty when the entity has none.
    #[serde(rename = "type")]
    pub entity_type: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WalkEdge {
    pub uuid: String,
    pub relation_type: Option<String>,
    pub fact: String,
    pub source: WalkEndpoint,
    pub target: WalkEndpoint,
    pub valid_at: Option<String>,
    pub invalid_at: Option<String>,
    pub score: f32,
    /// `Some(true/false)` for an `out`/`in` hop; `None` for `any`.
    pub direction_matched: Option<bool>,
    pub signature_mismatch: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WalkHop {
    /// 1-based hop number; the completion hop is `hops.len() + 1`.
    pub hop: usize,
    pub completion: bool,
    /// Candidates remaining after the visited-node exclusion, before any filtering.
    pub candidate_count: usize,
    pub edges: Vec<WalkEdge>,
}

/// Which hops (1-based) had a filter fall back to "all candidates".
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct WalkFallbacks {
    pub relation_type: Vec<usize>,
    pub direction: Vec<usize>,
    pub answer_type: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WalkOutcome {
    pub hops: Vec<WalkHop>,
    pub fallbacks: WalkFallbacks,
    pub completion_hop_ran: bool,
    /// 1-based hop at which the frontier was empty (no candidates), ending the walk early.
    pub dead_end_at: Option<usize>,
    pub facts_text: String,
}

// ── Pure helpers ──────────────────────────────────────────────────────────────

/// Primary entity type: the last label that is not the `Entity` root; empty if none.
pub fn primary_type(labels: &[String]) -> String {
    labels
        .iter()
        .rev()
        .find(|l| l.as_str() != "Entity")
        .cloned()
        .unwrap_or_default()
}

fn has_type(labels: &[String], ty: &str) -> bool {
    labels.iter().any(|l| l == ty)
}

/// Declared `(source_types, target_types)` of a relation type. Isolated so the #816 list shape
/// (`source_types`/`target_types`) is a one-function change.
fn declared_signature(def: &crate::ontology::RelationTypeDef) -> (Vec<&str>, Vec<&str>) {
    (
        def.source_type.iter().map(String::as_str).collect(),
        def.target_type.iter().map(String::as_str).collect(),
    )
}

/// Whether the edge's endpoint types violate its relation's declared signature. Prefers the flag
/// persisted on the edge (`attributes.signature_mismatch`, written by #816); otherwise recomputes
/// from the ontology. `false` with no ontology, no declared signature or an untyped endpoint.
fn signature_mismatch(
    ontology: Option<&Ontology>,
    edge: &RelatesToEdge,
    src_labels: &[String],
    tgt_labels: &[String],
) -> bool {
    if let Ok(serde_json::Value::Object(attrs)) = serde_json::from_str(&edge.attributes) {
        if let Some(b) = attrs.get(SIGNATURE_MISMATCH_KEY).and_then(|v| v.as_bool()) {
            return b;
        }
    }
    let Some(onto) = ontology else {
        return false;
    };
    let rel = relation_name(edge);
    let Some(def) = onto.relation_types.iter().find(|r| r.name == rel) else {
        return false;
    };
    let (src_decl, tgt_decl) = declared_signature(def);
    let violates = |decl: &[&str], labels: &[String]| {
        !decl.is_empty()
            && !primary_type(labels).is_empty()
            && !decl.iter().any(|t| has_type(labels, t))
    };
    violates(&src_decl, src_labels) || violates(&tgt_decl, tgt_labels)
}

fn relation_name(edge: &RelatesToEdge) -> &str {
    edge.relation_type.as_deref().unwrap_or(&edge.name)
}

/// One candidate edge at a hop, oriented relative to the frontier.
struct Candidate {
    edge: RelatesToEdge,
    far: String,
    /// `Some` for `out`/`in` hops.
    matched: Option<bool>,
    score: f32,
}

fn date_part(ts: &str) -> &str {
    ts.get(..10).unwrap_or(ts)
}

/// Renders one fact in the typed reader format:
/// `- <fact> (<source> [<Type>] -> <target> [<Type>]) [from <date> until <date>]`.
/// The `[Type]` marker is omitted for an untyped endpoint; the validity suffix is omitted when
/// both dates are absent, and shows only the side that is present otherwise.
pub fn format_fact(e: &WalkEdge) -> String {
    let end = |p: &WalkEndpoint| {
        if p.entity_type.is_empty() {
            p.name.clone()
        } else {
            format!("{} [{}]", p.name, p.entity_type)
        }
    };
    let mut s = format!("- {} ({} -> {})", e.fact, end(&e.source), end(&e.target));
    match (e.valid_at.as_deref(), e.invalid_at.as_deref()) {
        (Some(v), Some(i)) => {
            s.push_str(&format!(" [from {} until {}]", date_part(v), date_part(i)))
        }
        (Some(v), None) => s.push_str(&format!(" [from {}]", date_part(v))),
        (None, Some(i)) => s.push_str(&format!(" [until {}]", date_part(i))),
        (None, None) => {}
    }
    s
}

/// Deduplicated (by edge uuid), hop-order-then-rank-order facts, one per line.
pub fn facts_text(hops: &[WalkHop]) -> String {
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    for h in hops {
        for e in &h.edges {
            if seen.insert(e.uuid.clone()) {
                lines.push(format_fact(e));
            }
        }
    }
    lines.join("\n")
}

// ── The walk ──────────────────────────────────────────────────────────────────

struct Walker<'a, G: WalkGraph> {
    graph: &'a G,
    ontology: Option<&'a Ontology>,
    entities: HashMap<String, EntityRow>,
}

impl<'a, G: WalkGraph> Walker<'a, G> {
    fn ensure_entities(&mut self, uuids: impl IntoIterator<Item = String>) -> Result<(), Error> {
        let missing: Vec<String> = uuids
            .into_iter()
            .filter(|u| !self.entities.contains_key(u))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        for row in self.graph.entities(&missing)? {
            self.entities.insert(row.uuid.clone(), row);
        }
        Ok(())
    }

    fn labels(&self, uuid: &str) -> &[String] {
        self.entities
            .get(uuid)
            .map(|e| e.labels.as_slice())
            .unwrap_or(&[])
    }

    fn endpoint(&self, uuid: &str) -> WalkEndpoint {
        WalkEndpoint {
            uuid: uuid.to_string(),
            name: self
                .entities
                .get(uuid)
                .map(|e| e.name.clone())
                .unwrap_or_default(),
            entity_type: primary_type(self.labels(uuid)),
        }
    }

    fn to_walk_edge(&self, c: &Candidate) -> WalkEdge {
        let e = &c.edge;
        WalkEdge {
            uuid: e.uuid.clone(),
            relation_type: e
                .relation_type
                .clone()
                .or_else(|| (!e.name.is_empty()).then(|| e.name.clone())),
            fact: e.fact.clone(),
            source: self.endpoint(&e.source_node_uuid),
            target: self.endpoint(&e.target_node_uuid),
            valid_at: e.valid_at.clone(),
            invalid_at: e.invalid_at.clone(),
            score: c.score,
            direction_matched: c.matched,
            signature_mismatch: signature_mismatch(
                self.ontology,
                e,
                self.labels(&e.source_node_uuid),
                self.labels(&e.target_node_uuid),
            ),
        }
    }

    /// Candidates at a frontier: incident edges whose far end is unvisited (this also drops
    /// self-loops, since a frontier entity is always visited), oriented against `direction`.
    fn candidates(
        &self,
        frontier: &[String],
        visited: &HashSet<String>,
        include_invalidated: bool,
        direction: Direction,
    ) -> Result<Vec<Candidate>, Error> {
        let fset: HashSet<&str> = frontier.iter().map(String::as_str).collect();
        let mut out = Vec::new();
        for edge in self.graph.incident_edges(frontier, include_invalidated)? {
            let src_in = fset.contains(edge.source_node_uuid.as_str());
            let tgt_in = fset.contains(edge.target_node_uuid.as_str());
            let far = if src_in {
                edge.target_node_uuid.clone()
            } else if tgt_in {
                edge.source_node_uuid.clone()
            } else {
                continue;
            };
            if visited.contains(&far) {
                continue;
            }
            let matched = match direction {
                Direction::Out => Some(src_in),
                Direction::In => Some(tgt_in),
                Direction::Any => None,
            };
            out.push(Candidate {
                edge,
                far,
                matched,
                score: 0.0,
            });
        }
        Ok(out)
    }
}

/// Scores `cands` against `target` (+ `bonus` for direction matches), sorts best-first (edges
/// without a usable embedding last; ties by uuid) and keeps the top `beam`.
fn rank(cands: &mut Vec<Candidate>, target: &[f32], bonus: f32, beam: usize) {
    for c in cands.iter_mut() {
        let has_emb = c.edge.fact_embedding.iter().any(|x| *x != 0.0);
        c.score = if has_emb {
            cosine_similarity(target, &c.edge.fact_embedding)
                + if c.matched == Some(true) { bonus } else { 0.0 }
        } else {
            0.0
        };
    }
    cands.sort_by(|a, b| {
        let a_emb = a.edge.fact_embedding.iter().any(|x| *x != 0.0);
        let b_emb = b.edge.fact_embedding.iter().any(|x| *x != 0.0);
        b_emb
            .cmp(&a_emb)
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.edge.uuid.cmp(&b.edge.uuid))
    });
    cands.truncate(beam);
}

/// Runs the plan. `plan` must already be validated; `emb.hops` must have one vector per hop.
pub fn run<G: WalkGraph>(
    graph: &G,
    ontology: Option<&Ontology>,
    plan: &WalkPlan,
    emb: &WalkEmbeddings,
) -> Result<WalkOutcome, Error> {
    plan.validate()?;
    if emb.hops.len() != plan.hops.len() {
        return Err(Error::Ipc(
            "internal: hop embeddings do not match hops".to_string(),
        ));
    }
    let include_invalidated = plan.mode == WalkMode::History;
    let beam = plan.mode.beam();
    let mut w = Walker {
        graph,
        ontology,
        entities: HashMap::new(),
    };

    let mut visited: HashSet<String> = plan.start.iter().cloned().collect();
    let mut frontier: Vec<String> = plan.start.clone();
    let mut out_hops: Vec<WalkHop> = Vec::new();
    let mut fallbacks = WalkFallbacks::default();
    let mut dead_end_at = None;
    let last = plan.hops.len().saturating_sub(1);

    for (i, hop) in plan.hops.iter().enumerate() {
        let n = i + 1;
        let mut cands = w.candidates(&frontier, &visited, include_invalidated, hop.direction)?;
        let candidate_count = cands.len();
        if cands.is_empty() {
            dead_end_at = Some(n);
            break;
        }

        // Relation-type filter.
        if !hop.relation_types.is_empty() {
            let keep: Vec<bool> = cands
                .iter()
                .map(|c| {
                    hop.relation_types
                        .iter()
                        .any(|r| r == relation_name(&c.edge))
                })
                .collect();
            if keep.iter().any(|k| *k) {
                let mut it = keep.into_iter();
                cands.retain(|_| it.next().unwrap_or(false));
            } else {
                fallbacks.relation_type.push(n);
            }
        }

        // Answer-type filter (last hop, entity-type answer, not history).
        if i == last && plan.mode != WalkMode::History {
            if let Some(at) = plan.answer_type.as_deref() {
                w.ensure_entities(cands.iter().map(|c| c.far.clone()))?;
                let keep: Vec<bool> = cands
                    .iter()
                    .map(|c| has_type(w.labels(&c.far), at))
                    .collect();
                if keep.iter().any(|k| *k) {
                    let mut it = keep.into_iter();
                    cands.retain(|_| it.next().unwrap_or(false));
                } else {
                    fallbacks.answer_type.push(n);
                }
            }
        }

        // Hard direction filter.
        let mut bonus = 0.0;
        if hop.direction != Direction::Any {
            match plan.direction_mode {
                DirectionMode::Soft => bonus = DIRECTION_BONUS,
                DirectionMode::Hard => {
                    if cands.iter().any(|c| c.matched == Some(true)) {
                        cands.retain(|c| c.matched == Some(true));
                    } else {
                        fallbacks.direction.push(n);
                    }
                }
            }
        }

        rank(&mut cands, &emb.hops[i], bonus, beam);
        frontier = unique_far_ends(&cands);
        visited.extend(frontier.iter().cloned());
        let mut ents: Vec<String> = frontier.clone();
        for c in &cands {
            ents.push(c.edge.source_node_uuid.clone());
            ents.push(c.edge.target_node_uuid.clone());
        }
        w.ensure_entities(ents)?;
        out_hops.push(WalkHop {
            hop: n,
            completion: false,
            candidate_count,
            edges: cands.iter().map(|c| w.to_walk_edge(c)).collect(),
        });
    }

    // Completion hop.
    let mut completion_hop_ran = false;
    if dead_end_at.is_none() && plan.mode != WalkMode::History {
        if let (Some(at), Some(qemb)) = (plan.answer_type.as_deref(), emb.query.as_ref()) {
            if plan.has_query() {
                w.ensure_entities(frontier.iter().cloned())?;
                let reached = frontier.iter().any(|u| has_type(w.labels(u), at));
                if !reached {
                    let n = plan.hops.len() + 1;
                    let mut cands = w.candidates(&frontier, &visited, false, Direction::Any)?;
                    let candidate_count = cands.len();
                    w.ensure_entities(cands.iter().map(|c| c.far.clone()))?;
                    cands.retain(|c| has_type(w.labels(&c.far), at));
                    if !cands.is_empty() {
                        completion_hop_ran = true;
                        rank(&mut cands, qemb, 0.0, beam);
                        let mut ents = Vec::new();
                        for c in &cands {
                            ents.push(c.edge.source_node_uuid.clone());
                            ents.push(c.edge.target_node_uuid.clone());
                        }
                        w.ensure_entities(ents)?;
                        out_hops.push(WalkHop {
                            hop: n,
                            completion: true,
                            candidate_count,
                            edges: cands.iter().map(|c| w.to_walk_edge(c)).collect(),
                        });
                    }
                }
            }
        }
    }

    let facts_text = facts_text(&out_hops);
    Ok(WalkOutcome {
        hops: out_hops,
        fallbacks,
        completion_hop_ran,
        dead_end_at,
        facts_text,
    })
}

fn unique_far_ends(cands: &[Candidate]) -> Vec<String> {
    let mut seen = HashSet::new();
    cands
        .iter()
        .filter(|c| seen.insert(c.far.clone()))
        .map(|c| c.far.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory hand-built graph.
    #[derive(Default)]
    struct MemGraph {
        edges: Vec<RelatesToEdge>,
        ents: Vec<EntityRow>,
    }

    impl MemGraph {
        fn ent(&mut self, uuid: &str, ty: &str) -> &mut Self {
            self.ents.push(EntityRow {
                uuid: uuid.into(),
                name: uuid.to_uppercase(),
                labels: if ty.is_empty() {
                    vec!["Entity".into()]
                } else {
                    vec!["Entity".into(), ty.into()]
                },
                ..Default::default()
            });
            self
        }

        /// `emb` is the fact embedding (2-D for easy cosines).
        fn edge(
            &mut self,
            uuid: &str,
            src: &str,
            dst: &str,
            rel: &str,
            emb: [f32; 2],
        ) -> &mut Self {
            self.edges.push(RelatesToEdge {
                uuid: uuid.into(),
                name: rel.into(),
                source_node_uuid: src.into(),
                target_node_uuid: dst.into(),
                fact: format!("{src} {rel} {dst}"),
                fact_embedding: emb.to_vec(),
                attributes: "{}".into(),
                relation_type: Some(rel.into()),
                ..Default::default()
            });
            self
        }

        fn invalidate(&mut self, uuid: &str) -> &mut Self {
            for e in &mut self.edges {
                if e.uuid == uuid {
                    e.invalid_at = Some("2024-05-01 00:00:00".into());
                }
            }
            self
        }
    }

    impl WalkGraph for MemGraph {
        fn incident_edges(
            &self,
            frontier: &[String],
            include_invalidated: bool,
        ) -> Result<Vec<RelatesToEdge>, Error> {
            Ok(self
                .edges
                .iter()
                .filter(|e| {
                    (frontier.contains(&e.source_node_uuid)
                        || frontier.contains(&e.target_node_uuid))
                        && (include_invalidated || e.invalid_at.is_none())
                })
                .cloned()
                .collect())
        }

        fn entities(&self, uuids: &[String]) -> Result<Vec<EntityRow>, Error> {
            Ok(self
                .ents
                .iter()
                .filter(|e| uuids.contains(&e.uuid))
                .cloned()
                .collect())
        }
    }

    fn hop(rels: &[&str], dir: Direction) -> Hop {
        Hop {
            relation_types: rels.iter().map(|s| s.to_string()).collect(),
            phrase: "p".into(),
            direction: dir,
        }
    }

    fn plan(start: &str, hops: Vec<Hop>) -> WalkPlan {
        WalkPlan {
            start: vec![start.into()],
            hops,
            answer_type: None,
            query: None,
            mode: WalkMode::Single,
            direction_mode: DirectionMode::Soft,
        }
    }

    /// Every hop's phrase embeds to `[1, 0]`, the query too.
    fn emb(n: usize) -> WalkEmbeddings {
        WalkEmbeddings {
            hops: vec![vec![1.0, 0.0]; n],
            query: Some(vec![1.0, 0.0]),
        }
    }

    fn uuids(h: &WalkHop) -> Vec<&str> {
        h.edges.iter().map(|e| e.uuid.as_str()).collect()
    }

    #[test]
    fn two_hop_chain_is_found_with_typed_facts() {
        let mut g = MemGraph::default();
        g.ent("a", "Person").ent("b", "Project").ent("c", "Company");
        g.edge("e1", "a", "b", "OWNS", [1.0, 0.0])
            .edge("e2", "b", "c", "SUPPLIED_BY", [1.0, 0.0]);
        let p = plan(
            "a",
            vec![
                hop(&["OWNS"], Direction::Out),
                hop(&["SUPPLIED_BY"], Direction::Out),
            ],
        );
        let o = run(&g, None, &p, &emb(2)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e1"]);
        assert_eq!(uuids(&o.hops[1]), vec!["e2"]);
        assert_eq!(o.facts_text, "- a OWNS b (A [Person] -> B [Project])\n- b SUPPLIED_BY c (B [Project] -> C [Company])");
        assert_eq!(o.hops[1].edges[0].target.entity_type, "Company");
        assert!(!o.completion_hop_ran);
        assert_eq!(o.dead_end_at, None);
        assert_eq!(o.fallbacks, WalkFallbacks::default());
    }

    #[test]
    fn relation_filter_falls_back_and_empty_list_is_no_filter() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T");
        g.edge("e1", "a", "b", "R1", [1.0, 0.0]);
        let o = run(
            &g,
            None,
            &plan("a", vec![hop(&["NOPE"], Direction::Any)]),
            &emb(1),
        )
        .unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e1"]);
        assert_eq!(o.fallbacks.relation_type, vec![1]);
        let o = run(
            &g,
            None,
            &plan("a", vec![hop(&[], Direction::Any)]),
            &emb(1),
        )
        .unwrap();
        assert!(o.fallbacks.relation_type.is_empty());
    }

    #[test]
    fn soft_direction_ranks_matching_edge_above_equal_reversed_one() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T").ent("c", "T");
        // Reversed edge sorts first by uuid and has an identical embedding.
        g.edge("e1", "b", "a", "R", [1.0, 0.0])
            .edge("e2", "a", "c", "R", [1.0, 0.0]);
        let o = run(
            &g,
            None,
            &plan("a", vec![hop(&["R"], Direction::Out)]),
            &emb(1),
        )
        .unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e2", "e1"]);
        assert_eq!(o.hops[0].edges[0].direction_matched, Some(true));
        assert_eq!(o.hops[0].edges[1].direction_matched, Some(false));
        assert!(
            (o.hops[0].edges[0].score - o.hops[0].edges[1].score - DIRECTION_BONUS).abs() < 1e-6
        );
    }

    #[test]
    fn hard_direction_drops_reversed_and_falls_back_when_none_match() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T").ent("c", "T");
        g.edge("e1", "b", "a", "R", [1.0, 0.0])
            .edge("e2", "a", "c", "R", [1.0, 0.0]);
        let mut p = plan("a", vec![hop(&["R"], Direction::Out)]);
        p.direction_mode = DirectionMode::Hard;
        let o = run(&g, None, &p, &emb(1)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e2"]);
        assert!(o.fallbacks.direction.is_empty());

        // Only the reversed edge exists: hard falls back and says so.
        let mut g2 = MemGraph::default();
        g2.ent("a", "T").ent("b", "T");
        g2.edge("e1", "b", "a", "R", [1.0, 0.0]);
        let o = run(&g2, None, &p, &emb(1)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e1"]);
        assert_eq!(o.fallbacks.direction, vec![1]);
    }

    #[test]
    fn direction_any_applies_no_bonus_or_flag() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T");
        g.edge("e1", "b", "a", "R", [1.0, 0.0]);
        let o = run(
            &g,
            None,
            &plan("a", vec![hop(&["R"], Direction::Any)]),
            &emb(1),
        )
        .unwrap();
        assert_eq!(o.hops[0].edges[0].direction_matched, None);
        assert!((o.hops[0].edges[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn answer_type_filter_on_last_hop_and_its_fallback() {
        let mut g = MemGraph::default();
        g.ent("a", "Person").ent("b", "Project").ent("c", "Company");
        // The non-company edge scores higher; the type filter must still win.
        g.edge("e1", "a", "b", "R", [1.0, 0.0])
            .edge("e2", "a", "c", "R", [0.5, 1.0]);
        let mut p = plan("a", vec![hop(&["R"], Direction::Out)]);
        p.answer_type = Some("Company".into());
        let o = run(&g, None, &p, &emb(1)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e2"]);
        assert!(o.fallbacks.answer_type.is_empty());

        p.answer_type = Some("Nonexistent".into());
        let o = run(&g, None, &p, &emb(1)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e1", "e2"]);
        assert_eq!(o.fallbacks.answer_type, vec![1]);
    }

    #[test]
    fn answer_type_filter_only_applies_on_last_hop_and_not_in_history() {
        let mut g = MemGraph::default();
        g.ent("a", "Person")
            .ent("b", "Project")
            .ent("c", "Company")
            .ent("d", "Company");
        g.edge("e1", "a", "b", "R", [1.0, 0.0])
            .edge("e2", "a", "c", "R", [0.5, 1.0])
            .edge("e3", "b", "d", "R", [1.0, 0.0]);
        let mut p = plan(
            "a",
            vec![hop(&["R"], Direction::Out), hop(&["R"], Direction::Out)],
        );
        p.answer_type = Some("Company".into());
        let o = run(&g, None, &p, &emb(2)).unwrap();
        // Hop 1 is not the last hop: unfiltered, the Project edge wins.
        assert_eq!(uuids(&o.hops[0]), vec!["e1", "e2"]);

        let mut p1 = plan("a", vec![hop(&["R"], Direction::Out)]);
        p1.answer_type = Some("Company".into());
        p1.mode = WalkMode::History;
        let o = run(&g, None, &p1, &emb(1)).unwrap();
        assert_eq!(
            uuids(&o.hops[0]),
            vec!["e1", "e2"],
            "no type filter in history mode"
        );
    }

    #[test]
    fn completion_hop_reaches_answer_typed_entity() {
        let mut g = MemGraph::default();
        g.ent("a", "Person")
            .ent("b", "Project")
            .ent("c", "Company")
            .ent("x", "Person");
        g.edge("e1", "a", "b", "OWNS", [1.0, 0.0])
            .edge("e2", "b", "x", "STAFFED_BY", [1.0, 0.0])
            .edge("e3", "b", "c", "SUPPLIED_BY", [0.5, 1.0]);
        let mut p = plan("a", vec![hop(&["OWNS"], Direction::Out)]);
        p.answer_type = Some("Company".into());
        p.query = Some("who supplies the project".into());
        let o = run(&g, None, &p, &emb(1)).unwrap();
        // Hop 1's answer-type filter finds no Company (falls back), then the completion hop runs.
        assert_eq!(o.fallbacks.answer_type, vec![1]);
        assert!(o.completion_hop_ran);
        assert_eq!(o.hops.len(), 2);
        assert!(o.hops[1].completion);
        assert_eq!(o.hops[1].hop, 2);
        assert_eq!(
            uuids(&o.hops[1]),
            vec!["e3"],
            "only the Company-typed far end"
        );
        assert!(o
            .facts_text
            .contains("b SUPPLIED_BY c (B [Project] -> C [Company])"));

        // No query → no completion hop.
        p.query = None;
        let o = run(
            &g,
            None,
            &p,
            &WalkEmbeddings {
                hops: vec![vec![1.0, 0.0]],
                query: None,
            },
        )
        .unwrap();
        assert!(!o.completion_hop_ran);
        assert_eq!(o.hops.len(), 1);
    }

    #[test]
    fn no_completion_hop_when_answer_type_already_reached() {
        let mut g = MemGraph::default();
        g.ent("a", "Person").ent("c", "Company").ent("d", "Company");
        g.edge("e1", "a", "c", "R", [1.0, 0.0])
            .edge("e2", "c", "d", "R", [1.0, 0.0]);
        let mut p = plan("a", vec![hop(&["R"], Direction::Out)]);
        p.answer_type = Some("Company".into());
        p.query = Some("q".into());
        let o = run(&g, None, &p, &emb(1)).unwrap();
        assert!(!o.completion_hop_ran);
        assert_eq!(o.hops.len(), 1);
    }

    #[test]
    fn empty_hops_runs_only_the_completion_hop() {
        let mut g = MemGraph::default();
        g.ent("a", "Person").ent("c", "Company");
        g.edge("e1", "a", "c", "R", [1.0, 0.0]);
        let mut p = plan("a", vec![]);
        p.answer_type = Some("Company".into());
        p.query = Some("q".into());
        let o = run(&g, None, &p, &emb(0)).unwrap();
        assert!(o.completion_hop_ran);
        assert_eq!(uuids(&o.hops[0]), vec!["e1"]);

        // Without answer_type + query an empty plan is a validation error.
        assert!(run(&g, None, &plan("a", vec![]), &emb(0)).is_err());
    }

    #[test]
    fn visited_nodes_are_not_revisited() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T").ent("c", "T");
        // a-b, b-a (back edge, higher score), b-c.
        g.edge("e1", "a", "b", "R", [1.0, 0.0])
            .edge("e2", "b", "a", "R", [1.0, 0.0])
            .edge("e3", "b", "c", "R", [0.5, 1.0]);
        let o = run(
            &g,
            None,
            &plan(
                "a",
                vec![hop(&["R"], Direction::Any), hop(&["R"], Direction::Any)],
            ),
            &emb(2),
        )
        .unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e1", "e2"]);
        // Hop 2 from b: the edges back to `a` are excluded; only b→c remains.
        assert_eq!(uuids(&o.hops[1]), vec!["e3"]);
    }

    #[test]
    fn self_loops_do_not_produce_a_frontier() {
        let mut g = MemGraph::default();
        g.ent("a", "T");
        g.edge("e1", "a", "a", "R", [1.0, 0.0]);
        let o = run(
            &g,
            None,
            &plan("a", vec![hop(&["R"], Direction::Any)]),
            &emb(1),
        )
        .unwrap();
        assert!(o.hops.is_empty());
        assert_eq!(o.dead_end_at, Some(1));
    }

    #[test]
    fn history_mode_includes_invalidated_edges_and_live_modes_do_not() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T").ent("c", "T");
        g.edge("e1", "a", "b", "R", [1.0, 0.0])
            .edge("e2", "a", "c", "R", [0.9, 0.1]);
        g.invalidate("e1");
        let mut p = plan("a", vec![hop(&["R"], Direction::Any)]);
        let o = run(&g, None, &p, &emb(1)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e2"]);
        p.mode = WalkMode::History;
        let o = run(&g, None, &p, &emb(1)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e1", "e2"]);
        assert_eq!(
            o.hops[0].edges[0].invalid_at.as_deref(),
            Some("2024-05-01 00:00:00")
        );
        assert!(o
            .facts_text
            .lines()
            .next()
            .unwrap()
            .ends_with("[until 2024-05-01]"));
    }

    #[test]
    fn beam_is_three_for_single_and_ten_for_list() {
        let mut g = MemGraph::default();
        g.ent("a", "T");
        for i in 0..12 {
            let n = format!("n{i:02}");
            g.ent(&n, "T");
            g.edge(&format!("e{i:02}"), "a", &n, "R", [1.0, 0.0]);
        }
        let mut p = plan("a", vec![hop(&["R"], Direction::Any)]);
        assert_eq!(run(&g, None, &p, &emb(1)).unwrap().hops[0].edges.len(), 3);
        p.mode = WalkMode::List;
        assert_eq!(run(&g, None, &p, &emb(1)).unwrap().hops[0].edges.len(), 10);
        assert_eq!(
            run(&g, None, &p, &emb(1)).unwrap().hops[0].candidate_count,
            12
        );
    }

    #[test]
    fn ranking_is_by_cosine_with_uuid_tiebreak_and_missing_embeddings_last() {
        let mut g = MemGraph::default();
        g.ent("a", "T");
        for n in ["b", "c", "d", "e"] {
            g.ent(n, "T");
        }
        g.edge("e4", "a", "e", "R", [0.0, 0.0]) // missing/zero embedding
            .edge("e2", "a", "c", "R", [1.0, 0.0])
            .edge("e1", "a", "b", "R", [1.0, 0.0]) // ties with e2 → uuid order
            .edge("e3", "a", "d", "R", [0.0, 1.0]);
        let mut p = plan("a", vec![hop(&["R"], Direction::Any)]);
        p.mode = WalkMode::List;
        let o = run(&g, None, &p, &emb(1)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e1", "e2", "e3", "e4"]);
        assert_eq!(o.hops[0].edges[3].score, 0.0);
    }

    #[test]
    fn untyped_endpoint_renders_without_a_type_marker() {
        let mut g = MemGraph::default();
        g.ent("a", "Person").ent("b", "");
        g.edge("e1", "a", "b", "R", [1.0, 0.0]);
        let o = run(
            &g,
            None,
            &plan("a", vec![hop(&["R"], Direction::Any)]),
            &emb(1),
        )
        .unwrap();
        assert_eq!(o.hops[0].edges[0].target.entity_type, "");
        assert_eq!(o.facts_text, "- a R b (A [Person] -> B)");
    }

    #[test]
    fn facts_text_is_deduplicated_across_multiple_starts() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T").ent("c", "T");
        g.edge("e1", "a", "b", "R", [1.0, 0.0])
            .edge("e2", "b", "c", "R", [1.0, 0.0]);
        let mut p = plan("a", vec![hop(&["R"], Direction::Any)]);
        p.start = vec!["a".into(), "c".into()];
        p.mode = WalkMode::List;
        let o = run(&g, None, &p, &emb(1)).unwrap();
        // e1 reached from a, e2 reached from c; each fact appears once.
        assert_eq!(o.facts_text.lines().count(), 2);
        let mut dup = o.hops.clone();
        dup.push(dup[0].clone());
        assert_eq!(facts_text(&dup).lines().count(), 2);
    }

    #[test]
    fn dead_end_stops_the_walk_and_reports_the_hop() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T");
        g.edge("e1", "a", "b", "R", [1.0, 0.0]);
        let p = plan(
            "a",
            vec![hop(&["R"], Direction::Any), hop(&["R"], Direction::Any)],
        );
        let o = run(&g, None, &p, &emb(2)).unwrap();
        assert_eq!(o.hops.len(), 1);
        assert_eq!(o.dead_end_at, Some(2));
    }

    #[test]
    fn signature_mismatch_uses_persisted_flag_then_ontology() {
        use crate::ontology::{OntologyMode, RelationTypeDef};
        let onto = Ontology {
            mode: OntologyMode::Open,
            entity_types: vec![],
            relation_types: vec![RelationTypeDef {
                name: "OWNS".into(),
                description: None,
                source_type: Some("Person".into()),
                target_type: Some("Project".into()),
                aliases: vec![],
                keywords: vec![],
                extract: true,
            }],
            ancestor_map: HashMap::new(),
        };
        let mut g = MemGraph::default();
        g.ent("a", "Person").ent("b", "Project").ent("c", "Company");
        g.edge("ok", "a", "b", "OWNS", [1.0, 0.0])
            .edge("bad", "c", "b", "OWNS", [1.0, 0.0])
            .edge("flagged", "a", "b", "OWNS", [1.0, 0.0]);
        g.edges[2].attributes = r#"{"signature_mismatch": true}"#.into();
        let mut p = plan("b", vec![hop(&["OWNS"], Direction::Any)]);
        p.mode = WalkMode::List;
        let o = run(&g, Some(&onto), &p, &emb(1)).unwrap();
        let by = |u: &str| {
            o.hops[0]
                .edges
                .iter()
                .find(|e| e.uuid == u)
                .unwrap()
                .signature_mismatch
        };
        assert!(!by("ok"));
        assert!(by("bad"));
        assert!(by("flagged"));
        // No ontology: never a mismatch (unless persisted).
        let o = run(&g, None, &p, &emb(1)).unwrap();
        assert!(
            !o.hops[0]
                .edges
                .iter()
                .find(|e| e.uuid == "bad")
                .unwrap()
                .signature_mismatch
        );
    }

    #[test]
    fn parse_helpers_validate() {
        assert!(WalkMode::parse("nope").is_err());
        assert!(DirectionMode::parse("nope").is_err());
        assert!(Direction::parse("nope").is_err());
        assert_eq!(parse_answer_type(Some("value")), None);
        assert_eq!(parse_answer_type(Some(" ")), None);
        assert_eq!(parse_answer_type(Some("Company")), Some("Company".into()));
    }
}
