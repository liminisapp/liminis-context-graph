//! Graph-aware search (issue #797, ADR-0797): one deterministic read that composes entity
//! linking, neighbourhood expansion, belief time, vector and full-text search.
//!
//! The pipeline embeds the query **once** (`EmbedRole::Query`, ADR-0790) and then runs every
//! stage on one connection:
//!
//! 1. **Seeds** — four entity candidate lists (exact name via `lookup_key` n-gram probes, BM25
//!    over name + summary, name vector, summary vector) fused with RRF, gated by absolute
//!    evidence (ANN always fills `k`, #613) and capped.
//! 2. **Expansion** — up to [`MAX_HOPS`] hops of batched frontier lookups. Each fact is scored
//!    from its fact embedding (cosine against the query) plus a BM25 bonus, decays with hop
//!    distance, and is pruned by per-entity fan-out, a beam and a total cap.
//! 3. **Belief time** — believed-only by default for this read (ADR-0705's anticipated
//!    "believed-by-default read"); `believed_only` / `as_of` mean what they mean elsewhere.
//!    A superseded edge is never a traversal step, so paths break at it.
//! 4. **Fusion** — RRF over (a) passages supporting the selected facts and reached entities,
//!    (b) plain passage search and (c) the facts themselves.
//!
//! Every result carries an evidence trail (lists, seeds, path, supporting episodes). Derived and
//! answer content (ADR-0752) may be cited but never adds rank: it is excluded from list (a) and
//! from the fact list, and is attached to a primary sibling fact as a non-corroborating citation.
//!
//! Existing reads are untouched: this module composes the `Conn`-level primitives directly
//! instead of calling (or refactoring) `search.rs`'s hybrid searches, each of which embeds.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde::Serialize;

use crate::belief::{self, BeliefAnnotation, SupersessionCause, SupersessionInfo};
use crate::db::{compute_lookup_key, cosine_similarity, Conn, Db, EpisodePassage, FrontierEdge};
use crate::embedder::{EmbedRole, Embedder};
use crate::error::Error;
use crate::origin::Origin;
use crate::search::rrf_fuse_scored;
use crate::types::{EntityRow, PassageResult, RelatesToEdge};

// ── Tuning constants (internal defaults; see ADR-0797) ────────────────────────

/// Candidates drawn from each seed list before fusion.
pub const SEED_CANDIDATES: usize = 40;
/// Maximum seeds kept after the admission gate.
pub const SEED_CAP: usize = 5;
/// A BM25 hit ranked above this (0-based) is admitted as a seed on lexical evidence alone.
pub const SEED_BM25_RANK: usize = 10;
/// A vector-only seed needs at least this cosine similarity (name or summary)...
pub const SEED_SIM_FLOOR: f64 = 0.5;
/// ...and must be within this margin of the best vector similarity among the candidates.
pub const SEED_SIM_MARGIN: f64 = 0.08;
/// Longest word n-gram of the query probed as an exact entity name.
pub const MAX_NGRAM_WORDS: usize = 4;
/// Maximum distinct n-grams probed.
pub const MAX_NGRAMS: usize = 128;
/// Hop limit of the walk.
pub const MAX_HOPS: usize = 2;
/// Multiplicative score decay per hop beyond the first.
pub const HOP_DECAY: f64 = 0.6;
/// Edges kept per frontier entity per hop, ranked by relevance.
pub const PER_ENTITY_FANOUT: usize = 20;
/// Hop-n edges whose far endpoints form the hop-(n+1) frontier.
pub const BEAM: usize = 10;
/// Maximum facts the whole walk keeps.
pub const TOTAL_EDGE_CAP: usize = 200;
/// Per-direction row cap of one frontier query.
pub const FRONTIER_ROW_CAP: usize = 1000;
/// Size of the global fact BM25 list used for the lexical edge bonus.
pub const EDGE_BM25_LIMIT: usize = 100;
/// Maximum lexical bonus (BM25 rank 0) added to a fact's cosine relevance.
pub const EDGE_BM25_BONUS: f64 = 0.15;
/// Maximum supporting-passage candidates fetched and scored.
pub const PASSAGE_CANDIDATE_CAP: usize = 300;
/// Largest accepted `num_results`.
pub const MAX_NUM_RESULTS: usize = 100;

/// Which stages run (FR-018). The full pipeline is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stages {
    /// Seeds only: list (a) restricted to passages mentioning a seed.
    Seeds,
    /// Seeds + expansion: lists (a) and (c).
    Expand,
    /// Everything: lists (a), (b) and (c).
    Full,
}

impl Stages {
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "seeds" => Ok(Stages::Seeds),
            "expand" => Ok(Stages::Expand),
            "full" => Ok(Stages::Full),
            other => Err(Error::Ipc(format!(
                "stages {other:?} is invalid: must be one of seeds, expand, full"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Stages::Seeds => "seeds",
            Stages::Expand => "expand",
            Stages::Full => "full",
        }
    }
}

/// A parsed `knowledge_graph_search` request.
#[derive(Debug, Clone)]
pub struct GraphSearchParams {
    pub query: String,
    /// `None` = every group; `Some(vec![])` matches nothing (#413).
    pub group_ids: Option<Vec<String>>,
    pub num_results: usize,
    pub origins: Option<Vec<Origin>>,
    /// Drop facts (and passages) not believed / live at `as_of_us`.
    pub believed_only: bool,
    pub as_of_us: i64,
    pub stages: Stages,
}

// ── Response types ────────────────────────────────────────────────────────────

/// A seed entity and how it was matched.
#[derive(Debug, Clone, Serialize)]
pub struct SeedEvidence {
    pub uuid: String,
    pub name: String,
    pub group_id: String,
    pub kind: String,
    /// Fused seed score, normalised so the best seed scores 1.0.
    pub score: f64,
    /// Which lists matched it: `name`, `bm25`, `name_vector`, `summary_vector`.
    pub matched: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_similarity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_similarity: Option<f64>,
}

/// One edge walked, in traversal direction (`from` is the side the walk came from).
#[derive(Debug, Clone, Serialize)]
pub struct PathStep {
    pub edge_uuid: String,
    pub relation: String,
    pub fact: String,
    pub from_uuid: String,
    pub from_name: String,
    pub to_uuid: String,
    pub to_name: String,
    pub hop: usize,
}

/// One fused list that returned a result: its 1-based rank and raw list score.
#[derive(Debug, Clone, Serialize)]
pub struct ListEvidence {
    /// `supporting` (a), `passages` (b) or `facts` (c).
    pub list: &'static str,
    pub rank: usize,
    pub score: f64,
}

/// An episode that supports (or, when `corroborating` is false, merely cites) a result.
#[derive(Debug, Clone, Serialize)]
pub struct SupportingEpisode {
    pub uuid: String,
    pub source_description: String,
    pub origin: &'static str,
    pub corroborating: bool,
}

/// How a supporting passage was reached: through a fact it produced, or an entity it mentions.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Via {
    Fact { edge_uuid: String },
    Mention { entity_uuid: String },
}

