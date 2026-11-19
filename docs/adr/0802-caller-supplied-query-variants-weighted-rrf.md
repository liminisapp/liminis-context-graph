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
2. **A separate `search_passages_variants`** rather than generalising `search_passages`: a request without
   `queries` stays on the unchanged path of its mode.
3. **One batched query-role embed, then every query runs the request's full pipeline, concurrently.** Each
   query gets the same `mode` (`vector`, or `hybrid` with BM25 + vector fused per query, #789), `group_ids`,
   `origins`, `num_results * 3` probe size and `min_score` floor (on the vector list, as in hybrid mode),
   applied *per query before fusion*, each on its own connection (a scoped thread per query). A floor
   applied after fusion could surface a variant-only candidate the floor would have removed for the
   primary; applying the pipeline per query makes "variants never widen what the caller may see" true by
   construction. The first v2 measurement (below) showed a vector-only variants path is not enough: a
   variant's keywords only pay off when they reach BM25.
4. **Weighted RRF with a fixed variant budget.** `rrf_fuse_weighted`, `sum(weight / (rank + 60))` with the
   existing UUID tie-break. The primary weighs `PRIMARY_QUERY_WEIGHT = 2.0` (qmd's choice). The variants
   *share* `VARIANT_BUDGET_WEIGHT = 1.0`, split equally (`variant_weight(n) = 1.0 / n`), so the primary keeps
   at least 2/3 of the total weight for any number of variants (a compile-time assertion and a test). This
   replaces the first design (every variant weighing 1.0, so five variants outweighed the primary 5 : 2):
   on v2 (head `a0c7e95f`, BGE base v1.5) that scored overall nDCG@10 0.522 against 0.568 for
   `lcg-passages` (lower by 0.046, interval [0.016, 0.077]), multi-hop worse on 19
   questions and better on 7, paraphrase 0.085 against 0.150. The variants themselves were good; they
   were given too much say. Both constants are code, not request parameters; tuning them is out of scope.
5. **Evidence is a top-level `matched_queries: [usize]`** on each passage: ascending indices into the
   caller's original `[query, ...queries]`. It is skipped when absent, so no-`queries` responses are
   unchanged. It is simpler for an agent than ADR-0629's nested `search` object and follows its additive
   per-item-evidence precedent.
6. **Duplicates are dropped, indices stay original.** After trimming, a variant equal (exact,
   case-sensitive) to the primary or an earlier variant is not probed and its index is not reported. Keeping
   duplicates would let repetition boost a document, the exact failure the spec warns about. Case-folding
   was rejected: "Foo" and "foo" can embed differently, and dropping is for exact repeats only.
7. **With `queries`, `score` is the fused rank score and `similarity` the best cosine** across the vector
   legs that matched (absent for a lexical-only hit), the same contract as hybrid mode (ADR-0789). The
   first design kept `score` as the cosine, which left the list non-monotone and could not feed belief-aware
   ranking, which re-sorts by score. `min_score` still floors each query's vector list.
7a. **Belief-aware ranking runs once, after fusion**, on the fused candidate set (up to `3 * num_results`),
   never per variant: a derived page and the note it cites can be retrieved by different queries, and only
   the fused list contains both. It follows the request's own rule: on by default in `hybrid`, opt-in in
   `vector`.
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
10. **The report gains "Query variants vs `<base>`"** per variants config: per-category paired nDCG@k
    difference with its interval, p50/p95, and the merge-bar reading (paraphrase interval above zero; no
    category wholly below zero; p50 <= 100 ms). Each variants config is read against the same config
    without variants (`lcg-passages-variants` against `lcg-passages`, `lcg-passages-hybrid-belief-variants`
    against `lcg-passages-hybrid-belief`). The H1/H2 "best `lcg-*`" selection excludes variants rows so
    those readings keep their meaning.
11. **Eval configs.** `lcg-passages-variants` (vector, default list) and, opt-in, `lcg-passages-belief-variants`
    and `lcg-passages-hybrid-belief-variants`, each sending the mode's own request plus `queries`.

## Deferred

- **Rerank on the fused candidates, against the primary query (FR-007), and `lcg-passages-variants-rerank`
  (FR-017)**: rerank (#796) is not on `main`. The later PR must rerank once, on the fused list.
- **`knowledge_graph_search` `queries` (FR-019)**: #797 (#801) is not on `main`. Follow-up.
- **`believed_only` / `as_of` on passage search**: not passage-search parameters (they are edge-read
  parameters, ADR-0705). Belief handling for passages is `belief_aware` (ADR-0789), which variants honour as
  described in 7a.

## Consequences

- A caller that never sends `queries` sees no change.
- Cost with N variants is one batched embed plus N+1 retrievals run concurrently (one connection each).
- The merge bar (paraphrase improvement, no regression, p50 <= 100 ms) is measured on v2 with the committed
  fixture. The first measurement, of the vector-only per-variant-weight design, missed it (decision 4);
  the design here (full pipeline per query, fixed variant budget, belief once after fusion, concurrent
  retrieval) has not been re-measured yet.
