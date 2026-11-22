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
//! 1. candidates = edges incident to the frontier whose far end has not been visited; on the
//!    **last** hop of a value walk (`answer_type` absent or `"value"`), also the frontier
//!    entities' own self-loops, which carry literal-valued facts (#823, ADR-0823). A self-loop
//!    matches either direction (`direction_matched` is `null`; it gets the soft bonus and is
//!    always kept by the hard filter, whose match-or-fall-back decision is made over moving
//!    edges only) and never becomes frontier. When a hop's only candidates
//!    are self-loops the walk could not move, so the hop is still reported as `dead_end_at`,
//!    with the self-loop facts in it;
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
    /// `include_provenance` (#860): attach each returned edge's live producing episodes and
    /// their source files. Off by default; when off nothing extra is read or serialized.
    pub include_provenance: bool,
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

    /// Source identity of the **live** episodes among `uuids` (#860). An episode that no longer
    /// exists, is superseded, or is a pending document revision is simply absent. Only called
    /// when the plan asks for provenance. The default reports nothing.
    fn live_episode_sources(
        &self,
        _uuids: &[String],
    ) -> Result<HashMap<String, EpisodeSource>, Error> {
        Ok(HashMap::new())
    }

    /// For each entity uuid, the uuids of episodes (restricted to `group_ids`) that MENTION it
    /// (#860). Used only for the legacy-edge source fallback. The default reports nothing.
    fn mentioning_episodes(
        &self,
        _entity_uuids: &[String],
        _group_ids: &[String],
    ) -> Result<HashMap<String, Vec<String>>, Error> {
        Ok(HashMap::new())
    }
}

/// Maximum episodes reported per edge with `include_provenance`.
pub const MAX_PROVENANCE_EPISODES: usize = 20;

/// Source identity of one live episode (#853): its `source_file` when stored, and the composed
/// `source_description` display value (the only source a legacy episode has).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EpisodeSource {
    pub group_id: String,
    pub source_file: Option<String>,
    pub source_description: String,
}

impl EpisodeSource {
    /// The citation: `source_file` when set and non-empty, else `source_description` as stored
    /// (never split — the composed value may contain colons). `None` when both are empty.
    fn citation(&self) -> Option<&str> {
        self.source_file
            .as_deref()
            .filter(|f| !f.is_empty())
            .or_else(|| Some(self.source_description.as_str()).filter(|d| !d.is_empty()))
    }
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
    /// Present only when the plan asked for `include_provenance`; flattened so the keys are
    /// appended after `signature_mismatch` and absent (not `null`) when off.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<WalkProvenance>,
    /// Edge-side inputs to the provenance lookup; filled only when provenance is requested.
    #[serde(skip)]
    recorded: Option<RecordedProvenance>,
}

/// Where a walked edge came from (`include_provenance`, #860).
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct WalkProvenance {
    /// Live producing episodes in recorded order, at most [`MAX_PROVENANCE_EPISODES`].
    pub episodes: Vec<String>,
    /// Distinct sources of all live producing episodes (not only the capped list).
    pub source_files: Vec<String>,
    /// More than [`MAX_PROVENANCE_EPISODES`] live producing episodes exist.
    pub episodes_truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct RecordedProvenance {
    episodes: Vec<String>,
    group_id: String,
    source_descriptions: Vec<String>,
    warranted: bool,
    source: String,
    target: String,
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
    /// 1-based hop at which the walk could not move: no candidates (ending the walk early), or,
    /// on the last hop of a value walk, only the frontier's own self-loops (a self-loop is not a
    /// move; that hop is still returned with their facts).
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

/// Declared `(source_types, target_types)` of a relation type (#816 list shape). Undeclared
/// entity types are ignored, so a side with none left accepts any type.
fn declared_signature<'a>(
    onto: &Ontology,
    def: &'a crate::ontology::RelationTypeDef,
) -> (Vec<&'a str>, Vec<&'a str>) {
    (
        onto.effective_source_types(def),
        onto.effective_target_types(def),
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
    let (src_decl, tgt_decl) = declared_signature(onto, def);
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
    /// `Some` for `out`/`in` hops; always `None` for a self-loop.
    matched: Option<bool>,
    /// A frontier entity's own self-loop (a literal-object fact, ADR-0823). Only offered on the
    /// last hop of a value walk; it matches either direction and never extends the frontier.
    self_loop: bool,
    score: f32,
}

impl Candidate {
    /// Whether the candidate fits the hop's direction: a direction-matching edge, or a self-loop
    /// (which matches either direction).
    fn fits_direction(&self) -> bool {
        self.self_loop || self.matched == Some(true)
    }
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

/// Resolves the effective ontology of a group (by `group_id`), for `signature_mismatch`.
pub type OntologyResolver<'a> = &'a dyn Fn(&str) -> Option<&'a Ontology>;

struct Walker<'a, G: WalkGraph> {
    graph: &'a G,
    ontologies: OntologyResolver<'a>,
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

    fn to_walk_edge(&self, c: &Candidate, include_provenance: bool) -> WalkEdge {
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
                (self.ontologies)(&e.group_id),
                e,
                self.labels(&e.source_node_uuid),
                self.labels(&e.target_node_uuid),
            ),
            provenance: None,
            recorded: include_provenance.then(|| RecordedProvenance {
                episodes: e.episodes.clone(),
                group_id: e.group_id.clone(),
                source_descriptions: e.source_descriptions.clone(),
                warranted: e.warrant.is_some(),
                source: e.source_node_uuid.clone(),
                target: e.target_node_uuid.clone(),
            }),
        }
    }

    /// Candidates at a frontier: incident edges whose far end is unvisited, oriented against
    /// `direction`. A frontier entity is always visited, so this excludes self-loops, unless
    /// `include_self_loops` is set (the last hop of a value walk, #823 / ADR-0823). Then a
    /// frontier entity's own self-loops (its literal-valued facts) are candidates too, marked
    /// [`Candidate::self_loop`]: they match either direction (`matched` is `None`) and never
    /// extend the frontier.
    fn candidates(
        &self,
        frontier: &[String],
        visited: &HashSet<String>,
        include_invalidated: bool,
        direction: Direction,
        include_self_loops: bool,
    ) -> Result<Vec<Candidate>, Error> {
        let fset: HashSet<&str> = frontier.iter().map(String::as_str).collect();
        let mut out = Vec::new();
        for edge in self.graph.incident_edges(frontier, include_invalidated)? {
            if edge.source_node_uuid == edge.target_node_uuid {
                if include_self_loops && fset.contains(edge.source_node_uuid.as_str()) {
                    let far = edge.source_node_uuid.clone();
                    out.push(Candidate {
                        edge,
                        far,
                        matched: None,
                        self_loop: true,
                        score: 0.0,
                    });
                }
                continue;
            }
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
                self_loop: false,
                score: 0.0,
            });
        }
        Ok(out)
    }
}