/// Why a result was returned (FR-016).
#[derive(Debug, Clone, Serialize)]
pub struct GraphEvidence {
    pub lists: Vec<ListEvidence>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub seeds: Vec<SeedEvidence>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<PathStep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub supporting_episodes: Vec<SupportingEpisode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<Via>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warrant: Option<String>,
    /// Further paths that reached the same item (the best one is `path`).
    pub alternate_paths: usize,
}

/// Belief state of a passage, in the vocabulary of the edge `belief` annotation.
#[derive(Debug, Clone, Serialize)]
pub struct PassageBelief {
    /// `believed`, `superseded` or `not_yet_ingested`.
    pub state: &'static str,
    pub believed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersession: Option<SupersessionInfo>,
}

/// One ranked result: a fact or a passage.
#[derive(Debug, Clone, Serialize)]
pub struct GraphSearchResult {
    /// `fact` or `passage`.
    pub kind: &'static str,
    /// Fused RRF score.
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fact: Option<RelatesToEdge>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passage: Option<PassageResult>,
    pub belief: serde_json::Value,
    /// Per-origin support of a fact (ADR-0752), as `knowledge_find_relationships` reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corroboration: Option<usize>,
    pub evidence: GraphEvidence,
}

/// A cap that truncated the walk or a candidate pool.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Truncation {
    pub cap: &'static str,
    pub limit: usize,
}

/// Cost and shape of one request.
#[derive(Debug, Clone, Serialize)]
pub struct GraphSearchStats {
    pub stages: &'static str,
    /// `vector` until hybrid passage search (#789) is available to list (b).
    pub passage_mode: &'static str,
    pub believed_only: bool,
    pub as_of: String,
    pub embed_calls: usize,
    pub db_queries: usize,
    pub seed_candidates: usize,
    pub seeds: usize,
    pub expanded_edges: usize,
    pub hops: usize,
    pub truncated: bool,
    pub truncations: Vec<Truncation>,
    pub no_seeds: bool,
}

