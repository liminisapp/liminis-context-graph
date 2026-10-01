# ADR-0650: Identifier-Mismatch Veto for Extraction-Time Entity Dedup

**Status**: Accepted
**Date**: 2026-10-01
**Issue**: #650 (the LLM dedup check originally scoped here moved to #652)

## Context

Phase B entity resolution ([ADR-0029](0029-name-first-entity-resolution.md)) merges an extracted
entity into an existing one when the exact-name match misses and the best name-embedding
candidate scores cosine ≥ `DEDUP_THRESHOLD` (0.85) and the dedup adapter confirms. The default
adapter always confirms, so every deployment merges on cosine alone.

bge-base name embeddings treat numbers and identifiers as near-noise: `ADR 2018` / `ADR 2019`
score 0.946, `lcg 0.15.0` / `lcg 0.16.2` 0.971, while genuine aliases such as `PostgreSQL` /
`Postgres` score 0.934. No threshold separates them, and a wrong merge is unrecoverable without
re-ingestion. The edge-endpoint salvage step ([ADR-0051](0051-edge-endpoint-salvage-and-deferred-drop.md))
reuses the same threshold and has the same blind spot.

## Decision

A pure, deterministic **identifier-mismatch veto** (`crates/core/src/identifier_veto.rs`) sits in
front of every extraction-time embedding merge. Two names whose *distinguishing-token* sets
differ — including when only one side has any — are never duplicates.

### Tokenization

Start from `prompts::normalize_name` (control-strip, trim, lowercase). The name index's own
normalization (`compute_lookup_key`, ADR-0221) is only `trim().to_lowercase()` and does no
tokenization, so the veto adds its own tokenizer on top. Split on whitespace; per token:

- **Digit-bearing tokens.** Strip a leading `#`, trim surrounding non-alphanumerics (internal `.`
  is kept, so `0.15.0` is one token), split on `-`/`–`/`—`, and keep each piece containing a
  `char::is_numeric` character. `GPT-4` and `GPT 4` therefore agree; `COVID-19` → `{19}`;
  `#611` → `611`. `Web 2.0` vs `Web2.0` still differ (conservative). CJK names are a single
  whitespace token, so any digit difference vetoes.
- **Standalone single-letter designators** (`Phase A`, `Plan B`, `Vitamin C`, `World War I`): a
  token that is, after trimming, exactly one letter. `E-mail` / `T-shirt` do not qualify.
  Two exemptions keep ordinary names merging: a **trailing-period initial** (`Brett A.`,
  `Alice B. Smith`) and a **leading `a` of a multi-token name** (the article in
  `A Tale of Two Cities`).

### Placement: at candidate selection, not after

Both dedup paths return a single best candidate. Vetoing *after* that would let a vetoed best
match (`Postgres 15`) hide a valid lower-ranked alias (`PostgreSQL`). The veto therefore filters
during selection, via name-aware variants in `db.rs`
(`brute_force_similar_entity_kind_for_name`, `hybrid_dedup_similar_entity_kind_for_name`) that
rank candidates (similarity desc, uuid asc — identically on both paths), discard vetoed ones, and
return `DedupSelection { candidate, vetoed }`. The original functions keep their signatures and
behaviour, so `tests/db_dedup.rs` and the `benches/dedup_*` R-003 gate are untouched. The veto
runs *before* `state.dedup.is_duplicate`, so it behaves identically under any adapter and #652
can replace the adapter freely.

The salvage step applies the same predicate to its in-memory name comparison.

### Out of the veto's reach

The exact case-insensitive name path, `knowledge_merge_entities`, WAL replay (which re-executes
recorded mutations and never runs Phase B) and every non-extraction path. Existing WALs replay to
identical graphs; only newly extracted chunks differ.

### Observability

`episode::DedupPathCounts` — one named field per path — is returned per chunk on
`AddEpisodeResult.dedup_paths` and as an additive `dedup_paths` object on
`knowledge_process_chunk`: `exact_name`, `embedding_merge` (veto passed, adapter confirmed),
`vetoed` (above-threshold candidates existed, *all* were vetoed, entity inserted — counted once
per incoming entity, so it is testable against decisions), `adapter_rejected`, and
`salvage_vetoed`. **Extensibility contract for #652:** add `llm_confirmed` / `llm_rejected`
fields (and repurpose `adapter_rejected` if appropriate) without reshaping call sites. The
counters are per chunk; there is no process-lifetime view, avoiding `AppState` churn. Startup
logs `dedup: mode=<passthrough | local-adapter (LCG_DEDUP_LLM)> + identifier veto` once from
`main.rs`.

## Consequences

- Entities differing only by a number or identifier are never merged by extraction.
- **Accepted recall loss** (a missed merge leaves two entities; a wrong merge is unrecoverable):
  `Python 3` / `Python`, `Web 2.0` / `Web2.0`, mixed alphanumerics such as `K8s` aliases.
- **Known gap:** `Brett A.` vs `Brett B.` is *not* vetoed, because both are exempt initials; unit
  tested as documented behaviour.
- **Known gap:** Roman numerals other than a bare `I` are not tokens, so `World War II` /
  `World War III` and `Henry VII` / `Henry VIII` are not vetoed; unit tested as documented
  behaviour.
- The leading-article exemption applies only to the letter `a`, so `C Programming` /
  `D Programming` and `X Corp` / `Y Corp` are vetoed.
- Existing bad merges are not repaired; recovery is re-ingestion.
- The concurrent-ingest TOCTOU residual risk of ADR-0029 is unchanged.
