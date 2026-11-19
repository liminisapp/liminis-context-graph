use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::{
    db::Db,
    embedder::{EmbedRole, Embedder},
    error::{is_fts_inconsistent_error, Error},
    types::{
        EdgeSearchEvidence, EntityRow, EntitySearchEvidence, PassageResult, RelatesToEdge,
        ScoredEdge, ScoredEntity,
    },
};

/// Reciprocal Rank Fusion (AD-7).
///
/// `rrf_score(rank) = 1.0 / (rank + 60.0)`
/// Returns UUIDs sorted by descending fused score; UUID tie-breaking for determinism.
///
/// N-ary (issue #470) rather than a fixed 2-list signature, so `hybrid_entity_search` can fuse
/// a third input (the summary-vector list) alongside BM25 and the name-vector list, while
/// `hybrid_edge_search` keeps fusing exactly 2 — both call the same implementation.
///
/// This is the order-only view used by the extraction-time dedup path (issue #629 FR-010); it
/// shares [`rrf_rank`] with [`rrf_fuse_scored`], so the two can never disagree on order.
pub fn rrf_fuse(lists: &[&[(String, f64)]]) -> Vec<String> {
    rrf_rank(lists).into_iter().map(|(uuid, _)| uuid).collect()
}

/// Accumulates `1/(rank+60)` per uuid across `lists` (in list order) and sorts by descending
/// score with a UUID tie-break. Shared by [`rrf_fuse`] and [`rrf_fuse_scored`].
fn rrf_rank(lists: &[&[(String, f64)]]) -> Vec<(String, f64)> {
    rrf_rank_weighted(lists, &vec![1.0; lists.len()])
}

/// [`rrf_rank`] with a per-list weight: each list contributes `weight / (rank + 60)`. A weight of
/// `1.0` for every list is exactly [`rrf_rank`] (multiplying by `1.0` is exact in IEEE-754), which
/// is why the unweighted entity, edge and dedup callers share this implementation unchanged.
/// `weights` must have one entry per list.
fn rrf_rank_weighted(lists: &[&[(String, f64)]], weights: &[f64]) -> Vec<(String, f64)> {
    debug_assert_eq!(lists.len(), weights.len());
    let mut scores: HashMap<String, f64> = HashMap::new();

    for (list, weight) in lists.iter().zip(weights) {
        for (rank, (uuid, _)) in list.iter().enumerate() {
            *scores.entry(uuid.clone()).or_default() += weight / (rank as f64 + 60.0);
        }
    }

    let mut ranked: Vec<(String, f64)> = scores.into_iter().collect();
    // Descending score, UUID tie-break for determinism
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    ranked
}

/// One fused result with its evidence (issue #629): the fused RRF score and, per input list, the
/// raw value (BM25 score or cosine distance) if that list contained the item, else `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct FusedHit {
    pub uuid: String,
    pub rrf_score: f64,
    /// One slot per input list, in input order.
    pub raw: Vec<Option<f64>>,
}

/// [`rrf_fuse`] that also returns each item's fused score and per-list raw values. If a uuid
/// appears more than once in a list, its first occurrence supplies the raw value.
pub fn rrf_fuse_scored(lists: &[&[(String, f64)]]) -> Vec<FusedHit> {
    rrf_fuse_scored_weighted(lists, &vec![1.0; lists.len()])
}

/// [`rrf_fuse_scored`] with a per-list weight (see [`rrf_rank_weighted`]). Only hybrid passage
/// search uses non-unit weights; entity and edge search stay on the unweighted entry point.
pub fn rrf_fuse_scored_weighted(lists: &[&[(String, f64)]], weights: &[f64]) -> Vec<FusedHit> {
    let mut raws: HashMap<&str, Vec<Option<f64>>> = HashMap::new();
    for (i, list) in lists.iter().enumerate() {
        for (uuid, raw) in list.iter() {
            let slots = raws
                .entry(uuid.as_str())
                .or_insert_with(|| vec![None; lists.len()]);
            if slots[i].is_none() {
                slots[i] = Some(*raw);
            }
        }
    }
    rrf_rank_weighted(lists, weights)
        .into_iter()
        .map(|(uuid, rrf_score)| {
            let raw = raws.remove(uuid.as_str()).unwrap_or_default();
            FusedHit {
                uuid,
                rrf_score,
                raw,
            }
        })
        .collect()
}

/// Drops vector candidates whose cosine similarity (`1 - distance`) is below `floor`.
/// `None` keeps everything. Lists are distance-ascending, so this trims a tail and leaves the
/// surviving ranks unchanged. A NaN similarity fails the comparison and is dropped.
fn apply_similarity_floor(list: Vec<(String, f64)>, floor: Option<f64>) -> Vec<(String, f64)> {
    match floor {
        None => list,
        Some(f) => list.into_iter().filter(|(_, d)| 1.0 - d >= f).collect(),
    }
}

/// Cosine distance -> similarity, `None` when not finite (NaN is not valid JSON).
fn similarity(distance: Option<f64>) -> Option<f64> {
    distance.map(|d| 1.0 - d).filter(|s| s.is_finite())
}

/// A BM25 score for the evidence, `None` when absent or not finite.
fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

/// Reorders `rows` to follow `ranked_uuids`, the fused ranking they were fetched for.
///
/// The row lookups (`get_entities_by_uuids`, `get_relates_to_by_uuids`) are a Cypher
/// `WHERE uuid IN $uuids` with no `ORDER BY`, so they return rows in storage order. Without this
/// step every hybrid search returned the right top-k *set* in insertion order, silently
/// discarding the RRF ranking. Rows whose uuid is absent from the ranking (not expected) sort last.
fn order_by_rank<T>(
    mut rows: Vec<T>,
    ranked_uuids: &[String],
    uuid_of: impl Fn(&T) -> &str,
) -> Vec<T> {
    let rank: HashMap<&str, usize> = ranked_uuids
        .iter()
        .enumerate()
        .map(|(i, uuid)| (uuid.as_str(), i))
        .collect();
    rows.sort_by_key(|row| rank.get(uuid_of(row)).copied().unwrap_or(usize::MAX));
    rows
}