/// The `knowledge_graph_search` response (ADR-0020 envelope plus `seeds` and `stats`).
#[derive(Debug, Clone, Serialize)]
pub struct GraphSearchResponse {
    pub results: Vec<GraphSearchResult>,
    pub count: usize,
    pub seeds: Vec<SeedEvidence>,
    pub stats: GraphSearchStats,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Runs the graph-aware search. One embed call; every lookup on one connection in one
/// blocking task, holding no lock (the search-read convention).
pub async fn graph_search(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    params: GraphSearchParams,
) -> Result<GraphSearchResponse, Error> {
    if matches!(&params.group_ids, Some(g) if g.is_empty()) {
        // `group_ids: []` matches no group (#413): nothing to embed or look up.
        return Ok(empty_response(&params, 0));
    }
    let embedding = embedder.embed(&params.query, EmbedRole::Query).await?;
    tokio::task::spawn_blocking(move || -> Result<GraphSearchResponse, Error> {
        let conn = db.connect()?;
        run(&conn, &params, &embedding)
    })
    .await?
}

fn empty_response(p: &GraphSearchParams, embed_calls: usize) -> GraphSearchResponse {
    GraphSearchResponse {
        results: vec![],
        count: 0,
        seeds: vec![],
        stats: new_stats(p, embed_calls),
    }
}

fn new_stats(p: &GraphSearchParams, embed_calls: usize) -> GraphSearchStats {
    GraphSearchStats {
        stages: p.stages.as_str(),
        passage_mode: "vector",
        believed_only: p.believed_only,
        as_of: belief::format_us(p.as_of_us),
        embed_calls,
        db_queries: 0,
        seed_candidates: 0,
        seeds: 0,
        expanded_edges: 0,
        hops: 0,
        truncated: false,
        truncations: vec![],
        no_seeds: true,
    }
}

impl GraphSearchStats {
    fn truncate(&mut self, cap: &'static str, limit: usize) {
        self.truncated = true;
        let t = Truncation { cap, limit };
        if !self.truncations.contains(&t) {
            self.truncations.push(t);
        }
    }
}

// ── Pure helpers ──────────────────────────────────────────────────────────────

/// Word n-grams (1..=[`MAX_NGRAM_WORDS`]) of `query`, lowercased, with surrounding punctuation
/// and a trailing possessive `'s` stripped from each word — the exact-name candidates. Shortest
/// first, deduplicated, at most [`MAX_NGRAMS`].
pub fn query_ngrams(query: &str) -> Vec<String> {
    let is_punct = |c: char| !c.is_alphanumeric();
    let tokens: Vec<String> = query
        .split_whitespace()
        .filter_map(|t| {
            let t = t.trim_matches(is_punct);
            let t = t
                .strip_suffix("'s")
                .or_else(|| t.strip_suffix("\u{2019}s"))
                .unwrap_or(t);
            let t = t.trim_matches(is_punct);
            (!t.is_empty()).then(|| t.to_lowercase())
        })
        .collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for n in 1..=MAX_NGRAM_WORDS {
        for w in tokens.windows(n) {
            let gram = w.join(" ");
            if seen.insert(gram.clone()) {
                out.push(gram);
                if out.len() == MAX_NGRAMS {
                    return out;
                }
            }
        }
    }
    out
}

/// The query as plain alphanumeric terms for `QUERY_FTS_INDEX`, so punctuation (e.g. `→`, #646)
/// can never break the FTS parser. Replace with #789's `sanitize_fts_query` once it lands.
pub fn fts_terms(query: &str) -> String {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cosine similarity clamped to `[0, 1]`; empty, mismatched or non-finite inputs score 0.
fn relevance(query: &[f32], v: &[f32]) -> f64 {
    let s = cosine_similarity(query, v) as f64;
    if s.is_finite() {
        s.max(0.0)
    } else {
        0.0
    }
}

/// A fact's relevance: cosine of its embedding plus a bonus for its rank in the global fact
/// BM25 list (`None` when it is not in that list).
pub fn edge_relevance(cosine: f64, bm25_rank: Option<usize>) -> f64 {
    let bonus = bm25_rank
        .filter(|r| *r < EDGE_BM25_LIMIT)
        .map_or(0.0, |r| {
            EDGE_BM25_BONUS * (1.0 - r as f64 / EDGE_BM25_LIMIT as f64)
        });
    cosine + bonus
}

/// A path's score: the root seed's score, decayed per hop beyond the first, times the last
/// edge's relevance. Only the last edge's relevance counts (the beam gates the bridging edge),
/// so a weakly worded bridge does not double-penalise the multi-hop answer.
pub fn path_score(seed_score: f64, hop: usize, rel: f64) -> f64 {
    seed_score * HOP_DECAY.powi(hop.saturating_sub(1) as i32) * rel
}

/// An episode live at `t_us`: visible (a committed revision, if any), not superseded at `t_us`,
/// and ingested by then. The one liveness definition this read uses for supporting and plain
/// passages (ADR-0797).
pub fn episode_live_at(ep: &EpisodePassage, t_us: i64) -> bool {
    ep.supersession.live_at(t_us) && ep.ingested_at_us.is_none_or(|i| i <= t_us)
}

fn passage_belief(ep: &EpisodePassage, t_us: i64) -> PassageBelief {
    let s = &ep.supersession;
    let state = if ep.ingested_at_us.is_some_and(|i| i > t_us) || !s.visible_at(t_us) {
        "not_yet_ingested"
    } else if s.superseded_at_us.is_some_and(|at| at <= t_us) {
        "superseded"
    } else {
        "believed"
    };
    let supersession = s.superseded_at_us.map(|at| {
        let document = s.kind.as_deref() == Some("document");
        SupersessionInfo {
            cause: if document {
                SupersessionCause::DocumentSuperseded
            } else {
                SupersessionCause::Corrected
            },
            superseded_at: belief::format_us(at),
            superseded_by_document: if document {
                s.superseded_by.clone()
            } else {
                None
            },
            in_force_until: None,
            recorded_at: None,
        }
    });
    PassageBelief {
        state,
        believed: state == "believed",
        supersession,
    }
}

/// Descending score, ascending key: the one total order every ranked list here uses.
fn by_score_then_key<K: Ord>(a: (f64, &K), b: (f64, &K)) -> std::cmp::Ordering {
    b.0.partial_cmp(&a.0)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.1.cmp(b.1))
}

// ── Internal state ────────────────────────────────────────────────────────────

struct Seed {
    row: EntityRow,
    evidence: SeedEvidence,
}

/// How the walk reached an entity (seeds: hop 0, empty path).
#[derive(Clone)]
struct Reach {
    root: usize,
    score: f64,
    hop: usize,
    path: Vec<PathStep>,
}

struct FactHit {
    fe: FrontierEdge,
    score: f64,
    root: usize,
    path: Vec<PathStep>,
    alternates: usize,
    belief: BeliefAnnotation,
    /// Every surviving producing episode is derived / answer: cited, never ranked (ADR-0752).
    citation_only: bool,
}

struct EpInfo {
    source_description: String,
    origin: Origin,
}

#[derive(Clone)]
struct Source {
    score: f64,
    via: Via,
    root: usize,
    path: Vec<PathStep>,
}

fn via_key(v: &Via) -> (u8, &str) {
    match v {
        Via::Fact { edge_uuid } => (0, edge_uuid.as_str()),
        Via::Mention { entity_uuid } => (1, entity_uuid.as_str()),
    }
}

/// Keeps the best source per episode: max score (never a sum — a restatement adds no mass),
/// ties broken by the smaller via key so the choice is deterministic.
fn offer(sources: &mut BTreeMap<String, Source>, ep: &str, s: Source) {
    match sources.get(ep) {
        Some(cur)
            if cur.score > s.score
                || (cur.score == s.score && via_key(&cur.via) <= via_key(&s.via)) => {}
        _ => {
            sources.insert(ep.to_string(), s);
        }
    }
}

// ── The pipeline ──────────────────────────────────────────────────────────────

fn run(
    conn: &Conn<'_>,
    p: &GraphSearchParams,
    emb: &[f32],
) -> Result<GraphSearchResponse, Error> {
    let mut st = new_stats(p, 1);
    let t = p.as_of_us;
    let gid_refs: Option<Vec<&str>> = p
        .group_ids
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect());
    let gids = gid_refs.as_deref();
    let fts_q = fts_terms(&p.query);
    let k = p.num_results;
    let list_cap = (3 * k).max(30);

    // ── Stage 1: seeds ──
    let seeds = seed_stage(conn, p, emb, gids, &fts_q, &mut st)?;
    st.seeds = seeds.len();
    st.no_seeds = seeds.is_empty();

    let mut names: HashMap<String, String> = HashMap::new();
    let mut reach: BTreeMap<String, Reach> = BTreeMap::new();
    for (i, s) in seeds.iter().enumerate() {
        names.insert(s.row.uuid.clone(), s.row.name.clone());
        reach.insert(
            s.row.uuid.clone(),
            Reach {
                root: i,
                score: s.evidence.score,
                hop: 0,
                path: vec![],
            },
        );
    }

    // Episode metadata seen anywhere in the request, for evidence trails.
    let mut ep_info: HashMap<String, EpInfo> = HashMap::new();

    // ── Stage 2: expansion ──
    let mut facts: BTreeMap<String, FactHit> = BTreeMap::new();
    if p.stages != Stages::Seeds && !seeds.is_empty() {
        expand(
            conn,
            p,
            emb,
            gids,
            &fts_q,
            &seeds,
            &mut names,
            &mut reach,
            &mut facts,
            &mut ep_info,
            &mut st,
        )?;
    }
    st.expanded_edges = facts.len();

