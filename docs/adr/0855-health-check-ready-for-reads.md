# ADR-0855: `health_check` Gains `ready_for_reads`; `ok` / `healthy` / `ready_for_reads` Semantics

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #855 (implements #832). Follow-up to [ADR-0628](0628-health-check-busy-state.md).

## Context

Since ADR-0628, `health_check` answers `{ok: true, healthy: false, state: "busy"}` whenever the
write lock is held or a writer is queued: any ingest by any client, or a long
`knowledge_rebuild_from_wal`. ADR-0628 chose `healthy: false` on purpose, so readiness pollers that
gate on `healthy == true` do not mistake a rebuilding daemon for one ready for work.

Clients written before ADR-0628 treat `healthy` as liveness. Any ingest now makes them conclude LCG
is unavailable, although reads work throughout (they do not take the write lock). Three facts were
squeezed into two booleans: *alive*, *idle and accepting writes*, and *reads are served now*. The
last had no field.

## Decision

Keep ADR-0628's behaviour exactly, and add `ready_for_reads: bool` to every response shape.

| Field | Meaning | Gate on it for |
|---|---|---|
| `ok` | The process is alive and answered. | Liveness / restart decisions. |
| `healthy` | Idle and ready for writes: DB loaded, no write pending or in progress. | Sending writes; waiting for a rebuild to finish. |
| `ready_for_reads` | The database is loaded, so reads are served now. True while `busy`. | Sending reads. |

Per-`state` truth table:

| `state` | `ok` | `healthy` | `ready_for_reads` |
|---|---|---|---|
| `healthy` | `true` | `true` | `true` |
| `busy` (`activity: "writing"` or `"rebuilding"`) | `true` | `false` | `true` |
| `degraded` | `false` | `false` | `false` |

`ready_for_reads` is a constant per branch of `handle_health_check`: the `degraded` branch (DB not
loaded) is evaluated first and reports `false`; every branch after it has a loaded DB and reports
`true`. The busy path still never waits on the write lock and never touches the DB, so
`ready_for_reads: true` there follows from the DB being loaded, not from a probe.

`ready_for_reads` means "the DB is loaded", not "every read will succeed": indexes may still be
rebuilding, and a read racing a mid-clear rebuild can fail.

## Consequences

- Additive wire change; existing clients ignore the new key. No IPC parity or MCP registry change.
- `busy` still reports `healthy: false`. `healthy` was **not** redefined as liveness: that would
  undo ADR-0628's protection for write-readiness pollers.
- No MCP health-check tool is added (`health_check` stays non-MCP, ADR-0035). The semantics are
  documented in the MCP section of `docs/ipc-mcp-reference.md` instead.
- ADR-0628's known gaps are unchanged: the pre-lock window in which a registered rebuild job still
  reports `healthy`, and streaming rebuilds reporting `activity: "writing"` without progress.
  `ready_for_reads` is `true` in both.
- Raw-socket clients that treated `healthy` as liveness should use `ok` for liveness and
  `ready_for_reads` for read availability.

## Alternatives considered

- **Amend ADR-0628.** Rejected: blurs what 0628 decided with what was added later.
- **Make `healthy` mean liveness again.** Rejected: breaks write-readiness pollers (ADR-0628).
- **Probe the DB on the busy path to compute `ready_for_reads`.** Rejected: the busy path must not
  touch the DB (it may be mid-clear), and "loaded" already answers the question.