/// Whether the edge's fact embedding can be compared with `target`: non-zero and the same
/// dimension. A zero vector (embedder was unavailable at write time) or a length mismatch (the
/// embedder or its dimension changed since the edge was written) is unscorable —
/// `cosine_similarity` would return 0.0 for it and the direction bonus could lift it above a
/// genuinely relevant edge — so such edges rank last.
fn has_usable_embedding(c: &Candidate, target: &[f32]) -> bool {
    c.edge.fact_embedding.len() == target.len() && c.edge.fact_embedding.iter().any(|x| *x != 0.0)
}

/// Scores `cands` against `target` (+ `bonus` for direction matches), sorts best-first (edges
/// without a usable embedding last; ties by uuid) and keeps the top `beam`.
fn rank(cands: &mut Vec<Candidate>, target: &[f32], bonus: f32, beam: usize) {
    let usable: HashSet<String> = cands
        .iter()
        .filter(|c| has_usable_embedding(c, target))
        .map(|c| c.edge.uuid.clone())
        .collect();
    for c in cands.iter_mut() {
        c.score = if usable.contains(&c.edge.uuid) {
            cosine_similarity(target, &c.edge.fact_embedding)
                + if c.fits_direction() { bonus } else { 0.0 }
        } else {
            0.0
        };
    }
    cands.sort_by(|a, b| {
        usable
            .contains(&b.edge.uuid)
            .cmp(&usable.contains(&a.edge.uuid))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.edge.uuid.cmp(&b.edge.uuid))
    });
    cands.truncate(beam);
}

/// Runs the plan with a single ontology applied to every edge. `plan` must already be validated;
/// `emb.hops` must have one vector per hop.
pub fn run<G: WalkGraph>(
    graph: &G,
    ontology: Option<&Ontology>,
    plan: &WalkPlan,
    emb: &WalkEmbeddings,
) -> Result<WalkOutcome, Error> {
    run_with_ontologies(graph, &|_| ontology, plan, emb)
}

