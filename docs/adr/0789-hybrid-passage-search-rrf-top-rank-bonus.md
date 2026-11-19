# ADR-0789: Hybrid Passage Search — BM25 + Vector with Weighted RRF and Belief-Aware Ranking

**Status**: Accepted — hybrid ships opt-in; the default stays `vector`. The first design (unweighted RRF plus a top-rank bonus) measured worse than vector on v2 and was revised on 2026-11-19 to weighted RRF plus belief-aware re-ranking, with the bonus dropped; the revision is not yet measured (see the measurement section)
**Superseded in part by**: [ADR-0806](0806-belief-aware-passage-ranking-default.md) — decision 2's default (`belief_aware` is now on in `vector` too) and decision 9 (`lcg-passages` now tracks the shipped default; `lcg-passages-raw` is the old baseline).
**Date**: 2026-11-18 (measurement section added 2026-11-19)
**Issue**: #789 (builds on ADR-0629's evidence and similarity floor, ADR-0752's `origins` filter, ADR-0754's eval, ADR-0790's embedder roles)

## Context

`knowledge_search_passages` ranked episode passages by vector cosine alone, while entity and fact
search already fuse BM25 and HNSW with Reciprocal Rank Fusion. qmd, the search half of the "LLM wiki"
setups Liminis Notes is compared with, fuses BM25 and vector with a bonus for top-ranked documents,
and edges out vector-only LCG on *answer* questions, where the query's exact wording appears in the
note. On the provisional v2 baseline (`meridian-notebook-2`) vector-only LCG already leads overall
(nDCG@10 0.567 vs qmd 0.499) and loses mainly on *paraphrase* (0.150 vs 0.356), a category BM25 can
only dilute. So the change has to be measured per category, not adopted on faith.

## Decision

1. **A separate hybrid path next to the unchanged vector path.** `search::search_passages_hybrid`
   is new; `search::search_passages` is untouched, so `mode: "vector"` is byte-identical to the
   pre-change behaviour (the response omits the new `similarity` field in that mode). Rejected: one
   function branching internally, which risks altering vector ordering.
2. **Request surface: `mode` (`vector` | `hybrid`) and `belief_aware` (bool; default true in
   `hybrid`, false in `vector`).** No new IPC method, so no new `ToolSpec` and no scope or
   registry-count change. `belief_aware` is a separate boolean so the belief rules are an
   independent lever from fusion: `vector`, `hybrid` + `belief_aware:false` (weighted RRF alone) and
   `hybrid` + `belief_aware:true` are eval rows on one binary, and `vector` + `belief_aware:true`
   measures the rules on the vector ranking. An unknown or ill-typed value is an invalid-params
   error, never ignored. (The first version had a `top_rank_bonus` boolean here; see decision 5.)
3. **BM25 source: the existing `episode_content` FTS index, as is.** It indexes `content`, `source`
   and `source_description`, so a query term in a file name or description can match. Changing the
   index would need a rebuild and a marker bump, and the spec excludes a new text-indexing mechanism.
   If the eval shows harm, a content-only index is a follow-up.
