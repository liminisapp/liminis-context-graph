# ADR-0900: `lcg-eval` Ingest Concurrency — a Connection Pool, Ordered Folding, and the Cassette Rule

**Status**: Accepted
**Date**: 2026-11-23
**Issue**: #900

## Context

`lcg-eval retrieval` ingested a corpus one `knowledge_process_chunk` at a time. A live extraction of
the v2 corpus took about three hours, almost all of it waiting on one LLM call after another; E17
measured an endpoint with six slots about 17% busy. The serialisation was entirely in the harness:
`ingest()` awaited each chunk, and `UdsClient` holds one locked connection per request/response.

The service does not need it. Phase A (embed and extract) and Phase B (dedup, judge, summary
consolidation) run without a lock, and only Phase C commits under `write_lock`.

## Decision

- **A pool of N connections, not a multiplexed socket.** The service reads and dispatches one line
  at a time per connection (`crates/service/src/main.rs`, `handle_connection`), so pipelining N
  requests on one socket would still run them serially. N in flight need N connections.
  `UdsPool` (in `lcg.rs`) implements `RpcTransport`, bounds calls with a semaphore, connects lazily
  and parks connections for reuse. `ServiceGuard::ingest_transport(n)` returns the guard's own
  single client at `n == 1` (exactly the old path) and a pool otherwise. Retrievers keep the
  single client, so queries stay serialised.
- **A connection is returned to the pool only after an `Ok` call.** A failed call, a timeout, or a
  future dropped mid-flight (a sibling chunk failed) may leave a partial write or an unread
  response on its socket; dropping the connection retires it with no extra bookkeeping.
