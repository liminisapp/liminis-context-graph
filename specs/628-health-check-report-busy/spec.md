# Feature Specification: health_check reports `busy` instead of blocking behind the write lock

**Feature Branch**: `fabrik/issue-628`
**Created**: 2026-09-29
**Status**: Specified
**Input**: User description: "knowledge_health_check: report 'busy' instead of blocking behind write_lock during rebuild (implements #612)"

## Background

The IPC liveness method (`health_check`, implemented by `handle_health_check` in `crates/core/src/handlers.rs`; the issue calls it `knowledge_health_check`, but the wire name is `health_check`) awaits a shared acquisition of the service's write lock before probing the database. A non-dry-run `knowledge_rebuild_from_wal` holds that lock exclusively for the whole replay, and any other long write-lock holder does the same. A health check issued during that window queues until the holder finishes, which takes minutes on a large corpus.

Supervisors and orchestrators use short-timeout liveness probes. They read a busy-but-working daemon as dead and restart it mid-rebuild. The next boot starts the rebuild over, so on a large corpus it never completes (community report in #612, which describes the supervisor and orchestrator failure modes in detail).

The health check must answer promptly regardless of what holds the write lock, and must tell the caller "alive, but busy" rather than say nothing.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Supervisor probes a daemon that is rebuilding from WAL (Priority: P1)

A process supervisor or orchestrator probes the daemon with a short timeout while a long `knowledge_rebuild_from_wal` runs. It gets an immediate "alive, busy rebuilding" answer with progress, so it leaves the daemon alone and the rebuild completes.

**Why this priority**: This is the reported failure. Restart loops make large-corpus rebuilds impossible.

**Independent Test**: Start a rebuild job whose replay lasts well over 30 s. Call `health_check` twice a few seconds apart. Both calls return within about 100 ms with `state: "busy"`, `activity: "rebuilding"` and a `progress` object whose counters advance between the calls.

**Acceptance Scenarios**:

1. **Given** a running rebuild job holding the write lock, **When** a client calls `health_check`, **Then** the response arrives within about 100 ms with `ok: true`, `state: "busy"`, `activity: "rebuilding"`, the rebuild's `job_id`, and its progress (mutations replayed, WAL files processed/total, elapsed seconds).
2. **Given** the same rebuild, **When** `health_check` is called twice a few seconds apart, **Then** the progress in the second response is at or beyond the first, and advances while the replay runs.
3. **Given** the rebuild finishes and releases the lock, **When** `health_check` is called, **Then** it returns `state: "healthy"`.

---

### User Story 2 - Health check during any other long write (Priority: P1)

Some other operation holds the write lock, such as a WAL admin operation or a synchronous rebuild. The health check still answers immediately and says the daemon is busy writing. It reports no progress it does not have.

**Why this priority**: The root cause is queuing behind the lock, not rebuild specifically.

**Independent Test**: Acquire the write lock in a test with no rebuild job registered. Call `health_check`. It returns `busy` with `activity: "writing"` promptly.

**Acceptance Scenarios**:

1. **Given** the write lock is held for writing and no rebuild job is `running`, **When** `health_check` is called, **Then** it returns immediately with `ok: true`, `state: "busy"`, `activity: "writing"` and no `progress` or `job_id`.

---

### User Story 3 - Idle daemon behaves exactly as today (Priority: P1)

With nothing holding the write lock, existing callers see the same responses as before.

**Why this priority**: The change must be additive, so existing clients keep working.

**Independent Test**: Existing health-check tests, plus the readiness polling in `socket_service_e2e.rs`, pass unchanged.

**Acceptance Scenarios**:

1. **Given** the DB is open and no writer holds the lock, **When** `health_check` is called, **Then** it returns `{"ok": true, "healthy": true, "state": "healthy"}` after probing the DB.
2. **Given** the DB is not loaded, **When** `health_check` is called, **Then** it returns `ok: false`, `healthy: false`, `state: "degraded"` and `reason`, with or without a writer active.

---

### User Story 4 - Supervisor authors can rely on a documented contract (Priority: P2)

An operator writing a liveness or readiness probe reads the docs and learns what each state means, and which field to gate readiness on.

**Why this priority**: The states only help if supervisors interpret them consistently.

**Independent Test**: The docs list all four outcomes: `busy`, `healthy`, `degraded`, and no answer.

**Acceptance Scenarios**:

1. **Given** the updated docs, **When** an operator looks up `health_check`, **Then** they find: `busy` is alive and not ready, `healthy` is alive and ready, `degraded` is alive but unusable, and no answer means dead or wedged.

---

### Edge Cases

- **The write lock is released between the try-acquire and the response.** The response reflects the instant of the attempt. A `busy` answer that is stale by microseconds is acceptable.
- **A rebuild job entry exists but is finished (`completed` or `failed`) and the lock is held by something else.** Report `activity: "writing"`, not `rebuilding`. Only a job whose status is `running` counts.
- **More than one rebuild job is `running`.** Report one deterministically; the choice is decided in Research and Plan.
- **A synchronous (non-job) rebuild or startup replay holds the lock without a `rebuild_jobs` entry.** It reports `activity: "writing"` at minimum. Whether it is registered so it can report `rebuilding` with progress is decided in Research. A second progress system is out of bounds if `rebuild_jobs` can cover it.
- **The lock is held for writing while the DB is mid-clear.** The `busy` path must not connect to or query the DB, so it never observes a half-cleared database.
- **The DB is not loaded (`degraded`).** This is unchanged and takes precedence; it never reports `busy`.
- **A tokio `RwLock` write waiter is queued behind read holders.** `try_read()` fails while a writer is waiting. The response is then `busy` even though no writer holds the lock yet. This is acceptable and must be documented as "a write is pending or in progress".
- **The `rebuild_jobs` mutex is briefly contended or poisoned.** The health check must not block on it or fail. It falls back to `activity: "writing"` without progress.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `health_check` MUST NOT await the write lock. It MUST attempt a non-blocking shared acquisition and answer immediately whether or not it succeeds.
- **FR-002**: If the non-blocking attempt succeeds and the DB is loaded, `health_check` MUST probe the DB and return `{"ok": true, "healthy": true, "state": "healthy"}`, unchanged from today.
- **FR-003**: If the non-blocking attempt fails because the lock is write-held or a write is pending, `health_check` MUST return `ok: true`, `state: "busy"` and an `activity` field, without connecting to or querying the DB.
- **FR-004**: In the `busy` response, `healthy` MUST be `false`. `busy` means alive but not ready. The documented readiness signal (`result.healthy == true` / `state == "healthy"`) must not fire for a busy daemon. See Assumptions.
- **FR-005**: If a rebuild job in `state.rebuild_jobs` is `running`, the `busy` response MUST include `activity: "rebuilding"`, that job's `job_id`, and a `progress` object carrying the same progress values `knowledge_rebuild_status` reports: mutations replayed, WAL files processed and total, and elapsed seconds.
- **FR-006**: If the lock is write-held and no rebuild job is `running`, the `busy` response MUST report `activity: "writing"` and MUST NOT include `progress` or `job_id`.
- **FR-007**: When the DB is not loaded, `health_check` MUST return `ok: false`, `healthy: false`, `state: "degraded"` and `reason`, exactly as today. This check comes first, so `degraded` is never masked by `busy`.
- **FR-008**: Reading rebuild progress for the response MUST NOT block. It must use a non-blocking read or a bounded-cost read of existing state, and must not depend on the write lock.
- **FR-009**: The response change MUST be additive: a new `state` value (`busy`) and optional `activity`, `progress` and `job_id` fields. Existing fields and the `healthy` and `degraded` shapes are unchanged.
- **FR-010**: Rebuild correctness and duration MUST be unaffected. `health_check` acquires nothing that can delay or reorder the rebuild's own lock acquisition.
- **FR-011**: A test MUST assert that the `busy` path performs no DB access, for example by answering promptly while the write lock is held and the DB is in a state where connecting or probing would fail or hang.
- **FR-012**: Documentation MUST describe the health-check contract: `busy` is alive and not ready, `healthy` is alive and ready, `degraded` is alive but unusable, and no answer is dead or wedged. It MUST also update the existing readiness guidance (`docs/ipc-mcp-reference.md`, `docs/operations.md`, and the generated `docs/llms-full.txt` if it is kept in sync by a script) to say that pollers keep waiting on `busy`.
- **FR-013**: The change MUST be checked for callers that reject unknown `state` values: `crates/core/tests/ipc_parity.rs`, the socket e2e readiness helper, and the Python `service_protocol.py` side described in CLAUDE.md. Any in-repo caller that would misread `busy` MUST be fixed in this change. Any out-of-repo caller MUST be called out in the PR description.
- **FR-014**: If the MCP tool registry (`crates/service/src/mcp/tools.rs`) exposes a health-check tool, its description MUST be updated to match. If it does not, the PR MUST record that no MCP change was needed.

### Key Entities *(if the feature involves data)*

- **Health response**: the `health_check` result, with `ok`, `healthy`, `state` (`healthy` | `busy` | `degraded`) and, when busy, `activity` (`rebuilding` | `writing`), an optional `progress` and an optional `job_id`. When degraded it also carries `reason`.
- **Rebuild job**: the existing entry in `state.rebuild_jobs`, the source of progress and `job_id`. It is not modified by this change, except possibly for registering synchronous rebuilds (decided in Research).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: During a rebuild whose replay exceeds 30 s, every `health_check` call returns within about 100 ms with `state: "busy"` and `activity: "rebuilding"`.
- **SC-002**: Progress in two `health_check` responses taken several seconds apart during a running rebuild advances between the calls.
- **SC-003**: While a non-rebuild write holds the lock, `health_check` returns `busy` with `activity: "writing"` promptly.
- **SC-004**: With no write-lock holder, `health_check` responses are byte-for-byte unchanged for `healthy` and `degraded`, and the existing suite passes without modification to its health assertions.
- **SC-005**: A test proves the `busy` path answers while the lock is held and touches no DB connection.
- **SC-006**: Rebuild wall-clock time and resulting graph content are the same with and without concurrent health polling.

## Assumptions

- **`healthy: false` on `busy`.** The issue's example payload shows `"healthy": true` on `busy`. Its own contract, "`busy`: alive, not ready", contradicts that. Existing readiness pollers gate on `result.healthy == true` (see `wait_until_healthy` in `crates/service/tests/socket_service_e2e.rs` and the "Readiness" docs), so `healthy: true` would make a rebuilding daemon look ready. This spec therefore sets `ok: true` (alive) and `healthy: false` (not ready). Supervisors that only need liveness read `ok` or a successful response. The reviewer may reverse this.
- The wire method name is `health_check`; `knowledge_health_check` in the issue refers to the same handler.
- `state.rebuild_jobs` is the single source of progress. A separate progress mechanism is added only if Research shows some lock-holder cannot be covered by `rebuild_jobs`.
- The degraded-mode allow-list is unaffected. `health_check` stays allowed in degraded mode, and the `degraded` path never looks at the write lock.
- Patch-sized: no schema, storage or WAL format change.

## Out of Scope

- Changing which operations hold the write lock, or shortening rebuild duration.
- Making other read methods non-blocking behind the write lock (for example `knowledge_status`).
- Adding an `activity` taxonomy beyond `rebuilding` and `writing`, such as `startup_replay` or `indexing`, unless Research shows a cheap way to cover them.
- Changes to the Liminis Electron app or its Python client beyond what is needed to confirm compatibility. Out-of-repo callers are flagged, not edited.
- Supervisor or orchestrator configuration.

## Source References

- Issue #612 (community report of the supervisor restart loop)
- `crates/core/src/handlers.rs`: `handle_health_check` (about line 248), the rebuild handler (~3039–3403), `knowledge_rebuild_status` (~3487)
- `crates/core/src/app_state.rs`: `write_lock`, `rebuild_jobs`
- `crates/service/tests/socket_service_e2e.rs`: `wait_until_healthy`
- `docs/ipc-mcp-reference.md` (Readiness), `docs/operations.md` (readiness paragraph, `health_check`)
- `specs/26-service-handshake-methods/spec.md`; ADR-0009 (degraded-mode allow-list)
