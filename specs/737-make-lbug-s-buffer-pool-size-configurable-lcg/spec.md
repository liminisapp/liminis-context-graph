# Feature Specification: Make lbug's buffer pool size configurable (LCG_BUFFER_POOL_BYTES)

**Feature Branch**: `fabrik/issue-737`
**Created**: 2026-11-13
**Status**: Specified
**Input**: User description: "## Problem

`crates/core/src/db.rs` opens the database with `lbug::SystemConfig::default()`. That gives lbug its default buffer pool, about 80% of system memory, and there is no way for an operator to change it.

In a memory-limited container this leaves too little for everything else in the process tree, such as an embedder sidecar and the host application. The pool grows with the working set, so the failure shows up late. It appears once a graph gets big enough, typically during a full WAL rebuild on first start:

- a 4 GB container that had run for weeks started being OOM-killed every 3–5 minutes during startup rebuild;
- each restart began the rebuild again, so the daemon never became ready (`busy (rebuilding)` indefinitely);
- the only remedy available was to quadruple the container's memory, which moves the failure point rather than bounding it.

It may be worse where lbug reads physical memory rather than the cgroup limit, since the 80% is then taken from the host.

## Proposal

Make the buffer pool size configurable, and pass it through to `SystemConfig` when opening the database:

- an env var, e.g. `LCG_BUFFER_POOL_BYTES` (accepting a plain byte count; optionally a suffix such as `2GiB`), plus the matching CLI flag / config key if the daemon has one;
- unset keeps today's behaviour (the lbug default), so this is opt-in and backwards compatible;
- log the effective pool size at startup, so an operator can see what was applied;
- optionally `LCG_MAX_THREADS` for the same reason, since lbug also defaults its worker count from the host.

Also worth documenting in the operations docs: recommended sizing for containers, and that the first start after an lbug version change (FTS rebuild) is the memory peak.

## Acceptance

- Setting the variable bounds lbug's buffer pool to the given size; the startup log shows it.
- Unset, behaviour is unchanged.
- A full WAL rebuild of a graph larger than the pool completes (slower, but no OOM) under a container limit of pool size plus about 1.5 GB."

## Background

The service opens its embedded graph database (lbug) with lbug's stock configuration. By default lbug sizes its buffer pool at roughly 80% of system memory and its worker thread count from the host's CPUs, and the operator has no way to override either. This is acceptable on a dedicated machine but not in a memory-limited container where the service shares the memory budget with other processes (an embedder sidecar, the host application).

Because the pool grows with the working set, the problem is latent: a deployment can run fine for weeks and then fail once the graph is large enough, most visibly during a full WAL rebuild on first start. In the reported incident a 4 GB container was OOM-killed every 3–5 minutes during startup rebuild; every restart restarted the rebuild, so the service never became ready (`busy (rebuilding)` indefinitely). The only available remedy was to quadruple container memory, which moves the failure point rather than bounding it. Where lbug reads host physical memory instead of the cgroup limit, the 80% default is taken from the host and the problem is worse.

The service is already configured through `LCG_*` environment variables (see the operations and configuration docs), so an env-var knob is consistent with existing operator conventions. The first start after an lbug version change (which triggers a full-text-search index rebuild) is the memory peak and is worth calling out in operator guidance.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Bound the buffer pool with an environment variable (Priority: P1)

An operator running the service in a memory-limited container sets `LCG_BUFFER_POOL_BYTES` to a size that leaves room for the other processes. The service opens the database with a buffer pool of that size and tells the operator what it applied.

**Why this priority**: This is the core of the request and the only thing that prevents the OOM-restart loop. Everything else is supporting.

**Independent Test**: Start the service with `LCG_BUFFER_POOL_BYTES=1GiB` against a graph, check the startup log reports an effective pool of 1 GiB, and confirm the database opened with that bound. Start again without the variable and confirm behaviour and the reported pool match the previous (lbug default) behaviour.

**Acceptance Scenarios**:

1. **Given** `LCG_BUFFER_POOL_BYTES=2147483648`, **When** the service opens the database, **Then** lbug's buffer pool is bounded to 2147483648 bytes and the startup log states that effective size.
2. **Given** `LCG_BUFFER_POOL_BYTES=2GiB`, **When** the service opens the database, **Then** the result is identical to the plain-byte-count value for 2 GiB.
3. **Given** `LCG_BUFFER_POOL_BYTES` is unset, **When** the service opens the database, **Then** lbug's default pool is used, behaviour is unchanged from today, and the startup log indicates the default is in effect.
4. **Given** a graph larger than the configured pool and a container limit of pool size plus about 1.5 GB, **When** a full WAL rebuild runs, **Then** the rebuild completes without the process being OOM-killed (slower than with an unbounded pool is acceptable).

---

### User Story 2 - Fail clearly on an invalid setting (Priority: P1)

An operator mistypes the value (`2 gigs`, `-1`, `0`, empty). The service tells them exactly what was wrong rather than silently falling back to the 80%-of-memory default, which would reintroduce the failure the setting exists to prevent.

**Why this priority**: A silent fallback defeats the purpose of the setting and would be discovered only as a late OOM.

**Independent Test**: Start the service with each of several malformed values and confirm startup fails fast with an error naming the variable and the offending value.

**Acceptance Scenarios**:

1. **Given** `LCG_BUFFER_POOL_BYTES=banana`, **When** the service starts, **Then** it exits with an error naming `LCG_BUFFER_POOL_BYTES` and the rejected value, and does not open the database with a default pool.
2. **Given** `LCG_BUFFER_POOL_BYTES=0`, **When** the service starts, **Then** it is rejected as invalid in the same way.

---

### User Story 3 - Bound the worker thread count (Priority: P2)

An operator also sets `LCG_MAX_THREADS` so lbug's worker count is not derived from the host's CPU count, which is misleading inside a CPU- or memory-limited container.

**Why this priority**: Requested as optional in the issue; it addresses the same host-vs-container mismatch but is not what causes the OOM.

**Independent Test**: Start the service with `LCG_MAX_THREADS=2` and confirm the startup log reports 2 effective threads; start without it and confirm the lbug default is unchanged.

**Acceptance Scenarios**:

1. **Given** `LCG_MAX_THREADS=2`, **When** the service opens the database, **Then** lbug is configured with at most 2 worker threads and the startup log states that value.
2. **Given** `LCG_MAX_THREADS` is unset, **When** the service opens the database, **Then** lbug's default thread count is used.
3. **Given** `LCG_MAX_THREADS=0` or a non-numeric value, **When** the service starts, **Then** it exits with an error naming the variable and the rejected value.

---

### User Story 4 - Operator guidance for container sizing (Priority: P2)

An operator planning a container deployment reads the operations documentation and finds how to size the buffer pool relative to the container limit, and a warning that the first start after an lbug version change (full-text-search index rebuild) is the memory peak.

**Why this priority**: Without guidance the knob is hard to use correctly; but the code change is useful without it.

**Independent Test**: Read the operations and configuration docs; the new variables are listed with accepted formats and defaults, and the sizing recommendation and upgrade-peak warning are present.

**Acceptance Scenarios**:

1. **Given** the operations documentation, **When** an operator looks up memory sizing, **Then** it explains the recommended relationship between pool size and container limit (headroom of about 1.5 GB beyond the pool for the rest of the service) and the upgrade-time rebuild peak.
2. **Given** the configuration reference, **When** an operator looks up `LCG_BUFFER_POOL_BYTES` and `LCG_MAX_THREADS`, **Then** accepted formats, default (unset = lbug default) and the startup-log line are documented.

---

### Edge Cases

