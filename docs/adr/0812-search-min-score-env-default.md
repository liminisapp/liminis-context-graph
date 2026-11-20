# ADR-0812: Operator-Configurable Default `min_score` for Passage Search

**Status**: Accepted
**Date**: 2026-11-19
**Issue**: #812 (implements #811)

## Context

`knowledge_search_passages` used `min_score` 0.5 whenever a request omitted it. The right floor
depends on the embedding model and corpus: in the #811 report a CPU bge-base embedder scored
relevant passages at 0.73–0.77 and unrelated ones at 0.58–0.62, so 0.5 let the wrong passages
through. Most callers never set `min_score` and cannot know the embedder's score distribution; the
operator who chose the embedder can.

## Decision

1. **`LCG_SEARCH_MIN_SCORE` sets the default `min_score`.** Unset is 0.5, exactly as before. A finite
   value is clamped to 0.0–1.0, as the request parameter is. An explicit numeric request
   `min_score`, including `0.0`, always wins. A null or non-numeric request value still falls back
   silently to the default (unchanged behaviour).
2. **Strict validation, unlike `LCG_PASSAGE_SEARCH_MODE` / `LCG_PASSAGE_BELIEF_AWARE`
   (ADR-0789, ADR-0806).** An empty, non-numeric, NaN or infinite value fails startup with a message
   naming the variable and the value, following ADR-0737. A typo must not silently leave the wrong
   floor in place. The check sits before any database open and socket bind, and after the attached
   `--connect` early return (the daemon owns the variable). Rust's `f64` parser accepts `nan`,
   `inf` and `infinity`, so finiteness is checked explicitly. Whitespace is trimmed, as in ADR-0737.
3. **Resolved once at startup, not read per request.** `main` installs the value into a process-wide
   `OnceLock` (`search_config::install`), as `write_policy` does for the write allow-list; the
   handler reads it with a 0.5 fallback when nothing was installed (in-process tests, attached
   mode). Not an `AppState` field: that struct has ~94 constructor sites across ~62 files. The
   effective default is logged once at startup.
4. **Visible in `knowledge_status`** as an additive top-level `search_min_score_default`, added by a
   wrapper so every response shape (degraded, queryable, not queryable) carries it.
5. **Hybrid semantics are unchanged.** The floor applies only to the vector retriever's candidates;
   it is not a floor on the fused `score`. The MCP schema keeps `"default": 0.5` and only its
   description changes. No tool is added.

## Consequences

- With the variable unset nothing changes.
- A once-per-process `OnceLock` means in-process tests can install one value; each env-sensitive
  test lives in its own integration binary, and clamping is covered by pure unit tests.
- `min_similarity` on entity/relationship search (ADR-0629) deliberately has no default and is
  untouched. The eval harness removes the variable from the spawned service's environment and
  sends an explicit `min_score: 0.0`, so it is unaffected.