/// Like [`run`], but each edge's `signature_mismatch` is checked against the ontology of the
/// edge's own `group_id`, so a walk over several groups never applies one group's relation
/// signatures to another group's edges.
pub fn run_with_ontologies<'a, G: WalkGraph>(
    graph: &'a G,
    ontologies: OntologyResolver<'a>,
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
        ontologies,
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
        // #823 / ADR-0823: a literal-valued fact is stored as a self-loop on its entity. On the
        // last hop of a value walk (`answer_type` absent or "value"), the frontier's own
        // self-loops are candidates too, so the walk can return the entity's values. Never on
        // an earlier hop: they would take beam slots meant for moving to the next entity.
        let include_self_loops = i == last && plan.answer_type.is_none();
        let mut cands = w.candidates(
            &frontier,
            &visited,
            include_invalidated,
            hop.direction,
            include_self_loops,
        )?;
        let candidate_count = cands.len();
        if cands.is_empty() {
            dead_end_at = Some(n);
            break;
        }
        // A self-loop is not a move: when it is the only kind of candidate, the walk still could
        // not move here, so the hop is a dead end. Its facts are returned all the same. This
        // can only happen on the last hop, so nothing after it is skipped.
        if cands.iter().all(|c| c.self_loop) {
            dead_end_at = Some(n);
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
                    // The match/fallback decision is over *moving* candidates only: a self-loop
                    // (#823) fits either direction, so letting it count as a match would hide
                    // the fallback for real edges. Self-loops are kept either way.
                    if cands.iter().any(|c| !c.self_loop) {
                        if cands
                            .iter()
                            .any(|c| !c.self_loop && c.matched == Some(true))
                        {
                            cands.retain(Candidate::fits_direction);
                        } else {
                            fallbacks.direction.push(n);
                        }
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
            edges: cands
                .iter()
                .map(|c| w.to_walk_edge(c, plan.include_provenance))
                .collect(),
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
                    let mut cands =
                        w.candidates(&frontier, &visited, false, Direction::Any, false)?;
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
                            edges: cands
                                .iter()
                                .map(|c| w.to_walk_edge(c, plan.include_provenance))
                                .collect(),
                        });
                    }
                }
            }
        }
    }

    if plan.include_provenance {
        attach_provenance(graph, &mut out_hops)?;
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

/// Fills [`WalkEdge::provenance`] on every returned edge (#860) with one batched episode lookup
/// (plus one mention lookup when a legacy edge needs the fallback). A post-selection pass: it
/// never changes which edges are returned or their order.
///
/// Per edge:
/// 1. recorded producing episodes → those the graph reports live, in the edge's own group, in
///    recorded order. `episodes` is the first [`MAX_PROVENANCE_EPISODES`]; `episodes_truncated`
///    is "more than that"; `source_files` covers **all** live ones. None live → both empty, no
///    fallback (the stored descriptions would cite disowned sources).
/// 2. no recorded episodes (legacy row, direct assertion, cross-group edge) → `episodes` is
///    empty and `source_files` is the edge's own `source_descriptions`, else — for an edge with
///    no warrant — the sources of live episodes that mention **both** endpoints.
fn attach_provenance<G: WalkGraph>(graph: &G, hops: &mut [WalkHop]) -> Result<(), Error> {
    // Edges needing the mention fallback: no recorded episodes, no own descriptions, no warrant.
    let needs_mentions = |r: &RecordedProvenance| {
        r.episodes.is_empty() && r.source_descriptions.is_empty() && !r.warranted
    };
    let mut ep_uuids: Vec<String> = Vec::new();
    let mut ent_uuids: Vec<String> = Vec::new();
    let mut groups: Vec<String> = Vec::new();
    for e in hops.iter().flat_map(|h| h.edges.iter()) {
        let Some(r) = e.recorded.as_ref() else {
            continue;
        };
        ep_uuids.extend(r.episodes.iter().cloned());
        if needs_mentions(r) {
            ent_uuids.push(r.source.clone());
            ent_uuids.push(r.target.clone());
            groups.push(r.group_id.clone());
        }
    }
    let dedup = |v: &mut Vec<String>| {
        let mut seen = HashSet::new();
        v.retain(|x| seen.insert(x.clone()));
    };
    dedup(&mut ent_uuids);
    dedup(&mut groups);

    let mentions = if ent_uuids.is_empty() {
        HashMap::new()
    } else {
        graph.mentioning_episodes(&ent_uuids, &groups)?
    };
    ep_uuids.extend(mentions.values().flatten().cloned());
    dedup(&mut ep_uuids);
    let live = if ep_uuids.is_empty() {
        HashMap::new()
    } else {
        graph.live_episode_sources(&ep_uuids)?
    };

    for e in hops.iter_mut().flat_map(|h| h.edges.iter_mut()) {
        let Some(r) = e.recorded.take() else {
            continue;
        };
        let mut prov = WalkProvenance::default();
        if !r.episodes.is_empty() {
            let live_eps: Vec<(&String, &EpisodeSource)> = r
                .episodes
                .iter()
                .filter_map(|u| live.get(u).map(|s| (u, s)))
                .filter(|(_, s)| s.group_id == r.group_id)
                .collect();
            prov.episodes_truncated = live_eps.len() > MAX_PROVENANCE_EPISODES;
            prov.episodes = live_eps
                .iter()
                .take(MAX_PROVENANCE_EPISODES)
                .map(|(u, _)| (*u).clone())
                .collect();
            push_distinct(
                &mut prov.source_files,
                live_eps.iter().filter_map(|(_, s)| s.citation()),
            );
        } else if !r.source_descriptions.is_empty() {
            push_distinct(
                &mut prov.source_files,
                r.source_descriptions
                    .iter()
                    .map(String::as_str)
                    .filter(|d| !d.is_empty()),
            );
        } else if needs_mentions(&r) {
            let of = |ent: &str| -> HashSet<&str> {
                mentions
                    .get(ent)
                    .map(|v| v.iter().map(String::as_str).collect())
                    .unwrap_or_default()
            };
            let (a, b) = (of(&r.source), of(&r.target));
            let mut both: Vec<&str> = a.intersection(&b).copied().collect();
            both.sort_unstable();
            push_distinct(
                &mut prov.source_files,
                both.into_iter()
                    .filter_map(|u| live.get(u))
                    .filter(|s| s.group_id == r.group_id)
                    .filter_map(EpisodeSource::citation),
            );
        }
        e.provenance = Some(prov);
    }
    Ok(())
}

/// Appends each value not already present, keeping first-seen order.
fn push_distinct<'a>(out: &mut Vec<String>, vals: impl Iterator<Item = &'a str>) {
    for v in vals {
        if !out.iter().any(|o| o == v) {
            out.push(v.to_string());
        }
    }
}

/// The next frontier: the distinct far ends of the kept candidates. A self-loop never extends
/// the frontier (its far end is the entity it sits on, already visited).
fn unique_far_ends(cands: &[Candidate]) -> Vec<String> {
    let mut seen = HashSet::new();
    cands
        .iter()
        .filter(|c| !c.self_loop && seen.insert(c.far.clone()))
        .map(|c| c.far.clone())
        .collect()
}

