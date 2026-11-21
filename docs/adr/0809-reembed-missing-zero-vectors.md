# ADR-0809: `knowledge_reembed_missing` — Repair Zero-Vector Embeddings Across All Kinds

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #809 (implements the main proposal of #799; consumes the record built by #808)
**Relates to**: ADR-0808 (the durable zero-vector record and replay retry policy), ADR-0379 (indexed-column writes, drop/write/rebuild), ADR-0470 (summary backfill), ADR-0526 (vectors are a local cache), ADR-0440 / ADR-0790 (recompute on replay, document role), ADR-0742 (`--writable-groups`), ADR-0025 / ADR-0036 (auto-heal and `indices_built`)

## Context

A row stored with a fallback zero vector is invisible to vector search for good. ADR-0808 made
those rows *known* — a durable `(group, kind, uuid)` record, counted in `knowledge_status` — but
nothing acted on it. The only repairs were `knowledge_backfill_summary_embeddings`, which covers one
column and re-embeds every entity unconditionally, and a full `knowledge_rebuild_from_wal`, which
takes hours on a large group. Nothing could repair `name_embedding`, `fact_embedding` or
`content_embedding`.

All four embedding columns carry a database-wide HNSW index, and lbug rejects a `SET` on an indexed
column (ADR-0379). A repair must therefore drop the index, write, and rebuild — and because the
index spans every group, repairing one group blocks writes everywhere and rebuilds a database-wide
structure while it is down.

## Decision

### 1. An admin tool that repairs exactly the recorded rows

