# Feature Specification: health_check: add ready_for_reads and document ok / healthy semantics

**Feature Branch**: `fabrik/issue-855`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Implements #832.

## Problem
Since ADR-0628, `knowledge_health_check` returns `{ok: true, healthy: false, state: \"busy\"}` whenever the write lock is held or a writer is queued (`crates/core/src/handlers.rs` ~320–353). Any ingest by any client causes it. Clients written before ADR-0628 treat `healthy` as liveness, so they decide LCG is unavailable, even though reads work throughout because they don't take the write lock.

## Change
- **Keep ADR-0628's behaviour exactly.** `busy` still reports `healthy: false`.
- **Add `ready_for_reads: bool`** to every response shape: true when the database is loaded, including while busy, and false when degraded.
- **Document the fields:** `ok` means liveness, `healthy` means idle and ready for writes, `ready_for_reads` means reads are served now. Put this in the ADR-0628 follow-up (a new ADR or an amendment), `docs/ipc-mcp-reference.md` and the MCP tool description.
- **CHANGELOG:** note the 0.16-era change in what `healthy` means, for raw-socket clients, under the docs section.

## Acceptance
- With the write lock held, the response is `{ok: true, healthy: false, state: \"busy\", ready_for_reads: true}`.
- When idle, `ready_for_reads` is true. When degraded, it's false.
- Existing health-check tests are unchanged, apart from the added field."

## Background

