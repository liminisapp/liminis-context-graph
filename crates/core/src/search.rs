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
    let mut scores: HashMap<String, f64> = HashMap::new();

    for list in lists {
        for (rank, (uuid, _)) in list.iter().enumerate() {
            *scores.entry(uuid.clone()).or_default() += 1.0 / (rank as f64 + 60.0);
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
    rrf_rank(lists)
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
    /// BM25 over episode text + vector, fused with RRF (optionally with a top-rank bonus).
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

/// Top-rank bonus for a passage whose best rank in either retriever is 1 (qmd-style), as a
/// fraction of the single-list rank-1 RRF contribution (`1/60`). qmd's absolute 0.05 / 0.02 would
/// dwarf RRF scores of ~0.016 and degenerate to "first hit of either list wins", so the same 5:2
/// ratio is applied relative to RRF's own scale. Tuned on v1/v2 (ADR-0789).
pub const TOP1_BONUS_FRACTION: f64 = 0.25;
/// Bonus for a best rank of 2 or 3. See [`TOP1_BONUS_FRACTION`].
pub const TOP3_BONUS_FRACTION: f64 = 0.10;

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

/// Adds the top-rank bonus to fused hits and re-sorts (descending score, UUID tie-break).
///
/// Each passage gets at most one bonus, from its best (lowest) 0-based rank across `lists`:
/// rank 0 gets [`TOP1_BONUS_FRACTION`], ranks 1–2 get [`TOP3_BONUS_FRACTION`], both times
/// `1/60`. It does not stack across lists — RRF already rewards agreement by summing.
fn apply_top_rank_bonus(lists: &[&[(String, f64)]], mut hits: Vec<FusedHit>) -> Vec<FusedHit> {
    let mut best: HashMap<&str, usize> = HashMap::new();
    for list in lists {
        for (rank, (uuid, _)) in list.iter().enumerate().take(3) {
            let e = best.entry(uuid.as_str()).or_insert(rank);
            *e = (*e).min(rank);
        }
    }
    for hit in &mut hits {
        let fraction = match best.get(hit.uuid.as_str()) {
            Some(0) => TOP1_BONUS_FRACTION,
            Some(_) => TOP3_BONUS_FRACTION,
            None => 0.0,
        };
        hit.rrf_score += fraction / 60.0;
    }
    hits.sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.uuid.cmp(&b.uuid))
    });
    hits
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

/// Hybrid passage search (issue #789): BM25 over episode text + HNSW, fused with RRF and an
/// optional top-rank bonus (HOT path — no lock held).
///
/// - `min_score` floors the *vector* list's cosine similarity before fusion; lexical-only hits
///   are never floored (FR-007).
/// - `score` on each result is the fused score; `similarity` is the cosine where the vector
///   retriever matched.
/// - With no lexical hits the result is the vector ranking (FR-008), re-scored as fused.
#[allow(clippy::too_many_arguments)]
pub async fn search_passages_hybrid(
    db: Arc<Db>,
    embedder: Arc<dyn Embedder>,
    query: &str,
    group_ids: Option<Vec<String>>,
    num_results: usize,
    min_score: f64,
    origins: Option<Vec<crate::origin::Origin>>,
    top_rank_bonus: bool,
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

        let mut fused = rrf_fuse_scored(&[&bm25, &vector]);
        if top_rank_bonus {
            fused = apply_top_rank_bonus(&[&bm25, &vector], fused);
        }
        fused.truncate(num_results);

        let top_uuids: Vec<String> = fused.iter().map(|h| h.uuid.clone()).collect();
        let mut rows: HashMap<String, PassageResult> = vector_rows
            .into_iter()
            .map(|p| (p.uuid.clone(), p))
            .collect();
        let missing: Vec<String> = top_uuids
            .iter()
            .filter(|u| !rows.contains_key(*u))
            .cloned()
            .collect();
        for p in conn.get_passages_by_uuids(&missing)? {
            rows.insert(p.uuid.clone(), p);
        }

        // `fused` is already in rank order; rows are looked up, not re-sorted, so the order is
        // exactly the fused (deterministic) order.
        Ok(fused
            .into_iter()
            .filter_map(|hit| {
                let mut p = rows.remove(&hit.uuid)?;
                p.score = hit.rrf_score;
                p.similarity = similarity(hit.raw[1]);
                Some(p)
            })
            .collect())
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
    fn top_rank_bonus_lifts_a_rank_one_hit_over_a_much_deeper_double_hit() {
        // `x` is rank 1 in BM25 only; `d` is rank 41 in both lists (2/100 = 0.0200 vs x's plain
        // 1/60 = 0.0167). Without the bonus the double hit wins; with it x (0.0208) does.
        let mut bm25 = list(&["x"]);
        let mut vector = list(&["y"]);
        for i in 0..39 {
            bm25.push((format!("pb{i}"), 0.0));
            vector.push((format!("pv{i}"), 0.0));
        }
        bm25.push(("d".to_string(), 0.0));
        vector.push(("d".to_string(), 0.0));
        let plain = rrf_fuse_scored(&[&bm25, &vector]);
        assert_eq!(plain[0].uuid, "d");
        let bonus = apply_top_rank_bonus(&[&bm25, &vector], plain);
        assert!(
            ["x", "y"].contains(&bonus[0].uuid.as_str()),
            "{:?}",
            order(&bonus[..3])
        );
    }

    #[test]
    fn top_rank_bonus_tiers_and_does_not_stack() {
        let bm25 = list(&["a", "b", "c", "d"]);
        let vector = list(&["a", "b", "c", "d"]);
        let plain = rrf_fuse_scored(&[&bm25, &vector]);
        let base: HashMap<String, f64> = plain
            .iter()
            .map(|h| (h.uuid.clone(), h.rrf_score))
            .collect();
        let bonus = apply_top_rank_bonus(&[&bm25, &vector], plain);
        let got: HashMap<&str, f64> = bonus
            .iter()
            .map(|h| (h.uuid.as_str(), h.rrf_score))
            .collect();
        // Present in both lists at rank 1: one bonus, not two.
        assert!((got["a"] - base["a"] - TOP1_BONUS_FRACTION / 60.0).abs() < 1e-12);
        assert!((got["b"] - base["b"] - TOP3_BONUS_FRACTION / 60.0).abs() < 1e-12);
        assert!((got["c"] - base["c"] - TOP3_BONUS_FRACTION / 60.0).abs() < 1e-12);
        assert!((got["d"] - base["d"]).abs() < 1e-12, "rank 4 gets no bonus");
    }

    #[test]
    fn top_rank_bonus_uses_best_rank_across_lists() {
        let bm25 = list(&["a", "b", "z"]);
        let vector = list(&["z", "q"]);
        let hits = apply_top_rank_bonus(&[&bm25, &vector], rrf_fuse_scored(&[&bm25, &vector]));
        let z = hits.iter().find(|h| h.uuid == "z").unwrap();
        let plain = 1.0 / 62.0 + 1.0 / 60.0;
        assert!((z.rrf_score - plain - TOP1_BONUS_FRACTION / 60.0).abs() < 1e-12);
    }

    #[test]
    fn top_rank_bonus_is_deterministic_on_ties() {
        let bm25 = list(&["b"]);
        let vector = list(&["a"]);
        let hits = apply_top_rank_bonus(&[&bm25, &vector], rrf_fuse_scored(&[&bm25, &vector]));
        assert_eq!(order(&hits), vec!["a", "b"]);
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
