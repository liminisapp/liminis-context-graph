//! Orchestration of the optional passage-search stages (issue #796).
//!
//! `search_passages_staged` is only entered when a call (or the workspace config) opted in to
//! query expansion and/or rerank. A call that opts in to neither never reaches this module, which
//! is what makes the default response byte-identical to before the stages existed.
//!
//! ```text
//! original query ──┐
//!                  ├─ (expand) variants ── per-variant first-stage search ─┐
//! first stage ─────┘                                                       ├─ RRF ─ candidates
//!                                                                          ┘
//! candidates ── (rerank) top-N head ── cross-encoder ── belief/recency blend ── head + tail
//! ```
//!
//! Each variant is searched with the call's retrieval mode (so a hybrid workspace keeps its BM25
//! leg); only a hypothetical-answer (HyDE) variant, which is embedded as a document, is searched by
//! vector alone.
//!
//! Every failure of an optional stage degrades to the previous stage's output and is reported in
//! the returned [`StageReport`]s; it never fails the search (FR-006, FR-010, SC-007). Only a
//! failure of the *original* query's first-stage search is an error, exactly as without stages.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::{
    belief::EpisodeSupersession,
    db::Db,
    embedder::EmbedRole,
    error::Error,
    passage_rerank::{self, Candidate},
    query_expand::{QueryVariant, VariantKind},
    search::rrf_fuse_scored,
    search_stages::SearchStages,
    types::PassageResult,
};

/// One first-stage search: `(query text, embedding role, number of results wanted)`.
pub type FirstStageFn = Arc<
    dyn Fn(String, EmbedRole, usize) -> BoxFuture<'static, Result<Vec<PassageResult>, Error>>
        + Send
        + Sync,
>;

/// What the call asked for, after config-default resolution.
#[derive(Debug, Clone, Copy)]
pub struct StagedRequest {
    pub num_results: usize,
    /// `Some(n)`: rerank the top `n` first-stage candidates.
    pub rerank_candidates: Option<usize>,
    pub expand: bool,
}

/// Outcome of one stage, rendered into the response envelope. Present only for a stage that was
/// requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageReport {
    pub applied: bool,
    /// Rerank: candidates scored. Expansion: variants searched.
    pub count: usize,
    /// Why the stage did not apply (`not_configured`, `unavailable`, `no_candidates`,
    /// `no_variants`).
    pub reason: Option<&'static str>,
    /// Wall-clock time the stage took, in milliseconds (expansion: the LLM call plus the variant
    /// searches and fusion; rerank: the cross-encoder call plus the supersession lookup and
    /// blend). `None` when the stage did no work (`not_configured`, `no_candidates`).
    pub elapsed_ms: Option<u64>,
}

impl StageReport {
    /// Records the stage's wall-clock time, measured from `since`.
    fn timed(mut self, since: Instant) -> Self {
        self.elapsed_ms = Some(since.elapsed().as_millis() as u64);
        self
    }

    /// Records an already-measured duration.
    fn with_elapsed(mut self, d: std::time::Duration) -> Self {
        self.elapsed_ms = Some(d.as_millis() as u64);
        self
    }

    fn applied(count: usize) -> Self {
        Self {
            applied: true,
            count,
            reason: None,
            elapsed_ms: None,
        }
    }

    fn skipped(reason: &'static str) -> Self {
        Self {
            applied: false,
            count: 0,
            reason: Some(reason),
            elapsed_ms: None,
        }
    }

    /// Envelope JSON; `count_key` is `candidates` for rerank and `variants` for expansion.
    pub fn to_json(&self, count_key: &str) -> Value {
        let mut v = json!({ "applied": self.applied, count_key: self.count });
        if let Some(r) = self.reason {
            v["reason"] = json!(r);
        }
        if let Some(ms) = self.elapsed_ms {
            v["elapsed_ms"] = json!(ms);
        }
        v
    }
}