/// Pure vector-cosine passage search on Episodic nodes (HOT path — no lock held).
///
/// lbug HNSW returns distance (lower = closer); converts to similarity: `score = 1.0 - distance`.
/// Group filtering is pushed into the Cypher WHERE clause; results arrive ordered by score DESC.
pub async fn search_passages(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    query: &str,
    group_ids: Option<Vec<String>>,
    num_results: usize,
    min_score: f64,
    origins: Option<Vec<crate::origin::Origin>>,
) -> Result<Vec<PassageResult>, Error> {
    let embedding = embedder.embed(query, EmbedRole::Query).await?;
    let overdraw = num_results * 3;

    let results = tokio::task::spawn_blocking(move || -> Result<Vec<PassageResult>, Error> {
        let conn = db.connect()?;
        let gid_refs: Option<Vec<&str>> = group_ids
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());
        let mut passages = conn.vector_search_episodic_origins(
            &embedding,
            gid_refs.as_deref(),
            overdraw,
            origins.as_deref(),
        )?;

        for p in &mut passages {
            p.score = 1.0 - p.score;
        }

        passages.retain(|p| p.score >= min_score);
        passages.truncate(num_results);
        Ok(passages)
    })
    .await??;

    Ok(results)
}

// ── Hybrid passage search (issue #789) ────────────────────────────────────────

/// Retrieval mode for `knowledge_search_passages`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassageSearchMode {
    /// Pure vector cosine (the pre-#789 behaviour, unchanged).
    Vector,
    /// BM25 over episode text + vector, fused with weighted RRF, then belief-aware re-ranking.
    Hybrid,
}

impl PassageSearchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PassageSearchMode::Vector => "vector",
            PassageSearchMode::Hybrid => "hybrid",
        }
    }

    /// The service default when a request names no mode: `LCG_PASSAGE_SEARCH_MODE`, else
    /// `vector`. An unrecognised value warns on stderr (once per process) and falls back to
    /// `vector`. Read per request, so it needs no `AppState` field.
    pub fn default_from_env() -> Self {
        static WARNED: AtomicBool = AtomicBool::new(false);
        match std::env::var("LCG_PASSAGE_SEARCH_MODE") {
            Ok(v) if !v.trim().is_empty() => v.parse().unwrap_or_else(|_| {
                if !WARNED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "liminis-context-graph: LCG_PASSAGE_SEARCH_MODE={v:?} is not \"vector\" or \
                         \"hybrid\"; using vector"
                    );
                }
                PassageSearchMode::Vector
            }),
            _ => PassageSearchMode::Vector,
        }
    }
}

impl std::str::FromStr for PassageSearchMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "vector" => Ok(PassageSearchMode::Vector),
            "hybrid" => Ok(PassageSearchMode::Hybrid),
            other => Err(format!(
                "unknown passage search mode {other:?}; expected \"vector\" or \"hybrid\""
            )),
        }
    }
}

/// RRF weight of the lexical (BM25) list in hybrid passage search; the vector list has weight
/// `1.0`. Equal weights fuse two retrievers of unequal quality as if they were peers: on v2 the
/// vector leg is clearly stronger, and BM25's top hits on paraphrase questions are lexical decoys,
/// so unweighted RRF halved paraphrase nDCG (0.150 to 0.070, ADR-0789). A starting value in the
/// 0.3–0.5 range chosen by the maintainer; not yet measured, to be confirmed on the next v1/v2 run.
pub const LEXICAL_RRF_WEIGHT: f64 = 0.4;
// The lexical list must weigh something, and less than the (unit-weight) vector list.
const _: () = assert!(LEXICAL_RRF_WEIGHT > 0.0 && LEXICAL_RRF_WEIGHT < 1.0);

/// Maximum query tokens sent to the full-text index, bounding lexical work on very long queries.
const MAX_FTS_QUERY_TOKENS: usize = 64;

/// Reduces an arbitrary user query to plain alphanumeric tokens joined by spaces, so no
/// full-text query operator or special character ever reaches the index (FR-009). An empty
/// result means there is nothing to search lexically.
pub fn sanitize_fts_query(query: &str) -> String {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(MAX_FTS_QUERY_TOKENS)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Logs once per process that the lexical leg was unavailable and hybrid search fell back to
/// vector-only (FR-008) — the operator-visible signal.
fn note_lexical_unavailable(err: &Error) {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "liminis-context-graph: passage search: episode full-text index unavailable, \
             falling back to vector-only ranking: {err}"
        );
    }
}

/// The BM25 passage list (issue #789, FR-011): `(uuid, score)` best first, after the group and
/// origin filters, as a distinct reusable intermediate (graph-aware fusion, #797, consumes it).
/// Empty when the query has no lexical tokens, or when the index is missing or inconsistent
/// (FR-008) — the latter is logged once. Any other error propagates.
pub fn lexical_passage_list(
    conn: &crate::db::Conn,
    query: &str,
    group_ids: Option<&[&str]>,
    origins: Option<&[crate::origin::Origin]>,
    limit: usize,
) -> Result<Vec<(String, f64)>, Error> {
    let q = sanitize_fts_query(query);
    if q.is_empty() {
        return Ok(vec![]);
    }
    match conn.fts_search_episodic(&q, group_ids, origins, limit) {
        Ok(list) => Ok(list),
        Err(e) if crate::error::is_missing_index_error(&e) || is_fts_inconsistent_error(&e) => {
            note_lexical_unavailable(&e);
            Ok(vec![])
        }
        Err(e) => Err(e),
    }
}

// ── Belief-aware passage re-ranking (issue #789 steer, ADR-0789) ──────────────

/// What a passage's episode was superseded by (ADR-0705 / ADR-0707), when that is a thing that can
/// itself be ranked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Successor {
    /// A revision: the superseding episode's uuid.
    Episode(String),
    /// A document supersession: the superseding document's `doc_id`.
    Document(String),
}

/// The facts about one candidate passage that the belief-aware rules read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeliefFacts {
    pub uuid: String,
    pub group_id: String,
    /// Origin is `derived` or `answer` (not corroborating, ADR-0752).
    pub derived: bool,
    /// Origin is `primary` or `external` (corroborating, ADR-0752).
    pub corroborating: bool,
    pub doc_id: Option<String>,
    /// `[[wikilink]]` targets in the passage text: what a derived page cites.
    pub cites: Vec<String>,
    /// Set only when the episode's belief has ended (no longer live) *and* the supersession names
    /// a successor. A `chunk_removed` episode has none, so it is never demoted.
    pub successor: Option<Successor>,
}

/// `[[target]]`, `[[target|alias]]` and `[[target#heading]]` link targets in `content`, in order,
/// without duplicates. This is how a derived note records the source notes it restates.
pub fn wikilink_targets(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = content;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else { break };
        let inner = &after[..close];
        let target = inner.split('|').next().unwrap_or("");
        let target = target.split('#').next().unwrap_or("").trim();
        if !target.is_empty() && !out.iter().any(|t| t == target) {
            out.push(target.to_string());
        }
        rest = &after[close + 2..];
    }
    out
}