4. **Lexical list as a reusable intermediate.** `Conn::fts_search_episodic` returns plain
   `(uuid, score)` pairs with the group and origin predicates inside the FTS call (before `LIMIT`),
   and `search::lexical_passage_list` wraps it with sanitisation and the fallback. Graph-aware fused
   search (#797) consumes this list.
5. **Fusion: weighted RRF; the top-rank bonus is dropped.**
   - Both lists are over-drawn to `3 × num_results`, as in entity search, then fused with RRF where
     each list contributes `weight / (rank + 60)`. The lexical list weighs `LEXICAL_RRF_WEIGHT`
     (0.4, a starting value in the maintainer's 0.3–0.5 range), the vector list `1.0`.
     `rrf_fuse_scored` stays unweighted for entity and edge search and the extraction-time dedup path;
     it and the weighted form share one implementation, and unit weights reproduce it exactly.
   - **Why weighted:** the first design fused with equal weights, which treats two retrievers of
     unequal quality as peers. The vector leg is clearly stronger on v2, and BM25's top hits on
     paraphrase questions are lexical decoys that equal weights pull above good vector hits (see the
     measured negative result below).
   - **The top-rank bonus (`0.25/60` for rank 1, `0.10/60` for ranks 2–3, best rank across lists,
     non-stacking) was removed.** On both corpora it was statistically indistinguishable from plain
     RRF, so it did not pay for itself; the code, the `top_rank_bonus` parameter and the
     `lcg-passages-hybrid-bonus` configuration are gone. This supersedes FR-002's requirement that a
     bonus be switchable, on the maintainer's instruction. A request that still sends
     `top_rank_bonus` has it ignored, as any unknown parameter is.
5a. **Belief-aware re-ranking, after fusion and before truncation.** The issue title promised it and
   the measured symptoms are its absence: restating (derived) pages outranked their primary note
   more often (0.31 to 0.46) and the stale note outranked the current one more often (0.650 to
   0.775). Two rules re-rank the candidate set (up to `3 × num_results`, one extra batched
   supersession lookup and one hydration of lexical-only candidates):
   - **Derived rule (ADR-0752).** A `derived`/`answer` passage ranks below the `primary`/`external`
     note it cites. A citation is a `[[wikilink]]` in the passage text (alias and heading suffixes
     stripped, case-insensitive, matched on the cited note's `doc_id`, last path segment, `.md`
     stripped), within the passage's group. LCG stores no structured citation, so this reads the
     wikilink convention of the Notes pages; a derived page without wikilinks is not demoted.
   - **Stale rule (ADR-0705/0707).** A passage whose episode is not live now ranks below its
     successor: the revising episode (`superseded_by_kind = episode`) or the superseding document's
     passages (`document`). "Live" is the existing `EpisodeSupersession::live_at`, so a pending
     document supersession with a future effective instant is still believed. A removed chunk has
     no successor and is not demoted.
   - **Mechanics.** A rule applies only when the passage it ranks under is in the candidate set, and
     it means *below the best-ranked passage of that note or document* (every cited note, for a
     page citing several), the minimal demotion. The demoted score is set to just below that
     passage, so returned scores stay in rank order. Chains settle by iterating to a fixed point,
     bounded by the candidate count. Demoted passages are never dropped (FR-014).
   - **Default.** On in `hybrid`. `vector` mode is unchanged unless it is sent `belief_aware: true`,
     which runs the same rules on the vector ranking through a separate function, so the vector
     default stays byte-for-byte identical (FR-004, SC-007).
6. **Score and `min_score` semantics.** In hybrid mode `score` is the fused score and a new
   `similarity` (cosine, `Option<f64>`, skipped when absent) reports the vector retriever's value.
   `min_score` floors only the vector list before fusion (the ADR-0629 pattern); lexical-only hits are
   admitted. Callers that need cosine scores or a strict floor use `vector` mode.
7. **Degradation.** Query text is reduced to alphanumeric tokens (capped at 64) before it reaches the
   index, so no query can raise a parse error; an empty token list skips the lexical leg. A missing
   or "inconsistent" episode FTS index (ADR-0649) on the lexical leg yields an empty lexical list, so
   the result is the vector ranking, and one stderr line is logged per process. Vector-leg errors
   still reach the handler's existing build-indices-and-retry. An embedder failure still errors the
   call, as in entity search and today's passage search.
8. **Default mode: `LCG_PASSAGE_SEARCH_MODE`, read per request**, `vector` when unset or invalid (a
   warning is logged once). Read per request rather than stored on `AppState`, which avoids a field
   on every literal construction site for a value that costs one env lookup.
9. **The eval pins its mode.** Every `lcg-passages*` configuration sends `mode` and `belief_aware`
   explicitly: `lcg-passages` is `vector`/false, and the opt-in `lcg-passages-belief`,
   `lcg-passages-hybrid` and `lcg-passages-hybrid-belief` are `vector`/true, `hybrid`/false and
   `hybrid`/true, so a default flip can never silently change the baseline. The ids
   `lcg-passages-hybrid-rrf` and `lcg-passages-hybrid-bonus` (unweighted RRF, and RRF with the
   bonus) are retired rather than reused, because their meaning changed.

## Measurement and the keep-or-drop record (FR-015)

### Measured negative result: unweighted RRF, with and without the bonus (first design)

The maintainer ran `lcg-passages`, `lcg-passages-hybrid-rrf` (unweighted RRF) and
`lcg-passages-hybrid-bonus` (unweighted RRF plus the top-rank bonus) on head
`b70eef75` (2026-11-19): k = 10, release build (`lcg-build 0.19.1`), extraction replayed from the
Haiku 4.5 cassettes, Sonnet 4.6 judge. v2 is `meridian-notebook-2` / `meridian-questions-3` (244
questions, BGE-base v1.5 sidecar embedder); v1 is `fernhill-notebook-1` (40 questions,
`nomic-embed-text`). v2 labels are still provisional (#788).

**Provenance.** The numbers below are the maintainer's summary, posted as a comment on issue #789 on
2026-11-19 (the ADR was first written on 2026-11-18, before the run). The underlying `report.md` /
`report.json` are kept locally by the maintainer and are not committed to this repository, so this
section is a transcription, not a reproducible artifact; re-run the command below to regenerate it.
p95 latency was not part of that summary.

nDCG@10 on v2:

| config | overall | direct | paraphrase | temporal | answer | multi-hop | passage | p50 |
|---|---|---|---|---|---|---|---|---|
| `lcg-passages` (vector) | **0.568** | 0.756 | 0.150 | **0.628** | **0.789** | **0.662** | 0.477 | 24 ms |
| `lcg-passages-hybrid-rrf` | 0.527 | 0.749 | 0.070 | 0.568 | 0.748 | 0.609 | 0.484 | 27 ms |
| `lcg-passages-hybrid-bonus` | 0.531 | 0.739 | 0.070 | 0.578 | 0.740 | 0.613 | 0.513 | 26 ms |

Paired differences, vector minus hybrid, overall nDCG@10: vs `hybrid-rrf` +0.041 [+0.014, +0.067];
vs `hybrid-bonus` +0.037 [+0.012, +0.063]. Paraphrase: +0.080 [+0.004, +0.159]. On v1 the overall
difference is +0.011 [-0.069, +0.093] (noise), and paraphrase drops there too (0.781 to 0.619).
Hybrid raised direct recall on v2 (0.838 to 0.912) but lowered MRR, and it ranked derived pages above
their primary note more often (0.31 to 0.46) and the stale note above the current one more often
(0.650 to 0.775 rrf, 0.800 bonus).

| Lever | Verdict | Evidence |
|---|---|---|
| Unweighted RRF of BM25 + vector | **Dropped as designed; replaced by weighted RRF** | SC-001 and SC-002 fail on v2: overall nDCG is lower beyond its CI, and paraphrase halves. SC-003 fails: answer does not improve (0.789 to 0.748). SC-004 holds (p50 27 ms). The lexical list stays because #797 consumes it |
| Top-rank bonus | **Dropped** | Indistinguishable from plain RRF on v2 and v1 (0.531 vs 0.527; identical on v1): no measured gain, so the code, parameter and configuration were removed |
| Weighted RRF (lexical 0.4) | **Kept opt-in, not yet measured** | Designed from the negative result above: it lets the stronger vector leg lead, so a BM25 decoy cannot displace a good vector hit, while a passage both lists agree on still wins |
| Belief-aware re-ranking | **Kept opt-in, not yet measured** | Targets the measured symptoms: derived pages outranking their primary note (0.31 to 0.46) and stale notes outranking current ones (0.650 to 0.775) |

The default stays `vector`, as FR-005 requires while the gate has not passed. The workers that
implemented the revision have neither the recorded cassettes nor the embedder sidecar, so the new
design has unit and fixture tests but no eval numbers. The maintainer will run v1 and v2 on the new
head. **Merge bar:** hybrid at or above vector on v2 overall nDCG@10 (the paired interval not below
zero), paraphrase not below vector, and fewer stale and derived outranks. If it does not clear that
bar, hybrid ships opt-in, this ADR records why, and the default stays `vector`. The lexical weight
(0.4) is a constant (`LEXICAL_RRF_WEIGHT`) and is the first thing to tune if the bar is missed;
counting BM25 only when it agrees with a vector hit is the next experiment.

Reproduce the first run (unweighted configurations, retired ids; run on head `b70eef75`) from that
commit. To measure the revision:

```
cargo run --release -p lcg-eval -- retrieval --corpus crates/eval/fixtures/retrieval-v2 \
  --configs lcg-passages,lcg-passages-hybrid,lcg-passages-hybrid-belief,lcg-passages-belief \
  --embedder-http <url> --replay-cassette <cassette> --out report-hybrid-v2
```

## Consequences

- Vector-mode callers see no change unless they send `belief_aware: true`. Hybrid callers see
  `score` on a different scale (at most about 0.023) and a `similarity` field; both are documented
  in the IPC reference.
- Superseded episodes are still returned in both modes; `belief_aware` only moves them below their
  successor. The `explain` trace, query expansion and reranking are out of scope (#796, #797 and
  follow-ups).
- The derived rule depends on derived pages citing their sources with `[[wikilinks]]` whose target
  is the source's `doc_id`. A page that cites some other way is not demoted; a structured citation
  field would remove that dependency and is a follow-up.
- A selective `origins` or group filter can still return fewer than `num_results`; in hybrid mode the
  filter also runs inside the full-text query, so lexical recall is not crowded out.