`knowledge_reembed_missing(group_id, kinds?, dry_run?)` is an admin-scope streaming tool. Candidates
are the entries `list_zero_vector_rows` returns for the group, filtered by `kinds` (`name`, `fact`,
`summary`, `content`; default all). It never scans rows or reads a stored vector to detect a zero.
Each row is re-embedded from its co-located source text (entity name / summary, edge fact, episode
content — the pairing replay's recompute uses), through the document-role embedder path and the
retry/backoff/splitting policy of ADR-0808 (`EmbedRetryPolicy::from_env`, so the `LCG_REPLAY_EMBED_*`
variables apply; their names say "replay" but the policy is the same).

The result is `{group_id, dry_run, kinds: {<kind>: {candidates, reembedded, failed,
skipped_no_source}}, cancelled?, index_rebuild_errors?, write_error?}`; all four kinds are always
present. `candidates` is the number of *recorded* rows of the kind, so
`reembedded + failed + skipped_no_source = candidates` and a dry run agrees with the real run.

### 2. Embed first, then lock and drop

The summary backfill holds the write lock from its candidate read through the rebuild and embeds
inside the index-drop window. For a tool whose index is down database-wide, that is the wrong
order. Here the phases are: select and classify (brief read lock) → embed everything (no lock, no
index touched) → write (exclusive write lock; only the indexes of kinds that have a vector to write
are dropped, written, rebuilt). With nothing to write — no candidates, or every embed failed — no
lock is taken for repair work, no index is dropped and `indices_built` is not touched.

Because embedding happens outside the lock, the row's text can change before the write. The write
is therefore guarded — `MATCH (n {uuid: $uuid}) WHERE n.<text column> = $text SET …
RETURN count(*)` — and a row whose guard no longer matches counts as `failed` and stays recorded, so
a vector computed from stale text is never persisted and a re-run retries it. (Re-reading the text
under the lock was rejected: a second read pass for what the guard does in the same statement.)

### 3. One shared index lifecycle, restoring the prior `indices_built`

`index_lifecycle::with_dropped_indexes` owns the sequence — hold the write lock; remember
`indices_built` and store `false` *before* anything is dropped (so a racing search auto-heals via
`build_indices_once` instead of hard-failing); drop the named indexes; run the write closure; always
attempt to rebuild exactly the dropped indexes, even after a failed write; restore the **prior**
`indices_built` only if every rebuild succeeded, and otherwise leave it `false`. The summary backfill
is rewritten onto it. DDL goes through an `IndexDdl` trait so tests can record ordering and inject a
rebuild failure; per-index create/drop methods were added to `db.rs` (the aggregates now call them).

The one behavioural difference from the pre-#809 backfill is the flag. It used to store `true`
unconditionally. `indices_built` is a single flag for *all* indexes (FTS and lookup too), so storing
`true` after rebuilding one vector index could claim a missing FTS index was built whenever the flag
had been `false` beforehand. Restoring the prior value changes the backfill only in that already
broken case; a normal run still ends with `indices_built == true`, and the backfill now also
attempts its rebuild after a failed write instead of leaving the index dropped.

### 4. A failed rebuild is a successful result with an error field

By the time a rebuild fails, rows are already written and removed from the record, so the counters
are the truth and an `Err` would discard them. The result carries `index_rebuild_errors`, the flag is
left `false`, and the next search rebuilds every index through `build_indices_once` — at a latency
proportional to database size, which the docs call out.

### 5. Writes are unrecorded; the record is cleared per batch

Vectors are a local cache stripped from the WAL and recomputed on replay (ADR-0526), so a repair adds
nothing a replay could not recompute; the writes run through a prepared statement
(`conn.prepare(set_cypher(kind))` executed with `execute_prepared_returning_count`, which calls the
driver directly and never records a mutation), so nothing reaches the WAL. (The backfill stays WAL-recorded, as its behaviour must not change.) A row leaves the record
only after its new vector was written, per batch. A crash between the `SET` and the removal leaves a
real vector that is still recorded; the next run re-embeds it harmlessly.

### 6. Validation, stale rows and cancellation

- A vector of the wrong dimension, non-finite, or **all zeros** is a `failed` row and is never
  written. Replay's validator (`validate_recomputed_vector`) was made crate-visible and shared; the
  all-zero check is a separate `reject_zero_vector` only the repair calls, so replay semantics do not
  change.
- A recorded row that no longer exists has its stale record entry removed (the cleanup ADR-0808
  deferred to this tool) and counts `skipped_no_source`. A row with empty/whitespace text, or whose
  own `group_id` is not the requested group, counts `skipped_no_source` and stays recorded.
- Cancellation (client disconnect or shutdown) is honoured between embedding windows, where nothing
  is dirty. Once the write phase starts it always finishes and rebuilds, so a cancel can never strand
  a dropped index. `embed_with_retry` polls its cancel hook only on failure paths, so the loop polls
  it between windows itself.

### 7. Guard rails

Classified `GroupRequired` in `write_policy`, so `--writable-groups` refuses an unlisted group
before any embedder call, `dry_run` included. Registered in the admin MCP scope (55 tools, admin
bucket 17) and listed as streaming. No separate repair mutex: the exclusive write lock held from drop
through rebuild already serialises it against the backfill, `knowledge_build_indices`, rebuild and
recover.

## Consequences

- A group's recorded rows are repaired in time proportional to the number of rows plus one index
  rebuild per affected kind — not to the size of the graph, and far shorter than a full rebuild.
- While an affected index is down every write is blocked and vector search for every group waits for
  the rebuild. This is accepted (as for the backfill) and mitigated, not removed: embed-before-drop,
  per-kind drops, the zero-candidate no-op, and `dry_run` first.
- Per-row text-guard `RETURN count(*)` runs only inside the write window, in batches of 100.
- The in-memory embedding cache is unbounded (ADR-0440); a very large repair grows it as a replay
  does.
- The liminis app's `service_protocol.py` needs the new method added (additive; out of this repo).

## Out of scope

Re-embedding renamed entities or detecting changed source text (ADR-0379 decision 6); repairing
zero-vector rows that are not in the record; per-group or incremental index maintenance; automatic or
scheduled repair.