/// A document reference reduced to its comparable form: last path segment, without a `.md`
/// suffix, lower-cased (wikilinks name a note, not a path, and are case-insensitive).
fn doc_ref_key(r: &str) -> String {
    let leaf = r.rsplit('/').next().unwrap_or(r).trim();
    let leaf = leaf.strip_suffix(".md").unwrap_or(leaf);
    leaf.to_lowercase()
}

/// Whether a wikilink `target` names the document `doc_id`.
fn doc_ref_matches(target: &str, doc_id: &str) -> bool {
    target == doc_id || doc_ref_key(target) == doc_ref_key(doc_id)
}

/// For each candidate that must rank below something, the *groups* it must rank below. A group is
/// the set of candidate passages that stand for one thing (the chunks of one cited note, or of the
/// superseding document); the candidate must rank below the best-ranked member of **every** group
/// that has any member in the candidate set. Groups with no member present are dropped, so a
/// demotion never depends on a passage that was not retrieved.
///
/// - **Derived rule (ADR-0752):** a `derived`/`answer` passage ranks below the `primary`/`external`
///   note it cites by `[[wikilink]]`, matched on `doc_id`, within its group.
/// - **Stale rule (ADR-0705/0707):** a passage whose episode is no longer believed ranks below its
///   successor (the revising episode, or the superseding document's passages).
pub fn belief_anchor_groups(facts: &[BeliefFacts]) -> HashMap<String, Vec<Vec<String>>> {
    let mut out: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    for p in facts {
        let mut groups: Vec<Vec<String>> = Vec::new();
        if p.derived {
            for target in &p.cites {
                let members: Vec<String> = facts
                    .iter()
                    .filter(|q| {
                        q.uuid != p.uuid
                            && q.corroborating
                            && q.group_id == p.group_id
                            && q.doc_id
                                .as_deref()
                                .is_some_and(|d| doc_ref_matches(target, d))
                    })
                    .map(|q| q.uuid.clone())
                    .collect();
                if !members.is_empty() && !groups.contains(&members) {
                    groups.push(members);
                }
            }
        }
        if let Some(succ) = &p.successor {
            let members: Vec<String> = facts
                .iter()
                .filter(|q| {
                    q.uuid != p.uuid
                        && match succ {
                            Successor::Episode(u) => &q.uuid == u,
                            Successor::Document(d) => {
                                q.group_id == p.group_id && q.doc_id.as_deref() == Some(d.as_str())
                            }
                        }
                })
                .map(|q| q.uuid.clone())
                .collect();
            if !members.is_empty() && !groups.contains(&members) {
                groups.push(members);
            }
        }
        if !groups.is_empty() {
            out.insert(p.uuid.clone(), groups);
        }
    }
    out
}

/// Lowers each anchored passage's score to just below its anchors' best (see
/// [`belief_anchor_groups`]) and re-sorts by descending score, UUID tie-break. Only anchored
/// passages move, and only downwards; the demoted score stays strictly below the anchor so the
/// returned scores remain in rank order. Iterates to a fixed point so chains (a derived page that
/// is also stale) settle, bounded by the candidate count so a cyclic input still terminates.
pub fn demote_below_anchors(
    ranked: &mut [(String, f64)],
    anchors: &HashMap<String, Vec<Vec<String>>>,
) {
    if anchors.is_empty() {
        return;
    }
    for _ in 0..=ranked.len() {
        let snapshot: HashMap<String, f64> = ranked.iter().cloned().collect();
        let mut changed = false;
        for (uuid, score) in ranked.iter_mut() {
            let Some(groups) = anchors.get(uuid.as_str()) else {
                continue;
            };
            let mut bound: Option<f64> = None;
            for group in groups {
                let best = group
                    .iter()
                    .filter_map(|u| snapshot.get(u.as_str()).copied())
                    .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |x| x.max(v))));
                if let Some(b) = best {
                    bound = Some(bound.map_or(b, |x| x.min(b)));
                }
            }
            if let Some(b) = bound {
                if *score >= b {
                    let below = b - (b.abs() * 1e-9).max(1e-15);
                    *score = below;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
}

/// Builds [`BeliefFacts`] for `rows`, given each row's supersession state and the instant `now_us`
/// that decides whether belief has ended (a pending document supersession with a future
/// `superseded_at` is still live, ADR-0707).
fn belief_facts(
    rows: &[PassageResult],
    supersession: &HashMap<String, crate::belief::EpisodeSupersession>,
    now_us: i64,
) -> Vec<BeliefFacts> {
    rows.iter()
        .map(|r| {
            let origin = crate::origin::Origin::from_stored(Some(&r.origin));
            let sup = supersession.get(&r.uuid);
            let successor = sup.filter(|s| !s.live_at(now_us)).and_then(|s| {
                let by = s.superseded_by.clone()?;
                match s.kind.as_deref() {
                    Some("episode") => Some(Successor::Episode(by)),
                    Some("document") => Some(Successor::Document(by)),
                    _ => None,
                }
            });
            BeliefFacts {
                uuid: r.uuid.clone(),
                group_id: r.group_id.clone(),
                derived: !origin.is_corroborating(),
                corroborating: origin.is_corroborating(),
                doc_id: sup.and_then(|s| s.doc_id.clone()),
                cites: wikilink_targets(&r.content),
                successor,
            }
        })
        .collect()
}

/// Applies the belief-aware rules to `rows`, which are in rank order with `score` set, and returns
/// them re-ordered (and with demoted scores). One batched supersession lookup; a no-op, without
/// the lookup's cost mattering, when nothing is anchored.
fn apply_belief_ranking(
    conn: &crate::db::Conn,
    rows: Vec<PassageResult>,
) -> Result<Vec<PassageResult>, Error> {
    if rows.len() < 2 {
        return Ok(rows);
    }
    let uuids: Vec<String> = rows.iter().map(|r| r.uuid.clone()).collect();
    let supersession = conn.get_episode_supersession(&uuids)?;
    let facts = belief_facts(&rows, &supersession, chrono::Utc::now().timestamp_micros());
    let anchors = belief_anchor_groups(&facts);
    if anchors.is_empty() {
        return Ok(rows);
    }
    let mut ranked: Vec<(String, f64)> = rows.iter().map(|r| (r.uuid.clone(), r.score)).collect();
    demote_below_anchors(&mut ranked, &anchors);
    let mut by_uuid: HashMap<String, PassageResult> =
        rows.into_iter().map(|r| (r.uuid.clone(), r)).collect();
    Ok(ranked
        .into_iter()
        .filter_map(|(uuid, score)| {
            let mut r = by_uuid.remove(&uuid)?;
            r.score = score;
            Some(r)
        })
        .collect())
}

/// Hybrid passage search (issue #789): BM25 over episode text + HNSW, fused with weighted RRF
/// (the lexical list weighs [`LEXICAL_RRF_WEIGHT`], the vector list `1.0`), optionally followed by
/// belief-aware re-ranking (HOT path — no lock held).
///
/// - `min_score` floors the *vector* list's cosine similarity before fusion; lexical-only hits
///   are never floored (FR-007).
/// - `score` on each result is the fused score; `similarity` is the cosine where the vector
///   retriever matched.
/// - With no lexical hits the result is the vector ranking (FR-008), re-scored as fused.
/// - With `belief_aware`, the rules of [`belief_anchor_groups`] run on the fused candidate set
///   (up to `3 × num_results`) *before* truncating, so a demoted passage is replaced by the next
///   candidate rather than merely reshuffled.
#[allow(clippy::too_many_arguments)]
pub async fn search_passages_hybrid(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    query: &str,
    group_ids: Option<Vec<String>>,
    num_results: usize,
    min_score: f64,
    origins: Option<Vec<crate::origin::Origin>>,
    belief_aware: bool,
) -> Result<Vec<PassageResult>, Error> {
    let embedding = embedder.embed(query, EmbedRole::Query).await?;
    let candidate_limit = num_results * 3;
    let query_owned = query.to_string();

    tokio::task::spawn_blocking(move || -> Result<Vec<PassageResult>, Error> {
        let conn = db.connect()?;
        let gid_refs: Option<Vec<&str>> = group_ids
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());

        let bm25 = lexical_passage_list(
            &conn,
            &query_owned,
            gid_refs.as_deref(),
            origins.as_deref(),
            candidate_limit,
        )?;

        let vector_rows = conn.vector_search_episodic_origins(
            &embedding,
            gid_refs.as_deref(),
            candidate_limit,
            origins.as_deref(),
        )?;
        let vector = apply_similarity_floor(
            vector_rows
                .iter()
                .map(|p| (p.uuid.clone(), p.score))
                .collect(),
            Some(min_score),
        );

        let mut fused = rrf_fuse_scored_weighted(&[&bm25, &vector], &[LEXICAL_RRF_WEIGHT, 1.0]);
        // Belief-aware ranking needs the whole candidate set, not just the top `num_results`.
        fused.truncate(if belief_aware {
            candidate_limit
        } else {
            num_results
        });

        let mut rows: HashMap<String, PassageResult> = vector_rows
            .into_iter()
            .map(|p| (p.uuid.clone(), p))
            .collect();
        let missing: Vec<String> = fused
            .iter()
            .filter(|h| !rows.contains_key(&h.uuid))
            .map(|h| h.uuid.clone())
            .collect();
        for p in conn.get_passages_by_uuids(&missing)? {
            rows.insert(p.uuid.clone(), p);
        }

        // `fused` is already in rank order; rows are looked up, not re-sorted, so the order is
        // exactly the fused (deterministic) order.
        let mut ranked: Vec<PassageResult> = fused
            .into_iter()
            .filter_map(|hit| {
                let mut p = rows.remove(&hit.uuid)?;
                p.score = hit.rrf_score;
                p.similarity = similarity(hit.raw[1]);
                Some(p)
            })
            .collect();
        if belief_aware {
            ranked = apply_belief_ranking(&conn, ranked)?;
        }
        ranked.truncate(num_results);
        Ok(ranked)
    })
    .await?
}

