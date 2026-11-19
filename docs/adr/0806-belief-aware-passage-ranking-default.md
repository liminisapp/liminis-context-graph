# ADR-0806: Belief-Aware Passage Ranking Is the Default

**Status**: Accepted — partly supersedes ADR-0789 decisions 2 and 9
**Date**: 2026-11-19
**Issue**: #806 (follows #789 / ADR-0789)

## Context

ADR-0789 added belief-aware re-ranking to `knowledge_search_passages` behind a `belief_aware`
parameter: on by default in `hybrid`, off by default in `vector`, so the vector default stayed
byte-for-byte unchanged. It was then measured on the v2 corpus (244 questions, nDCG@10, k = 10):

| config | overall | temporal | p50 |
|---|---|---|---|
| `lcg-passages` (vector, raw) | 0.568 | 0.628 | 24 ms |
| `lcg-passages-belief` | 0.649 | 0.802 | 25 ms |
| `lcg-passages-hybrid-belief` | 0.652 | 0.802 | 27 ms |

Vector + belief beats vector by +0.081 [+0.063, +0.100] overall and +0.174 [+0.114, +0.238] on
temporal questions at unchanged latency. Hybrid + belief is indistinguishable overall and worse on
paraphrase (0.111 vs 0.161). Every client that omits `belief_aware`, including the Liminis Notes
agent, was missing the gain.

## Decision

1. **Belief-aware ranking is on by default in both modes.** The effective value is resolved as
   explicit request boolean, then the workspace switch, then `true`
   (`search::resolve_belief_aware`). A `null` request value counts as omitted; a non-boolean is
   still an invalid-params error.
2. **The opt-outs run the untouched raw code paths.** `belief_aware: false` in `vector` mode calls
   `search::search_passages` and in `hybrid` mode `search_passages_hybrid(.., false)`. Nothing was
   refactored, so the output is byte-for-byte the pre-change output. Tests compare the handler's
   response with those functions called directly.
3. **`LCG_PASSAGE_BELIEF_AWARE` is the workspace switch**, read per request like
   `LCG_PASSAGE_SEARCH_MODE` (ADR-0789 decision 8), with no `AppState` field. `off`, `0` and
   `false` disable; `on`, `1` and `true` enable; trimmed and case-insensitive. Unset or empty means
   unset. Anything else is ignored and warned about once per process. The switch is independent of
   mode, so with it off `hybrid` yields weighted RRF without the rules. That is what
   `belief_aware: false` gives, but not the pre-change hybrid default (rules on); accepted, because
   the switch means "turn the default off".
4. **The default mode stays `vector`.** Unchanged, and still overridable by `LCG_PASSAGE_SEARCH_MODE`
   or the request `mode`.
5. **The eval's `lcg-passages` tracks the shipped default.** It sends neither `mode` nor
   `belief_aware`, which reverses ADR-0789 decision 9 ("the eval pins its mode"): the point is to
   catch a regression in the default. To keep that honest the harness removes
   `LCG_PASSAGE_SEARCH_MODE` and `LCG_PASSAGE_BELIEF_AWARE` from the spawned service's environment.
   The new opt-in `lcg-passages-raw` (`mode: "vector"`, `belief_aware: false`) keeps the old
   baseline measurable. The other `lcg-passages-*` configurations keep sending both parameters.
   `lcg-passages` and `lcg-passages-belief` are now the same request.

## Consequences

- Results are re-ordered by default wherever a corpus has derived pages that cite a primary note or
  superseded passages, and a demoted passage's `score` is lowered as well. Callers that depend on
  cosine score thresholds or the old order opt out per request or per workspace. This is recorded
  as a behaviour change in the CHANGELOG.
- Vector mode now overdraws `3 × num_results` and reads supersession state on every default call;
  measured at about +1 ms p50.
- No other read method, request shape or response shape changes; the MCP registry is untouched
  (text only).
