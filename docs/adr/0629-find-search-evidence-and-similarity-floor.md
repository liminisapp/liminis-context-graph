# ADR-0629: Search Evidence and a `min_similarity` Floor for `find_entities` / `find_relationships`

**Status**: Accepted
**Date**: 2026-09-30
**Issue**: #629 (implements #613)

## Context

`knowledge_find_entities` and `knowledge_find_relationships` were pure top-k searches. Reciprocal
rank fusion (RRF) discards every input list's raw score, and each vector list returns its nearest
neighbours however distant, so a gibberish query filled every slot and a correct name hit was
followed by unrelated filler. The caller could not tell them apart. Exposing only the fused RRF
score would not help: it is rank-based, so the top hit of a gibberish query and of a perfect one
score almost identically.

## Decision

1. **Evidence is carried through fusion.** `rrf_fuse_scored` returns, per item, the fused score and
   each input list's raw value (BM25 score or cosine distance), `None` for a list that did not
   retrieve it. Each result gains an additive `search` object; similarities are `1 - distance`,
   and a non-finite value is reported as `null` so NaN never reaches JSON.
2. **The floor acts before fusion, on vector lists only.** `min_similarity` filters each vector
   candidate list (name, summary, fact). The lists are distance-ascending, so this trims a tail and
   leaves surviving ranks unchanged. BM25 lists are never filtered: a full-text hit stays eligible
   whatever its vector similarity, and reports `null` for a path whose candidate was floored out.
3. **Clamped, no default.** The value is clamped to 0–1 like `search_passages`' `min_score`, and a
   non-number is an error. Unlike `min_score` (default 0.5) there is no default: unset means no
   floor, so default results and order are unchanged.
4. **Evidence lives in wrapper types, not on the row structs.** `ScoredEntity` / `ScoredEdge`
   flatten `EntityRow` / `RelatesToEdge` beside `search`. The row structs are shared with the WAL
   and other read tools; leaving them alone avoids changing every constructor site and any other
   tool's serialised shape. The existing public search functions keep their signatures and wrap the
   scored variants with no floor.
5. **`rrf_fuse` keeps its contract.** Extraction-time dedup (`hybrid_dedup_similar_entity_kind`)
   uses its order. `rrf_fuse` and `rrf_fuse_scored` share one private accumulate-and-sort helper, so
   they cannot disagree on order and dedup behaviour is unchanged by construction.

## Consequences

- The IPC change is additive (one optional parameter, one new per-result object) with no storage
  schema change. The Python `service_protocol.py` (Liminis app repo) can adopt the fields later.
- With `kind`, the floor is applied on top of the kind path's bounded ANN probe, so a rare kind's
  recall can only shrink further; this is documented on the tool.
- A `text_match_only` flag (#613) and a BM25 score floor are out of scope; callers can filter on
  `text_match`.