/// Vector passage search followed by belief-aware re-ranking: the opt-in (`mode: "vector"` with
/// `belief_aware: true`) variant of [`search_passages`], which is left untouched so the vector
/// default stays byte-for-byte unchanged. `score` stays a cosine similarity, except that a
/// demoted passage's score is lowered to just below the passage it ranks under.
pub async fn search_passages_vector_belief(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    query: &str,
    group_ids: Option<Vec<String>>,
    num_results: usize,
    min_score: f64,
    origins: Option<Vec<crate::origin::Origin>>,
) -> Result<Vec<PassageResult>, Error> {
    let embedding = embedder.embed(query, EmbedRole::Query).await?;
    let overdraw = num_results * 3;

    tokio::task::spawn_blocking(move || -> Result<Vec<PassageResult>, Error> {
        let conn = db.connect()?;
        let gid_refs: Option<Vec<&str>> = group_ids
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());
        let mut passages = conn.vector_search_episodic_origins(
            &embedding,
            gid_refs.as_deref(),
            overdraw,
            origins.as_deref(),
        )?;
        for p in &mut passages {
            p.score = 1.0 - p.score;
        }
        passages.retain(|p| p.score >= min_score);
        let mut ranked = apply_belief_ranking(&conn, passages)?;
        ranked.truncate(num_results);
        Ok(ranked)
    })
    .await?
}

/// Hybrid BM25 + HNSW entity search with RRF fusion (HOT path).
/// `group_ids: None` searches across every group; `Some(vec![])` is a real filter and matches no
/// groups.
pub async fn hybrid_entity_search(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    query: &str,
    group_ids: Option<Vec<String>>,
    limit: usize,
) -> Result<Vec<EntityRow>, Error> {
    hybrid_entity_search_kind(db, embedder, query, group_ids, limit, None).await
}

/// [`hybrid_entity_search`] with an optional `kind` filter (issue #615): `Some(kind)` keeps only
/// entities of that kind; `None` is all kinds. The kind predicate is pushed into each candidate
/// query (BM25, name-vector, summary-vector) before the `limit * 3` cap. For BM25 the predicate
/// runs inside the FTS query, so a matching entity is not crowded out by rows of other kinds.
/// For the two vector queries it runs after the nearest-neighbour probe, whose size is
/// oversampled ([`crate::db`]'s `KIND_ANN_OVERSAMPLE`); that mitigates but does not guarantee
/// recall of a rare kind ranked below the oversampled probe. Every returned row carries its
/// `kind`.
pub async fn hybrid_entity_search_kind(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    query: &str,
    group_ids: Option<Vec<String>>,
    limit: usize,
    kind: Option<String>,
) -> Result<Vec<EntityRow>, Error> {
    let scored =
        hybrid_entity_search_scored(db, embedder, query, group_ids, limit, kind, None).await?;
    Ok(scored.into_iter().map(|s| s.entity).collect())
}