- **`ingest_with(t, fx, n)` folds results in manifest order.** `stream::iter(jobs).map(call)
  .buffered(n)` keeps up to `n` calls in flight but yields results in input order, so `episode_note`,
  the counters and *which error is reported* do not depend on completion order. With several
  failures the earliest chunk in manifest order among those completed is reported. The first `Err`
  drops the stream: no new call starts and the client abandons the in-flight ones. The service
  cancels an abandoned chunk that is not yet committing (cancel-on-disconnect, #861) and finishes
  any commit already started, so on the error path `retrieval` and `retrieval-score-probe` stop the
  service with `knowledge_close` and wait for it to exit (`ServiceGuard::stop_gracefully`, bounded
  by `SHUTDOWN_GRACE`) instead of letting `kill_on_drop` SIGKILL it mid-commit, which would leave a
  kept workspace with a torn write. Supersessions,
  `knowledge_build_indices` and the entity-link pass run after the stream is drained. `ingest(t, fx)`
  is `ingest_with(t, fx, 1)`, so N = 1 is byte-identical and no caller changed.
- **The per-call timeout scales with N.** A call's 900 s clock also counts the time it queues
  inside the service (the sidecar pools of `UDS_POOL_SIZE = 4`, ADR-0039/0042, and the Phase C write
  lock), so a run that finishes at N = 1 could fail at N = 12 on queueing alone. A pool of `n`
  connections uses `RPC_TIMEOUT * ceil(n / 4)` (`pool_call_timeout`); one connection keeps the plain
  `RPC_TIMEOUT`.
- **The flag is `--ingest-concurrency N`** (integer ≥ 1, default 1; 0, negative and non-numeric are
  usage errors) on `retrieval` and on `retrieval-score-probe`, the other caller of `ingest`. It is
  recorded as `ingest_concurrency` in `report.json` `meta` and in the `## Reproduction` block. It is
  **not** added to the `inputs` digest block (ADR-0872), so N = 1 digests and baselines are
  unchanged. A kept workspace (`--keep-workspace`) records it as `ingest_concurrency` in its
  `lcg-eval-workspace.json` manifest, only when above 1 (a sequential manifest is byte-identical),
  and `graph-qa` prints a note for such a workspace, so an N > 1 graph is not read as an extraction
  or dedup regression. `retrieval-score-probe` warns when it replays an `order_dependent` cassette,
  as `retrieval` does.

### The cassette rule

Measured offline by `crates/core/tests/cassette_concurrent_record_replay.rs`: a content-deterministic
stub extractor in `RecordingExtractor`, four chunks that all mention one shared entity, ingested with
every chunk in flight (the last finishes extraction first, so it commits first), then replayed one
chunk at a time in manifest order.

| Recording | Replay | `extract` | `classify_*` | `consolidate_summary` |
|---|---|---|---|---|
| N = 1 | N = 1 | 0 misses | 0 | 0 |
| N = 4 | N = 1 | 0 misses | 0 | **3 of 3 missed** |

- **State-independent keys replay exactly at any order.** `extract` hashes chunk text, group,
  reference time, instructions and rendered prompts; `classify_*` hash only their arguments.
- **`consolidate_summary` and `judge_duplicates` hash graph state** (the existing summary, the
  candidate pairs), which depends on the order chunks committed in. So (a) "replay stays exact" is
  **ruled out**: a cassette recorded at N > 1 misses them when replayed in any other order.
- **The miss is silent.** The service turns a `consolidate_summary` miss into its bounded fallback
  with one stderr line, and a `judge_duplicates` miss into "not a duplicate". The run exits 0 with
  a possibly different graph. The measurement above completes with 3 misses and no error. Counting
  them would need a service change or stderr parsing, so no miss counter is added.

The rule (option (c) of the issue):

1. **Recording at N > 1 is allowed** but warned about on stderr, and the cassette sidecar
   (`<cassette>.meta.json`) gains `ingest_concurrency` and `order_dependent: true`. At N = 1 both
   keys are omitted, so the sidecar is byte-identical to before; `format` stays 1 and older sidecars
   still parse. Refusing would block the main use (record a fast live extraction once), and the
   extract/classify entries, which carry the expensive LLM calls, still replay exactly.
2. **Replay always runs at N = 1.** Replay at N > 1 would be non-deterministic as well, because
   which state each concurrent call sees varies with timing. `--ingest-concurrency` is overridden
   with a stderr warning, and the report states the effective value with
   `ingest_concurrency_requested` showing what was asked for. (Replay still makes live embedder
   calls; forcing N = 1 gives up that part of the speed-up.)
3. **Replaying an `order_dependent` cassette warns** (`preflight_cassette`) that state-keyed entries
   may miss and degrade silently.

### Interaction with ADR-0029

Phase B runs without the write lock, so chunks in flight together do not see each other's
uncommitted entities: two chunks mentioning the same new entity both insert it. ADR-0029 accepted
that race for concurrent ingest. `--ingest-concurrency` makes it reachable from the eval, so a live
run at N > 1 can contain extra duplicate entities, not merely reordered summaries, and can differ
from run to run. Comparisons against an N = 1 baseline are not like for like; the report shows N.

### Effective cap

With `--extractor-uds` / `--embedder-uds` the service's own sidecar pools are `UDS_POOL_SIZE = 4`
(ADR-0039, ADR-0042), so N > 4 queues inside the service. HTTP endpoints have no such cap.

## Consequences

- A re-extraction approaches baseline / N wall-clock time, bounded by endpoint capacity.
- N = 1 behaviour, call order, params, sidecar bytes and digests are unchanged.
- Concurrent cassettes are explicitly second-class: usable, marked, and replayed sequentially, with
  the silent-degradation limitation documented rather than detected.
- Ingest timestamps (`created_at`) across notes follow completion order at N > 1 (ADR-0673 keeps
  ingest time separate from event time); no retrieval tie-break was found to depend on it.

## Alternatives considered

- **Multiplexing one socket.** Needs a service change (out of scope) and gains nothing: the service
  serves one request at a time per connection.
- **`buffer_unordered`.** Loses the deterministic fold and error attribution.
- **Refusing `--record-cassette` at N > 1.** Blocks the fast-record workflow for a hazard that a
  marker and a warning cover.
- **A replay miss counter.** Requires a service counter or stderr parsing; out of scope.
- **Adding N to the `inputs` digest.** Would perturb or add schema to digests that must stay
  byte-identical at N = 1.