/// Parses `hops` (absent ⇒ empty). Each hop: `{relation_types?, phrase, direction?}`;
/// `direction` defaults to `any`.
pub fn parse_hops(v: &serde_json::Value) -> Result<Vec<Hop>, Error> {
    let arr = match v {
        serde_json::Value::Null => return Ok(vec![]),
        serde_json::Value::Array(a) => a,
        _ => return Err(Error::Ipc("hops must be an array".to_string())),
    };
    arr.iter()
        .enumerate()
        .map(|(i, h)| {
            let n = i + 1;
            let obj = h
                .as_object()
                .ok_or_else(|| Error::Ipc(format!("hops[{n}] must be an object")))?;
            let relation_types = match obj.get("relation_types") {
                None | Some(serde_json::Value::Null) => vec![],
                Some(serde_json::Value::Array(a)) => a
                    .iter()
                    .map(|r| {
                        r.as_str().map(str::to_string).ok_or_else(|| {
                            Error::Ipc(format!(
                                "hops[{n}].relation_types must be an array of strings"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => {
                    return Err(Error::Ipc(format!(
                        "hops[{n}].relation_types must be an array of strings"
                    )))
                }
            };
            let phrase = obj
                .get("phrase")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| Error::Ipc(format!("hops[{n}].phrase is required")))?
                .to_string();
            let direction = match obj.get("direction") {
                None | Some(serde_json::Value::Null) => Direction::Any,
                Some(serde_json::Value::String(s)) => Direction::parse(s)?,
                Some(_) => return Err(Error::Ipc(format!("hops[{n}].direction must be a string"))),
            };
            Ok(Hop {
                relation_types,
                phrase,
                direction,
            })
        })
        .collect()
}

/// An optional boolean param: absent/`null` → `false`.
fn bool_param(v: &serde_json::Value, name: &str) -> Result<bool, Error> {
    match v {
        serde_json::Value::Null => Ok(false),
        serde_json::Value::Bool(b) => Ok(*b),
        _ => Err(Error::Ipc(format!("{name} must be a boolean"))),
    }
}

fn str_param<'a>(v: &'a serde_json::Value, name: &str) -> Result<Option<&'a str>, Error> {
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s.as_str())),
        _ => Err(Error::Ipc(format!("{name} must be a string"))),
    }
}

/// A parsed `knowledge_walk` request: the raw start name and a shape-validated plan whose `start`
/// is a placeholder until the handler resolves the name to entities.
#[derive(Debug, Clone)]
pub struct ParsedWalkRequest {
    pub start: String,
    pub plan: WalkPlan,
}

/// Parses and validates the params of a `knowledge_walk` request (everything except `group_ids`,
/// which the handler resolves against its default group). Pure: no DB, no embedder.
pub fn parse_request(p: &serde_json::Value) -> Result<ParsedWalkRequest, Error> {
    let start = p["start"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Ipc("start is required".to_string()))?
        .to_string();
    let mode = WalkMode::parse(str_param(&p["mode"], "mode")?.unwrap_or("single"))?;
    let direction_mode =
        DirectionMode::parse(str_param(&p["direction_mode"], "direction_mode")?.unwrap_or("soft"))?;
    let hops = parse_hops(&p["hops"])?;
    let answer_type = parse_answer_type(str_param(&p["answer_type"], "answer_type")?);
    let query = str_param(&p["query"], "query")?
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let include_provenance = bool_param(&p["include_provenance"], "include_provenance")?;
    // Validate the plan shape up front (start entities are filled in after resolution).
    let plan = WalkPlan {
        start: vec![String::new()],
        hops,
        answer_type,
        query,
        mode,
        direction_mode,
        include_provenance,
    };
    plan.validate()?;
    Ok(ParsedWalkRequest { start, plan })
}

#[cfg(test)]
mod tests {

    #[test]
    fn parse_request_accepts_a_plan_and_rejects_bad_shapes() {
        let ok = serde_json::json!({"start": " Tern Core ", "mode": "list", "direction_mode": "soft",
            "hops": [{"relation_types": ["OWNS"], "phrase": "owner", "direction": "in"}],
            "answer_type": "value", "query": "q"});
        let r = parse_request(&ok).unwrap();
        assert_eq!(r.start, "Tern Core");
        assert_eq!(r.plan.hops.len(), 1);
        assert_eq!(r.plan.hops[0].direction, Direction::In);
        assert_eq!(r.plan.mode, WalkMode::List);
        assert!(r.plan.answer_type.is_none(), "value means no type filter");
        let bad_dir =
            serde_json::json!({"start": "x", "hops": [{"phrase": "p", "direction": "up"}]});
        assert!(parse_request(&bad_dir).is_err());
        let no_hops = serde_json::json!({"start": "x", "hops": []});
        assert!(parse_request(&no_hops)
            .unwrap_err()
            .to_string()
            .contains("hops is empty"));
        let completion = serde_json::json!({"start": "x", "answer_type": "Person", "query": "who"});
        assert!(parse_request(&completion).is_ok());
        assert!(parse_request(&serde_json::json!({"hops": []})).is_err());
        assert!(parse_request(&serde_json::json!({"start": "x", "mode": 3,
            "hops": [{"phrase": "p"}]}))
        .is_err());
    }
    use super::*;

    /// In-memory hand-built graph.
    #[derive(Default)]
    struct MemGraph {
        edges: Vec<RelatesToEdge>,
        ents: Vec<EntityRow>,
        /// Live episodes (anything absent is superseded / deleted).
        live: HashMap<String, EpisodeSource>,
        /// entity uuid → mentioning episode uuids.
        mentions: HashMap<String, Vec<String>>,
        lookups: std::cell::Cell<usize>,
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