/// [`hybrid_entity_search_kind`] returning per-result [`EntitySearchEvidence`] and accepting an
/// optional `min_similarity` floor (issue #629).
///
/// The floor is applied to each vector candidate list (name, summary) *before* fusion, so
/// below-floor neighbours never enter the ranking; BM25 candidates are never filtered. With
/// `min_similarity: None` the results and order are identical to [`hybrid_entity_search_kind`].
pub async fn hybrid_entity_search_scored(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    query: &str,
    group_ids: Option<Vec<String>>,
    limit: usize,
    kind: Option<String>,
    min_similarity: Option<f64>,
) -> Result<Vec<ScoredEntity>, Error> {
    // Async: embed the query
    let embedding = embedder.embed(query, EmbedRole::Query).await?;

    // Sync: DB operations in spawn_blocking
    let query_owned = query.to_string();
    let results = tokio::task::spawn_blocking(move || -> Result<Vec<ScoredEntity>, Error> {
        let conn = db.connect()?;
        let gid_refs: Option<Vec<&str>> = group_ids
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());
        let kind_ref = kind.as_deref();
        let candidate_limit = limit * 3;

        let bm25 = conn.fts_search_entities_kind(
            &query_owned,
            gid_refs.as_deref(),
            kind_ref,
            candidate_limit,
        )?;
        let vector = apply_similarity_floor(
            conn.vector_search_entities_kind(
                &embedding,
                gid_refs.as_deref(),
                kind_ref,
                candidate_limit,
            )?,
            min_similarity,
        );
        // Third RRF input (issue #470): summary-vector matches, so a query that paraphrases an
        // entity's `summary` — sharing no vocabulary with it (no FTS match) and no similarity to
        // `name` (no name-vector match) — is still retrieved via meaning-based similarity.
        let vector_summary = apply_similarity_floor(
            conn.vector_search_entities_by_summary_kind(
                &embedding,
                gid_refs.as_deref(),
                kind_ref,
                candidate_limit,
            )?,
            min_similarity,
        );

        let fused: Vec<FusedHit> = rrf_fuse_scored(&[&bm25, &vector, &vector_summary])
            .into_iter()
            .take(limit)
            .collect();
        let top_uuids: Vec<String> = fused.iter().map(|h| h.uuid.clone()).collect();
        let rows = order_by_rank(conn.get_entities_by_uuids(&top_uuids)?, &top_uuids, |e| {
            &e.uuid
        });
        let mut evidence: HashMap<String, FusedHit> =
            fused.into_iter().map(|h| (h.uuid.clone(), h)).collect();
        Ok(rows
            .into_iter()
            .filter_map(|entity| {
                let hit = evidence.remove(&entity.uuid)?;
                let bm25_score = finite(hit.raw[0]);
                let search = EntitySearchEvidence {
                    rrf_score: hit.rrf_score,
                    text_match: hit.raw[0].is_some(),
                    bm25_score,
                    name_similarity: similarity(hit.raw[1]),
                    summary_similarity: similarity(hit.raw[2]),
                };
                Some(ScoredEntity { entity, search })
            })
            .collect())
    })
    .await??;

    Ok(results)
}

/// Hybrid BM25 + HNSW edge (fact) search with RRF fusion (HOT path).
/// `group_ids: None` searches across every group; `Some(vec![])` is a real filter and matches no
/// groups.
pub async fn hybrid_edge_search(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    query: &str,
    group_ids: Option<Vec<String>>,
    limit: usize,
) -> Result<Vec<RelatesToEdge>, Error> {
    let scored = hybrid_edge_search_scored(db, embedder, query, group_ids, limit, None).await?;
    Ok(scored.into_iter().map(|s| s.edge).collect())
}