#[derive(Debug)]
pub struct StagedOutcome {
    pub passages: Vec<PassageResult>,
    pub rerank: Option<StageReport>,
    pub expansion: Option<StageReport>,
}

fn now_us() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000) as i64
}

fn to_us(s: &str) -> Option<i64> {
    crate::db::parse_timestamp_str(s).map(|d| (d.unix_timestamp_nanos() / 1_000) as i64)
}

/// `valid_at`, falling back to `created_at`, as epoch microseconds.
fn passage_time_us(p: &PassageResult) -> Option<i64> {
    p.valid_at
        .as_deref()
        .and_then(to_us)
        .or_else(|| to_us(&p.created_at))
}

/// Fuses the original query's ranked list with one list per variant using RRF. The result is in
/// fused order; each passage keeps the row, and so the `score`, from the first list that contained
/// it. The original query's list comes first, so a passage it found keeps the score computed
/// against the user's own query; one found only by a variant carries that variant's score.
fn fuse_passage_lists(lists: Vec<Vec<PassageResult>>) -> Vec<PassageResult> {
    let ranked: Vec<Vec<(String, f64)>> = lists
        .iter()
        .map(|l| l.iter().map(|p| (p.uuid.clone(), p.score)).collect())
        .collect();
    let refs: Vec<&[(String, f64)]> = ranked.iter().map(Vec::as_slice).collect();
    let fused = rrf_fuse_scored(&refs);

    let mut rows: HashMap<String, PassageResult> = HashMap::new();
    for list in lists {
        for p in list {
            rows.entry(p.uuid.clone()).or_insert(p);
        }
    }
    fused
        .into_iter()
        .filter_map(|h| rows.remove(&h.uuid))
        .collect()
}

/// Looks up supersession state for `uuids`. A failure is an `Err`, never an empty map: an empty
/// map would read as "every candidate is belief-neutral", and a rerank blind to supersession
/// must not be reported as applied (the caller degrades the stage instead).
async fn load_supersession(
    db: Arc<Db>,
    uuids: Vec<String>,
) -> Result<HashMap<String, EpisodeSupersession>, ()> {
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.get_episode_supersession(&uuids)
    })
    .await;
    match res {
        Ok(Ok(m)) => Ok(m),
        Ok(Err(e)) => {
            eprintln!("[liminis-context-graph] rerank: supersession lookup failed: {e}");
            Err(())
        }
        Err(e) => {
            eprintln!("[liminis-context-graph] rerank: supersession lookup panicked: {e}");
            Err(())
        }
    }
}

