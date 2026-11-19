# ADR-0789: Hybrid Passage Search — BM25 + Vector with RRF and a Top-Rank Bonus

**Status**: Accepted — hybrid ships opt-in; the default stays `vector` because the v2 eval showed no gain (see the measurement section)
**Date**: 2026-11-18
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
2. **Request surface: `mode` (`vector` | `hybrid`) and `top_rank_bonus` (bool, default true).** No new
   IPC method, so no new `ToolSpec` and no scope or registry-count change. The bonus is a separate
   boolean, not a third mode value, so it stays an independent lever: `vector`, `hybrid` +
   `top_rank_bonus:false` (RRF alone) and `hybrid` + `top_rank_bonus:true` are three eval rows on one
   binary. An unknown or ill-typed value is an invalid-params error, never ignored.
3. **BM25 source: the existing `episode_content` FTS index, as is.** It indexes `content`, `source`
   and `source_description`, so a query term in a file name or description can match. Changing the
   index would need a rebuild and a marker bump, and the spec excludes a new text-indexing mechanism.
   If the eval shows harm, a content-only index is a follow-up.
4. **Lexical list as a reusable intermediate.** `Conn::fts_search_episodic` returns plain
   `(uuid, score)` pairs with the group and origin predicates inside the FTS call (before `LIMIT`),
   and `search::lexical_passage_list` wraps it with sanitisation and the fallback. Graph-aware fused
   search (#797) consumes this list.
5. **Fusion and the bonus.**
   - Both lists are over-drawn to `3 × num_results`, as in entity search, then fused with the same
     `rrf_fuse_scored`, so entity search and the extraction-time dedup path are untouched.
   - The bonus is a separate step. Each passage gets at most one bonus from its best rank across the
     two lists (rank 1: `0.25/60`; ranks 2–3: `0.10/60`), expressed relative to RRF's own scale. qmd's
     absolute 0.05 / 0.02 would dwarf RRF scores of about 0.016 and degenerate to "the first hit of
     either list wins"; the 5:2 ratio is kept. Agreement across lists is already rewarded by RRF's
     sum, so the bonus does not stack. At these weights the bonus reorders near-ties and lets a
     rank-1 hit overtake a double hit only when the latter sits far down both lists.
   - The weights are named constants (`TOP1_BONUS_FRACTION`, `TOP3_BONUS_FRACTION`) not yet
     tuned: the v1/v2 run could not distinguish the bonus from plain RRF.
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
9. **The eval pins its mode.** `lcg-passages` now sends `mode: "vector"` explicitly, and the opt-in
   configurations `lcg-passages-hybrid-rrf` and `lcg-passages-hybrid-bonus` send `mode: "hybrid"`
   with `top_rank_bonus` false and true, so a default flip can never silently change the baseline.

## Measurement and the keep-or-drop record (FR-015)

The maintainer ran `lcg-passages`, `lcg-passages-hybrid-rrf` and `lcg-passages-hybrid-bonus` on head
`b70eef75` (2026-11-19, PR #798 comment): k = 10, release build, extraction replayed from the Haiku
4.5 cassettes, Sonnet 4.6 judge. v2 is `meridian-notebook-2` / `meridian-questions-3` (244 questions,
BGE-base v1.5 sidecar embedder); v1 is `fernhill-notebook-1` (40 questions, `nomic-embed-text`). v2
labels are still provisional (#788). The full reports are kept locally by the maintainer.

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
| RRF fusion of BM25 + vector | **Not adopted as the default; kept opt-in** | SC-001 and SC-002 fail on v2: overall nDCG is lower beyond its CI, and paraphrase halves. SC-003 fails: answer does not improve (0.789 to 0.748). SC-004 holds (p50 27 ms). The lexical list stays because #797 consumes it |
| Top-rank bonus | **Kept opt-in, not shown to help** | Indistinguishable from plain RRF on v2 and v1 (0.531 vs 0.527; identical on v1), so the 0.25 / 0.10 fractions cannot be tuned from this data and remain untuned starting values |

The default stays `vector`, as FR-005 requires when the gate has not passed. The likely cause is
equal-weight RRF over two retrievers of unequal quality: on paraphrase questions BM25's top hits are
lexical decoys that are pulled above good vector hits. Weighted fusion (a smaller BM25 weight, or
counting BM25 only when it agrees with a vector hit) and belief-aware ranking are the follow-up
experiments, not part of this PR. Reproduce the run with:

```
cargo run --release -p lcg-eval -- retrieval --corpus crates/eval/fixtures/retrieval-v2 \
  --configs lcg-passages,lcg-passages-hybrid-rrf,lcg-passages-hybrid-bonus \
  --embedder-http <url> --replay-cassette <cassette> --out report-hybrid-v2
```

## Consequences

- Vector-mode callers see no change. Hybrid callers see `score` on a different scale
  (about 0.016–0.04) and a `similarity` field; both are documented in the IPC reference.
- Superseded episodes are still returned by default in both modes. Belief-aware ranking, the
  `explain` trace, query expansion and reranking are out of scope (#796, #797 and follow-ups).
- A selective `origins` or group filter can still return fewer than `num_results`; in hybrid mode the
  filter also runs inside the full-text query, so lexical recall is not crowded out.
