# Feature Specification: Warn on implicit default-group writes and document every method's group default

**Feature Branch**: `fabrik/issue-856`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Implements #833 (warn and document; the defaults do not change).

## Problem
When `group_id` is omitted, writes (`process_chunk`, `process_document`, and others) and `knowledge_walk` go to the default group `liminis` (`DEFAULT_GROUP_ID`). Most reads span every group. A third-party client that never sends `group_id` writes into the Liminis app's own graph, where its content goes through entity resolution together with the user's notes, and it gets no signal that this is happening.

**Decision (maintainer): don't change any defaults.** Changing them would break raw-socket clients. Warn and document instead.

## Change
- **Warn on implicit writes.** Every write method that fell back to `DEFAULT_GROUP_ID` because the request omitted `group_id` adds an additive warning to its response, for example `warnings: [\"group_id omitted; wrote to default group \\\"liminis\\\". Third-party writers should pass their own group_id.\"]`. Use the existing `warnings` array where the method has one, and add it consistently where it doesn't. An explicit `group_id: \"liminis\"` gets no warning.
- **Audit and document every method's default.** Go through every IPC method and record what an omitted group means: the default group, every group, or refused. Put the table in `docs/ipc-mcp-reference.md`. Fix only the docs, not the behaviour. If the audit finds a default that is clearly a bug, as opposed to a design choice, list it in the PR for a follow-up and don't change it here.
- **Add integrator guidance** in `docs/configuration.md` or the IPC reference: third-party writers use their own `group_id`, optionally alongside `--writable-groups` (ADR-0742).

## Acceptance
- `process_chunk` without `group_id` writes to `liminis` as today and returns the warning. With an explicit group, it returns no warning.
- The docs table covers every method, and a test asserts the warning on each write method that defaults.
- No read or write default changes behaviour."

## Background

Many IPC methods treat `group_id` as optional. When it is omitted, writes (`knowledge_process_chunk`, `knowledge_process_document`, and others) and `knowledge_walk` silently land in the default group `liminis`, while most reads span every group. The default group is the Liminis app's own graph, so a third-party raw-socket client that never sends `group_id` has its content entity-resolved together with the user's notes — and gets no signal that this is happening.