/// [`hybrid_edge_search`] returning per-result [`EdgeSearchEvidence`] and accepting an optional
/// `min_similarity` floor on the fact-vector candidates (issue #629). BM25 is never filtered;
/// `None` reproduces [`hybrid_edge_search`] exactly.
pub async fn hybrid_edge_search_scored(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    query: &str,
    group_ids: Option<Vec<String>>,
    limit: usize,
    min_similarity: Option<f64>,
) -> Result<Vec<ScoredEdge>, Error> {
    let embedding = embedder.embed(query, EmbedRole::Query).await?;

    let query_owned = query.to_string();
    let results = tokio::task::spawn_blocking(move || -> Result<Vec<ScoredEdge>, Error> {
        let conn = db.connect()?;
        let gid_refs: Option<Vec<&str>> = group_ids
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());
        let candidate_limit = limit * 3;

        let bm25 = conn.fts_search_edges(&query_owned, gid_refs.as_deref(), candidate_limit)?;
        let vector = apply_similarity_floor(
            conn.vector_search_edges(&embedding, gid_refs.as_deref(), candidate_limit)?,
            min_similarity,
        );

        let fused: Vec<FusedHit> = rrf_fuse_scored(&[&bm25, &vector])
            .into_iter()
            .take(limit)
            .collect();
        let top_uuids: Vec<String> = fused.iter().map(|h| h.uuid.clone()).collect();
        let rows = order_by_rank(conn.get_relates_to_by_uuids(&top_uuids)?, &top_uuids, |e| {
            &e.uuid
        });
        let mut evidence: HashMap<String, FusedHit> =
            fused.into_iter().map(|h| (h.uuid.clone(), h)).collect();
        Ok(rows
            .into_iter()
            .filter_map(|edge| {
                let hit = evidence.remove(&edge.uuid)?;
                let search = EdgeSearchEvidence {
                    rrf_score: hit.rrf_score,
                    text_match: hit.raw[0].is_some(),
                    bm25_score: finite(hit.raw[0]),
                    fact_similarity: similarity(hit.raw[1]),
                };
                Some(ScoredEdge { edge, search })
            })
            .collect())
    })
    .await??;

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_fts_query_strips_operators_and_punctuation() {
        assert_eq!(sanitize_fts_query("shift rostering"), "shift rostering");
        assert_eq!(
            sanitize_fts_query("a OR (b* AND \"c\") title:d"),
            "a OR b AND c title d"
        );
        assert_eq!(sanitize_fts_query("?!... --- ::"), "");
        assert_eq!(sanitize_fts_query("   "), "");
        assert_eq!(sanitize_fts_query("café-déjà vu"), "café déjà vu");
    }

    #[test]
    fn sanitize_fts_query_caps_token_count() {
        let long = (0..200)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            sanitize_fts_query(&long).split(' ').count(),
            MAX_FTS_QUERY_TOKENS
        );
    }

    #[test]
    fn passage_search_mode_parses_strictly() {
        assert_eq!(
            "vector".parse::<PassageSearchMode>(),
            Ok(PassageSearchMode::Vector)
        );
        assert_eq!(
            "hybrid".parse::<PassageSearchMode>(),
            Ok(PassageSearchMode::Hybrid)
        );
        assert!("Hybrid".parse::<PassageSearchMode>().is_err());
        assert!("".parse::<PassageSearchMode>().is_err());
        assert!("bm25".parse::<PassageSearchMode>().is_err());
        assert_eq!(PassageSearchMode::Hybrid.as_str(), "hybrid");
    }

    fn list(ids: &[&str]) -> Vec<(String, f64)> {
        ids.iter().map(|u| (u.to_string(), 0.0)).collect()
    }

    fn order(hits: &[FusedHit]) -> Vec<&str> {
        hits.iter().map(|h| h.uuid.as_str()).collect()
    }

    #[test]
    fn unit_weights_reproduce_unweighted_fusion_exactly() {
        let bm25 = list(&["a", "b", "c"]);
        let vector = list(&["c", "d", "a"]);
        assert_eq!(
            rrf_fuse_scored_weighted(&[&bm25, &vector], &[1.0, 1.0]),
            rrf_fuse_scored(&[&bm25, &vector])
        );
    }

    #[test]
    fn lexical_weight_keeps_a_bm25_decoy_below_the_top_vector_hit() {
        // Both are rank 1 in their own list only. Unweighted they tie on score, and the UUID
        // tie-break puts the BM25 decoy first; weighted, the vector hit wins on merit.
        let bm25 = list(&["decoy"]);
        let vector = list(&["good"]);
        let unweighted = rrf_fuse_scored(&[&bm25, &vector]);
        assert_eq!(order(&unweighted), vec!["decoy", "good"]);
        let weighted = rrf_fuse_scored_weighted(&[&bm25, &vector], &[LEXICAL_RRF_WEIGHT, 1.0]);
        assert_eq!(order(&weighted), vec!["good", "decoy"]);
        assert!((weighted[1].rrf_score - LEXICAL_RRF_WEIGHT / 60.0).abs() < 1e-12);
    }

    #[test]
    fn lexical_weight_still_rewards_agreement_between_the_lists() {
        // `x` is rank 1 in BM25 and rank 4 in the vector list: with the lexical weight it still
        // outranks a vector-only rank-1 hit, so BM25 helps when it agrees with the vector leg.
        let bm25 = list(&["x"]);
        let vector = list(&["g", "v2", "v3", "x"]);
        let hits = rrf_fuse_scored_weighted(&[&bm25, &vector], &[LEXICAL_RRF_WEIGHT, 1.0]);
        assert_eq!(hits[0].uuid, "x");
    }

    // ── belief-aware re-ranking ──

    fn facts(
        uuid: &str,
        derived: bool,
        doc_id: Option<&str>,
        cites: &[&str],
        successor: Option<Successor>,
    ) -> BeliefFacts {
        BeliefFacts {
            uuid: uuid.to_string(),
            group_id: "g".to_string(),
            derived,
            corroborating: !derived,
            doc_id: doc_id.map(str::to_string),
            cites: cites.iter().map(|c| c.to_string()).collect(),
            successor,
        }
    }

    fn ranked(items: &[(&str, f64)]) -> Vec<(String, f64)> {
        items.iter().map(|(u, s)| (u.to_string(), *s)).collect()
    }

    fn uuids(r: &[(String, f64)]) -> Vec<&str> {
        r.iter().map(|(u, _)| u.as_str()).collect()
    }

    #[test]
    fn wikilink_targets_strip_alias_heading_and_duplicates() {
        let t = wikilink_targets("See [[note-a]], [[Note B|the b note]], [[note-c#Plan]] and [[note-a]]. [[ ]] [[unclosed");
        assert_eq!(t, vec!["note-a", "Note B", "note-c"]);
        assert!(wikilink_targets("no links here").is_empty());
    }

    #[test]
    fn doc_ref_matches_ids_paths_and_case() {
        assert!(doc_ref_matches("note-a", "note-a"));
        assert!(doc_ref_matches("Note-A", "notes/note-a.md"));
        assert!(doc_ref_matches("notes/note-a", "note-a"));
        assert!(!doc_ref_matches("note-b", "note-a"));
    }

    #[test]
    fn derived_page_ranks_below_the_primary_it_cites_even_when_bm25_put_it_first() {
        // The restating page repeats the fact's wording, so BM25 (and the fusion) puts it first.
        let f = vec![
            facts("page", true, Some("page"), &["src"], None),
            facts("src", false, Some("src"), &[], None),
            facts("other", false, Some("other"), &[], None),
        ];
        let mut r = ranked(&[("page", 0.030), ("src", 0.025), ("other", 0.020)]);
        demote_below_anchors(&mut r, &belief_anchor_groups(&f));
        assert_eq!(uuids(&r), vec!["src", "page", "other"]);
        assert!(
            r[1].1 < r[0].1 && r[1].1 > r[2].1,
            "scores stay in rank order: {r:?}"
        );
    }

    #[test]
    fn derived_page_already_below_its_source_is_untouched() {
        let f = vec![
            facts("src", false, Some("src"), &[], None),
            facts("page", true, Some("page"), &["src"], None),
        ];
        let mut r = ranked(&[("src", 0.030), ("page", 0.025)]);
        let before = r.clone();
        demote_below_anchors(&mut r, &belief_anchor_groups(&f));
        assert_eq!(r, before);
    }

    #[test]
    fn derived_page_is_not_demoted_when_its_source_is_not_in_the_candidates() {
        let f = vec![facts("page", true, Some("page"), &["src"], None)];
        assert!(belief_anchor_groups(&f).is_empty());
        // A cited note in another group, or an uncorroborating (derived) one, is no anchor.
        let mut other_group = facts("src", false, Some("src"), &[], None);
        other_group.group_id = "elsewhere".to_string();
        let f = vec![
            facts("page", true, Some("page"), &["src"], None),
            other_group,
        ];
        assert!(belief_anchor_groups(&f).is_empty());
        let f = vec![
            facts("page", true, Some("page"), &["src"], None),
            facts("src", true, Some("src"), &[], None),
        ];
        assert!(belief_anchor_groups(&f).is_empty());
    }

    #[test]
    fn a_primary_passage_is_never_demoted_for_citing() {
        // Only derived/answer origin is subject to the derived rule.
        let f = vec![
            facts("a", false, Some("a"), &["b"], None),
            facts("b", false, Some("b"), &[], None),
        ];
        assert!(belief_anchor_groups(&f).is_empty());
    }

    #[test]
    fn derived_page_goes_below_the_best_chunk_of_each_cited_note() {
        // `src` has two chunks present; the page must end up under the better one, not the worse.
        let f = vec![
            facts("page", true, Some("page"), &["src", "src2"], None),
            facts("src#1", false, Some("src"), &[], None),
            facts("src#0", false, Some("src"), &[], None),
            facts("src2#0", false, Some("src2"), &[], None),
        ];
        let mut r = ranked(&[
            ("page", 0.040),
            ("src#1", 0.030),
            ("src#0", 0.020),
            ("src2#0", 0.010),
        ]);
        demote_below_anchors(&mut r, &belief_anchor_groups(&f));
        // Below src2's only chunk (the lowest "best chunk"), so below every cited note.
        assert_eq!(uuids(&r), vec!["src#1", "src#0", "src2#0", "page"]);
    }

    #[test]
    fn stale_passage_ranks_below_its_revising_episode() {
        let f = vec![
            facts(
                "old",
                false,
                Some("n"),
                &[],
                Some(Successor::Episode("new".into())),
            ),
            facts("new", false, Some("n"), &[], None),
        ];
        let mut r = ranked(&[("old", 0.030), ("new", 0.020)]);
        demote_below_anchors(&mut r, &belief_anchor_groups(&f));
        assert_eq!(uuids(&r), vec!["new", "old"]);
    }

    #[test]
    fn stale_passage_ranks_below_the_superseding_document() {
        let f = vec![
            facts(
                "old#0",
                false,
                Some("old"),
                &[],
                Some(Successor::Document("new".into())),
            ),
            facts("new#0", false, Some("new"), &[], None),
            facts("new#1", false, Some("new"), &[], None),
            facts("unrelated", false, Some("u"), &[], None),
        ];
        let mut r = ranked(&[
            ("old#0", 0.050),
            ("unrelated", 0.040),
            ("new#1", 0.030),
            ("new#0", 0.020),
        ]);
        demote_below_anchors(&mut r, &belief_anchor_groups(&f));
        // Below the best chunk of the new document (new#1), still above what ranked under it.
        assert_eq!(uuids(&r), vec!["unrelated", "new#1", "old#0", "new#0"]);
    }

    #[test]
    fn stale_passage_with_no_successor_in_the_candidates_or_none_at_all_stays() {
        // chunk_removed: belief ended, nothing replaced it, so there is nothing to rank below.
        let f = vec![facts("gone", false, Some("n"), &[], None)];
        assert!(belief_anchor_groups(&f).is_empty());
        let f = vec![facts(
            "old",
            false,
            Some("n"),
            &[],
            Some(Successor::Episode("absent".into())),
        )];
        assert!(belief_anchor_groups(&f).is_empty());
    }

    #[test]
    fn a_derived_and_stale_passage_settles_below_both_anchors() {
        let f = vec![
            facts(
                "page",
                true,
                Some("page"),
                &["src"],
                Some(Successor::Episode("page2".into())),
            ),
            facts("src", false, Some("src"), &[], None),
            facts("page2", true, Some("page"), &["src"], None),
        ];
        let mut r = ranked(&[("page", 0.050), ("page2", 0.040), ("src", 0.030)]);
        demote_below_anchors(&mut r, &belief_anchor_groups(&f));
        // page2 must fall under src, and page under both.
        assert_eq!(uuids(&r), vec!["src", "page2", "page"]);
    }

    #[test]
    fn demotion_terminates_on_cyclic_anchors() {
        let anchors: HashMap<String, Vec<Vec<String>>> = HashMap::from([
            ("a".to_string(), vec![vec!["b".to_string()]]),
            ("b".to_string(), vec![vec!["a".to_string()]]),
        ]);
        let mut r = ranked(&[("a", 0.02), ("b", 0.01)]);
        demote_below_anchors(&mut r, &anchors);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn demotion_does_not_touch_unanchored_scores_and_is_deterministic() {
        let f = vec![
            facts("page", true, Some("page"), &["src"], None),
            facts("src", false, Some("src"), &[], None),
            facts("x", false, Some("x"), &[], None),
        ];
        let anchors = belief_anchor_groups(&f);
        let mut r1 = ranked(&[("page", 0.030), ("src", 0.025), ("x", 0.020)]);
        let mut r2 = r1.clone();
        demote_below_anchors(&mut r1, &anchors);
        demote_below_anchors(&mut r2, &anchors);
        assert_eq!(r1, r2);
        let score = |r: &[(String, f64)], u: &str| r.iter().find(|(x, _)| x == u).unwrap().1;
        assert_eq!(score(&r1, "src"), 0.025);
        assert_eq!(score(&r1, "x"), 0.020);
    }

    #[test]
    fn belief_facts_treat_a_pending_document_supersession_as_still_live() {
        let row = PassageResult {
            uuid: "old".into(),
            name: "old".into(),
            content: "text".into(),
            source_description: String::new(),
            group_id: "g".into(),
            created_at: String::new(),
            valid_at: None,
            score: 0.0,
            attributes: "{}".into(),
            origin: "primary".into(),
            similarity: None,
        };
        let mk = |superseded_at_us| crate::belief::EpisodeSupersession {
            superseded_at_us,
            kind: Some("document".into()),
            superseded_by: Some("new".into()),
            reason: Some("document_superseded".into()),
            group_id: "g".into(),
            doc_id: Some("old".into()),
            ..Default::default()
        };
        let now = 1_000_000;
        let live = HashMap::from([("old".to_string(), mk(Some(now + 5)))]);
        let ended = HashMap::from([("old".to_string(), mk(Some(now - 5)))]);
        let rows = std::slice::from_ref(&row);
        assert_eq!(belief_facts(rows, &live, now)[0].successor, None);
        assert_eq!(
            belief_facts(rows, &ended, now)[0].successor,
            Some(Successor::Document("new".into()))
        );
    }

    #[test]
    fn fusion_of_overlapping_lists_has_each_passage_once() {
        let bm25 = list(&["a", "b"]);
        let vector = list(&["b", "c"]);
        let hits = rrf_fuse_scored(&[&bm25, &vector]);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].uuid, "b");
    }

    #[test]
    fn order_by_rank_follows_the_fused_ranking_not_fetch_order() {
        // Rows as a storage-order `WHERE uuid IN` lookup might return them.
        let fetched = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let ranked = vec![
            "c".to_string(),
            "a".to_string(),
            "d".to_string(),
            "b".to_string(),
        ];
        let ordered = order_by_rank(fetched, &ranked, |s| s.as_str());
        assert_eq!(ordered, ranked);
    }

    #[test]
    fn order_by_rank_puts_unranked_rows_last() {
        let fetched = vec!["x".to_string(), "b".to_string(), "a".to_string()];
        let ranked = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            order_by_rank(fetched, &ranked, |s| s.as_str()),
            vec!["a", "b", "x"]
        );
    }

    #[test]
    fn test_rrf_fuse_empty() {
        let empty: &[(String, f64)] = &[];
        let result = rrf_fuse(&[empty, empty]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_rrf_fuse_single_list() {
        let bm25 = vec![
            ("a".to_string(), 1.0),
            ("b".to_string(), 0.8),
            ("c".to_string(), 0.5),
        ];
        let empty: &[(String, f64)] = &[];
        let result = rrf_fuse(&[&bm25, empty]);
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_rrf_fuse_overlap_boosts() {
        // 'b' appears in both lists at rank 0 → should score highest
        let bm25 = vec![("a".to_string(), 1.0), ("b".to_string(), 0.9)];
        let vector = vec![("b".to_string(), 0.1), ("c".to_string(), 0.05)];
        let result = rrf_fuse(&[&bm25, &vector]);
        assert_eq!(result[0], "b", "overlapping entry should rank first");
    }

    #[test]
    fn test_rrf_fuse_deterministic_tie_break() {
        // Two entries with identical scores → sorted by UUID alphabetically
        let bm25 = vec![("z".to_string(), 1.0), ("a".to_string(), 1.0)];
        let vector = vec![("a".to_string(), 1.0), ("z".to_string(), 1.0)];
        let result = rrf_fuse(&[&bm25, &vector]);
        // Both have same rrf score; UUID tie-break gives "a" < "z"
        assert_eq!(result[0], "a");
        assert_eq!(result[1], "z");
    }

    #[test]
    fn test_rrf_fuse_three_lists() {
        // 'b' appears in all three lists at rank 0 → should score highest; a third list alone
        // contributing an entry ('d') must still surface it (issue #470: summary-vector list).
        let bm25 = vec![("a".to_string(), 1.0), ("b".to_string(), 0.9)];
        let vector = vec![("b".to_string(), 0.1), ("c".to_string(), 0.05)];
        let vector_summary = vec![("b".to_string(), 0.2), ("d".to_string(), 0.05)];
        let result = rrf_fuse(&[&bm25, &vector, &vector_summary]);
        assert_eq!(
            result[0], "b",
            "entry present in all three lists should rank first"
        );
        assert!(
            result.contains(&"d".to_string()),
            "an entry found only via the third (summary-vector) list must still surface: {result:?}"
        );
    }

    fn l(items: &[(&str, f64)]) -> Vec<(String, f64)> {
        items.iter().map(|(u, v)| (u.to_string(), *v)).collect()
    }

    #[test]
    fn rrf_fuse_scored_matches_rrf_fuse_order() {
        let bm25 = l(&[("z", 3.0), ("a", 2.0), ("m", 1.0)]);
        let vector = l(&[("a", 0.1), ("q", 0.2), ("z", 0.3)]);
        let summary = l(&[("q", 0.4), ("a", 0.5)]);
        let order = rrf_fuse(&[&bm25, &vector, &summary]);
        let scored: Vec<String> = rrf_fuse_scored(&[&bm25, &vector, &summary])
            .into_iter()
            .map(|h| h.uuid)
            .collect();
        assert_eq!(order, scored);
    }

    #[test]
    fn rrf_fuse_scored_carries_raw_values_and_none_for_absent_lists() {
        let bm25 = l(&[("a", 2.5), ("b", 1.0)]);
        let vector = l(&[("b", 0.25), ("c", 0.5)]);
        let hits = rrf_fuse_scored(&[&bm25, &vector]);
        let by = |u: &str| hits.iter().find(|h| h.uuid == u).unwrap().clone();
        assert_eq!(by("a").raw, vec![Some(2.5), None]);
        assert_eq!(by("b").raw, vec![Some(1.0), Some(0.25)]);
        assert_eq!(by("c").raw, vec![None, Some(0.5)]);
    }

    #[test]
    fn rrf_fuse_scored_score_is_summed_reciprocal_rank() {
        let bm25 = l(&[("a", 1.0), ("b", 1.0)]);
        let vector = l(&[("b", 0.1)]);
        let hits = rrf_fuse_scored(&[&bm25, &vector]);
        let b = hits.iter().find(|h| h.uuid == "b").unwrap();
        assert_eq!(b.rrf_score, 1.0 / 61.0 + 1.0 / 60.0);
        assert_eq!(hits[0].uuid, "b");
    }

    #[test]
    fn rrf_fuse_scored_first_duplicate_wins() {
        let bm25 = l(&[("a", 2.0), ("a", 9.0)]);
        let hits = rrf_fuse_scored(&[&bm25]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].raw, vec![Some(2.0)]);
    }

    #[test]
    fn rrf_fuse_scored_tie_break_matches_rrf_fuse() {
        let bm25 = l(&[("z", 1.0), ("a", 1.0)]);
        let vector = l(&[("a", 1.0), ("z", 1.0)]);
        let hits = rrf_fuse_scored(&[&bm25, &vector]);
        assert_eq!(hits[0].uuid, "a");
        assert_eq!(hits[1].uuid, "z");
    }

    #[test]
    fn similarity_floor_drops_tail_and_preserves_surviving_order() {
        let list = l(&[("a", 0.1), ("b", 0.4), ("c", 0.6), ("d", 0.9)]);
        let kept = apply_similarity_floor(list, Some(0.5));
        assert_eq!(kept, l(&[("a", 0.1), ("b", 0.4)]));
    }

    #[test]
    fn similarity_floor_boundary_is_inclusive() {
        let kept = apply_similarity_floor(l(&[("a", 0.5)]), Some(0.5));
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn similarity_floor_none_keeps_everything() {
        let list = l(&[("a", 0.1), ("b", 1.9), ("c", f64::NAN)]);
        assert_eq!(apply_similarity_floor(list.clone(), None).len(), list.len());
    }

    #[test]
    fn similarity_floor_drops_nan() {
        let kept = apply_similarity_floor(l(&[("a", f64::NAN), ("b", 0.1)]), Some(0.0));
        assert_eq!(kept, l(&[("b", 0.1)]));
    }

    #[test]
    fn similarity_is_one_minus_distance_and_none_when_non_finite() {
        assert_eq!(similarity(Some(0.25)), Some(0.75));
        assert_eq!(similarity(None), None);
        assert_eq!(similarity(Some(f64::NAN)), None);
        assert_eq!(similarity(Some(f64::INFINITY)), None);
        assert_eq!(finite(Some(f64::NAN)), None);
        assert_eq!(finite(Some(2.0)), Some(2.0));
    }
}
