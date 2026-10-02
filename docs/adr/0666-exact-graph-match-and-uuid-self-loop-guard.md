# ADR-0666: Exact Graph Match Before Salvage, UUID-Level Self-Loop Guard, Short-Code Veto

**Status**: Accepted
**Date**: 2026-10-02
**Issue**: #666 (remainder of #645; builds on [ADR-0650](0650-identifier-mismatch-veto-for-extraction-dedup.md))

## Context

#650 shipped the identifier-mismatch veto, which covers #645's digit-sibling cases. Four gaps
remained, all in extraction (`crates/core/src/episode.rs`, `identifier_veto.rs`). A missed merge is
recoverable; a wrong merge or a wrongly re-pointed edge is not, short of re-ingestion, so every
decision below is biased toward conservatism.

1. Off-list endpoint salvage ([ADR-0051](0051-edge-endpoint-salvage-and-deferred-drop.md))
   cosine-matched a missing endpoint against **only the batch's own entities**. An endpoint naming
   an entity ingested earlier (`Foo` stored, `Fooz` in the batch) was rewritten onto the merely
   similar batch entity.
2. The post-salvage self-loop filter compares names, and Phase C had no guard: two differently
   named endpoints resolving to one UUID were inserted as a self-loop.
3. The veto sees only digit tokens and single letters, so `ACDS` / `ACDM` still merged.
4. For `identity: true` kinds ([ADR-0616](0616-ontology-driven-kind-assignment.md)) fuzzy matching arguably
   should not happen at all.

## Decision

### 1. An exact stored-graph match beats salvage

Before embedding the off-list names for salvage, `add_episode` probes the persisted graph for each
unique one. The probe is `resolve_stored_endpoint`, the same function Phase C uses for its own
stored-graph lookup, so eligibility cannot drift: same group, case-insensitive, default kind plus
the group's identity-bearing kinds ([ADR-0616](0616-ontology-driven-kind-assignment.md)), scan fallback on a
`lookup_key` miss ([ADR-0283](0283-name-index-scan-fallback-for-endpoint-authority.md)).

- `Found` **and** `Ambiguous` both mean "an exact hit": the name is removed from the salvage set
  and is not embedded. Salvaging an ambiguous name onto a batch entity would be the guess
  [ADR-0615](0615-kind-scoped-entity-identity.md) forbids; Phase C drops it as ambiguous.
- The name is left untouched. Phase C re-resolves it under the write lock and remains the sole
  authority (ADR-0051); the probe only decides "skip salvage".
- The probe is lock-free like Phase B, so the [ADR-0029](0029-name-first-entity-resolution.md)
  TOCTOU caveat applies: an entity created by a concurrent ingest in between can still lose to
  salvage, exactly as before.
- Cost: one scan-fallback lookup per unique off-list name that misses (Phase C repeats it under
  the lock), bounded by the deduplicated set. Exact hits save the embed call.

Consequence: `edges_dropped_unresolvable` can rise for identity-flagged groups, because an
ambiguous exact hit is no longer rescued by salvage. That is the intended conservative outcome.

### 2. UUID-level self-loop guard in Phase C

After both endpoints resolve, `src_uuid == dst_uuid` drops the edge (logged) and increments
`edges_dropped_self_loop`, a top-level field on `AddEpisodeResult` and `knowledge_process_chunk`.
It is **not** folded into `dropped_edges`/`edges_dropped_unresolvable` (ADR-0051 promises one
`dropped_edges` entry per unresolvable drop and `UnresolvedEndpoint` has no self-loop variant) and
not into `DedupPathCounts`, which describes Phase B entity-resolution paths.

### 3. Short all-caps code veto

`identifier_mismatch` gains a second condition. A *code token* is a whitespace-separated token of
the **original-case** name that, after trimming punctuation, is 2–6 alphabetic characters, all
uppercase, and not a Roman numeral. The veto fires only on **mutual exclusion**: each side has a
code token the other lacks.

| Vetoed | Not vetoed |
|---|---|
| `ACDS`/`ACDM`, `UK`/`USA`, `US Army`/`UK Army`, `ACDS Platform`/`ACDM Platform` | `IBM`/`International Business Machines`, `IBM`/`IBM Corp`, `NASA`/`nasa`, `PROJECT AURORA DOCS`/`Project Aurora`, `REST API`/`RESTful API`, `PostgreSQL`/`Postgres`, `New York`/`New York City` |

Plain set inequality was rejected: it would stop acronym-to-expansion aliases (`AWS` / `Amazon Web
Services`). Deferring to the opt-in LLM check ([ADR-0652](0652-llm-verified-extraction-dedup-via-extractor.md))
was rejected because `dedup_mode` defaults to `veto-only`, which would leave `ACDS`/`ACDM`
unprotected by default. Roman numerals (strict grammar) are excluded so the documented ADR-0650
gap (`World War II` / `World War III`, `Henry VII` / `Henry VIII`) is unchanged and its tests still
pass. The predicate is shared, so salvage, brute-force and hybrid dedup all pick the rule up with
no call-site change; `vetoed` and `salvage_vetoed` may rise for all-caps names.

**Remaining gaps** (fall back to the embedding, then to the opt-in LLM check): a shout-case name
against a lone mixed-case one (`ACDM` vs `Acme`), codes longer than six characters, and codes
where one side has no all-caps token.

### 4. Identity kinds keep fuzzy matching (rejected)

Disabling embedding-path merges and salvage onto `identity: true` kinds was considered and **not
adopted**:

- Phase B is already kind-scoped ("dedup never compares across kinds", ADR-0616), so the
  cross-kind merge `identity: true` exists to prevent is already impossible.
- The flag targets stable types such as `Person` and `Organization`, where aliases
  (`Brett Adamson` / `Brett A.`) are the main legitimate use of the embedding path. Disabling it
  would fragment those entities for no new protection; the wrong-merge risk is covered by the
  veto (including §3) and by ADR-0652.
- It would silently change identity semantics ADR-0616 defines as "decided once, at creation".

Related gap left alone (out of scope): salvage is kind-blind, so it can rewrite an off-list name
onto a batch entity of an identity kind. Phase C then resolves it by name, which stays unique
unless the name collides across kinds.

## Consequences

- WAL replay is unaffected: it re-executes recorded mutations and never runs `add_episode`.
  No schema change; the response gains one additive field.
- Existing bad merges are not repaired; recovery remains re-ingestion.
