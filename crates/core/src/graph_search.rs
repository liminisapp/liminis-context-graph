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
use std::time::Instant;

use serde::Serialize;

use crate::belief::{self, BeliefAnnotation, SupersessionCause, SupersessionInfo};
use crate::db::{compute_lookup_key, Conn, Db, EpisodePassage, FrontierEdge};
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
/// Most `lookup_key`s one exact-name probe sends: n-grams × (group, kind) pairs are cut to fit.
pub const MAX_EXACT_KEYS: usize = 2048;
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
/// Most mention episodes fetched for one entity. A hub mentioned more often has its mentions
/// fetched ranked by query relevance and filtered in the query, so no request loads them all.
pub const MENTIONS_PER_ENTITY: usize = 50;
/// Most hub entities whose mentions are fetched (one ranked query each), best score first.
pub const MAX_HUB_MENTION_QUERIES: usize = 10;
/// Extra seeds taken from the top of the summary-vector list on relative evidence (see the seed
/// stage), beyond [`SEED_CAP`]. Tuned on v2 (#797 steer, 2026-11-19).
pub const SUMMARY_SEEDS: usize = 3;
/// How far below the best summary similarity a summary-vector seed may be.
pub const SUMMARY_SEED_MARGIN: f64 = 0.03;
/// Exponent on a supporting passage's source (path or seed) score in list (a):
/// `source_score^SUPPORT_SOURCE_EXP × similarity`. At 1 a weak seed's notes could not outrank a
/// strong seed's, so a paraphrase target seeded only by its summary (score ≈ 0.3) lost its own,
/// most query-similar note to a generic seed's digest mentions; at 0.25 the passage's own
/// similarity decides, and the path only tilts it. Tuned on v2.
pub const SUPPORT_SOURCE_EXP: f64 = 0.25;

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
    /// Report every seed candidate and why it was admitted or rejected (`seed_trace`).
    pub explain: bool,
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
    /// Which lists matched it: `name` (exact), `bm25`, `name_vector`, `summary_vector` (a vector
    /// list counts only at or above [`SEED_SIM_FLOOR`]).
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
    /// `believed`, `superseded`, `not_yet_ingested` (ingested after `as_of`) or
    /// `pending_revision` (ingested by `as_of`, but its document revision had not committed by
    /// then, so it is not yet evidence). The first three are the edge vocabulary. The fourth is
    /// passage-only: an edge reports the same pending case as `not_yet_ingested`
    /// ([`belief::BeliefState::NotYetIngested`], #785), because an edge has no revision of its own.
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

/// One seed candidate and the admission decision on it (`explain: true`). Ranks are 1-based
/// positions in each candidate list; a list that did not return the entity is omitted.
#[derive(Debug, Clone, Serialize)]
pub struct SeedCandidate {
    pub uuid: String,
    pub name: String,
    pub kind: String,
    /// Fused RRF score over the four candidate lists.
    pub rrf_score: f64,
    pub exact_name: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25_rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_vector_rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_vector_rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_similarity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_similarity: Option<f64>,
    /// Admitted as a seed (within the cap and every filter).
    pub seeded: bool,
    /// Why: `exact_name`, `bm25_rank`, `vector` or `summary_vector` when admitted;
    /// `no_evidence`, `merged`, `no_row`, `origin_filter` or `seed_cap` when not.
    pub reason: &'static str,
}

/// Wall-clock milliseconds per stage of one request. Not part of the ranking: two identical
/// requests return identical results but, in general, different timings.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StageTimings {
    pub embed: f64,
    pub seeds: f64,
    pub expand: f64,
    pub supporting: f64,
    pub passages: f64,
    pub fusion: f64,
    pub total: f64,
}

fn ms(d: std::time::Duration) -> f64 {
    (d.as_secs_f64() * 100_000.0).round() / 100.0
}

/// Milliseconds since `clock`, restarting it.
fn lap(clock: &mut Instant) -> f64 {
    let d = ms(clock.elapsed());
    *clock = Instant::now();
    d
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
    /// `vector_belief`: list (b) is `knowledge_search_passages` with `mode: "vector"` and
    /// `belief_aware: true` (#789), the best passage list measured on v2, with belief evaluated at
    /// `as_of`.
    pub passage_mode: &'static str,
    /// The plain-passage lists that ran (stage `full` only), in fusion order: `passages` (vector,
    /// query embedded with the profile's query instruction), `passages_raw` (vector, bare query;
    /// only when the profile has a query instruction) and `passages_lexical` (BM25).
    pub passage_lists: Vec<&'static str>,
    /// Query embeddings computed: 2 when the profile has a query instruction (the bare and the
    /// instructed query, from one `embed_batch`), 1 otherwise, 0 when nothing was embedded.
    /// `embed_calls` counts model calls and stays at 1 either way.
    pub query_vectors: usize,
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
    pub timings_ms: StageTimings,
}

