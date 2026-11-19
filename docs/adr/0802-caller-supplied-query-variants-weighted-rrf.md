# ADR-0802: Caller-Supplied Query Variants, Fused with Weighted RRF

**Status**: Accepted
**Date**: 2026-11-18
**Issue**: #802 (named for the issue per the repo's ADR numbering rule)

## Context

On the v2 retrieval corpus (#788) `lcg-passages` scores paraphrase nDCG@10 0.150, against 0.347 for qmd
and 0.475 for qmd-rerank. qmd closes most of that gap by rewriting each query with a local LLM. LCG's
main caller is already an LLM, and the maintainer prefers that LCG not run one at query time by default
(#796 tests an opt-in in-LCG expansion). `knowledge_search_passages` took exactly one `query`, so the caller
had nowhere to put rewrites it can write for free.

## Decisions

1. **An optional `queries` array beside the required `query`.** At most 5 entries, each non-empty after
   trimming, at most 2000 Unicode scalar values in total, never truncated. Absent, `null` and `[]` mean no
   variants and take the *original* `search_passages` code path, so a request without `queries` is
   byte-identical by construction (FR-002), not by a test of two paths that happen to agree.
2. **A separate `search_passages_variants`** rather than generalising `search_passages`; the diff stays
   additive against the open hybrid (#798), rerank (#800) and graph-search (#801) PRs, which rewrite the
   same functions.
3. **One batched query-role embed, one connection, one probe per query.** Every probe gets the same
   `group_ids`, `origins`, `num_results * 3` probe size, and `min_score` floor, applied *per query before
   fusion*. A floor applied after fusion could surface a variant-only candidate the floor would have
   removed for the primary; applying it per query makes "variants never widen what the caller may see" true
   by construction.
4. **Weighted RRF**: `rrf_fuse_weighted`, `sum(weight / (rank + 60))` with the existing UUID tie-break. The
   primary weighs `PRIMARY_QUERY_WEIGHT = 2.0` (qmd's choice), variants `VARIANT_QUERY_WEIGHT = 1.0`. The
   weight is a constant, not a request parameter. Tuning it is out of scope.
5. **Evidence is a top-level `matched_queries: [usize]`** on each passage: ascending indices into the
   caller's original `[query, ...queries]`. It is skipped when absent, so no-`queries` responses are
   unchanged. It is simpler for an agent than ADR-0629's nested `search` object and follows its additive
   per-item-evidence precedent.
6. **Duplicates are dropped, indices stay original.** After trimming, a variant equal (exact,
   case-sensitive) to the primary or an earlier variant is not probed and its index is not reported. Keeping
   duplicates would let repetition boost a document, the exact failure the spec warns about. Case-folding
   was rejected: "Foo" and "foo" can embed differently, and dropping is for exact repeats only.
7. **`score` is the best cosine similarity across matching queries; order is fused RRF.** Showing the RRF
   score would break `min_score` semantics and comparability with the single-query response. The cost is
   that the list is not score-monotone, which the reference documents.
8. **The eval simulates the agent without leaking answers.** `query-variants.jsonl` holds up to 5 variants per
   question, stamped with the questions hash, written once by an LLM that sees only the question text under a
   committed prompt (`query-variants.prompt.md`) via an offline `lcg-eval retrieval-variants`
   `--export-prompts` / `--replies` exchange (no API key). Loading refuses a stale hash, an unknown id,
   more than 5 variants and a leak. A leak is a variant containing, verbatim, the title (first `# ` heading,
   whole or with `Notes on ` / `Q&A: ` stripped) or the key-fact string of any relevant note,
   case-insensitive over normalised whitespace. Needles under 8 chars are skipped, and a needle already in
   the question text is not a leak: a direct question names its subject and the writer saw that text. The check
   catches verbatim leaks only.
9. **The eval keys variants by question id** through a defaulted `Retriever::retrieve_for`, not by text (two
   questions can share text). `lcg-passages-variants` is in the default config list; without the fixture
   file its row is *skipped*, a stale or leaking file aborts the run.
10. **The report gains "Query variants vs `lcg-passages`"**: per-category paired nDCG@k difference with its
    interval, p50/p95, and the merge-bar reading (paraphrase interval above zero; no category wholly below
    zero; p50 <= 100 ms). The H1/H2 "best `lcg-*`" selection excludes the variants row so those readings keep
    their meaning.

## Deferred

- **Rerank on the fused candidates, against the primary query (FR-007), the hybrid half of FR-005, and
  `lcg-passages-variants-rerank` (FR-017)**: hybrid (#789) and rerank (#796) are not on `main`. The
  later PR must pass the same arguments to every probe and rerank once on the fused list.
- **`knowledge_graph_search` `queries` (FR-019)**: #797 (#801) is not on `main`. Follow-up.
- **`believed_only` / `as_of` on passage search**: not passage-search parameters on `main` (they are
  edge-read parameters, ADR-0705). Passage-search belief handling arrives with #798; it inherits the
  per-probe argument passing here, so it applies to every variant without further work.

## Consequences

- A caller that never sends `queries` sees no change.
- Cost with N variants is one batched embed plus N+1 ANN probes on one connection.
- The merge bar (paraphrase improvement, no regression, p50 <= 100 ms) can only be measured once the
  maintainer commits `query-variants.jsonl`; until then the row is skipped.
