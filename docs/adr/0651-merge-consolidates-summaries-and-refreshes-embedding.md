# ADR-0651: Entity Merge Consolidates Summaries and Always Re-embeds `summary_embedding`

**Status:** Accepted
**Date:** 2026-10-01
**Issue:** #651 (implements #647)
**Partially supersedes:** [ADR-0470](0470-entity-summary-embedding.md) Decision 2, for extraction merges only
**Relates to:** [ADR-0526](0526-vectors-are-a-local-cache.md) (vectors are a local cache),
[ADR-0543](0543-narrow-write-lock-around-embedder-round-trip.md) (no embedder I/O under `write_lock`)

## Context

When extraction merged an extracted entity into an existing one (exact-name path or
embedding-candidate path), the merged summary was `format!("{} {}", existing, extracted)`. Over a
multi-chunk document a frequently mentioned entity's summary grew into a run-on of per-chunk
descriptions that often contradicted each other, and — since summaries are searchable (FTS, summary
vector search #470, agents reading summaries) — fed that noise into retrieval. The merge also wrote
only `summary`; `summary_embedding` stayed the embedding of the *first* summary ever extracted, so
summary vector search ranked on a meaning the text no longer had.

ADR-0470 Decision 2 had accepted that staleness because lbug's HNSW index was believed to reject a
plain `SET` on an indexed column.

## Decision

1. **Consolidate, don't concatenate.** After a merge has been decided (consolidation never affects
   *whether* entities merge), `episode.rs` Phase B produces one bounded summary
   (`crates/core/src/summary_merge.rs`):
   - incoming empty, or a substring of the current summary → keep the current summary: **no
     extractor call, no re-embed**;
   - current summary empty → use the incoming one (capped), no call;
   - otherwise → `Extractor::consolidate_summary(name, existing, incoming)`, a small chat call on
     the configured extractor (Anthropic or OpenAI-compatible; `LlmRouter` forwards with its usual
     primary/fallback), prompted to merge into one concise, current description, preferring the most
     recent / most specific statement on conflict and dropping superseded claims.
2. **Lock-free and cancellable.** The call sits in the Phase B decision loop next to
   `dedup.is_duplicate`, wrapped in the same `tokio::select!` on `cancel_token`; it never runs under
   `write_lock`. Phase C only `SET`s precomputed values.
3. **Bounded on every path.** `MERGED_SUMMARY_CAP = 600` chars applies to extractor output
   (`cap_summary`: cut at the last sentence boundary that fits, else hard-truncate) and to the
   deterministic fallback (`fallback_merge`: `existing + " " + incoming`, then drop whole *leading*
   sentences until it fits — most recent text wins; a single over-long sentence keeps its last 600
   chars). A sentence boundary is `.`/`!`/`?` followed by whitespace or end of text. An existing
   summary already over the cap (legacy run-on) is sent to the extractor as-is and the output is
   capped; the substring skip leaves it untouched.
4. **Failure degrades, never fails the chunk.** `Error::Config` (the trait's default impl, and
   `UnconfiguredExtractor`) falls back silently; any other error logs one `eprintln!` line and
   falls back; an empty reply falls back. Eval cassettes record and replay the call
   like the other LLM calls (`call_type: consolidate_summary`, key hashed over the names, both
   summaries and the rendered prompts); an unrecorded request is a loud `CassetteMiss`, which the
   merge path logs and degrades to the fallback. `LlmRouter` tries the fallback model for a failed
   consolidation call but does not latch `primary_failed`: consolidation is best-effort and must
   not demote the primary model for extraction.
5. **Always re-embed, in the same statement.** After the loop, one lock-free, cancellable
   `embed_batch` covers every merged summary that changed. An embedder error fails the chunk (as the
   pre-lock embedding pass does) rather than leaving a silently stale vector. Phase C then runs
   `SET e.summary = $summary, e.summary_embedding = $summary_embedding`.
6. **WAL / replay (FR-009, FR-010).** Before this change replay did *not* re-embed a merged
   summary — the merge `SET` never named `$summary_embedding`, so live and rebuilt databases agreed
   only because both kept the first vector. Now the logged template names `$summary_embedding`,
   `log_mutation` strips the vector (ADR-0526) and replay recomputes it from the co-located
   `summary` — no replay change was needed, replay applies the recorded summary verbatim and never
   touches an `Extractor`. `summary` and `summary_embedding` must stay in one statement: a separate
   vector-only `SET` would have no text and replay would skip it.
7. **Within-chunk chaining.** Phase B keeps a per-existing-uuid running summary, so a second merge
   into the same entity in one chunk consolidates on top of the first. Previously each decision
   started from the pre-chunk summary and the last Phase C `SET` silently discarded earlier merges.
   Only the latest merge per uuid carries the embedding; earlier ones write nothing, so the WAL
   holds one merge `SET` per uuid per chunk.
8. **One call per changed merge**, not per-chunk batches (FR-004 is "where practical"): chaining
   needs each call to see the previous result, a plain-text-in-JSON single-entity reply is the most
   robust against model output errors, and the skip rules already remove most calls.

### Probe: the HNSW write restriction no longer holds on lbug 0.21.0

The research stage expected the single `SET` to fail on a live database because
`entity_summary_embedding_idx` is an HNSW index (ADR-0470 Decision 2; `update_entity_core`'s doc
comment). `probe_plain_set_on_indexed_summary_embedding` (`tests/summary_semantic_search.rs`) shows
that on the pinned lbug 0.21.0 a plain `SET` on the indexed `summary_embedding` succeeds *and the
index follows the update* (the new vector is the nearest neighbour of itself; the old vector no
longer lands on the row). So the planned drop-index / `SET` / rebuild bracket, with its O(entities)
HNSW rebuild per merging chunk and missing-index window, was **not implemented**, and incremental
WAL-tail replay into an indexed database is not affected either. The probe stays as a regression
test: if a future lbug re-introduces the restriction it fails here, and the bracket from
`backfill_summary_embeddings.rs` is the documented remedy.

## Consequences

- Entity summaries after a merge are one concise description within 600 chars; summary vector
  search for merged entities reflects the current text; live and WAL-rebuilt databases embed the
  same way.
- Each *changed* merge costs one extra small LLM call plus (per chunk) one extra embed batch;
  unchanged merges cost nothing. No new CLI flag, schema, IPC or MCP surface (FR-012).
- ADR-0470 Decision 2's "accepted staleness" no longer applies to extraction merges. It still
  describes the **re-assert** path (`knowledge_assert_entity` / `update_entity_core`), which is
  deliberately unchanged here; given the probe, refreshing the vector there is now cheap and a
  candidate follow-up, and `reassert_with_changed_summary_leaves_summary_embedding_stale` still pins
  today's behaviour.
- Existing graphs keep their run-on summaries; they are only bounded when next merged into with
  new information. Wrongly merged entities (#650's territory) cannot be split automatically;
  recovery is re-ingestion.