    // ── List (c): facts ──
    let mut fact_order: Vec<(&String, &FactHit)> =
        facts.iter().filter(|(_, f)| !f.citation_only).collect();
    fact_order.sort_by(|a, b| by_score_then_key((a.1.score, a.0), (b.1.score, b.0)));
    let list_facts: Vec<(String, f64)> = fact_order
        .iter()
        .take(list_cap)
        .map(|(u, f)| (format!("f:{u}"), f.score))
        .collect();

    // ── List (a): supporting passages ──
    let mut sources: BTreeMap<String, Source> = BTreeMap::new();
    if p.stages != Stages::Seeds {
        for (uuid, f) in &fact_order {
            for ep in &f.fe.edge.episodes {
                offer(
                    &mut sources,
                    ep,
                    Source {
                        score: f.score,
                        via: Via::Fact {
                            edge_uuid: (*uuid).clone(),
                        },
                        root: f.root,
                        path: f.path.clone(),
                    },
                );
            }
        }
    }
    let mention_entities: Vec<String> = reach
        .iter()
        .filter(|(_, r)| p.stages != Stages::Seeds || r.hop == 0)
        .map(|(u, _)| u.clone())
        .collect();
    if !mention_entities.is_empty() {
        st.db_queries += 1;
        for (entity, ep, _) in conn.entity_mention_origins(&mention_entities)? {
            let r = &reach[&entity];
            offer(
                &mut sources,
                &ep,
                Source {
                    score: r.score,
                    via: Via::Mention {
                        entity_uuid: entity.clone(),
                    },
                    root: r.root,
                    path: r.path.clone(),
                },
            );
        }
    }
    let mut cand: Vec<(&String, &Source)> = sources.iter().collect();
    cand.sort_by(|a, b| by_score_then_key((a.1.score, a.0), (b.1.score, b.0)));
    if cand.len() > PASSAGE_CANDIDATE_CAP {
        cand.truncate(PASSAGE_CANDIDATE_CAP);
        st.truncate("passage_candidates", PASSAGE_CANDIDATE_CAP);
    }
    let cand_uuids: Vec<String> = cand.iter().map(|(u, _)| (*u).clone()).collect();
    let fetched = if cand_uuids.is_empty() {
        vec![]
    } else {
        st.db_queries += 1;
        conn.get_episode_passages(&cand_uuids, true)?
    };
    // Passage rows and their query similarity, for every passage any list may return.
    let mut passages: HashMap<String, (EpisodePassage, f64)> = HashMap::new();
    let mut list_support: Vec<(String, f64)> = Vec::new();
    for ep in fetched {
        ep_info.insert(
            ep.passage.uuid.clone(),
            EpInfo {
                source_description: ep.passage.source_description.clone(),
                origin: ep.origin,
            },
        );
        let live_ok = !p.believed_only || episode_live_at(&ep, t);
        let origin_ok = p.origins.as_ref().is_none_or(|f| f.contains(&ep.origin));
        // Derived / answer content never corroborates (ADR-0752), so it never adds rank here.
        if !(live_ok && origin_ok && ep.origin.is_corroborating()) {
            continue;
        }
        let sim = relevance(emb, &ep.content_embedding);
        let score = sources[&ep.passage.uuid].score * sim;
        list_support.push((ep.passage.uuid.clone(), score));
        passages.insert(ep.passage.uuid.clone(), (ep, sim));
    }
    list_support.sort_by(|a, b| by_score_then_key((a.1, &a.0), (b.1, &b.0)));
    list_support.truncate(list_cap);
    let list_support: Vec<(String, f64)> = list_support
        .into_iter()
        .map(|(u, s)| (format!("p:{u}"), s))
        .collect();

    // ── List (b): plain passages ──
    let mut list_plain: Vec<(String, f64)> = Vec::new();
    if p.stages == Stages::Full {
        st.db_queries += 1;
        let mut hits =
            conn.vector_search_episodic_origins(emb, gids, list_cap * 3, p.origins.as_deref())?;
        for h in &mut hits {
            h.score = 1.0 - h.score;
        }
        hits.retain(|h| h.score.is_finite());
        let uuids: Vec<String> = hits.iter().map(|h| h.uuid.clone()).collect();
        let rows = if uuids.is_empty() {
            vec![]
        } else {
            st.db_queries += 1;
            conn.get_episode_passages(&uuids, false)?
        };
        let by_uuid: HashMap<String, EpisodePassage> = rows
            .into_iter()
            .map(|r| (r.passage.uuid.clone(), r))
            .collect();
        for h in hits {
            let Some(ep) = by_uuid.get(&h.uuid) else {
                continue;
            };
            if p.believed_only && !episode_live_at(ep, t) {
                continue;
            }
            if list_plain.len() == list_cap {
                break;
            }
            ep_info.insert(
                h.uuid.clone(),
                EpInfo {
                    source_description: h.source_description.clone(),
                    origin: ep.origin,
                },
            );
            list_plain.push((format!("p:{}", h.uuid), h.score));
            passages
                .entry(h.uuid.clone())
                .or_insert_with(|| (ep.clone(), h.score));
        }
    }

    // ── Stage 4: fusion ──
    let named: Vec<(&'static str, &Vec<(String, f64)>)> = match p.stages {
        Stages::Seeds => vec![("supporting", &list_support)],
        Stages::Expand => vec![("supporting", &list_support), ("facts", &list_facts)],
        Stages::Full => vec![
            ("supporting", &list_support),
            ("passages", &list_plain),
            ("facts", &list_facts),
        ],
    };
    let lists: Vec<&[(String, f64)]> = named.iter().map(|(_, l)| l.as_slice()).collect();
    let ranks: Vec<HashMap<&str, usize>> = named
        .iter()
        .map(|(_, l)| {
            l.iter()
                .enumerate()
                .map(|(i, (key, _))| (key.as_str(), i + 1))
                .collect()
        })
        .collect();
    let fused: Vec<_> = rrf_fuse_scored(&lists).into_iter().take(k).collect();