The maintainer has decided (issue #833) that **no default changes**: altering them would break existing raw-socket clients. The remedy is therefore two-fold: make the implicit fallback visible to the caller at write time, and document precisely what an omitted group means for every method. The existing `--writable-groups` allow-list (ADR-0742) already treats an omitted `group_id` as the default group; this work complements it for integrators.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Implicit default-group writes return a warning (Priority: P1)

A third-party client calls a write method such as `knowledge_process_chunk` without `group_id`. The write succeeds exactly as it does today, into `liminis`, but the response now carries an additive warning telling the caller that the default group was used and that third-party writers should pass their own `group_id`.

**Why this priority**: This is the only runtime signal an unaware client can get; it is the core of the change.

**Independent Test**: Call `knowledge_process_chunk` with no `group_id`, assert the content is written to `liminis` and the response includes the warning; repeat with `group_id: "other"` and with `group_id: "liminis"`, and assert no warning in either.

**Acceptance Scenarios**:

1. **Given** a running service, **When** `knowledge_process_chunk` is called without `group_id`, **Then** the data is written to `liminis` and the response `warnings` contains the default-group warning.
2. **Given** a running service, **When** the same call passes an explicit `group_id` other than `liminis`, **Then** the write goes to that group and no default-group warning is present.
3. **Given** a running service, **When** the call passes an explicit `group_id: "liminis"`, **Then** the write goes to `liminis` and no default-group warning is present.
4. **Given** a write method that already returns a `warnings` array, **When** it falls back to the default group, **Then** the default-group warning is appended to the existing warnings without displacing any of them.
5. **Given** a write method that has no `warnings` field today, **When** it falls back to the default group, **Then** the response gains a `warnings` array carrying the warning; when it does not fall back, the response shape is unchanged.

---

### User Story 2 - Every method's omitted-group behaviour is documented (Priority: P1)

An integrator reads `docs/ipc-mcp-reference.md` and finds a table listing every IPC method with what an omitted `group_id` (or `group_ids`) means for it: the default group, every group, or refused.

**Why this priority**: The warning only reaches callers who write; the table is how integrators learn the behaviour of reads, `knowledge_walk`, and admin methods before they ship.

**Independent Test**: Compare the set of methods in the dispatch table against the rows in the documented table; every method appears exactly once with one of the three classifications.

**Acceptance Scenarios**:

1. **Given** the reference document, **When** an integrator looks up any IPC method, **Then** the table states whether an omitted group means the default group, every group, or a refusal (or that the method takes no group).
2. **Given** a method whose parameter is `group_ids` rather than `group_id`, **When** it is listed, **Then** the table states the omitted-vs-empty distinction already documented for it, consistently with the existing "group_ids semantics" section.
3. **Given** the audit finds a default that looks like an outright bug rather than a design choice, **When** the work is delivered, **Then** it is listed in the PR description for a follow-up issue and its behaviour is unchanged.

---

### User Story 3 - Integrator guidance for third-party writers (Priority: P2)

A developer building a third-party client finds guidance (in the configuration docs or IPC reference) saying to always pass their own `group_id`, and optionally to run the daemon with `--writable-groups` so that writes to unlisted groups — including the implicit default — are refused.

**Why this priority**: It turns the warning and table into an actionable recommendation, but the feature is usable without it.

**Independent Test**: Read the documented guidance and confirm it names `group_id`, `--writable-groups`/`LCG_WRITABLE_GROUPS`, links ADR-0742, and notes that the default group must be listed to stay writable.

**Acceptance Scenarios**:

1. **Given** the docs, **When** a third-party integrator searches for how to avoid writing into the app's graph, **Then** they find explicit guidance to use their own `group_id`, with `--writable-groups` as an optional enforcement.

---

### Edge Cases

- `group_id: null` or a non-string value is treated as omitted by the existing default resolution, so it falls back to `liminis` and warns; the warning logic must agree with the resolution logic.
- `group_id: ""` is a distinct group (not the default) and gets no default-group warning; its existing handling (e.g. refusal under `--writable-groups`) is unchanged.
- Under `--writable-groups` not listing `liminis`, an implicit default write is refused as today; a refused call returns the existing refusal, not a warning-bearing success.
- `dry_run` calls that would have written to the default group still report the warning, since the fallback occurred.
- A request that fails for another reason returns its existing error; no warning is required on errors.
- The same write arriving via MCP-over-stdio (including attached `--connect`) carries the warning in the same way as over the IPC socket.
- Methods that take an explicit group in a different shape (e.g. `group_ids`, `source_group_id`, cross-group edges) only warn if they genuinely resolved to the default group because the group was omitted.
- Methods with a required `group_id` never default and so never warn.
- Auto-synthesised internal writes (WAL replay, auto-heal, daemon-internal work) are not client requests and do not produce client-visible warnings.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Every write method that resolves to the default group (`liminis`) because the request omitted `group_id` (or sent `null`/non-string) MUST include a default-group warning in its response, with text conveying that `group_id` was omitted, that the write went to the default group `liminis`, and that third-party writers should pass their own `group_id`.
- **FR-002**: A write whose request carries an explicit `group_id` string — including exactly `"liminis"` — MUST NOT include the default-group warning.
- **FR-003**: Where a write method's response already has a `warnings` array, the default-group warning MUST be appended to it; existing warnings MUST be preserved.
- **FR-004**: Where a write method's response has no `warnings` array, a `warnings` array MUST be added only when the default-group warning applies; responses for explicit-group calls MUST be shape-identical to today.
- **FR-005**: The warning MUST be additive only: it MUST NOT change the response's other fields, the data written, the group written to, or whether the call succeeds.
- **FR-006**: No read or write default may change behaviour: an omitted `group_id` resolves to the same group (or all groups, or a refusal) as before this change for every method, including `knowledge_walk`.
- **FR-007**: `docs/ipc-mcp-reference.md` MUST contain a table covering every IPC method, stating what an omitted group means for it: the default group, every group, refused, or not applicable (method takes no group).
- **FR-008**: The documented table MUST be accurate to current behaviour, established by auditing each method's handler, and MUST be consistent with the existing `--writable-groups` and `group_ids` omitted-vs-empty sections of the reference.
- **FR-009**: Any default found during the audit that is clearly a bug (as opposed to a design choice) MUST be listed in the PR description for a follow-up issue and MUST NOT be changed in this work.
- **FR-010**: The documentation (configuration guide or IPC reference) MUST give integrator guidance: third-party writers should use their own `group_id`, optionally together with `--writable-groups` (ADR-0742), noting that the default group must be listed to stay writable under that flag.
- **FR-011**: The documentation MUST describe the new `warnings` behaviour for implicit default-group writes, including the warning's meaning and the cases that do not produce it.
- **FR-012**: Automated tests MUST assert the default-group warning on every write method that defaults to the default group, and the absence of the warning for an explicit group (including explicit `"liminis"`), for at least `knowledge_process_chunk`.
- **FR-013**: The set of write methods tested and the set documented as defaulting to the default group MUST agree, so a newly added defaulting write method that lacks the warning or the documentation is caught by the tests.
- **FR-014**: The MCP-over-stdio tool surface MUST pass the warning through unchanged for the same methods.

### Key Entities *(if applicable)*

- **Default group**: The group `liminis` (`DEFAULT_GROUP_ID`), the Liminis app's own graph, used when a request omits `group_id`.
- **Default-group warning**: A human-readable string in a response's `warnings` array signalling an implicit fallback to the default group.
- **Group-default table**: The per-method documentation of what an omitted group means (default group / every group / refused / not applicable).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A `knowledge_process_chunk` call with no `group_id` writes to `liminis` as before and returns exactly one default-group warning; the same call with any explicit `group_id` (including `"liminis"`) returns none.
- **SC-002**: 100% of write methods that resolve an omitted `group_id` to the default group return the warning, verified by an automated test per method.
- **SC-003**: 100% of IPC methods appear in the documented group-default table with a classification, verified against the method dispatch list.
- **SC-004**: The pre-existing test suite passes unchanged apart from tests that strictly compare full response bodies of implicit-default calls, confirming no behavioural default changed.
- **SC-005**: An integrator can find, from the docs alone, the recommendation to use their own `group_id` and the optional `--writable-groups` enforcement.

## Assumptions

- "Write method" means any IPC/MCP method that mutates graph or WAL state for a group; the audit determines exactly which of them resolve an omitted `group_id` to the default group (the likely set includes the ones listed in the ADR-0742 table as "default group if omitted"), and only those get the warning.
- Methods with a required `group_id`, or that act on every group, do not default and get no warning.
- `knowledge_walk` defaults to the default group but is a read-style traversal; it is documented in the table but is not a "write", so it receives no runtime warning unless the audit shows it mutates state. (The issue names it in the problem statement but only requires warnings for writes.)
- The exact warning text follows the issue's example wording; tests may assert on a stable substring or a shared constant rather than the whole sentence.
- Pass-through of `warnings` on the MCP path needs no new work if MCP already forwards the response data unchanged.
- Documentation edits go in `docs/ipc-mcp-reference.md` (table) and either it or `docs/configuration.md` (guidance); `docs/llms-full.txt` or other derived docs are updated only if the repository already generates them from these files.

## Out of Scope *(optional)*

- Changing any default (read or write), including making `group_id` required or making reads default to the default group.
- Rejecting or auto-namespacing implicit writes.
- New access-control mechanisms or per-client identity beyond the existing `--writable-groups`.
- Fixing any default the audit flags as a bug — those are listed in the PR for a follow-up issue.
- Emitting server-side log warnings (the requirement is a response warning).

## Source References *(optional)*

- Issue #833 (parent decision: warn and document, do not change defaults)
- ADR-0742 — per-group write allow-list (`--writable-groups`)
- `docs/ipc-mcp-reference.md` — existing "group_ids semantics: omitted vs. empty" and write-policy sections