/// The `knowledge_graph_search` response (ADR-0020 envelope plus `seeds` and `stats`).
#[derive(Debug, Clone, Serialize)]
pub struct GraphSearchResponse {
    pub results: Vec<GraphSearchResult>,
    pub count: usize,
    pub seeds: Vec<SeedEvidence>,
    pub stats: GraphSearchStats,
    /// Every seed candidate and its admission decision; present only with `explain: true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_trace: Option<Vec<SeedCandidate>>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Runs the graph-aware search. One embed call (see [`embed_query`]); every lookup on one
/// connection in one blocking task, holding no lock (the search-read convention).
pub async fn graph_search(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    params: GraphSearchParams,
) -> Result<GraphSearchResponse, Error> {
    if matches!(&params.group_ids, Some(g) if g.is_empty()) {
        // `group_ids: []` matches no group (#413): nothing to embed or look up.
        return Ok(empty_response(&params, 0));
    }
    let t0 = Instant::now();
    let (embedding, raw) = embed_query(embedder.as_ref(), &params.query).await?;
    let embed = t0.elapsed();
    let mut r = tokio::task::spawn_blocking(move || -> Result<GraphSearchResponse, Error> {
        let conn = db.connect()?;
        run(&conn, &params, &embedding, raw.as_deref())
    })
    .await??;
    r.stats.timings_ms.embed = ms(embed);
    r.stats.timings_ms.total = ms(t0.elapsed());
    Ok(r)
}

/// Embeds the query for every vector leg in one model call, returning `(instructed, bare)`.
///
/// With a query instruction in effect, one [`Embedder::embed_batch_plain`] of
/// `[query, prefix + query]` returns both vectors (#797): the instructed one feeds every graph
/// stage and list (b)'s `passages`, the bare one only list (b)'s `passages_raw`. The instruction
/// helps paraphrase and hurts named-entity lookups on v2, so the read keeps both. The instructed
/// text is exactly what `embed(query, Query)` would send, so the instructed vector, and every
/// stage built on it, is unchanged. Without an instruction there is one vector and one `embed`.
pub async fn embed_query(
    embedder: &dyn Embedder,
    query: &str,
) -> Result<(Vec<f32>, Option<Vec<f32>>), Error> {
    let prefix = embedder.instructions().query_prefix;
    if prefix.is_empty() || query.trim().is_empty() {
        return Ok((embedder.embed(query, EmbedRole::Query).await?, None));
    }
    let instructed = format!("{prefix}{query}");
    let texts = [query, instructed.as_str()];
    let mut v = embedder.embed_batch_plain(&texts, EmbedRole::Query).await?;
    if v.len() != 2 {
        return Err(Error::Ipc(format!(
            "embedder returned {} vectors for 2 query texts",
            v.len()
        )));
    }
    let instructed = v.pop().unwrap_or_default();
    let bare = v.pop().unwrap_or_default();
    // An embedder that ignores the instruction gives one vector twice; a second list would only
    // double-count it in the fusion.
    Ok(if bare == instructed {
        (instructed, None)
    } else {
        (instructed, Some(bare))
    })
}

fn empty_response(p: &GraphSearchParams, embed_calls: usize) -> GraphSearchResponse {
    GraphSearchResponse {
        results: vec![],
        count: 0,
        seeds: vec![],
        stats: new_stats(p, embed_calls),
        seed_trace: p.explain.then(Vec::new),
    }
}

fn new_stats(p: &GraphSearchParams, embed_calls: usize) -> GraphSearchStats {
    GraphSearchStats {
        stages: p.stages.as_str(),
        passage_mode: "vector_belief",
        passage_lists: vec![],
        query_vectors: embed_calls,
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
        timings_ms: StageTimings::default(),
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
/// stripped from each word — the exact-name candidates. Each n-gram is emitted with a trailing
/// possessive `'s` stripped from its words ("Cedar Core's supplier" probes "cedar core") and,
/// when that differs, also as written, so a name that itself contains a possessive ("Alice's
/// Restaurant", "Women's Health") is still matched exactly. Shortest first, deduplicated, at most
/// [`MAX_NGRAMS`].
pub fn query_ngrams(query: &str) -> Vec<String> {
    let is_punct = |c: char| !c.is_alphanumeric();
    // (possessive stripped, as written) per word.
    let tokens: Vec<(String, String)> = query
        .split_whitespace()
        .filter_map(|t| {
            let raw = t.trim_matches(is_punct);
            let stripped = raw
                .strip_suffix("'s")
                .or_else(|| raw.strip_suffix("\u{2019}s"))
                .unwrap_or(raw)
                .trim_matches(is_punct);
            (!stripped.is_empty()).then(|| (stripped.to_lowercase(), raw.to_lowercase()))
        })
        .collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for n in 1..=MAX_NGRAM_WORDS {
        for w in tokens.windows(n) {
            let stripped = w
                .iter()
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let raw = w
                .iter()
                .map(|(_, r)| r.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            for gram in [stripped, raw] {
                if seen.insert(gram.clone()) {
                    out.push(gram);
                    if out.len() == MAX_NGRAMS {
                        return out;
                    }
                }
            }
        }
    }
    out
}

/// The exact-name `lookup_key`s to probe, at most `cap`, and whether the cap cut any. N-grams
/// are taken in [`query_ngrams`] order (shortest first) and each is probed in every `(group,
/// kind)` pair before the next is added, so a cut drops the longest n-grams everywhere rather
/// than whole groups — every group keeps the same chance of an exact match.
pub fn exact_name_keys(
    pairs: &[(String, String)],
    ngrams: &[String],
    cap: usize,
) -> (Vec<String>, bool) {
    let mut keys = Vec::new();
    if pairs.is_empty() {
        return (keys, false);
    }
    for n in ngrams {
        if keys.len() + pairs.len() > cap {
            return (keys, true);
        }
        keys.extend(pairs.iter().map(|(g, k)| compute_lookup_key(g, k, n)));
    }
    (keys, false)
}

/// The query as plain alphanumeric terms for `QUERY_FTS_INDEX`, so punctuation (e.g. `→`, #646)
/// can never break the FTS parser. #789's `sanitize_fts_query` now exists; moving to it (which also
/// drops stop words) is a measured follow-up, not part of #797.
pub fn fts_terms(query: &str) -> String {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A fact's relevance: cosine of its embedding plus a bonus for its rank in the global fact
/// BM25 list (`None` when it is not in that list).
pub fn edge_relevance(cosine: f64, bm25_rank: Option<usize>) -> f64 {
    let bonus = bm25_rank.filter(|r| *r < EDGE_BM25_LIMIT).map_or(0.0, |r| {
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

/// The facts kept when the walk exceeds `cap`: best score first (key tie-break), but a fact is
/// kept only together with every edge on its path, so no kept fact's evidence names an edge the
/// response dropped. Hop decay can score a weakly worded hop-1 bridge below the hop-2 facts it
/// leads to; cutting by score alone would keep those facts and drop their bridge. A fact whose
/// path no longer fits is skipped, and a later, shorter one may still fit. Each candidate is
/// `(score, edge uuid, path edge uuids)`; the path ends with the edge itself.
pub fn path_closed_keep(
    mut candidates: Vec<(f64, String, Vec<String>)>,
    cap: usize,
) -> HashSet<String> {
    candidates.sort_by(|a, b| by_score_then_key((a.0, &a.1), (b.0, &b.1)));
    let mut keep: HashSet<String> = HashSet::new();
    for (_, uuid, path) in &candidates {
        if keep.contains(uuid) {
            continue;
        }
        let mut need: Vec<&String> = path
            .iter()
            .chain(std::iter::once(uuid))
            .filter(|e| !keep.contains(*e))
            .collect();
        need.sort();
        need.dedup();
        if keep.len() + need.len() <= cap {
            keep.extend(need.into_iter().cloned());
        }
    }
    keep
}

/// An episode live at `t_us`: visible (a committed revision, if any), not superseded at `t_us`,
/// and ingested by then. The one liveness definition this read uses for supporting and plain
/// passages (ADR-0797).
pub fn episode_live_at(ep: &EpisodePassage, t_us: i64) -> bool {
    ep.supersession.live_at(t_us) && ep.ingested_at_us.is_none_or(|i| i <= t_us)
}

fn passage_belief(ep: &EpisodePassage, t_us: i64) -> PassageBelief {
    let s = &ep.supersession;
    let state = if ep.ingested_at_us.is_some_and(|i| i > t_us) {
        "not_yet_ingested"
    } else if !s.visible_at(t_us) {
        // Ingested, but its document revision had not committed by `t`: not yet evidence.
        "pending_revision"
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
    /// Whether the episode may enter list (a) ([`supports_list_a`]), known exactly because the
    /// full row was fetched.
    admits_support: bool,
}

/// Whether an episode may enter list (a), the supporting passages: live at `t` when filtering,
/// within `origins`, and corroborating (derived / answer content never adds rank, ADR-0752).
fn supports_list_a(ep: &EpisodePassage, p: &GraphSearchParams, t: i64) -> bool {
    let live_ok = !p.believed_only || episode_live_at(ep, t);
    let origin_ok = p.origins.as_ref().is_none_or(|f| f.contains(&ep.origin));
    live_ok && origin_ok && ep.origin.is_corroborating()
}

/// [`supports_list_a`]'s necessary conditions, checkable from a mention row's origin info alone:
/// origin and supersession, but not a pending revision or `ingested_at`. Used to drop mention
/// episodes that can never enter list (a) **before** the candidate cap, so they cannot crowd out
/// passages that would. Never rejects an episode the exact check would keep.
fn may_support_list_a(
    info: &crate::origin::EpisodeOriginInfo,
    p: &GraphSearchParams,
    t: i64,
) -> bool {
    let live_ok = !p.believed_only || info.is_live(t);
    let origin_ok = p.origins.as_ref().is_none_or(|f| f.contains(&info.origin));
    live_ok && origin_ok && info.origin.is_corroborating()
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
    raw_emb: Option<&[f32]>,
) -> Result<GraphSearchResponse, Error> {
    let mut st = new_stats(p, 1);
    if raw_emb.is_some() {
        st.query_vectors = 2;
    }
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
    let mut clock = Instant::now();
    let (seeds, seed_trace) = seed_stage(conn, p, emb, gids, &fts_q, &mut st)?;
    st.timings_ms.seeds = lap(&mut clock);
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
    st.timings_ms.expand = lap(&mut clock);

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
        for (entity, ep, info) in mention_rows(conn, p, emb, &reach, &mention_entities, &mut st)? {
            if !may_support_list_a(&info, p, t) {
                continue;
            }
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
    // Fact-produced episodes were fetched during expansion, so their admission is known exactly:
    // drop the ones that cannot enter list (a) before the cap, not after it.
    let mut cand: Vec<(&String, &Source)> = sources
        .iter()
        .filter(|(ep, _)| ep_info.get(*ep).is_none_or(|i| i.admits_support))
        .collect();
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
        conn.get_episode_passages_scored(&cand_uuids, emb)?
    };
    // Passage rows and their query similarity, for every passage any list may return.
    let mut passages: HashMap<String, (EpisodePassage, f64)> = HashMap::new();
    let mut list_support: Vec<(String, f64)> = Vec::new();
    for ep in fetched {
        let admits_support = supports_list_a(&ep, p, t);
        ep_info.insert(
            ep.passage.uuid.clone(),
            EpInfo {
                source_description: ep.passage.source_description.clone(),
                origin: ep.origin,
                admits_support,
            },
        );
        // Derived / answer content never corroborates (ADR-0752), so it never adds rank here.
        if !admits_support {
            continue;
        }
        let sim = ep.similarity.unwrap_or(0.0).max(0.0);
        let score = sources[&ep.passage.uuid].score.powf(SUPPORT_SOURCE_EXP) * sim;
        list_support.push((ep.passage.uuid.clone(), score));
        passages.insert(ep.passage.uuid.clone(), (ep, sim));
    }
    list_support.sort_by(|a, b| by_score_then_key((a.1, &a.0), (b.1, &b.0)));
    list_support.truncate(list_cap);
    let list_support: Vec<(String, f64)> = list_support
        .into_iter()
        .map(|(u, s)| (format!("p:{u}"), s))
        .collect();

    st.timings_ms.supporting = lap(&mut clock);

    // ── List (b): plain passages ──
    // Collected broadly (#797): vector search with the instructed and (when the profile has an
    // instruction) the bare query, plus BM25 for names. Each is its own list in the fusion.
    let mut plain: Vec<(&'static str, Vec<(String, f64)>)> = Vec::new();
    if p.stages == Stages::Full {
        let mut vectors: Vec<(&'static str, &[f32])> = vec![("passages", emb)];
        if let Some(raw) = raw_emb {
            vectors.push(("passages_raw", raw));
        }
        for (name, v) in vectors {
            let list = vector_passage_list(
                conn,
                p,
                v,
                emb,
                gids,
                list_cap,
                &mut ep_info,
                &mut passages,
                &mut st,
            )?;
            plain.push((name, list));
        }
        let list = lexical_passage_list(
            conn,
            p,
            emb,
            gids,
            list_cap,
            &mut ep_info,
            &mut passages,
            &mut st,
        )?;
        plain.push(("passages_lexical", list));
    }
    st.passage_lists = plain.iter().map(|(n, _)| *n).collect();

    st.timings_ms.passages = lap(&mut clock);

    // ── Stage 4: fusion ──
    let named: Vec<(&'static str, &Vec<(String, f64)>)> = match p.stages {
        Stages::Seeds => vec![("supporting", &list_support)],
        Stages::Expand => vec![("supporting", &list_support), ("facts", &list_facts)],
        Stages::Full => std::iter::once(("supporting", &list_support))
            .chain(plain.iter().map(|(n, l)| (*n, l)))
            .chain(std::iter::once(("facts", &list_facts)))
            .collect(),
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
    // The whole fused candidate list is kept until the cut to `k`: a final reranking stage
    // (#796) slots in here, reordering the head of `fused` before the cut. That stage must then
    // re-apply belief ordering so a superseded passage never lands above its replacement. Today
    // there is no post-fusion belief step: belief-aware ranking runs inside each plain list, and
    // under the believed-only default superseded passages are filtered out of every list anyway.
    // With `believed_only: false` the fused order across lists is not belief-adjusted.
    let fused = rrf_fuse_scored(&lists);
    let fused: Vec<_> = fused.into_iter().take(k).collect();

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
            source_description: info
                .map(|i| i.source_description.clone())
                .unwrap_or_default(),
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
            let mut eps: Vec<SupportingEpisode> =
                f.fe.edge
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
    st.timings_ms.fusion = lap(&mut clock);
    Ok(GraphSearchResponse {
        results,
        count,
        seeds: seeds.iter().map(|s| s.evidence.clone()).collect(),
        stats: st,
        seed_trace,
    })
}

/// Passage rows and their query similarity, for every passage any list may return.
type PassageRows = HashMap<String, (EpisodePassage, f64)>;

/// Records a plain-list passage for evidence and result assembly, unless already known.
fn note_plain_passage(
    ep: &EpisodePassage,
    sim: f64,
    p: &GraphSearchParams,
    t: i64,
    ep_info: &mut HashMap<String, EpInfo>,
    passages: &mut PassageRows,
) {
    let uuid = &ep.passage.uuid;
    ep_info.insert(
        uuid.clone(),
        EpInfo {
            source_description: ep.passage.source_description.clone(),
            origin: ep.origin,
            admits_support: supports_list_a(ep, p, t),
        },
    );
    passages
        .entry(uuid.clone())
        .or_insert_with(|| (ep.clone(), sim));
}

/// One vector plain-passage list: `knowledge_search_passages`' default ranking (vector with
/// belief-aware ranking, #789/#806) for `emb`, with belief evaluated at `as_of`. The list ranks
/// by its own cosine (against `emb`), but every passage reports its cosine against `instructed`
/// as its similarity, as every other passage this read returns does, so `passage.score` means
/// the same thing whichever list found it.
#[allow(clippy::too_many_arguments)]
fn vector_passage_list(
    conn: &Conn<'_>,
    p: &GraphSearchParams,
    emb: &[f32],
    instructed: &[f32],
    gids: Option<&[&str]>,
    list_cap: usize,
    ep_info: &mut HashMap<String, EpInfo>,
    passages: &mut PassageRows,
    st: &mut GraphSearchStats,
) -> Result<Vec<(String, f64)>, Error> {
    let t = p.as_of_us;
    st.db_queries += 1;
    // The group and origin filters run after the ANN probe, so oversample whenever either is
    // set: otherwise a minority group's passages are crowded out of the nearest neighbours by
    // other groups' and the list comes back short or empty.
    let limit = list_cap * 3;
    let probe = if gids.is_some() || p.origins.is_some() {
        limit.saturating_mul(crate::db::KIND_ANN_OVERSAMPLE)
    } else {
        limit
    };
    let mut hits =
        conn.vector_search_episodic_probed(emb, gids, limit, probe, p.origins.as_deref())?;
    for h in &mut hits {
        h.score = 1.0 - h.score;
    }
    hits.retain(|h| h.score.is_finite());
    // Belief-aware ranking (#789, `belief_aware: true`), evaluated at `as_of`: a derived page
    // ranks below the primary note it cites, a superseded passage below its successor. It runs
    // over the whole over-drawn candidate set before the liveness filter and the cut, as
    // `knowledge_search_passages` does, so a demoted passage is replaced, not just reshuffled.
    if hits.len() > 1 {
        st.db_queries += 1;
    }
    let hits = crate::search::apply_belief_ranking_at(conn, hits, t)?;
    let uuids: Vec<String> = hits.iter().map(|h| h.uuid.clone()).collect();
    let rows = if uuids.is_empty() {
        vec![]
    } else {
        st.db_queries += 1;
        conn.get_episode_passages_scored(&uuids, instructed)?
    };
    let by_uuid: HashMap<String, EpisodePassage> = rows
        .into_iter()
        .map(|r| (r.passage.uuid.clone(), r))
        .collect();
    let mut list = Vec::new();
    for h in hits {
        let Some(ep) = by_uuid.get(&h.uuid) else {
            continue;
        };
        if p.believed_only && !episode_live_at(ep, t) {
            continue;
        }
        if list.len() == list_cap {
            break;
        }
        let sim = ep.similarity.unwrap_or(0.0).max(0.0);
        note_plain_passage(ep, sim, p, t, ep_info, passages);
        list.push((format!("p:{}", h.uuid), h.score));
    }
    Ok(list)
}

/// The BM25 plain-passage list (#797, "for names"): #789's lexical passage leg
/// ([`crate::search::lexical_passage_list`]: sanitised query, group and origin filters in the
/// query, empty when the FTS index is unavailable), with the same belief-aware ranking and
/// liveness rule as the vector lists. A passage's reported similarity is its cosine against
/// the instructed query, as for every other passage this read returns.
#[allow(clippy::too_many_arguments)]
fn lexical_passage_list(
    conn: &Conn<'_>,
    p: &GraphSearchParams,
    emb: &[f32],
    gids: Option<&[&str]>,
    list_cap: usize,
    ep_info: &mut HashMap<String, EpInfo>,
    passages: &mut PassageRows,
    st: &mut GraphSearchStats,
) -> Result<Vec<(String, f64)>, Error> {
    let t = p.as_of_us;
    st.db_queries += 1;
    let hits = crate::search::lexical_passage_list(
        conn,
        &p.query,
        gids,
        p.origins.as_deref(),
        list_cap * 3,
    )?;
    if hits.is_empty() {
        return Ok(vec![]);
    }
    let uuids: Vec<String> = hits.iter().map(|(u, _)| u.clone()).collect();
    st.db_queries += 1;
    let by_uuid: HashMap<String, EpisodePassage> = conn
        .get_episode_passages_scored(&uuids, emb)?
        .into_iter()
        .map(|r| (r.passage.uuid.clone(), r))
        .collect();
    let rows: Vec<PassageResult> = hits
        .iter()
        .filter(|(_, score)| score.is_finite())
        .filter_map(|(u, score)| {
            by_uuid.get(u).map(|ep| PassageResult {
                score: *score,
                ..ep.passage.clone()
            })
        })
        .collect();
    if rows.len() > 1 {
        st.db_queries += 1;
    }
    let ranked = crate::search::apply_belief_ranking_at(conn, rows, t)?;
    let mut list = Vec::new();
    for h in ranked {
        let ep = &by_uuid[&h.uuid];
        if p.believed_only && !episode_live_at(ep, t) {
            continue;
        }
        if list.len() == list_cap {
            break;
        }
        let sim = ep.similarity.unwrap_or(0.0).max(0.0);
        note_plain_passage(ep, sim, p, t, ep_info, passages);
        list.push((format!("p:{}", h.uuid), h.score));
    }
    Ok(list)
}

/// Stage 1: entity linking.
fn seed_stage(
    conn: &Conn<'_>,
    p: &GraphSearchParams,
    emb: &[f32],
    gids: Option<&[&str]>,
    fts_q: &str,
    st: &mut GraphSearchStats,
) -> Result<(Vec<Seed>, Option<Vec<SeedCandidate>>), Error> {
    let bm25 = if fts_q.is_empty() {
        vec![]
    } else {
        st.db_queries += 1;
        conn.fts_search_entities_kind(fts_q, gids, None, SEED_CANDIDATES)?
    };
    // Vector lists keep only finite similarities: a zero-filled (un-backfilled) summary
    // embedding yields NaN and so contributes nothing.
    let finite_only = |l: Vec<(String, f64)>| -> Vec<(String, f64)> {
        l.into_iter()
            .filter(|(_, d)| (1.0 - d).is_finite())
            .collect()
    };
    st.db_queries += 2;
    let name_vec =
        finite_only(conn.vector_search_entities_kind(emb, gids, None, SEED_CANDIDATES)?);
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

    // Exact names: query n-grams × (group, kind) pairs, one batched lookup_key probe. With
    // `group_ids` the pairs are those groups × every registered kind (no scan). Without, they
    // are the pairs actually present among entities (one scan) — every group, not just the
    // groups the capped lists above happened to fill from, and no key for a kind a group lacks.
    // Either way the key list is capped at MAX_EXACT_KEYS.
    let pairs: Vec<(String, String)> = match gids {
        Some(g) => {
            let mut kinds = conn.known_kinds();
            kinds.sort();
            g.iter()
                .flat_map(|g| kinds.iter().map(move |k| (g.to_string(), k.clone())))
                .collect()
        }
        None => {
            st.db_queries += 1;
            conn.entity_group_kinds()?
        }
    };
    let (keys, keys_truncated) = exact_name_keys(&pairs, &query_ngrams(&p.query), MAX_EXACT_KEYS);
    if keys_truncated {
        st.truncate("exact_name_keys", MAX_EXACT_KEYS);
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
    // The best similarity among candidates that can be seeded at all: a missing row or a
    // `Merged` tombstone must not raise the bar the margin is measured from.
    let global_best = fused
        .iter()
        .filter(|h| {
            rows.get(&h.uuid)
                .is_some_and(|r| !r.labels.iter().any(|l| l == "Merged"))
        })
        .filter_map(|h| best_vec(&h.raw))
        .fold(f64::NEG_INFINITY, f64::max);

    // Admission. `decision` records why each candidate was (or was not) seeded, for the trace;
    // a later stage may overwrite an earlier verdict (e.g. `no_evidence` → `summary_vector`).
    let mut decision: HashMap<String, (&'static str, bool)> = HashMap::new();
    let by_uuid: HashMap<&str, &crate::search::FusedHit> =
        fused.iter().map(|h| (h.uuid.as_str(), h)).collect();
    let mut admitted: Vec<(crate::search::FusedHit, EntityRow, &'static str)> = Vec::new();
    for h in &fused {
        let Some(row) = rows.get(&h.uuid) else {
            decision.insert(h.uuid.clone(), ("no_row", false));
            continue;
        };
        if row.labels.iter().any(|l| l == "Merged") {
            decision.insert(h.uuid.clone(), ("merged", false));
            continue;
        }
        let exact_hit = h.raw[0].is_some();
        let lexical = bm25_rank
            .get(h.uuid.as_str())
            .is_some_and(|r| *r < SEED_BM25_RANK);
        let vector = best_vec(&h.raw)
            .is_some_and(|s| s >= SEED_SIM_FLOOR && s >= global_best - SEED_SIM_MARGIN);
        let reason = if exact_hit {
            "exact_name"
        } else if lexical {
            "bm25_rank"
        } else if vector {
            "vector"
        } else {
            decision.insert(h.uuid.clone(), ("no_evidence", false));
            continue;
        };
        admitted.push((h.clone(), row.clone(), reason));
    }

    // `origins`: a seed needs a matching live mention (an asserted entity counts as primary).
    let origin_ok =
        |uuids: Vec<String>, st: &mut GraphSearchStats| -> Result<Option<HashSet<String>>, Error> {
            let Some(filter) = p.origins.as_deref() else {
                return Ok(None);
            };
            if uuids.is_empty() {
                return Ok(Some(HashSet::new()));
            }
            st.db_queries += 1;
            let sup = crate::origin_reads::entity_support(conn, &uuids, Some(filter), p.as_of_us)?;
            Ok(Some(
                sup.into_iter()
                    .filter(|(_, s)| s.matches)
                    .map(|(u, _)| u)
                    .collect(),
            ))
        };
    if let Some(ok) = origin_ok(
        admitted.iter().map(|(h, _, _)| h.uuid.clone()).collect(),
        st,
    )? {
        admitted.retain(|(h, _, _)| {
            let keep = ok.contains(&h.uuid);
            if !keep {
                decision.insert(h.uuid.clone(), ("origin_filter", false));
            }
            keep
        });
    }
    for (h, _, _) in admitted.iter().skip(SEED_CAP) {
        decision.insert(h.uuid.clone(), ("seed_cap", false));
    }
    admitted.truncate(SEED_CAP);

    // Summary-vector seeds, on relative evidence. A role description ("the rule covering new-hire
    // induction") names nothing, so its entity matches only through its summary: one list, a low
    // fused score, no BM25 hit. On v2 such entities sit at the top of the summary list yet lost
    // every one of the slots above to generic concept entities that also match BM25 on common
    // words. So up to SUMMARY_SEEDS more seeds are taken in summary-list order, each within
    // SUMMARY_SEED_MARGIN of the best summary similarity *and* at least that margin above the
    // list's median. Both tests are relative to the list, never an absolute cosine floor (whose
    // right value depends on the embedder). The second is what keeps ANN filler out: a query that
    // matches no summary well (#613) yields a flat list whose best barely clears its median, so
    // nothing qualifies.
    let (quota, margin) = (SUMMARY_SEEDS, SUMMARY_SEED_MARGIN);
    if quota > 0 {
        // The list's statistics are taken over the candidates that could be seeded at all —
        // with a row, not a `Merged` tombstone, within `origins` — so a tombstone or a filtered
        // entity at the top cannot raise the bar for every genuine summary match.
        let eligible: Vec<(String, f64)> = sum_vec
            .iter()
            .filter(|(u, _)| {
                rows.get(u)
                    .is_some_and(|r| !r.labels.iter().any(|l| l == "Merged"))
            })
            .filter_map(|(u, d)| sim(Some(*d)).map(|s| (u.clone(), s)))
            .collect();
        let ok = origin_ok(eligible.iter().map(|(u, _)| u.clone()).collect(), st)?;
        let eligible: Vec<(String, f64)> = eligible
            .into_iter()
            .filter(|(u, _)| {
                let keep = ok.as_ref().is_none_or(|ok| ok.contains(u));
                if !keep {
                    decision.insert(u.clone(), ("origin_filter", false));
                }
                keep
            })
            .collect();
        let best_sum = eligible.first().map(|(_, s)| *s);
        let median_sum = {
            let mut v: Vec<f64> = eligible.iter().map(|(_, s)| *s).collect();
            v.sort_by(|a, b| a.total_cmp(b));
            v.get(v.len() / 2).copied()
        };
        let seeded: HashSet<String> = admitted.iter().map(|(h, _, _)| h.uuid.clone()).collect();
        let pool: Vec<String> = eligible
            .iter()
            .filter(|(u, s)| {
                !seeded.contains(u)
                    && best_sum.is_some_and(|b| *s >= b - margin)
                    && median_sum.is_some_and(|m| *s >= m + margin)
            })
            .map(|(u, _)| u.clone())
            .take(quota)
            .collect();
        for u in pool {
            let h = by_uuid[u.as_str()];
            admitted.push(((*h).clone(), rows[&u].clone(), "summary_vector"));
        }
    }

    let trace = p.explain.then(|| {
        let rank_in = |l: &[(String, f64)]| -> HashMap<String, usize> {
            l.iter()
                .enumerate()
                .map(|(i, (u, _))| (u.clone(), i + 1))
                .collect()
        };
        let (name_rank, sum_rank) = (rank_in(&name_vec), rank_in(&sum_vec));
        for (h, _, reason) in &admitted {
            decision.insert(h.uuid.clone(), (reason, true));
        }
        // Candidate order: fused RRF score, uuid tie-break (deterministic).
        let mut t: Vec<SeedCandidate> = fused
            .iter()
            .map(|h| {
                let row = rows.get(&h.uuid);
                let (reason, seeded) = decision
                    .get(&h.uuid)
                    .copied()
                    .unwrap_or(("no_evidence", false));
                SeedCandidate {
                    uuid: h.uuid.clone(),
                    name: row.map(|r| r.name.clone()).unwrap_or_default(),
                    kind: row.map(|r| r.kind.clone()).unwrap_or_default(),
                    rrf_score: h.rrf_score,
                    exact_name: h.raw[0].is_some(),
                    bm25_rank: bm25_rank.get(h.uuid.as_str()).map(|r| r + 1),
                    name_vector_rank: name_rank.get(&h.uuid).copied(),
                    summary_vector_rank: sum_rank.get(&h.uuid).copied(),
                    name_similarity: sim(h.raw[2]),
                    summary_similarity: sim(h.raw[3]),
                    seeded,
                    reason,
                }
            })
            .collect();
        t.sort_by(|a, b| by_score_then_key((a.rrf_score, &a.uuid), (b.rrf_score, &b.uuid)));
        t
    });

    // Normalise against the best fused score among *all* final seeds. Summary-vector seeds are
    // appended after the RRF-ordered ones, so the first seed is not necessarily the best.
    let top = admitted
        .iter()
        .map(|(h, _, _)| h.rrf_score)
        .fold(f64::NEG_INFINITY, f64::max);
    let top = if top.is_finite() && top > 0.0 {
        top
    } else {
        1.0
    };
    let seeds = admitted
        .into_iter()
        .map(|(h, row, reason)| {
            let mut matched = Vec::new();
            if h.raw[0].is_some() {
                matched.push("name");
            }
            if h.raw[1].is_some() {
                matched.push("bm25");
            }
            // ANN always fills its list (#613), so a vector list counts as a match only when
            // its similarity clears the seed floor; the raw similarities are reported anyway.
            let floor_ok = |d: Option<f64>| sim(d).is_some_and(|s| s >= SEED_SIM_FLOOR);
            if floor_ok(h.raw[2]) {
                matched.push("name_vector");
            }
            // A summary-vector seed was admitted on the summary list's relative evidence, so it
            // always names that list, whatever the absolute similarity.
            if floor_ok(h.raw[3]) || reason == "summary_vector" {
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
        .collect();
    Ok((seeds, trace))
}

/// The next hop's frontier: the far endpoints of this hop's best edges, at most `beam` of them,
/// newly reached only. Each `(score, edge uuid, far uuid)` is taken best score first (edge-uuid
/// tie-break). An already-reached entity or one already chosen is skipped *before* counting
/// toward `beam`, so edges back to seeds or between reached entities cannot use up the slots and
/// starve the walk of the new entities a multi-hop path needs. Sorted by uuid.
pub fn select_beam(
    mut hop_edges: Vec<(f64, String, String)>,
    visited: &HashSet<String>,
    beam: usize,
) -> Vec<String> {
    hop_edges.sort_by(|a, b| by_score_then_key((a.0, &a.1), (b.0, &b.1)));
    let mut next: Vec<String> = Vec::new();
    for (_, _, far) in hop_edges {
        if next.len() == beam {
            break;
        }
        if !visited.contains(&far) && !next.contains(&far) {
            next.push(far);
        }
    }
    next.sort();
    next
}

/// Mention episodes of `entities` for list (a), bounded per entity. Entities mentioned at most
/// [`MENTIONS_PER_ENTITY`] times are fetched in one batch, as before. A hub mentioned more often
/// gets its [`MENTIONS_PER_ENTITY`] most query-relevant mentions that can count — corroborating,
/// within `origins`, and not superseded at `as_of` when filtering — so neither a hub's size nor its
/// superseded or derived mentions decide which of its notes compete. At most
/// [`MAX_HUB_MENTION_QUERIES`] hubs are fetched, best score first. Both cuts are reported.
fn mention_rows(
    conn: &Conn<'_>,
    p: &GraphSearchParams,
    emb: &[f32],
    reach: &BTreeMap<String, Reach>,
    entities: &[String],
    st: &mut GraphSearchStats,
) -> Result<Vec<(String, String, crate::origin::EpisodeOriginInfo)>, Error> {
    st.db_queries += 1;
    let counts = conn.entity_mention_counts(entities)?;
    let (mut small, mut hubs): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for e in entities {
        match counts.get(e).copied().unwrap_or(0) {
            0 => {}
            n if n <= MENTIONS_PER_ENTITY => small.push(e.clone()),
            _ => hubs.push(e.clone()),
        }
    }
    small.sort();
    let mut out = Vec::new();
    if !small.is_empty() {
        st.db_queries += 1;
        out.extend(conn.entity_mention_origins(&small)?);
    }
    if !hubs.is_empty() {
        let score = |u: &String| reach.get(u).map_or(0.0, |r| r.score);
        hubs.sort_by(|a, b| by_score_then_key((score(a), a), (score(b), b)));
        if hubs.len() > MAX_HUB_MENTION_QUERIES {
            hubs.truncate(MAX_HUB_MENTION_QUERIES);
            st.truncate("mention_hubs", MAX_HUB_MENTION_QUERIES);
        }
        let allowed: Vec<Origin> = [
            Origin::Primary,
            Origin::External,
            Origin::Derived,
            Origin::Answer,
        ]
        .into_iter()
        .filter(|o| o.is_corroborating() && p.origins.as_ref().is_none_or(|f| f.contains(o)))
        .collect();
        let live_at = p.believed_only.then_some(p.as_of_us);
        for hub in hubs {
            st.db_queries += 1;
            // A hub is classified by its raw mention count, but the ranked query filters derived,
            // out-of-`origins` and superseded mentions in the query, so its qualifying mentions
            // may all fit. Fetch one row past the bound and report a cut only when it comes back.
            let mut rows =
                conn.entity_mentions_ranked(&hub, emb, &allowed, live_at, MENTIONS_PER_ENTITY + 1)?;
            if rows.len() > MENTIONS_PER_ENTITY {
                rows.truncate(MENTIONS_PER_ENTITY);
                st.truncate("mentions_per_entity", MENTIONS_PER_ENTITY);
            }
            out.extend(rows.into_iter().map(|(ep, info)| (hub.clone(), ep, info)));
        }
    }
    Ok(out)
}

/// One frontier entity's steps after fan-out, in their given (relevance) order: at most `cap`
/// corroborating steps and, on a separate budget, at most `cap` citation-only ones. A
/// citation-only edge (every producer derived / answer) can neither rank nor carry the walk, so
/// it must never take a slot from a primary edge (ADR-0752); it is kept only to be cited. Returns
/// the kept steps and whether each budget cut anything.
pub fn split_fanout<T>(
    steps: Vec<T>,
    is_citation: impl Fn(&T) -> bool,
    cap: usize,
) -> (Vec<T>, bool, bool) {
    let (mut primary, mut citations) = (0usize, 0usize);
    let (mut cut_primary, mut cut_citations) = (false, false);
    let mut kept = Vec::with_capacity(steps.len().min(2 * cap));
    for s in steps {
        let (n, cut) = if is_citation(&s) {
            (&mut citations, &mut cut_citations)
        } else {
            (&mut primary, &mut cut_primary)
        };
        if *n < cap {
            *n += 1;
            kept.push(s);
        } else {
            *cut = true;
        }
    }
    (kept, cut_primary, cut_citations)
}

/// One hop's frontier rows, at most `cap` per direction. The batched lookup is tried first (two
/// queries). Its cut is by `rn.uuid` across the whole frontier, so when it hits the cap one hub
/// could take every row and starve the other frontier entities. In that case each entity is
/// refetched on its own with an equal share of the same budget (`cap / frontier`, at least one),
/// so no entity loses edges to another's. Within one entity the cut is still by uuid, because it
/// happens before relevance is scored (one `fact_similarities` query over the rows that survive);
/// the per-entity fan-out then ranks by relevance.
fn fetch_frontier(
    conn: &Conn<'_>,
    emb: &[f32],
    frontier: &[String],
    gids: Option<&[&str]>,
    cap: usize,
    st: &mut GraphSearchStats,
) -> Result<Vec<FrontierEdge>, Error> {
    st.db_queries += 2 + usize::from(!emb.is_empty()); // two directions + one scoring query
    let (rows, truncated) = conn.expand_frontier_scored(frontier, gids, cap, emb)?;
    if !truncated {
        return Ok(rows);
    }
    st.truncate("frontier_rows", cap);
    if frontier.len() == 1 {
        return Ok(rows);
    }
    let share = (cap / frontier.len()).max(1);
    let mut out = Vec::new();
    for entity in frontier {
        st.db_queries += 2 + usize::from(!emb.is_empty()); // two directions + one scoring query
        let (r, cut) =
            conn.expand_frontier_scored(std::slice::from_ref(entity), gids, share, emb)?;
        if cut {
            st.truncate("entity_rows", share);
        }
        out.extend(r);
    }
    Ok(out)
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
        let mut rows = fetch_frontier(conn, emb, &frontier, gids, FRONTIER_ROW_CAP, st)?;
        rows.retain(|r| !r.far_merged);
        // An edge walked on an earlier hop is never revisited. Reaching it again is an alternate
        // path, unless the entity it is reached from was itself reached through that edge: that
        // is the walk doubling back along its own path (A → B, then B → A), not a second path.
        rows.retain(|r| match facts.get_mut(&r.edge.uuid) {
            Some(f) => {
                let back_edge = reach
                    .get(&r.from_uuid)
                    .is_some_and(|rc| rc.path.iter().any(|s| s.edge_uuid == r.edge.uuid));
                if !back_edge {
                    f.alternates += 1;
                }
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
                    admits_support: supports_list_a(e, p, t),
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
            let own: Vec<&EpisodePassage> =
                fe.edge.episodes.iter().filter_map(|u| eps.get(u)).collect();
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
            // Judge corroboration over the producers that still count at `as_of`: an edge whose
            // primary producer was corrected away but whose derived restatement is live is
            // believed only on the derived page's word, so it must not rank or carry the walk
            // (ADR-0752). Without filtering, every listed producer counts.
            let surviving: Vec<&&EpisodePassage> = own
                .iter()
                .filter(|e| !p.believed_only || episode_live_at(e, t))
                .collect();
            let citation_only =
                !surviving.is_empty() && surviving.iter().all(|e| !e.origin.is_corroborating());
            let rel = edge_relevance(
                fe.fact_similarity.unwrap_or(0.0).max(0.0),
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
            // A step that cannot carry the walk — citation-only, or (with `believed_only: false`)
            // not believed at `as_of` — goes on the separate budget, so a hub's superseded or
            // derived facts can never take the slots its walkable believed edges need.
            let (kept, cut_primary, cut_citations) = split_fanout(
                steps,
                |s| s.citation_only || !s.belief.believed,
                PER_ENTITY_FANOUT,
            );
            steps = kept;
            if cut_primary {
                st.truncate("per_entity_fanout", PER_ENTITY_FANOUT);
            }
            if cut_citations {
                st.truncate("per_entity_citations", PER_ENTITY_FANOUT);
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
                // A citation-only edge (every producer derived / answer) is recorded so it can be
                // cited, but it carries the walk nowhere: it neither reaches its far endpoint nor
                // enters the beam. Otherwise an agent-written page asserting "A relates to B"
                // would pull B's primary notes into the ranking — rank from derived content,
                // which ADR-0752 forbids. Likewise a fact not believed at `as_of` is never a step:
                // with `believed_only: false` it may still be *returned*, with its belief state,
                // but the walk never continues through it (FR-013), so a path breaks at it.
                let walks = !s.citation_only && s.belief.believed;
                if walks {
                    hop_edges.push((score, s.fe.edge.uuid.clone(), s.fe.far_uuid.clone()));
                }

                // The reached entity keeps its best same-hop path; earlier hops win outright.
                if walks && !visited.contains(&s.fe.far_uuid) {
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

        frontier = select_beam(hop_edges, &visited, BEAM);
    }

    // The total cap counts only facts that can rank or carry the walk. A citation-only fact is
    // an attachment: it takes no slot, and it survives the trim only while every walk edge
    // before it on its path does.
    let primary = facts.values().filter(|f| !f.citation_only).count();
    if primary > TOTAL_EDGE_CAP {
        let candidates: Vec<(f64, String, Vec<String>)> = facts
            .iter()
            .filter(|(_, f)| !f.citation_only)
            .map(|(u, f)| {
                let path = f.path.iter().map(|s| s.edge_uuid.clone()).collect();
                (f.score, u.clone(), path)
            })
            .collect();
        let keep = path_closed_keep(candidates, TOTAL_EDGE_CAP);
        facts.retain(|u, f| {
            if f.citation_only {
                f.path
                    .iter()
                    .filter(|s| &s.edge_uuid != u)
                    .all(|s| keep.contains(&s.edge_uuid))
            } else {
                keep.contains(u)
            }
        });
        // An entity reached only through a trimmed edge is no longer reached: its mention
        // passages must not enter list (a) with a path through an edge the response omits.
        reach.retain(|_, r| r.path.iter().all(|s| keep.contains(&s.edge_uuid)));
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
        assert!(!grams.iter().any(|g| g.contains('?')));
        // The possessive is also probed as written, for names that contain one.
        assert!(grams.contains(&"cedar core's".to_string()), "{grams:?}");
        assert!(grams.contains(&"core's".to_string()), "{grams:?}");
        let alice = query_ngrams("Where is Alice's Restaurant?");
        assert!(
            alice.contains(&"alice's restaurant".to_string()),
            "{alice:?}"
        );
        assert!(alice.contains(&"alice restaurant".to_string()), "{alice:?}");
        // A query without possessives yields no extra variants.
        assert_eq!(query_ngrams("a b"), vec!["a", "b", "a b"]);
        // Shortest first.
        assert_eq!(grams[0], "what");
        assert!(grams
            .iter()
            .all(|g| g.split(' ').count() <= MAX_NGRAM_WORDS));
    }

    #[test]
    fn ngrams_dedupe_and_cap() {
        assert_eq!(query_ngrams("a a a"), vec!["a", "a a", "a a a"]);
        let long: String = (0..100).map(|i| format!("w{i} ")).collect();
        assert_eq!(query_ngrams(&long).len(), MAX_NGRAMS);
        assert!(query_ngrams("  ?! ").is_empty());
        // A curly-apostrophe possessive: stripped first, then as written.
        assert_eq!(
            query_ngrams("Fable\u{2019}s price"),
            vec![
                "fable",
                "fable\u{2019}s",
                "price",
                "fable price",
                "fable\u{2019}s price"
            ]
        );
    }

    #[test]
    fn beam_slots_go_to_newly_reached_entities_only() {
        let e = |score: f64, edge: &str, far: &str| (score, edge.to_string(), far.to_string());
        let visited: HashSet<String> = ["seed_a", "seed_b"].iter().map(|s| s.to_string()).collect();
        // The three best edges lead back to seeds or to an entity already chosen; the new
        // entities sit below them.
        let edges = vec![
            e(0.9, "e1", "seed_b"),
            e(0.8, "e2", "seed_a"),
            e(0.7, "e3", "x"),
            e(0.6, "e4", "x"),
            e(0.5, "e5", "y"),
            e(0.4, "e6", "z"),
        ];
        // A cut taken before the filter would keep e1 and e2, both seeds, and yield nothing.
        assert_eq!(select_beam(edges.clone(), &visited, 2), vec!["x", "y"]);
        assert_eq!(
            select_beam(edges.clone(), &visited, 10),
            vec!["x", "y", "z"]
        );
        assert!(select_beam(edges, &visited, 0).is_empty());
    }

    #[test]
    fn a_hub_cannot_starve_the_rest_of_the_frontier_of_rows() {
        const DIM: usize = 4;
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap();
        let c = db.connect().unwrap();
        c.init_schema(DIM).unwrap();
        let entity = |uuid: &str| {
            c.insert_entity(&EntityRow {
                uuid: uuid.into(),
                name: uuid.into(),
                group_id: "g".into(),
                labels: vec!["Entity".into()],
                created_at: "2026-01-01 00:00:00".into(),
                name_embedding: vec![0.1; DIM],
                attributes: "{}".into(),
                ..Default::default()
            })
            .unwrap()
        };
        let edge = |uuid: &str, src: &str, dst: &str| {
            c.insert_relates_to_edge(&RelatesToEdge {
                uuid: uuid.into(),
                name: "REL".into(),
                source_node_uuid: src.into(),
                target_node_uuid: dst.into(),
                group_id: "g".into(),
                fact: uuid.into(),
                fact_embedding: vec![0.5; DIM],
                created_at: "2026-01-01 00:00:00".into(),
                attributes: "{}".into(),
                ..Default::default()
            })
            .unwrap()
        };
        // Hub `a` has five edges whose uuids all sort before `z`'s single edge.
        for e in ["a", "z", "y", "b1", "b2", "b3", "b4", "b5"] {
            entity(e);
        }
        for i in 1..=5 {
            edge(&format!("e{i}"), "a", &format!("b{i}"));
        }
        edge("f1", "z", "y");

        let frontier = vec!["a".to_string(), "z".to_string()];
        let p = GraphSearchParams {
            query: "q".into(),
            group_ids: None,
            num_results: 10,
            origins: None,
            believed_only: true,
            as_of_us: 0,
            stages: Stages::Full,
            explain: false,
        };
        // A batched cut of 4 rows by uuid keeps e1..e4 and drops z's only edge.
        let (batched, cut) = c.expand_frontier(&frontier, None, 4).unwrap();
        assert!(cut);
        assert!(!batched.iter().any(|r| r.edge.uuid == "f1"));

        let mut st = new_stats(&p, 1);
        let rows = fetch_frontier(&c, &[], &frontier, None, 4, &mut st).unwrap();
        let mut got: Vec<&str> = rows.iter().map(|r| r.edge.uuid.as_str()).collect();
        got.sort();
        assert_eq!(
            got,
            vec!["e1", "e2", "f1"],
            "each entity gets an equal share of 4"
        );
        assert!(st.truncations.iter().any(|t| t.cap == "frontier_rows"));
        assert!(st
            .truncations
            .iter()
            .any(|t| t.cap == "entity_rows" && t.limit == 2));

        // Under the cap nothing is refetched.
        let mut st = new_stats(&p, 1);
        assert_eq!(
            fetch_frontier(&c, &[], &frontier, None, 100, &mut st)
                .unwrap()
                .len(),
            6
        );
        assert!(!st.truncated);
        assert_eq!(st.db_queries, 2);
    }

    #[test]
    fn exact_name_keys_are_capped_by_dropping_longest_ngrams_everywhere() {
        let pairs: Vec<(String, String)> = [("g1", "Entity"), ("g1", "Person"), ("g2", "Entity")]
            .iter()
            .map(|(g, k)| (g.to_string(), k.to_string()))
            .collect();
        let grams = query_ngrams("cedar core supplier");
        assert_eq!(grams.len(), 6);

        let (all, cut) = exact_name_keys(&pairs, &grams, 1000);
        assert!(!cut);
        assert_eq!(all.len(), pairs.len() * grams.len());
        assert!(all.contains(&compute_lookup_key("g2", "Entity", "cedar core")));

        // Room for 4 n-grams × 3 pairs: the two longest n-grams go, for every pair alike.
        let (keys, cut) = exact_name_keys(&pairs, &grams, 13);
        assert!(cut);
        assert_eq!(keys.len(), 12);
        for (g, k) in &pairs {
            assert!(keys.contains(&compute_lookup_key(g, k, "cedar core")));
            assert!(!keys.contains(&compute_lookup_key(g, k, "cedar core supplier")));
        }
        assert!(exact_name_keys(&[], &grams, 10).0.is_empty());
        assert_eq!(exact_name_keys(&pairs, &grams, 2), (vec![], true));
    }

    #[test]
    fn fts_terms_keep_only_alphanumeric_words() {
        assert_eq!(
            fts_terms("Cedar Core → Fable's $87/unit"),
            "Cedar Core Fable s 87 unit"
        );
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
    fn total_cap_keeps_whole_paths_never_an_orphaned_hop_two_fact() {
        let c = |score: f64, uuid: &str, path: &[&str]| {
            (
                score,
                uuid.to_string(),
                path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            )
        };
        // A weak bridge (0.1) leads to two strong hop-2 facts; one unrelated hop-1 fact (0.5).
        let cands = vec![
            c(0.1, "bridge", &["bridge"]),
            c(0.9, "far1", &["bridge", "far1"]),
            c(0.8, "far2", &["bridge", "far2"]),
            c(0.5, "near", &["near"]),
        ];
        // Cut by score alone, a cap of 2 keeps far1 + far2 and drops their bridge.
        let keep = path_closed_keep(cands.clone(), 2);
        assert_eq!(
            keep,
            HashSet::from(["bridge".to_string(), "far1".to_string()])
        );
        let keep = path_closed_keep(cands.clone(), 3);
        assert_eq!(
            keep,
            HashSet::from(["bridge".into(), "far1".into(), "far2".into()])
        );
        // With a cap of 1 neither two-edge path fits, so the best fact that fits alone is `near`.
        assert_eq!(
            path_closed_keep(cands.clone(), 1),
            HashSet::from(["near".into()])
        );
        // Every kept fact's path is fully kept.
        for cap in 0..=5 {
            let keep = path_closed_keep(cands.clone(), cap);
            assert!(keep.len() <= cap);
            for (_, u, path) in &cands {
                if keep.contains(u) {
                    assert!(path.iter().all(|e| keep.contains(e)), "cap {cap}: {u}");
                }
            }
        }
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
                similarity: None,
            },
            origin: Origin::Primary,
            supersession: belief::EpisodeSupersession {
                superseded_at_us: superseded,
                revision_id_present: pending,
                ..Default::default()
            },
            ingested_at_us: ingested,
            content_embedding: vec![],
            similarity: None,
        }
    }

    #[test]
    fn episode_liveness_honours_supersession_ingest_and_pending_revisions() {
        assert!(episode_live_at(&ep(None, Some(10), false), 10));
        assert!(!episode_live_at(&ep(None, Some(10), false), 9));
        assert!(episode_live_at(&ep(Some(20), Some(10), false), 19));
        assert!(!episode_live_at(&ep(Some(20), Some(10), false), 20));
        assert!(!episode_live_at(&ep(None, Some(10), true), 100));
        // A pending document revision, ingested long ago, is its own state — not "not yet
        // ingested" — and is not believed.
        let pending = passage_belief(&ep(None, Some(10), true), 100);
        assert_eq!(pending.state, "pending_revision");
        assert!(!pending.believed);
        // Ingested after `as_of` wins over a pending revision.
        assert_eq!(
            passage_belief(&ep(None, Some(10), true), 5).state,
            "not_yet_ingested"
        );
        assert_eq!(
            passage_belief(&ep(Some(20), Some(10), false), 25).state,
            "superseded"
        );
        assert_eq!(
            passage_belief(&ep(None, Some(10), false), 5).state,
            "not_yet_ingested"
        );
        let b = passage_belief(&ep(Some(20), Some(10), false), 15);
        assert_eq!(b.state, "believed");
        assert!(b.believed);
        assert!(b.supersession.is_some());
    }

    #[test]
    fn pre_cap_mention_filter_never_rejects_an_admissible_passage() {
        let params = |believed_only: bool, origins: Option<Vec<Origin>>| GraphSearchParams {
            query: "q".into(),
            group_ids: None,
            num_results: 10,
            origins,
            believed_only,
            as_of_us: 0,
            stages: Stages::Full,
            explain: false,
        };
        let all = [
            Origin::Primary,
            Origin::External,
            Origin::Derived,
            Origin::Answer,
        ];
        let mut rejected = 0;
        for believed_only in [true, false] {
            for filter in [None, Some(vec![Origin::Primary])] {
                let p = params(believed_only, filter);
                for origin in all {
                    for superseded in [None, Some(20)] {
                        for ingested in [None, Some(10)] {
                            for pending in [false, true] {
                                for t in [5, 15, 25] {
                                    let mut e = ep(superseded, ingested, pending);
                                    e.origin = origin;
                                    let info = crate::origin::EpisodeOriginInfo {
                                        origin,
                                        doc_id: None,
                                        superseded_at_us: superseded,
                                        group_id: "g".into(),
                                    };
                                    let exact = supports_list_a(&e, &p, t);
                                    let pre = may_support_list_a(&info, &p, t);
                                    assert!(
                                        !exact || pre,
                                        "pre-filter dropped an admissible passage: \
                                         {origin:?} sup={superseded:?} t={t} bo={believed_only}"
                                    );
                                    rejected += usize::from(!pre);
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(rejected > 0, "the pre-filter must actually drop something");
        // The cases the finding names: derived, answer, out-of-filter and superseded mentions.
        let p = params(true, Some(vec![Origin::Primary]));
        let info = |origin, superseded_at_us| crate::origin::EpisodeOriginInfo {
            origin,
            doc_id: None,
            superseded_at_us,
            group_id: "g".into(),
        };
        assert!(may_support_list_a(&info(Origin::Primary, None), &p, 15));
        assert!(!may_support_list_a(&info(Origin::Derived, None), &p, 15));
        assert!(!may_support_list_a(&info(Origin::External, None), &p, 15));
        assert!(!may_support_list_a(
            &info(Origin::Primary, Some(10)),
            &p,
            15
        ));
        let unfiltered = params(true, None);
        assert!(!may_support_list_a(
            &info(Origin::Answer, None),
            &unfiltered,
            15
        ));
    }

    #[test]
    fn citation_only_steps_never_take_a_primary_fanout_slot() {
        // Relevance order: three on-topic derived restatements ahead of three primary edges.
        let steps = vec![
            ("d1", true),
            ("d2", true),
            ("d3", true),
            ("p1", false),
            ("p2", false),
            ("p3", false),
        ];
        let (kept, cut_primary, cut_citations) = split_fanout(steps.clone(), |s| s.1, 2);
        let ids: Vec<&str> = kept.iter().map(|s| s.0).collect();
        // Old behaviour (one shared budget of 2) would have kept d1, d2 and no primary edge.
        assert_eq!(
            ids,
            vec!["d1", "d2", "p1", "p2"],
            "order kept, budgets separate"
        );
        assert!(cut_primary && cut_citations);

        let (kept, cut_primary, cut_citations) = split_fanout(steps.clone(), |s| s.1, 3);
        assert_eq!(kept.len(), 6);
        assert!(!cut_primary && !cut_citations);

        let only_derived: Vec<(&str, bool)> = (0..5).map(|_| ("d", true)).collect();
        let (kept, cut_primary, cut_citations) = split_fanout(only_derived, |s| s.1, 2);
        assert_eq!(kept.len(), 2);
        assert!(!cut_primary, "no primary edge was cut");
        assert!(cut_citations);
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
        offer(
            &mut m,
            "p",
            src(
                0.2,
                Via::Mention {
                    entity_uuid: "z".into(),
                },
            ),
        );
        offer(
            &mut m,
            "p",
            src(
                0.5,
                Via::Mention {
                    entity_uuid: "y".into(),
                },
            ),
        );
        offer(
            &mut m,
            "p",
            src(
                0.1,
                Via::Fact {
                    edge_uuid: "a".into(),
                },
            ),
        );
        assert_eq!(m["p"].score, 0.5);
        offer(
            &mut m,
            "p",
            src(
                0.5,
                Via::Fact {
                    edge_uuid: "b".into(),
                },
            ),
        );
        assert!(matches!(&m["p"].via, Via::Fact { edge_uuid } if edge_uuid == "b"));
        offer(
            &mut m,
            "p",
            src(
                0.5,
                Via::Fact {
                    edge_uuid: "c".into(),
                },
            ),
        );
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
