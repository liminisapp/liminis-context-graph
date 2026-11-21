# ADR-0808: Replay Embedding Recompute — Bounded Retry, Window Splitting and a Durable Zero-Vector Record

**Status**: Accepted — supersedes, for the WAL-replay recompute path only, the "no retry on timeout" decision of ADR-0510 and ADR-0541, and closes the whole-window blast radius ADR-0486 accepted
**Date**: 2026-11-19
**Issue**: #808 (implements items 1 and 2 of #799's follow-up; the repair tool `knowledge_reembed_missing` is a separate issue that consumes the record built here)
**Relates to**: ADR-0440 (recompute on replay), ADR-0486 (batched recompute window), ADR-0510 / ADR-0541 (embedder timeouts, no retry-on-timeout), ADR-0526 (vectors are a local cache; skip vs. zero-fill), ADR-0047 (replay transaction boundaries), ADR-0737 (strict startup validation of env settings)

## Context

During a long WAL replay a transient embedder error (timeout, connection reset, HTTP 5xx/503, 429)
makes recompute fail for that call. `Embedder::embed_batch` is all-or-nothing, so one failure
failed a whole replay window (default 64 rows, up to four vector keys each). Each failed request
stored a same-dimension zero vector — invisible to vector search — and bumped
`ReplayStats::embeddings_recompute_failed`.

Two gaps compounded this (#799: a cold replay of ~80k mutations against a local CPU embedder ran
~4 h with intermittent `error sending request`, caused by a second client competing for it):

1. **No retry on timeout.** ADR-0510/ADR-0541 deliberately excluded retry-on-timeout from the
   *transport*: their purpose was to bound how long a hang holds the write lock, and a retry
   multiplies that hold.
2. **Failures were neither durable nor visible.** The count lived only in that replay's result,
   per-row logs stop after 10, and `knowledge_status` said nothing about it — an operator could not
   tell search was degraded, and nothing recorded *which* rows needed repair.

## Decision

### 1. Retry, split and bound — in replay only, above the transport

`resolve_embed_window` (the single place a window's embed request is issued) calls
`replay_embed_retry::embed_with_retry` instead of the embed closure directly.

- A **transient** error — timeout, connection failure/reset/broken pipe, HTTP 5xx (incl. 503),
  HTTP 429 — is retried with exponential backoff and jitter: attempt *n* waits
  `min(cap, base·2ⁿ)`, jittered uniformly in its upper half. Classification is
  `embedder::classify_embed_error`; HTTP errors are structured (`reqwest`), UDS errors are strings,
  so the UDS status message is built from one shared prefix constant and pinned by tests.
- When a multi-request call exhausts its retries it is **split** in half and the halves resolved
  recursively down to a single request, so one text the embedder can never process fails only its
  own row. A single-request call that exhausts its retries fails; it is not split further.
- A **permanent** error (other 4xx, response-shape mismatch) is never retried. On a multi-request
  call it splits once straight away — a batch-level 4xx is usually one text's doing, and acceptance
  requires that one poison text fail only itself — and a single request hitting one is attempted
  exactly once. Per-vector failures (wrong dimension, non-finite values) stay in the per-row
  validation and are never retried.
- Results are collected per request, so a retry or split can neither duplicate a row nor overwrite
  an embedding already obtained from another half. The #440 cache wrapper is unchanged and caches
  only successful sub-calls.

**What bounds it** (the reason this is a reversal of ADR-0510/0541 and not an unbounded one):

| Bound | Value |
|---|---|
| Attempts per request | `LCG_REPLAY_EMBED_RETRIES`, 0–10, default 4 (0 disables retry and splitting) |
| One backoff wait | `LCG_REPLAY_EMBED_BACKOFF_MS` (1–60000, default 500), capped by `LCG_REPLAY_EMBED_BACKOFF_MAX_MS` (1–300000, default 10000, ≥ base) |
| Slow work per window | At most `(retries+1)·(depth+2)+depth` *transiently failing* calls across the whole split tree, `depth = ⌈log₂ rows⌉` (46 for 64 rows at defaults). Successes and permanent errors are free — prompt, and structurally ≤ `2n-1`. Once spent, the rest of the window fails with no further calls |
| Dead embedder | If every request of a window failed transiently, the next window gets one attempt, no retry, no split, until a window succeeds — about one timeout per window, the pre-change cost, from the second window on (the first window of an outage, and the first after each recovery, can spend its whole budget, about 23 minutes at the defaults); a permanent error still splits while the breaker is open |
| Cancellation | Backoff sleeps in 100 ms slices polling `cancel_fn`; a cancelled window is dropped with nothing counted or recorded. A healthy window adds no `cancel_fn` polls |

Worst case for one request: `(retries+1)·LCG_EMBEDDING_TIMEOUT_MS + Σ backoff` — about 160 s with
the defaults.

**Why the reversal is safe here.** ADR-0510/0541's concern was a hung embedder holding the
write lock *indefinitely*. Replay already holds that lock for the duration of a rebuild and is the
one path where a failed row is silently degraded rather than failing the operation; every retry is
bounded as above and cancellable. Live ingest keeps the old rule: it has no zero-vector fallback (an
embedder failure fails the ingest), so there is nothing to repair, and its latency matters
interactively. The transport's own single retry of a definitely-broken connection is untouched.

The three settings are validated at service startup (`EmbedRetryPolicy::from_env`, called next to
`LbugTuning::from_env`, fatal and naming the variable, per ADR-0737) and again, lazily, when a
replay starts.

### 2. A durable per-group record in the database, written in the replay transaction

A row stored with a fallback zero vector *because the embedder failed* is recorded as
`(group, kind, uuid)` in a new lcg-only node table `EmbeddingZeroRow(id, wal_group, kind, uuid)`
(`id = "{group}|{kind}|{uuid}"`; the column is `wal_group`, not `group_id`, so a stray entry never
makes a group look "present" to the write-policy group enumeration; kinds `name`, `fact`, `summary`, `content`, derived from the vector
param). It is derived state: written through the unrecorded path, never reaching the WAL, and with
no counterpart in graphiti's schema (no `kuzu_driver.py` parity change, like `WalPosition`).

**Why a table rather than a sidecar.** `flush_batch` already runs each same-template batch in one
transaction (ADR-0047). The record's effects ride along with the rows (`ZeroEffect::Add`/`Clear`
carried through `ResolvedRow` and `ReplayBatch`) and are applied in that same transaction, so an
entry commits or rolls back with the zero-vector row it describes — no phantom entry after a
rollback, cancel or crash, no missing entry. A sidecar cannot be made atomic with the database
and survives a database wipe, leaving entries for rows that no longer exist. The database table
also follows `drop_lbug_wal` recovery together with the data, and is reset by anything that
rebuilds the database.