Since ADR-0628 (#628), `health_check` never waits on the write lock. While a write is pending or in progress (any ingest, or a long `knowledge_rebuild_from_wal`) it answers `{ok: true, healthy: false, state: "busy", activity: ...}`. ADR-0628 deliberately chose `healthy: false` for `busy` so that readiness pollers gating on `healthy == true` don't mistake a rebuilding daemon for a ready one.

The side effect: clients written before ADR-0628 treat `healthy` as a liveness signal. Any ingest by any client now makes those clients conclude LCG is unavailable, even though reads work throughout (they do not take the write lock). Three distinct facts are currently squeezed into two booleans: *alive* (`ok`), *idle and accepting writes* (`healthy`), and *reads are served now* (no field). This issue adds a third field for the last fact and documents all three, without changing ADR-0628's behaviour.

Current docs state the contract only in terms of `ok`/`healthy`/`state` (`docs/ipc-mcp-reference.md` "Health check contract" and "Readiness"; ADR-0628). The MCP tool registry has no health-check tool (`health_check` is classified non-MCP; ADR-0035 / ADR-0628), so "the MCP tool description" has no existing counterpart (see Assumptions).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Read-only client keeps working during an ingest (Priority: P1)

A client (including a raw-socket client predating ADR-0628) polls `health_check` to decide whether LCG can serve queries. While another client is ingesting, `health_check` reports `busy`; the client reads `ready_for_reads: true` and keeps issuing reads instead of declaring LCG unavailable.

**Why this priority**: This is the reported bug and the core value of the change.

**Independent Test**: Hold the write lock, call `health_check`, and assert the response is `{ok: true, healthy: false, state: "busy", ..., ready_for_reads: true}`.

**Acceptance Scenarios**:

1. **Given** the DB is loaded and the write lock is held (or a writer is queued), **When** `health_check` is called, **Then** the response has `ok: true`, `healthy: false`, `state: "busy"` and `ready_for_reads: true`, plus the existing `activity` (and `job_id`/`progress` while rebuilding).
2. **Given** the DB is loaded and no write is pending, **When** `health_check` is called, **Then** the response has `healthy: true`, `state: "healthy"` and `ready_for_reads: true`.
3. **Given** the DB is not loaded (degraded), **When** `health_check` is called, **Then** the response has `ok: false`, `healthy: false`, `state: "degraded"`, `reason`, and `ready_for_reads: false`.

---

### User Story 2 - Integrator understands what each field means (Priority: P1)

An integrator reading the docs can tell, for each of `ok`, `healthy` and `ready_for_reads`, what it signals and which one to gate on for liveness, writes, and reads.

**Why this priority**: The bug arose from ambiguous semantics; the new field is only useful if its meaning is documented next to the other two.

**Independent Test**: Read the ADR follow-up and `docs/ipc-mcp-reference.md`; each states `ok` = liveness, `healthy` = idle and ready for writes, `ready_for_reads` = reads are served now, with the response table including the new field for every `state`.

**Acceptance Scenarios**:

1. **Given** the reference docs, **When** an integrator looks up the health-check contract, **Then** the three fields' meanings and the per-`state` values are listed.
2. **Given** a raw-socket client written before ADR-0628, **When** its maintainer reads the CHANGELOG, **Then** they find a note that `healthy` stopped meaning liveness in the 0.16 era and that `ok` / `ready_for_reads` are the signals to use.

---

### Edge Cases

- **Rebuild in progress**: `busy` with `activity: "rebuilding"` also reports `ready_for_reads: true` when the DB is loaded.
- **Rebuild window before the lock is taken**: ADR-0628 documents that `health_check` can report `healthy` before the rebuild takes the lock; `ready_for_reads` is true there as well. Unchanged.
- **Degraded takes precedence**: DB not loaded is evaluated first, so `busy` never masks `degraded` and `ready_for_reads` is false.
- **Write-fair queueing**: `busy` may alternate with `healthy` during batched passes; `ready_for_reads` stays true throughout.
- **Probe failure in the healthy path**: a DB probe error still surfaces as an error as today; no new response shape is introduced.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Every `health_check` response shape (`healthy`, `busy` in both `writing` and `rebuilding` activities, and `degraded`) MUST include a boolean `ready_for_reads` field.
- **FR-002**: `ready_for_reads` MUST be `true` whenever the database is loaded, including while `state` is `busy`, and `false` when the database is not loaded (`degraded`).
- **FR-003**: All existing `health_check` fields and values (`ok`, `healthy`, `state`, `activity`, `job_id`, `progress`, `reason`) and ADR-0628's decision logic MUST be unchanged; in particular `busy` still reports `healthy: false`, and `health_check` still never waits on the write lock or touches the DB when busy.
- **FR-004**: `ready_for_reads: true` while `busy` MUST NOT be derived from a DB probe; it follows from the DB being loaded.
- **FR-005**: A documentation ADR follow-up to ADR-0628 (a new ADR named per the repo's issue-number rule, or an amendment of ADR-0628) MUST define `ok` as liveness, `healthy` as idle and ready for writes, and `ready_for_reads` as reads being served now, with the per-`state` truth table.
- **FR-006**: `docs/ipc-mcp-reference.md`'s health-check contract (response table and Readiness text) MUST document `ready_for_reads` and the three-field semantics, including guidance on which field to gate on for liveness, writes and reads.
- **FR-007**: The semantics documentation MUST be reflected wherever the MCP-facing surface describes `health_check`. Because no MCP health-check tool exists, this means the MCP section of the reference (and any tool-description text that mentions health) states the semantics; no new MCP tool is added.
- **FR-008**: `CHANGELOG.md` MUST gain an entry under a documentation section of `[Unreleased]` noting that, since ADR-0628 (0.16 era), `healthy` means idle and ready for writes rather than liveness, so raw-socket clients should use `ok` for liveness and `ready_for_reads` for read availability.
- **FR-009**: Existing health-check tests MUST keep their assertions apart from the added field; new test coverage MUST assert `ready_for_reads` for idle, busy (write lock held) and degraded.

### Key Entities *(if applicable)*

- **`health_check` response**: JSON object with `ok`, `healthy`, `state`, optional `activity`/`job_id`/`progress`/`reason`, and the new `ready_for_reads`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With the write lock held, `health_check` returns `{ok: true, healthy: false, state: "busy", ready_for_reads: true}` (plus `activity`).
- **SC-002**: When idle, `ready_for_reads` is `true`; when degraded, it is `false`; no response shape lacks the field.
- **SC-003**: Pre-existing health-check test assertions pass unmodified except for the added field.
- **SC-004**: The three field meanings appear consistently in the ADR follow-up and `docs/ipc-mcp-reference.md`, and the CHANGELOG carries the `healthy`-semantics note.

## Assumptions

- The ADR follow-up is a new ADR named `docs/adr/0855-<slug>.md` (issue-number rule), cross-referenced from ADR-0628 and `docs/adr/index.md`, rather than an amendment; ADR-0628's "no MCP change" statement is left consistent with FR-007.
- "MCP tool description" cannot be satisfied literally: `health_check` is intentionally not in the MCP registry. We document the semantics in the MCP section of the reference rather than adding a tool, which would be a scope and registry-count change.
- The CHANGELOG has no `### Documentation` heading under `[Unreleased]` today, so one is added there.
- `ready_for_reads` reflects "DB loaded" only; it does not promise that every read will succeed (e.g. indexes may still be rebuilding), consistent with the issue's definition.
- The wire change is additive, so no IPC parity or MCP registry count changes are expected.

## Out of Scope *(optional)*

- Changing `busy`'s `healthy: false` behaviour, or making `healthy` mean liveness again.
- Adding an MCP health-check tool.
- Closing ADR-0628's known gaps (streaming-rebuild `activity`, pre-lock `healthy` window).
- Changes to the Python-side client or other out-of-repo consumers.

## Source References *(optional)*

- Issue #832 (origin), ADR-0628 (`docs/adr/0628-health-check-busy-state.md`), `docs/ipc-mcp-reference.md` ("Readiness", "Health check contract"), `crates/core/src/handlers.rs` (`handle_health_check`, `busy_health_payload`), `crates/service/src/mcp/tools.rs` (`NON_MCP_CLASSIFIED`).
