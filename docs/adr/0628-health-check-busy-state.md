# ADR-0628: `health_check` Reports `busy` Instead of Blocking Behind `write_lock`

**Status**: Accepted
**Date**: 2026-09-29
**Issue**: #628 (implements community report #612)

## Context

`handle_health_check` awaited a shared acquisition of `state.write_lock` before probing the DB. A
non-dry-run `knowledge_rebuild_from_wal` holds that lock exclusively for the whole replay, so a
health check issued meanwhile queued for minutes. Supervisors with short probe timeouts read the
silence as a dead daemon and restarted it mid-rebuild; the next boot started the rebuild over, so
on a large corpus it never completed.

## Decision

`health_check` never awaits the write lock.

1. DB not loaded → `degraded`, exactly as before. This is checked first, so `busy` never masks it
   (ADR-0009).
2. `write_lock.try_read()` succeeds → probe the DB and return `healthy`, byte-identical to before.
3. `try_read()` fails → return `{"ok": true, "healthy": false, "state": "busy", "activity": ...}`
   **without connecting to or querying the DB** (the DB may be mid-clear or mid-swap, ADR-0003).
   `activity` is `rebuilding` — with `job_id` and `progress` (`mutations_replayed`,
   `wal_files_processed`, `wal_files_total`, `elapsed_seconds`, as in `knowledge_rebuild_status`) —
   when `state.rebuild_jobs` holds a `Running` job (earliest `start_time` wins), else `writing`
   with no progress. `rebuild_jobs` is read with `try_lock`; contention or poison falls back to
   `writing`.

**`healthy: false` on `busy`.** The issue's example showed `healthy: true`, contradicting its own
"alive, not ready" contract. Existing readiness pollers gate on `result.healthy == true`, so
`true` would make a rebuilding daemon look ready. `ok: true` carries liveness; `healthy` carries
readiness.

**Fairness semantics.** tokio's `RwLock` is write-fair, so `try_read()` also fails while a writer is
merely queued. `busy` therefore means "a write is pending or in progress". Batched passes
(ADR-0030) alternate between `busy` and `healthy`. A successful `try_read()` guard still blocks new
writers during the probe, as before, so the rebuild's own lock acquisition is unaffected.

## Known gaps (accepted)

- A streaming (`_progress_token`) rebuild takes the lock inline and is not in `rebuild_jobs`: it
  reports `writing` without progress. Registering it in `rebuild_jobs` would alter the existing-job
  dedup, `knowledge_rebuild_status` listings and the shutdown abort loop — a follow-up.
- A job is registered `Running` before its task takes the write lock (and, with `force_clear`,
  after the group is cleared). In that window `try_read()` succeeds and `health_check` reports
  `healthy`. Closing it needs a `dry_run`/kind field on `RebuildJob`.
- Startup work runs before the accept loop, so probes then get no answer (unchanged).

## Consequences

Additive wire change: new `state` value `busy` plus optional `activity`, `progress`, `job_id`.
`healthy` and `degraded` shapes are unchanged. There is no MCP change: the MCP registry has no
health-check tool (ADR-0035). Out-of-repo callers that treat any `ok: true` as ready must gate on
`healthy` instead.
