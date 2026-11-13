# ADR-0737: Configurable lbug Tuning via Strictly-Parsed Environment Variables

**Status**: Accepted
**Date**: 2026-11-13
**Issue**: #737

## Context

`Db::open` used `lbug::SystemConfig::default()`: a buffer pool of ~80% of system memory and a worker
count derived from the host's CPUs. In a memory-limited container this left too little for the rest
of the process tree; the pool grows with the working set, so a long-healthy deployment started being
OOM-killed during startup WAL rebuild, and since every restart began the rebuild again the service
never became ready.

## Decision

**Two opt-in environment variables**, `LCG_BUFFER_POOL_BYTES` and `LCG_MAX_THREADS`, are parsed by
`lbug_config::LbugTuning` and applied to `SystemConfig`. Unset keeps lbug's defaults (backwards
compatible). `LCG_*` only — there is no legacy `GRAPHITI_*` name to preserve, and no CLI flag or
config key.

**Parsing is strict; invalid input is fatal.** The pool accepts a positive integer with an optional
binary suffix (`KiB`/`MiB`/`GiB`, case-insensitive); zero, empty, fractions, decimal suffixes and
overflow are rejected, as are zero/non-numeric thread counts. A silent fallback to the
80%-of-memory default would reintroduce exactly the failure the setting exists to prevent, and
`0` cannot mean "default" because lbug itself treats `0` as auto-detect.

**Validate once, before any open.** `async_main` calls `LbugTuning::from_env()` ahead of workspace
migration (which opens the DB) and the socket bind, logs the effective values once, and exits on
error. This keeps a bad value away from the startup error classifier (`is_recoverable` matches
substrings of the message) so it can never be routed into degraded mode or autonomous recovery.

**Every open re-reads the environment.** `Db::open(path)` keeps its signature and calls
`LbugTuning::from_env()` then `Db::open_with(path, &tuning)`. Recovery, `full_rebuild`,
`knowledge_clear` and migration-validation reopens therefore honour the setting with no call-site
changes. Rule for contributors: **every lbug database open must go through `Db::open` /
`Db::open_with`** so the tuning applies. `open_with` also lets tests exercise a tuning without
mutating the process environment, which would race with the parallel test suite.

**Open failures are annotated, not rewritten.** When a knob is set and lbug fails to open, the
error is wrapped as `Error::Config` that names the configured values and embeds the original lbug
message verbatim, so substring classification of genuine corruption is unchanged.

## Consequences

- No IPC, on-disk or WAL change.
- The service imposes no floor or ceiling; lbug owns the minimum pool size.
- A rebuild of a graph larger than the pool completes more slowly rather than OOMing. That outcome
  (container limit ≈ pool + ~1.5 GB) needs a container run and is not asserted in CI.
- Surfacing the effective values in `knowledge_status` is a possible follow-up.