        /// Registers a live episode in group `""` (the edges' default group).
        fn episode(&mut self, uuid: &str, file: Option<&str>, desc: &str) -> &mut Self {
            self.live.insert(
                uuid.into(),
                EpisodeSource {
                    group_id: String::new(),
                    source_file: file.map(str::to_string),
                    source_description: desc.into(),
                },
            );
            self
        }

        fn produced_by(&mut self, edge: &str, eps: &[&str]) -> &mut Self {
            for e in &mut self.edges {
                if e.uuid == edge {
                    e.episodes = eps.iter().map(|s| s.to_string()).collect();
                }
            }
            self
        }

        fn mention(&mut self, ent: &str, ep: &str) -> &mut Self {
            self.mentions.entry(ent.into()).or_default().push(ep.into());
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

        fn live_episode_sources(
            &self,
            uuids: &[String],
        ) -> Result<HashMap<String, EpisodeSource>, Error> {
            self.lookups.set(self.lookups.get() + 1);
            Ok(uuids
                .iter()
                .filter_map(|u| self.live.get(u).map(|s| (u.clone(), s.clone())))
                .collect())
        }

        fn mentioning_episodes(
            &self,
            entity_uuids: &[String],
            _group_ids: &[String],
        ) -> Result<HashMap<String, Vec<String>>, Error> {
            self.lookups.set(self.lookups.get() + 1);
            Ok(entity_uuids
                .iter()
                .filter_map(|u| self.mentions.get(u).map(|v| (u.clone(), v.clone())))
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
            include_provenance: false,
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

        // Entity-typed answer: self-loops are never candidates, so the walk dead-ends empty.
        let mut p = plan("a", vec![hop(&["R"], Direction::Any)]);
        p.answer_type = Some("T".into());
        let o = run(&g, None, &p, &emb(1)).unwrap();
        assert!(o.hops.is_empty());
        assert_eq!(o.dead_end_at, Some(1));

        // Value walk (#823): the self-loop's fact is returned on the last hop, but it is not a
        // move, so the hop is still the dead end.
        let o = run(
            &g,
            None,
            &plan("a", vec![hop(&["R"], Direction::Any)]),
            &emb(1),
        )
        .unwrap();
        assert_eq!(o.hops.len(), 1);
        assert_eq!(uuids(&o.hops[0]), vec!["e1"]);
        assert_eq!(o.hops[0].candidate_count, 1);
        assert_eq!(o.dead_end_at, Some(1));
    }

    /// #823 / ADR-0823: a literal-valued fact is a self-loop on its entity. A 2-hop value walk
    /// returns the second entity's self-loop on the last hop, ranked with the other candidates.
    #[test]
    fn value_walk_returns_last_entitys_self_loop_fact() {
        let mut g = MemGraph::default();
        g.ent("a", "Person").ent("b", "Project").ent("c", "Company");
        g.edge("e1", "a", "b", "OWNS", [1.0, 0.0])
            .edge("e2", "b", "b", "HAS_MEASURE", [1.0, 0.0])
            .edge("e3", "b", "c", "SUPPLIED_BY", [0.5, 1.0]);
        let p = plan(
            "a",
            vec![
                hop(&["OWNS"], Direction::Out),
                hop(&["HAS_MEASURE"], Direction::Out),
            ],
        );
        let o = run(&g, None, &p, &emb(2)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e1"]);
        // The relation filter keeps the self-loop and drops the SUPPLIED_BY move.
        assert_eq!(uuids(&o.hops[1]), vec!["e2"]);
        let e = &o.hops[1].edges[0];
        assert_eq!(
            e.direction_matched, None,
            "a self-loop matches either direction"
        );
        assert!(
            (e.score - 1.0 - DIRECTION_BONUS).abs() < 1e-6,
            "a self-loop gets the soft-direction bonus: {}",
            e.score
        );
        assert!(o.fallbacks.relation_type.is_empty());
        assert_eq!(
            o.facts_text,
            "- a OWNS b (A [Person] -> B [Project])\n- b HAS_MEASURE b (B [Project] -> B [Project])"
        );
        assert_eq!(
            o.facts_text.matches("(B [Project] -> B [Project])").count(),
            1,
            "the self-loop renders once"
        );
        // Only self-loops survived the filter, but a move existed, so no dead end.
        assert_eq!(o.dead_end_at, None);
    }

    #[test]
    fn self_loop_is_not_a_candidate_on_a_non_final_hop() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T").ent("c", "T");
        // a's self-loop would outrank a→b if it were a candidate on hop 1.
        g.edge("e0", "a", "a", "R", [1.0, 0.0])
            .edge("e1", "a", "b", "R", [0.5, 1.0])
            .edge("e2", "b", "c", "R", [1.0, 0.0]);
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
        assert_eq!(uuids(&o.hops[0]), vec!["e1"]);
        assert_eq!(o.hops[0].candidate_count, 1);
        // Hop 2 runs from b, not from a: a's self-loop is not on b's frontier.
        assert_eq!(uuids(&o.hops[1]), vec!["e2"]);
    }

    #[test]
    fn self_loop_never_becomes_frontier() {
        let mut g = MemGraph::default();
        g.ent("b", "T").ent("c", "T");
        g.edge("e1", "b", "b", "R", [1.0, 0.0])
            .edge("e2", "b", "c", "R", [0.5, 1.0]);
        let w = Walker {
            graph: &g,
            ontologies: &|_| None,
            entities: HashMap::new(),
        };
        let visited: HashSet<String> = ["b".to_string()].into_iter().collect();
        let cands = w
            .candidates(&["b".into()], &visited, false, Direction::Any, true)
            .unwrap();
        assert_eq!(cands.len(), 2);
        assert!(cands.iter().any(|c| c.self_loop && c.edge.uuid == "e1"));
        assert_eq!(unique_far_ends(&cands), vec!["c".to_string()]);
        // Self-loops only: no frontier at all.
        let only: Vec<Candidate> = cands.into_iter().filter(|c| c.self_loop).collect();
        assert!(unique_far_ends(&only).is_empty());
    }

    #[test]
    fn hard_direction_keeps_self_loops_as_matching() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T").ent("c", "T");
        g.edge("e1", "a", "a", "R", [1.0, 0.0])
            .edge("e2", "b", "a", "R", [1.0, 0.0])
            .edge("e3", "a", "c", "R", [1.0, 0.0]);
        let mut p = plan("a", vec![hop(&["R"], Direction::Out)]);
        p.direction_mode = DirectionMode::Hard;
        let o = run(&g, None, &p, &emb(1)).unwrap();
        // A matching move exists: the reversed move is dropped, the self-loop is kept.
        let mut got = uuids(&o.hops[0]);
        got.sort();
        assert_eq!(got, vec!["e1", "e3"]);
        assert!(o.fallbacks.direction.is_empty());
    }

    /// A self-loop does not count as a hard-direction match: when no *moving* edge matches, the
    /// hard filter still falls back (and records it) for the real edges, and the self-loop stays.
    #[test]
    fn hard_direction_fallback_is_decided_over_moving_edges_only() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T");
        g.edge("e1", "a", "a", "R", [1.0, 0.0])
            .edge("e2", "b", "a", "R", [1.0, 0.0]);
        let mut p = plan("a", vec![hop(&["R"], Direction::Out)]);
        p.direction_mode = DirectionMode::Hard;
        let o = run(&g, None, &p, &emb(1)).unwrap();
        let mut got = uuids(&o.hops[0]);
        got.sort();
        assert_eq!(
            got,
            vec!["e1", "e2"],
            "the incoming edge survives the fallback"
        );
        assert_eq!(o.fallbacks.direction, vec![1]);

        // Only self-loops: there is no real edge to relax the constraint for, so no fallback.
        let mut g2 = MemGraph::default();
        g2.ent("a", "T");
        g2.edge("e1", "a", "a", "R", [1.0, 0.0]);
        let o = run(&g2, None, &p, &emb(1)).unwrap();
        assert_eq!(uuids(&o.hops[0]), vec!["e1"]);
        assert!(o.fallbacks.direction.is_empty());
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
                source_types: vec!["Person".into()],
                target_types: vec!["Project".into()],
                aliases: vec![],
                inverse_aliases: vec![],
                symmetric: false,
                keywords: vec![],
                extract: true,
            }],
            ancestor_map: HashMap::from([
                ("Person".to_string(), vec!["Person".to_string()]),
                ("Project".to_string(), vec!["Project".to_string()]),
            ]),
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
    fn signature_mismatch_uses_each_edges_group_ontology() {
        use crate::ontology::{OntologyMode, RelationTypeDef};
        let onto = Ontology {
            mode: OntologyMode::Open,
            entity_types: vec![],
            relation_types: vec![RelationTypeDef {
                name: "OWNS".into(),
                description: None,
                source_types: vec!["Person".into()],
                target_types: vec!["Project".into()],
                aliases: vec![],
                inverse_aliases: vec![],
                symmetric: false,
                keywords: vec![],
                extract: true,
            }],
            ancestor_map: HashMap::from([
                ("Person".to_string(), vec!["Person".to_string()]),
                ("Project".to_string(), vec!["Project".to_string()]),
            ]),
        };
        let mut g = MemGraph::default();
        g.ent("a", "Company").ent("b", "Project");
        // Same shape in two groups; only group `ga` has an ontology.
        g.edge("in_ga", "a", "b", "OWNS", [1.0, 0.0])
            .edge("in_gb", "a", "b", "OWNS", [1.0, 0.0]);
        g.edges[0].group_id = "ga".into();
        g.edges[1].group_id = "gb".into();
        let mut p = plan("b", vec![hop(&["OWNS"], Direction::Any)]);
        p.mode = WalkMode::List;
        let resolve = |gid: &str| (gid == "ga").then_some(&onto);
        let o = run_with_ontologies(&g, &resolve, &p, &emb(1)).unwrap();
        let by = |u: &str| {
            o.hops[0]
                .edges
                .iter()
                .find(|e| e.uuid == u)
                .unwrap()
                .signature_mismatch
        };
        assert!(by("in_ga"), "checked against its own group's ontology");
        assert!(!by("in_gb"), "group gb has no ontology: never a mismatch");
    }