    // Support tallies for the returned facts (same shape as `knowledge_find_relationships`).
    let fact_keys: Vec<crate::origin_reads::EdgeKey> = fused
        .iter()
        .filter_map(|h| h.uuid.strip_prefix("f:"))
        .map(|u| {
            let e = &facts[u].fe.edge;
            crate::origin_reads::EdgeKey {
                uuid: e.uuid.clone(),
                group_id: e.group_id.clone(),
                source_uuid: e.source_node_uuid.clone(),
                target_uuid: e.target_node_uuid.clone(),
                name: e.name.clone(),
            }
        })
        .collect();
    let support = if fact_keys.is_empty() {
        HashMap::new()
    } else {
        st.db_queries += 3;
        crate::origin_reads::edge_support(conn, &fact_keys, None, t)?
    };

    // Citation-only edges, by same-fact key (ADR-0752's sibling grouping).
    type FactKey = (String, String, String, String);
    let key_of = |e: &RelatesToEdge| -> FactKey {
        (
            e.group_id.clone(),
            e.source_node_uuid.clone(),
            e.target_node_uuid.clone(),
            e.name.clone(),
        )
    };
    let mut citations: BTreeMap<FactKey, Vec<String>> = BTreeMap::new();
    for f in facts.values().filter(|f| f.citation_only) {
        citations
            .entry(key_of(&f.fe.edge))
            .or_default()
            .extend(f.fe.edge.episodes.iter().cloned());
    }

    let seed_ev = |i: usize| vec![seeds[i].evidence.clone()];
    let supporting = |uuid: &str, corroborating_default: Option<bool>| -> SupportingEpisode {
        let info = ep_info.get(uuid);
        let origin = info.map_or(Origin::Primary, |i| i.origin);
        SupportingEpisode {
            uuid: uuid.to_string(),
            source_description: info.map(|i| i.source_description.clone()).unwrap_or_default(),
            origin: origin.as_str(),
            corroborating: corroborating_default.unwrap_or(origin.is_corroborating()),
        }
    };

    let mut results = Vec::with_capacity(fused.len());
    for hit in fused {
        let lists_ev: Vec<ListEvidence> = named
            .iter()
            .enumerate()
            .filter_map(|(i, (name, _))| {
                hit.raw[i].map(|score| ListEvidence {
                    list: name,
                    rank: ranks[i][hit.uuid.as_str()],
                    score,
                })
            })
            .collect();
        if let Some(uuid) = hit.uuid.strip_prefix("f:") {
            let f = &facts[uuid];
            let mut eps: Vec<SupportingEpisode> = f
                .fe
                .edge
                .episodes
                .iter()
                .map(|u| supporting(u, None))
                .collect();
            if let Some(cites) = citations.get(&key_of(&f.fe.edge)) {
                eps.extend(cites.iter().map(|u| supporting(u, Some(false))));
            }
            let sup = support.get(uuid);
            results.push(GraphSearchResult {
                kind: "fact",
                score: hit.rrf_score,
                fact: Some(f.fe.edge.clone()),
                passage: None,
                belief: serde_json::to_value(&f.belief).unwrap_or_default(),
                support: sup.map(|s| s.support.clone()),
                corroboration: sup.map(|s| s.corroboration),
                evidence: GraphEvidence {
                    lists: lists_ev,
                    seeds: seed_ev(f.root),
                    path: f.path.clone(),
                    supporting_episodes: eps,
                    via: None,
                    warrant: f.fe.edge.warrant.clone(),
                    alternate_paths: f.alternates,
                },
            });
        } else if let Some(uuid) = hit.uuid.strip_prefix("p:") {
            let (ep, sim) = &passages[uuid];
            let mut passage = ep.passage.clone();
            passage.score = *sim;
            let in_support = hit.raw[0].is_some();
            let src = in_support.then(|| &sources[uuid]);
            results.push(GraphSearchResult {
                kind: "passage",
                score: hit.rrf_score,
                passage: Some(passage),
                fact: None,
                belief: serde_json::to_value(passage_belief(ep, t)).unwrap_or_default(),
                support: None,
                corroboration: None,
                evidence: GraphEvidence {
                    lists: lists_ev,
                    seeds: src.map(|s| seed_ev(s.root)).unwrap_or_default(),
                    path: src.map(|s| s.path.clone()).unwrap_or_default(),
                    supporting_episodes: if in_support {
                        vec![supporting(uuid, None)]
                    } else {
                        vec![]
                    },
                    via: src.map(|s| s.via.clone()),
                    warrant: None,
                    alternate_paths: 0,
                },
            });
        }
    }

    let count = results.len();
    Ok(GraphSearchResponse {
        results,
        count,
        seeds: seeds.iter().map(|s| s.evidence.clone()).collect(),
        stats: st,
    })
}