- **Excluded**: rows with no source text (`embeddings_recompute_skipped_no_text`), vector-only
  `SET`s that are skipped (`embeddings_skip_rows`) and `MATCH`-prefixed rows that matched nothing —
  none stored a zero vector because the embedder failed (#526).
- **Stale entries**: a successful recompute of a recorded `(kind, uuid)`, or a delete-form row for
  that uuid (a `uuid` param or a `uuids` list), emits a `Clear`, and a delete-form row carrying a
  `gids` list (a replayed group purge) emits a `ClearGroups` that drops the entries of those groups of the kinds that live
  on the deleted table (`Entity` → `name`/`summary`, `Episodic` → `content`, `RelatesToNode_` →
  `fact`), so a purge of one table never wipes entries for rows still present in another. Other delete shapes are not recognised (a relationship-hop delete removes no row;
  free-form Cypher can leave a stale entry for the repair tool to remove). To avoid a per-row `DELETE` across an 80k-row clean replay, `Clear`s
  are only produced when the group's record was non-empty at the start of the run or an `Add` has
  happened since.
- **Full rebuild**: `group_purge::purge_groups` (shared by `knowledge_delete_by_group` and the
  `force_clear` rebuild) clears the group's entries, so the rebuild records only its own failures.
- **API for the repair tool**: `Conn::zero_vector_counts[_all]`, `list_zero_vector_rows(group,
  kind?)`, `remove_zero_vector_rows(group, entries)` (an absent entry is a no-op) and
  `clear_zero_vector_rows`.
- A failure row with no `uuid` param cannot be keyed; it is logged (capped) and not recorded.

### 3. Status: `wal_groups.<gid>.zero_vector_rows`

`knowledge_status` adds, next to `embedding_model_status`, `zero_vector_rows`:
`{"total": N, "by_kind": {"name": …, "fact": …, "summary": …, "content": …}}` — always present,
all zeros when clean, exact past the 10-row log cap, durable across restart. One grouped count
query runs inside the already read-locked closure for all groups at once (counts, never a list), so
it adds no lock and does not lengthen the status path. A failed read degrades to zeros. The name is
`zero_vector_rows`, not `embeddings_recompute_failed`: that name already means "this run's
failures" in `ReplayStats`, while the new field is the *current persisted* count.

## Consequences

- A replay against an intermittently failing embedder completes with `embeddings_recompute_failed
  == 0` and no zero vectors, instead of zero-filling whole windows; one poison text costs one row.
- Retrying makes a replay against a *persistently* failing embedder slower: the first window of a
  dead-embedder run (and the first window after each recovery) can spend its whole transient-call
  budget — up to `budget × timeout`, about 23 minutes for a 64-row window at the defaults — before
  the breaker drops later windows to about the old cost of one timeout each. An intermittent outage
  can pay that budget repeatedly.
- Operators can see how many rows are degraded, and which, after any replay or recovery; repair
  tooling has a stable list/remove contract to build on.
- Identity stamping is unchanged: `embeddings_recompute_had_no_failures()` still gates stamping the
  embedding identity per run, so a later clean run can stamp it while old entries remain. The record
  is the authority on degraded rows; the docs say so.
- Several *transient* poison texts in one window can spend its budget; the unresolved rows then
  fail and are recorded, so they remain repairable rather than lost.
- Adds one table to the schema (idempotent `CREATE ... IF NOT EXISTS`); older databases gain it on
  open. Readers tolerate its absence.

## Alternatives considered

- **Retry inside the transport.** Would also change live ingest and the startup probe and reverse
  ADR-0510/0541 globally. Rejected.
- **Sidecar file for the record.** Not atomic with the stored rows and survives a database wipe.
  Rejected (above).
- **Record at resolve time, not at flush.** Leaves phantom entries on rollback or cancel. Rejected.
- **Naive recursive split with a full retry budget per sub-request.** A 64-row window yields up to
  127 sub-requests, so a dead embedder would stall a window for hours. Rejected in favour of the
  per-window budget and cross-window breaker.
- **Retrying a permanent batch-level error in place.** Pointless — it fails identically. It
  splits once instead.
