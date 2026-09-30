use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    db::Db,
    embedder::Embedder,
    error::Error,
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
) -> Result<Vec<PassageResult>, Error> {
    let embedding = embedder.embed(query).await?;
    let overdraw = num_results * 3;

    let results = tokio::task::spawn_blocking(move || -> Result<Vec<PassageResult>, Error> {
        let conn = db.connect()?;
        let gid_refs: Option<Vec<&str>> = group_ids
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());
        let mut passages =
            conn.vector_search_episodic(&embedding, gid_refs.as_deref(), overdraw)?;

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
    let embedding = embedder.embed(query).await?;

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
    let embedding = embedder.embed(query).await?;

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