/// Stage 1: entity linking.
fn seed_stage(
    conn: &Conn<'_>,
    p: &GraphSearchParams,
    emb: &[f32],
    gids: Option<&[&str]>,
    fts_q: &str,
    st: &mut GraphSearchStats,
) -> Result<Vec<Seed>, Error> {
    let bm25 = if fts_q.is_empty() {
        vec![]
    } else {
        st.db_queries += 1;
        conn.fts_search_entities_kind(fts_q, gids, None, SEED_CANDIDATES)?
    };
    // Vector lists keep only finite similarities: a zero-filled (un-backfilled) summary
    // embedding yields NaN and so contributes nothing.
    let finite_only = |l: Vec<(String, f64)>| -> Vec<(String, f64)> {
        l.into_iter().filter(|(_, d)| (1.0 - d).is_finite()).collect()
    };
    st.db_queries += 2;
    let name_vec = finite_only(conn.vector_search_entities_kind(emb, gids, None, SEED_CANDIDATES)?);
    let sum_vec = finite_only(conn.vector_search_entities_by_summary_kind(
        emb,
        gids,
        None,
        SEED_CANDIDATES,
    )?);

    // Rows of the ranked candidates (names, groups, tombstone labels).
    let mut cand: Vec<String> = bm25
        .iter()
        .chain(&name_vec)
        .chain(&sum_vec)
        .map(|(u, _)| u.clone())
        .collect();
    cand.sort();
    cand.dedup();
    let mut rows: HashMap<String, EntityRow> = HashMap::new();
    if !cand.is_empty() {
        st.db_queries += 1;
        for r in conn.get_entities_by_uuids(&cand)? {
            rows.insert(r.uuid.clone(), r);
        }
    }

    // Exact names: every query n-gram × group × known kind, one batched lookup_key probe.
    let groups: Vec<String> = match gids {
        Some(g) => g.iter().map(|s| s.to_string()).collect(),
        None => {
            let mut g: Vec<String> = rows.values().map(|r| r.group_id.clone()).collect();
            g.sort();
            g.dedup();
            g
        }
    };
    let mut kinds = conn.known_kinds();
    kinds.sort();
    let ngrams = query_ngrams(&p.query);
    let mut keys: Vec<String> = Vec::new();
    for g in &groups {
        for kind in &kinds {
            for n in &ngrams {
                keys.push(compute_lookup_key(g, kind, n));
            }
        }
    }
    let mut exact: Vec<(String, f64)> = Vec::new();
    if !keys.is_empty() {
        st.db_queries += 1;
        let mut hits = conn.entities_by_lookup_keys(&keys)?;
        // Longer names are more specific matches: rank by word count, then uuid.
        hits.sort_by(|a, b| {
            let wa = a.name.split_whitespace().count();
            let wb = b.name.split_whitespace().count();
            wb.cmp(&wa).then_with(|| a.uuid.cmp(&b.uuid))
        });
        for r in hits {
            exact.push((r.uuid.clone(), r.name.split_whitespace().count() as f64));
            rows.entry(r.uuid.clone()).or_insert(r);
        }
    }

    let fused = rrf_fuse_scored(&[&exact, &bm25, &name_vec, &sum_vec]);
    st.seed_candidates = fused.len();
    let bm25_rank: HashMap<&str, usize> = bm25
        .iter()
        .enumerate()
        .map(|(i, (u, _))| (u.as_str(), i))
        .collect();
    let sim = |d: Option<f64>| d.map(|d| 1.0 - d).filter(|s| s.is_finite());
    let best_vec = |raw: &[Option<f64>]| -> Option<f64> {
        [sim(raw[2]), sim(raw[3])]
            .into_iter()
            .flatten()
            .fold(None, |m: Option<f64>, s| Some(m.map_or(s, |m| m.max(s))))
    };
    let global_best = fused
        .iter()
        .filter_map(|h| best_vec(&h.raw))
        .fold(f64::NEG_INFINITY, f64::max);

    let mut admitted = Vec::new();
    for h in fused {
        let Some(row) = rows.get(&h.uuid) else {
            continue;
        };
        if row.labels.iter().any(|l| l == "Merged") {
            continue;
        }
        let exact_hit = h.raw[0].is_some();
        let lexical = bm25_rank
            .get(h.uuid.as_str())
            .is_some_and(|r| *r < SEED_BM25_RANK);
        let vector = best_vec(&h.raw)
            .is_some_and(|s| s >= SEED_SIM_FLOOR && s >= global_best - SEED_SIM_MARGIN);
        if exact_hit || lexical || vector {
            admitted.push((h, row.clone()));
        }
    }

    // `origins`: a seed needs a matching live mention (an asserted entity counts as primary).
    if let Some(filter) = p.origins.as_deref() {
        let uuids: Vec<String> = admitted.iter().map(|(h, _)| h.uuid.clone()).collect();
        if !uuids.is_empty() {
            st.db_queries += 1;
            let sup = crate::origin_reads::entity_support(conn, &uuids, Some(filter), p.as_of_us)?;
            admitted.retain(|(h, _)| sup.get(&h.uuid).is_some_and(|s| s.matches));
        }
    }
    admitted.truncate(SEED_CAP);

    let top = admitted.first().map_or(1.0, |(h, _)| h.rrf_score);
    Ok(admitted
        .into_iter()
        .map(|(h, row)| {
            let mut matched = Vec::new();
            if h.raw[0].is_some() {
                matched.push("name");
            }
            if h.raw[1].is_some() {
                matched.push("bm25");
            }
            if h.raw[2].is_some() {
                matched.push("name_vector");
            }
            if h.raw[3].is_some() {
                matched.push("summary_vector");
            }
            let evidence = SeedEvidence {
                uuid: row.uuid.clone(),
                name: row.name.clone(),
                group_id: row.group_id.clone(),
                kind: row.kind.clone(),
                score: h.rrf_score / top,
                matched,
                bm25_score: h.raw[1].filter(|v| v.is_finite()),
                name_similarity: sim(h.raw[2]),
                summary_similarity: sim(h.raw[3]),
            };
            Seed { row, evidence }
        })
        .collect())
}