    #[test]
    fn wrong_dimension_embedding_ranks_last_not_by_bonus() {
        let mut g = MemGraph::default();
        g.ent("a", "T").ent("b", "T").ent("c", "T");
        // `stale` has a 3-D embedding (embedder changed) but matches the direction (+0.05 bonus);
        // `ok` is reversed but genuinely relevant. cosine(stale) would be 0.0 + 0.05.
        g.edge("ok", "b", "a", "R", [0.01, 1.0])
            .edge("stale", "a", "c", "R", [0.0, 0.0]);
        g.edges[1].fact_embedding = vec![1.0, 0.0, 0.0];
        let p = plan("a", vec![hop(&["R"], Direction::Out)]);
        let o = run(&g, None, &p, &emb(1)).unwrap();
        // emb(1) phrase is [1, 0]: ok scores ~0.01 (below the 0.05 bonus a stale edge would get), stale is unscorable (0.0) and ranks last.
        assert_eq!(o.hops[0].edges[0].uuid, "ok");
        assert_eq!(o.hops[0].edges[1].uuid, "stale");
        assert_eq!(o.hops[0].edges[1].score, 0.0);
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

    // ── include_provenance (#860) ─────────────────────────────────────────────

    fn prov_plan(start: &str, hops: Vec<Hop>) -> WalkPlan {
        WalkPlan {
            include_provenance: true,
            ..plan(start, hops)
        }
    }

    fn walk_one(g: &MemGraph, p: &WalkPlan) -> WalkOutcome {
        run(g, None, p, &emb(p.hops.len())).unwrap()
    }

    fn prov_of(o: &WalkOutcome) -> &WalkProvenance {
        o.hops[0].edges[0].provenance.as_ref().expect("provenance")
    }

    fn one_edge_graph() -> MemGraph {
        let mut g = MemGraph::default();
        g.ent("a", "Person").ent("b", "Project");
        g.edge("e1", "a", "b", "OWNS", [1.0, 0.0]);
        g
    }

    #[test]
    fn parse_request_include_provenance() {
        let base = |v: serde_json::Value| {
            let mut j = serde_json::json!({"start": "x", "hops": [{"phrase": "p"}]});
            if !v.is_null() {
                j["include_provenance"] = v;
            }
            parse_request(&j)
        };
        assert!(
            !base(serde_json::Value::Null)
                .unwrap()
                .plan
                .include_provenance
        );
        assert!(
            !base(serde_json::json!(false))
                .unwrap()
                .plan
                .include_provenance
        );
        assert!(
            base(serde_json::json!(true))
                .unwrap()
                .plan
                .include_provenance
        );
        let err = base(serde_json::json!("yes")).unwrap_err().to_string();
        assert!(
            err.contains("include_provenance must be a boolean"),
            "{err}"
        );
        assert!(base(serde_json::json!(1)).is_err());
    }

    #[test]
    fn provenance_returns_live_episodes_and_files_in_recorded_order() {
        let mut g = one_edge_graph();
        g.episode("ep2", Some("b.md"), "b.md:1")
            .episode("ep1", Some("a.md"), "a.md:0")
            .produced_by("e1", &["ep1", "ep2"]);
        let o = walk_one(&g, &prov_plan("a", vec![hop(&[], Direction::Out)]));
        let p = prov_of(&o);
        assert_eq!(p.episodes, vec!["ep1", "ep2"]);
        assert_eq!(p.source_files, vec!["a.md", "b.md"]);
        assert!(!p.episodes_truncated);
        assert!(g.lookups.get() >= 1);
    }

    #[test]
    fn superseded_and_missing_episodes_are_excluded_even_in_history() {
        let mut g = one_edge_graph();
        // ep1 superseded (absent from `live`), ep3 deleted, ep2 live.
        g.episode("ep2", Some("b.md"), "b.md:1")
            .produced_by("e1", &["ep1", "ep2", "ep3"]);
        let mut p = prov_plan("a", vec![hop(&[], Direction::Out)]);
        for mode in [WalkMode::Single, WalkMode::History] {
            p.mode = mode;
            let o = walk_one(&g, &p);
            let pr = prov_of(&o);
            assert_eq!(pr.episodes, vec!["ep2"]);
            assert_eq!(pr.source_files, vec!["b.md"]);
        }
    }

    #[test]
    fn all_producers_dead_gives_empty_lists_and_no_fallback() {
        let mut g = one_edge_graph();
        g.produced_by("e1", &["gone"]);
        g.edges[0].source_descriptions = vec!["stale.md:3".into()];
        let o = walk_one(&g, &prov_plan("a", vec![hop(&[], Direction::Out)]));
        assert_eq!(o.hops[0].edges.len(), 1, "provenance never filters edges");
        let p = prov_of(&o);
        assert!(p.episodes.is_empty() && p.source_files.is_empty() && !p.episodes_truncated);
    }

    #[test]
    fn episode_cap_is_twenty_and_truncation_is_exact() {
        for (n, truncated) in [(19usize, false), (20, false), (21, true), (25, true)] {
            let mut g = one_edge_graph();
            let ids: Vec<String> = (0..n).map(|i| format!("ep{i:02}")).collect();
            for (i, id) in ids.iter().enumerate() {
                g.episode(id, Some(&format!("f{i:02}.md")), "d");
            }
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            g.produced_by("e1", &refs);
            let o = walk_one(&g, &prov_plan("a", vec![hop(&[], Direction::Out)]));
            let p = prov_of(&o);
            assert_eq!(p.episodes.len(), n.min(MAX_PROVENANCE_EPISODES), "n={n}");
            assert_eq!(p.episodes_truncated, truncated, "n={n}");
            assert_eq!(
                p.episodes,
                ids[..n.min(20)].to_vec(),
                "recorded order, n={n}"
            );
            // source_files covers every live episode, not only the 20 shown.
            assert_eq!(p.source_files.len(), n, "n={n}");
            // Deterministic across runs.
            assert_eq!(
                prov_of(&walk_one(
                    &g,
                    &prov_plan("a", vec![hop(&[], Direction::Out)])
                )),
                p
            );
        }
    }

    #[test]
    fn source_files_dedup_and_legacy_description_is_unsplit() {
        let mut g = one_edge_graph();
        g.episode("ep1", Some("https://h:8080/x.md"), "https://h:8080/x.md:4")
            .episode("ep2", Some("https://h:8080/x.md"), "https://h:8080/x.md:5")
            .episode("ep3", None, "legacy.md:7")
            .produced_by("e1", &["ep1", "ep2", "ep3"]);
        let o = walk_one(&g, &prov_plan("a", vec![hop(&[], Direction::Out)]));
        assert_eq!(
            prov_of(&o).source_files,
            vec!["https://h:8080/x.md", "legacy.md:7"]
        );
    }

    #[test]
    fn group_mismatch_is_not_live_for_the_edge() {
        let mut g = one_edge_graph();
        g.episode("ep1", Some("a.md"), "d")
            .produced_by("e1", &["ep1"]);
        g.live.get_mut("ep1").unwrap().group_id = "other".into();
        let o = walk_one(&g, &prov_plan("a", vec![hop(&[], Direction::Out)]));
        assert!(prov_of(&o).episodes.is_empty());
    }

    #[test]
    fn no_recorded_episodes_falls_back_to_edge_source_descriptions() {
        let mut g = one_edge_graph();
        g.edges[0].source_descriptions =
            vec!["old.md:1".into(), "old.md:1".into(), "x.md:2".into()];
        let o = walk_one(&g, &prov_plan("a", vec![hop(&[], Direction::Out)]));
        let p = prov_of(&o);
        assert!(p.episodes.is_empty());
        assert_eq!(p.source_files, vec!["old.md:1", "x.md:2"]);
    }

    #[test]
    fn mention_fallback_needs_both_endpoints_live_and_no_warrant() {
        let mut g = one_edge_graph();
        g.episode("epB", Some("both.md"), "both.md:1")
            .episode("epA", Some("onlya.md"), "onlya.md:1")
            .episode("epD", Some("dead-not-registered"), "x");
        g.live.remove("epD");
        g.mention("a", "epB")
            .mention("b", "epB")
            .mention("a", "epA")
            .mention("a", "epD")
            .mention("b", "epD");
        let p = prov_plan("a", vec![hop(&[], Direction::Out)]);
        let o = walk_one(&g, &p);
        let pr = prov_of(&o);
        assert!(pr.episodes.is_empty(), "mention episodes are not producers");
        assert_eq!(pr.source_files, vec!["both.md"]);

        g.edges[0].warrant = Some("assertion".into());
        let o = walk_one(&g, &p);
        assert!(
            prov_of(&o).source_files.is_empty(),
            "no fallback for a warrant"
        );
    }

    #[test]
    fn completion_hop_and_self_loops_carry_provenance() {
        let mut g = MemGraph::default();
        g.ent("a", "Person").ent("b", "Project").ent("c", "Company");
        g.edge("e1", "a", "b", "OWNS", [1.0, 0.0])
            .edge("e2", "b", "c", "SUPPLIED_BY", [1.0, 0.0])
            .edge("loop", "b", "b", "HAS_NOTE", [1.0, 0.0]);
        g.episode("ep1", Some("a.md"), "d")
            .produced_by("e1", &["ep1"])
            .produced_by("e2", &["ep1"])
            .produced_by("loop", &["ep1"]);
        // Value walk, last hop: edge + self-loop.
        let o = walk_one(
            &g,
            &prov_plan(
                "a",
                vec![hop(&[], Direction::Out), hop(&[], Direction::Out)],
            ),
        );
        let all: Vec<&WalkEdge> = o.hops.iter().flat_map(|h| h.edges.iter()).collect();
        assert!(all.iter().any(|e| e.uuid == "loop"));
        assert!(all.iter().all(|e| e.provenance.is_some()));
        // Completion hop.
        let mut p = prov_plan("a", vec![hop(&[], Direction::Out)]);
        p.answer_type = Some("Company".into());
        p.query = Some("q".into());
        let o = walk_one(&g, &p);
        assert!(o.completion_hop_ran);
        assert!(o
            .hops
            .iter()
            .flat_map(|h| h.edges.iter())
            .all(|e| e.provenance.is_some()));
    }

    #[test]
    fn flag_off_adds_nothing_and_flag_on_changes_nothing_else() {
        let mut g = one_edge_graph();
        g.episode("ep1", Some("a.md"), "d")
            .produced_by("e1", &["ep1"]);
        let off = walk_one(&g, &plan("a", vec![hop(&[], Direction::Out)]));
        assert_eq!(g.lookups.get(), 0, "no lookup without the flag");
        let e = &off.hops[0].edges[0];
        assert!(e.provenance.is_none());
        let j = serde_json::to_value(e).unwrap();
        for k in [
            "episodes",
            "source_files",
            "episodes_truncated",
            "provenance",
        ] {
            assert!(j.get(k).is_none(), "{k} must be absent when off");
        }
        let on = walk_one(&g, &prov_plan("a", vec![hop(&[], Direction::Out)]));
        assert_eq!(on.facts_text, off.facts_text);
        let mut on_json = serde_json::to_value(&on.hops[0].edges[0]).unwrap();
        assert!(on_json["episodes"].is_array());
        assert!(on_json["source_files"].is_array());
        assert_eq!(on_json["episodes_truncated"], false);
        let obj = on_json.as_object_mut().unwrap();
        obj.remove("episodes");
        obj.remove("source_files");
        obj.remove("episodes_truncated");
        assert_eq!(on_json, j, "all pre-existing fields identical");
    }
}