- Suffix handling: binary suffixes (`KiB`, `MiB`, `GiB`) are accepted, case-insensitively, with or without surrounding whitespace; a value like `2.5GiB` or an unknown suffix is rejected rather than guessed.
- Values that overflow the platform's size type are rejected with a clear error.
- Empty string is treated as invalid (not as "unset").
- A pool smaller than lbug can operate with: the service surfaces lbug's own error at open time rather than silently raising the value; the error message makes clear the configured value was the cause.
- A configured pool larger than available memory is accepted as-is (operator's responsibility); it is reported in the startup log.
- Every code path that opens the database (normal start, degraded-mode recovery, rebuild-from-WAL, any admin/recovery tool that reopens the DB) must honour the setting, not only the first open.
- Legacy `GRAPHITI_*` aliases: new variables are `LCG_*` only, since there is no legacy name to preserve.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The service MUST read an optional `LCG_BUFFER_POOL_BYTES` environment variable and, when set, open the database with the buffer pool bounded to that many bytes.
- **FR-002**: `LCG_BUFFER_POOL_BYTES` MUST accept a plain non-negative-integer byte count and a count with a binary suffix (`KiB`, `MiB`, `GiB`; case-insensitive).
- **FR-003**: When `LCG_BUFFER_POOL_BYTES` is unset, the service MUST behave exactly as it does today (lbug's default buffer pool).
- **FR-004**: The service MUST log the effective buffer pool size once at startup, in human-readable form, and MUST indicate when the lbug default is in effect.
- **FR-005**: An invalid `LCG_BUFFER_POOL_BYTES` (unparseable, zero, empty, unknown suffix, overflow) MUST cause startup to fail with an error that names the variable and the rejected value; the service MUST NOT silently fall back to the default.
- **FR-006**: The service MUST read an optional `LCG_MAX_THREADS` environment variable and, when set, configure lbug with at most that many worker threads; unset keeps lbug's default.
- **FR-007**: An invalid `LCG_MAX_THREADS` (non-numeric, zero, empty) MUST fail startup with an error naming the variable and the rejected value.
- **FR-008**: The service MUST log the effective thread setting at startup alongside the buffer pool line, indicating when the default is in effect.
- **FR-009**: The configured values MUST apply to every database open in the process (initial open, degraded-mode recovery, rebuild/recovery operations), not only the first.
- **FR-010**: The operations documentation MUST describe container sizing guidance (pool size relative to container limit, with about 1.5 GB headroom for the rest of the service) and state that the first start after an lbug version change (FTS rebuild) is the memory peak.
- **FR-011**: The configuration reference MUST list `LCG_BUFFER_POOL_BYTES` and `LCG_MAX_THREADS` with accepted formats, defaults and behaviour on invalid input.
- **FR-012**: The change MUST NOT alter the IPC protocol, the on-disk database format, or WAL contents.

### Key Entities

- **Buffer pool size**: The upper bound on memory lbug uses to cache database pages; the main lever on the service's memory footprint during large rebuilds.
- **Worker thread limit**: The maximum number of threads lbug uses for query/index work.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With `LCG_BUFFER_POOL_BYTES` set, the startup log shows the requested size as the effective pool size on every start.
- **SC-002**: With the variable unset, the service's startup behaviour and existing test suite results are unchanged.
- **SC-003**: A full WAL rebuild of a graph larger than the configured pool completes without an OOM kill when the container limit is the pool size plus about 1.5 GB.
- **SC-004**: 100% of invalid values for either variable produce a startup failure whose message names the variable and the bad value, with no database opened.
- **SC-005**: The operations and configuration docs contain the sizing guidance and the upgrade-time memory-peak warning, and list both variables.

## Assumptions

- Configuration is via environment variables only; the daemon's existing configuration convention for engine tuning is `LCG_*` env vars, so no new CLI flag or config-file key is added. Adding one later is a separate, small follow-up.
- Binary (1024-based) suffixes only; decimal suffixes (`GB`) and fractional values are not accepted, to avoid ambiguity.
- Failing fast on invalid input is preferred over warn-and-fall-back, because a silent fallback reintroduces the failure mode this setting exists to prevent.
- A zero value is invalid for both variables rather than meaning "default" or "unlimited".
- The "pool size plus about 1.5 GB" headroom figure from the issue is documented as a starting recommendation, not a guarantee; actual headroom depends on the embedder sidecar and host application.
- Minimum viable pool size is determined by lbug; the service does not impose its own floor.

## Out of Scope *(optional)*

- Automatically detecting the container (cgroup) memory limit and deriving a pool size from it.
- Changing the default pool size or thread count when the variables are unset.
- Reducing the intrinsic memory cost of the FTS rebuild or WAL rebuild beyond what the pool bound provides.
- A CLI flag or config-file key for these settings.
- Exposing the effective values through `knowledge_status` (possible follow-up).
- Other lbug `SystemConfig` options (e.g. compression, checkpoint threshold, max DB size).

## Source References *(optional)*

- Issue #737 and the reported 4 GB container OOM-restart loop during startup WAL rebuild
- `docs/operations.md` and `docs/configuration.md` (existing `LCG_*` conventions)