/// Runs the requested stages and returns the final passages (at most `req.num_results`).
pub async fn search_passages_staged(
    db: Arc<Db>,
    stages: Option<Arc<SearchStages>>,
    query: &str,
    req: StagedRequest,
    first_stage: FirstStageFn,
) -> Result<StagedOutcome, Error> {
    let fetch = req
        .rerank_candidates
        .map_or(req.num_results, |n| n.max(req.num_results));

    // ── Expansion (concurrent with the original query's first stage) ──────────────────────────
    let expander = stages.as_ref().and_then(|s| s.expander.clone());
    let expansion_fut = async {
        if !req.expand {
            return None;
        }
        // Timed here, around the expander alone: the join below also waits for the original
        // query's first stage, which must not be counted as expansion time.
        let llm_started = Instant::now();
        let outcome = match expander {
            None => Err("not_configured"),
            Some(e) => match e.expand(query).await {
                Ok(v) if v.is_empty() => Err("no_variants"),
                Ok(v) => Ok(v),
                Err(err) => {
                    eprintln!("[liminis-context-graph] query expansion failed: {err}");
                    Err("unavailable")
                }
            },
        };
        Some((outcome, llm_started.elapsed()))
    };
    let original_fut = (first_stage)(query.to_string(), EmbedRole::Query, fetch);
    let (expansion, original) = futures::join!(expansion_fut, original_fut);
    let original = original?;

    let mut expansion_report = None;
    let mut candidates = original.clone();
    match expansion {
        None => {}
        Some((Err(reason), llm_elapsed)) => {
            // `not_configured` did no work; any other reason spent time waiting on the LLM.
            let r = StageReport::skipped(reason);
            expansion_report = Some(if reason == "not_configured" {
                r
            } else {
                r.with_elapsed(llm_elapsed)
            });
        }
        Some((Ok(variants), llm_elapsed)) => {
            // Variant searches and fusion start only now; add them to the LLM time.
            let variants_started = Instant::now();
            let variant_lists =
                futures::future::join_all(variants.iter().map(|v: &QueryVariant| {
                    let role = match v.kind {
                        VariantKind::Hyde => EmbedRole::Document,
                        VariantKind::Lexical | VariantKind::Semantic => EmbedRole::Query,
                    };
                    (first_stage)(v.text.clone(), role, fetch)
                }))
                .await;
            let mut lists = vec![original];
            let mut searched = 0usize;
            for r in variant_lists {
                match r {
                    Ok(l) => {
                        searched += 1;
                        lists.push(l);
                    }
                    // A variant that fails to search is simply not fused.
                    Err(e) => {
                        eprintln!("[liminis-context-graph] expansion variant search failed: {e}")
                    }
                }
            }
            if searched == 0 {
                expansion_report = Some(
                    StageReport::skipped("unavailable")
                        .with_elapsed(llm_elapsed + variants_started.elapsed()),
                );
            } else {
                candidates = fuse_passage_lists(lists);
                expansion_report = Some(
                    StageReport::applied(searched)
                        .with_elapsed(llm_elapsed + variants_started.elapsed()),
                );
            }
        }
    }

    // ── Rerank ────────────────────────────────────────────────────────────────────────────────
    let Some(n) = req.rerank_candidates else {
        candidates.truncate(req.num_results);
        return Ok(StagedOutcome {
            passages: candidates,
            rerank: None,
            expansion: expansion_report,
        });
    };

    let rerank_started = Instant::now();
    let finish_degraded = |mut c: Vec<PassageResult>, reason: &'static str| {
        c.truncate(req.num_results);
        // Time was only spent when the cross-encoder was actually called.
        let r = StageReport::skipped(reason);
        let r = if reason == "unavailable" {
            r.timed(rerank_started)
        } else {
            r
        };
        StagedOutcome {
            passages: c,
            rerank: Some(r),
            expansion: expansion_report.clone(),
        }
    };

    if candidates.is_empty() {
        return Ok(finish_degraded(candidates, "no_candidates"));
    }
    let Some(reranker) = stages.as_ref().and_then(|s| s.reranker.clone()) else {
        return Ok(finish_degraded(candidates, "not_configured"));
    };
    let blend_cfg = stages.as_ref().map(|s| s.config.blend).unwrap_or_default();

    let head_len = n.min(candidates.len());
    let documents: Vec<String> = candidates[..head_len]
        .iter()
        .map(|p| p.content.clone())
        .collect();
    let head_uuids: Vec<String> = candidates[..head_len]
        .iter()
        .map(|p| p.uuid.clone())
        .collect();

    let (logits, supersession) = futures::join!(
        reranker.rerank(query, &documents),
        load_supersession(Arc::clone(&db), head_uuids)
    );
    let logits = match logits {
        Ok(l) if l.len() == head_len => l,
        Ok(l) => {
            eprintln!(
                "[liminis-context-graph] rerank returned {} scores for {head_len} documents",
                l.len()
            );
            return Ok(finish_degraded(candidates, "unavailable"));
        }
        Err(e) => {
            eprintln!("[liminis-context-graph] rerank failed: {e}");
            return Ok(finish_degraded(candidates, "unavailable"));
        }
    };

    // Without belief data the blend cannot demote superseded notes, so degrade rather than report
    // a belief-blind rerank as applied.
    let Ok(supersession) = supersession else {
        return Ok(finish_degraded(candidates, "unavailable"));
    };

    let cands: Vec<Candidate> = candidates[..head_len]
        .iter()
        .zip(&logits)
        .map(|(p, logit)| Candidate {
            uuid: p.uuid.clone(),
            logit: *logit,
            time_us: passage_time_us(p),
            belief: supersession.get(&p.uuid).cloned(),
        })
        .collect();
    let blended = passage_rerank::blend(&cands, now_us(), blend_cfg);

    let head_order: Vec<usize> = blended.iter().map(|b| b.index).collect();
    for b in &blended {
        let p = &mut candidates[b.index];
        p.rerank_score = Some(logits[b.index]);
        p.belief = b.state.as_wire();
    }
    let order = passage_rerank::assemble(&head_order, head_len, candidates.len(), req.num_results);
    let mut slots: Vec<Option<PassageResult>> = candidates.into_iter().map(Some).collect();
    let passages: Vec<PassageResult> = order.into_iter().filter_map(|i| slots[i].take()).collect();

    Ok(StagedOutcome {
        passages,
        rerank: Some(StageReport::applied(head_len).timed(rerank_started)),
        expansion: expansion_report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(uuid: &str, score: f64) -> PassageResult {
        PassageResult {
            uuid: uuid.into(),
            name: uuid.into(),
            content: format!("content {uuid}"),
            source_description: String::new(),
            group_id: "g".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            valid_at: None,
            score,
            attributes: "{}".into(),
            origin: "primary".into(),
            similarity: None,
            matched_queries: None,
            rerank_score: None,
            belief: None,
        }
    }

    #[test]
    fn fusion_promotes_items_found_by_several_lists_and_keeps_the_first_lists_score() {
        let fused = fuse_passage_lists(vec![
            vec![p("a", 0.9), p("b", 0.8), p("c", 0.7)],
            vec![p("c", 0.95), p("d", 0.6)],
        ]);
        let ids: Vec<&str> = fused.iter().map(|x| x.uuid.as_str()).collect();
        // c: rank 2 in list 1 and rank 0 in list 2 → highest fused score.
        assert_eq!(ids[0], "c");
        assert_eq!(ids.len(), 4);
        // c keeps the score from the first list that held it (the original query's), not the
        // higher one a variant computed.
        assert_eq!(fused[0].score, 0.7);
        // d was found only by the variant, so it carries the variant's score.
        assert_eq!(fused[3].score, 0.6);
    }

    #[test]
    fn fusion_has_no_duplicates() {
        let fused = fuse_passage_lists(vec![
            vec![p("a", 0.9)],
            vec![p("a", 0.8)],
            vec![p("a", 0.7)],
        ]);
        assert_eq!(fused.len(), 1);
    }

    #[test]
    fn stage_report_json_shapes() {
        assert_eq!(
            StageReport::applied(3).to_json("candidates"),
            json!({"applied": true, "candidates": 3})
        );
        assert_eq!(
            StageReport::skipped("not_configured").to_json("variants"),
            json!({"applied": false, "variants": 0, "reason": "not_configured"})
        );
        let mut timed = StageReport::applied(2);
        timed.elapsed_ms = Some(17);
        assert_eq!(
            timed.to_json("variants"),
            json!({"applied": true, "variants": 2, "elapsed_ms": 17})
        );
    }

    #[test]
    fn passage_time_prefers_valid_at_then_created_at() {
        let mut x = p("a", 0.0);
        let created = passage_time_us(&x).unwrap();
        x.valid_at = Some("2026-06-01T00:00:00Z".into());
        assert!(passage_time_us(&x).unwrap() > created);
        x.valid_at = Some("not a date".into());
        assert_eq!(passage_time_us(&x), Some(created));
    }
}