/// Stage 2: neighbourhood expansion with belief and origin applied at every step.
#[allow(clippy::too_many_arguments)]
fn expand(
    conn: &Conn<'_>,
    p: &GraphSearchParams,
    emb: &[f32],
    gids: Option<&[&str]>,
    fts_q: &str,
    seeds: &[Seed],
    names: &mut HashMap<String, String>,
    reach: &mut BTreeMap<String, Reach>,
    facts: &mut BTreeMap<String, FactHit>,
    ep_info: &mut HashMap<String, EpInfo>,
    st: &mut GraphSearchStats,
) -> Result<(), Error> {
    let t = p.as_of_us;
    let edge_bm25: HashMap<String, usize> = if fts_q.is_empty() {
        HashMap::new()
    } else {
        st.db_queries += 1;
        conn.fts_search_edges(fts_q, gids, EDGE_BM25_LIMIT)?
            .into_iter()
            .enumerate()
            .map(|(i, (u, _))| (u, i))
            .collect()
    };

    let mut frontier: Vec<String> = seeds.iter().map(|s| s.row.uuid.clone()).collect();
    frontier.sort();
    for hop in 1..=MAX_HOPS {
        if frontier.is_empty() {
            break;
        }
        st.hops = hop;
        st.db_queries += 2;
        let (mut rows, truncated) = conn.expand_frontier(&frontier, gids, FRONTIER_ROW_CAP)?;
        if truncated {
            st.truncate("frontier_rows", FRONTIER_ROW_CAP);
        }
        rows.retain(|r| !r.far_merged);
        // An edge walked on an earlier hop is never revisited; reaching it again is an
        // alternate path.
        rows.retain(|r| match facts.get_mut(&r.edge.uuid) {
            Some(f) => {
                f.alternates += 1;
                false
            }
            None => true,
        });
        if rows.is_empty() {
            break;
        }

        let mut edge_uuids: Vec<String> = rows.iter().map(|r| r.edge.uuid.clone()).collect();
        edge_uuids.sort();
        edge_uuids.dedup();
        st.db_queries += 3;
        let ctx = belief::load_context(conn, &edge_uuids)?;
        let mut ep_uuids: Vec<String> = rows
            .iter()
            .flat_map(|r| r.edge.episodes.iter().cloned())
            .collect();
        ep_uuids.sort();
        ep_uuids.dedup();
        let eps: HashMap<String, EpisodePassage> = if ep_uuids.is_empty() {
            HashMap::new()
        } else {
            st.db_queries += 1;
            conn.get_episode_passages(&ep_uuids, false)?
                .into_iter()
                .map(|e| (e.passage.uuid.clone(), e))
                .collect()
        };
        for e in eps.values() {
            ep_info.insert(
                e.passage.uuid.clone(),
                EpInfo {
                    source_description: e.passage.source_description.clone(),
                    origin: e.origin,
                },
            );
        }

        // `origins`: the far endpoint must have a matching live mention, so an entity known
        // only from derived pages is never traversed through.
        let far_ok: Option<HashSet<String>> = match p.origins.as_deref() {
            None => None,
            Some(filter) => {
                let mut far: Vec<String> = rows.iter().map(|r| r.far_uuid.clone()).collect();
                far.sort();
                far.dedup();
                st.db_queries += 1;
                let sup = crate::origin_reads::entity_support(conn, &far, Some(filter), t)?;
                Some(
                    sup.into_iter()
                        .filter(|(_, s)| s.matches)
                        .map(|(u, _)| u)
                        .collect(),
                )
            }
        };

        // Score and filter each candidate step, grouped by the entity it was reached from.
        struct Step {
            fe: FrontierEdge,
            rel: f64,
            belief: BeliefAnnotation,
            citation_only: bool,
        }
        let mut by_from: BTreeMap<String, Vec<Step>> = BTreeMap::new();
        for fe in rows {
            let annotation = ctx.annotate(&ctx.row(&fe.edge.uuid), t);
            if p.believed_only && !annotation.believed {
                continue;
            }
            let own: Vec<&EpisodePassage> = fe
                .edge
                .episodes
                .iter()
                .filter_map(|u| eps.get(u))
                .collect();
            if let Some(filter) = p.origins.as_deref() {
                let ok = if fe.edge.episodes.is_empty() {
                    filter.contains(&Origin::Primary)
                } else {
                    own.iter().any(|e| {
                        filter.contains(&e.origin) && (!p.believed_only || episode_live_at(e, t))
                    })
                };
                if !ok {
                    continue;
                }
            }
            if far_ok.as_ref().is_some_and(|s| !s.contains(&fe.far_uuid)) {
                continue;
            }
            let citation_only = !own.is_empty() && own.iter().all(|e| !e.origin.is_corroborating());
            let rel = edge_relevance(
                relevance(emb, &fe.edge.fact_embedding),
                edge_bm25.get(&fe.edge.uuid).copied(),
            );
            by_from.entry(fe.from_uuid.clone()).or_default().push(Step {
                fe,
                rel,
                belief: annotation,
                citation_only,
            });
        }

        let visited: HashSet<String> = reach.keys().cloned().collect();
        let mut hop_edges: Vec<(f64, String, String)> = Vec::new(); // (score, edge, far)
        for (from, mut steps) in by_from {
            steps.sort_by(|a, b| {
                by_score_then_key((a.rel, &a.fe.edge.uuid), (b.rel, &b.fe.edge.uuid))
            });
            if steps.len() > PER_ENTITY_FANOUT {
                steps.truncate(PER_ENTITY_FANOUT);
                st.truncate("per_entity_fanout", PER_ENTITY_FANOUT);
            }
            let Some(origin) = reach.get(&from).cloned() else {
                continue;
            };
            let from_name = names.get(&from).cloned().unwrap_or_default();
            for s in steps {
                let score = path_score(seeds[origin.root].evidence.score, hop, s.rel);
                let mut path = origin.path.clone();
                path.push(PathStep {
                    edge_uuid: s.fe.edge.uuid.clone(),
                    relation: s.fe.edge.name.clone(),
                    fact: s.fe.edge.fact.clone(),
                    from_uuid: from.clone(),
                    from_name: from_name.clone(),
                    to_uuid: s.fe.far_uuid.clone(),
                    to_name: s.fe.far_name.clone(),
                    hop,
                });
                names
                    .entry(s.fe.far_uuid.clone())
                    .or_insert_with(|| s.fe.far_name.clone());
                hop_edges.push((score, s.fe.edge.uuid.clone(), s.fe.far_uuid.clone()));

                // The reached entity keeps its best same-hop path; earlier hops win outright.
                if !visited.contains(&s.fe.far_uuid) {
                    let better = reach.get(&s.fe.far_uuid).is_none_or(|r| score > r.score);
                    if better {
                        reach.insert(
                            s.fe.far_uuid.clone(),
                            Reach {
                                root: origin.root,
                                score,
                                hop,
                                path: path.clone(),
                            },
                        );
                    }
                }

                match facts.get_mut(&s.fe.edge.uuid) {
                    // Reached from both endpoints in one hop: keep the better path.
                    Some(f) => {
                        f.alternates += 1;
                        if score > f.score {
                            f.score = score;
                            f.root = origin.root;
                            f.path = path;
                        }
                    }
                    None => {
                        facts.insert(
                            s.fe.edge.uuid.clone(),
                            FactHit {
                                fe: s.fe,
                                score,
                                root: origin.root,
                                path,
                                alternates: 0,
                                belief: s.belief,
                                citation_only: s.citation_only,
                            },
                        );
                    }
                }
            }
        }

        // Beam: far endpoints of the best edges of this hop, newly reached, seed the next hop.
        hop_edges.sort_by(|a, b| by_score_then_key((a.0, &a.1), (b.0, &b.1)));
        let mut next: Vec<String> = Vec::new();
        for (_, _, far) in hop_edges.iter().take(BEAM) {
            if !visited.contains(far) && !next.contains(far) {
                next.push(far.clone());
            }
        }
        next.sort();
        frontier = next;
    }

    if facts.len() > TOTAL_EDGE_CAP {
        let mut order: Vec<(f64, String)> =
            facts.iter().map(|(u, f)| (f.score, u.clone())).collect();
        order.sort_by(|a, b| by_score_then_key((a.0, &a.1), (b.0, &b.1)));
        let keep: HashSet<String> = order
            .into_iter()
            .take(TOTAL_EDGE_CAP)
            .map(|(_, u)| u)
            .collect();
        facts.retain(|u, _| keep.contains(u));
        st.truncate("total_edges", TOTAL_EDGE_CAP);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ngrams_strip_punctuation_and_possessives() {
        let grams = query_ngrams("what does Cedar Core's supplier charge?");
        assert!(grams.contains(&"cedar core".to_string()), "{grams:?}");
        assert!(grams.contains(&"cedar".to_string()));
        assert!(grams.contains(&"charge".to_string()));
        assert!(grams.contains(&"does cedar core supplier".to_string()));
        assert!(!grams.iter().any(|g| g.contains('?') || g.contains('\'')));
        // Shortest first.
        assert_eq!(grams[0], "what");
        assert!(grams.iter().all(|g| g.split(' ').count() <= MAX_NGRAM_WORDS));
    }

    #[test]
    fn ngrams_dedupe_and_cap() {
        assert_eq!(query_ngrams("a a a"), vec!["a", "a a", "a a a"]);
        let long: String = (0..100).map(|i| format!("w{i} ")).collect();
        assert_eq!(query_ngrams(&long).len(), MAX_NGRAMS);
        assert!(query_ngrams("  ?! ").is_empty());
        assert_eq!(query_ngrams("Fable\u{2019}s price"), vec!["fable", "price", "fable price"]);
    }

    #[test]
    fn fts_terms_keep_only_alphanumeric_words() {
        assert_eq!(fts_terms("Cedar Core → Fable's $87/unit"), "Cedar Core Fable s 87 unit");
        assert_eq!(fts_terms("→ ?"), "");
    }

    #[test]
    fn hop_decay_orders_equal_relevance_by_distance() {
        let one = path_score(1.0, 1, 0.8);
        let two = path_score(1.0, 2, 0.8);
        assert!(one > two);
        assert_eq!(one, 0.8);
        assert!((two - 0.8 * HOP_DECAY).abs() < 1e-12);
        assert_eq!(path_score(0.5, 1, 0.8), 0.4);
    }

    #[test]
    fn edge_relevance_adds_a_rank_decaying_bm25_bonus() {
        assert_eq!(edge_relevance(0.5, None), 0.5);
        assert!((edge_relevance(0.5, Some(0)) - (0.5 + EDGE_BM25_BONUS)).abs() < 1e-12);
        assert!(edge_relevance(0.5, Some(10)) < edge_relevance(0.5, Some(0)));
        assert_eq!(edge_relevance(0.5, Some(EDGE_BM25_LIMIT)), 0.5);
    }

    #[test]
    fn relevance_is_clamped_and_finite() {
        assert_eq!(relevance(&[1.0, 0.0], &[-1.0, 0.0]), 0.0);
        assert_eq!(relevance(&[1.0, 0.0], &[]), 0.0);
        assert_eq!(relevance(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert!((relevance(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
    }

    fn ep(superseded: Option<i64>, ingested: Option<i64>, pending: bool) -> EpisodePassage {
        EpisodePassage {
            passage: PassageResult {
                uuid: "e".into(),
                name: String::new(),
                content: String::new(),
                source_description: String::new(),
                group_id: "g".into(),
                created_at: String::new(),
                valid_at: None,
                score: 0.0,
                attributes: String::new(),
                origin: "primary".into(),
            },
            origin: Origin::Primary,
            supersession: belief::EpisodeSupersession {
                superseded_at_us: superseded,
                revision_id_present: pending,
                ..Default::default()
            },
            ingested_at_us: ingested,
            content_embedding: vec![],
        }
    }

    #[test]
    fn episode_liveness_honours_supersession_ingest_and_pending_revisions() {
        assert!(episode_live_at(&ep(None, Some(10), false), 10));
        assert!(!episode_live_at(&ep(None, Some(10), false), 9));
        assert!(episode_live_at(&ep(Some(20), Some(10), false), 19));
        assert!(!episode_live_at(&ep(Some(20), Some(10), false), 20));
        assert!(!episode_live_at(&ep(None, Some(10), true), 100));
        assert_eq!(passage_belief(&ep(Some(20), Some(10), false), 25).state, "superseded");
        assert_eq!(passage_belief(&ep(None, Some(10), false), 5).state, "not_yet_ingested");
        let b = passage_belief(&ep(Some(20), Some(10), false), 15);
        assert_eq!(b.state, "believed");
        assert!(b.believed);
        assert!(b.supersession.is_some());
    }

    #[test]
    fn offer_keeps_the_max_source_and_breaks_ties_deterministically() {
        let mut m = BTreeMap::new();
        let src = |score: f64, via: Via| Source {
            score,
            via,
            root: 0,
            path: vec![],
        };
        offer(&mut m, "p", src(0.2, Via::Mention { entity_uuid: "z".into() }));
        offer(&mut m, "p", src(0.5, Via::Mention { entity_uuid: "y".into() }));
        offer(&mut m, "p", src(0.1, Via::Fact { edge_uuid: "a".into() }));
        assert_eq!(m["p"].score, 0.5);
        offer(&mut m, "p", src(0.5, Via::Fact { edge_uuid: "b".into() }));
        assert!(matches!(&m["p"].via, Via::Fact { edge_uuid } if edge_uuid == "b"));
        offer(&mut m, "p", src(0.5, Via::Fact { edge_uuid: "c".into() }));
        assert!(matches!(&m["p"].via, Via::Fact { edge_uuid } if edge_uuid == "b"));
    }

    #[test]
    fn stages_parse_round_trips() {
        for s in ["seeds", "expand", "full"] {
            assert_eq!(Stages::parse(s).unwrap().as_str(), s);
        }
        assert!(Stages::parse("all").is_err());
    }
}
