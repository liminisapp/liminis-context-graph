# Feature Specification: Per-group write allow-list (`--writable-groups`) enforced in the daemon

**Feature Branch**: `fabrik/issue-742`
**Created**: 2026-11-13
**Status**: Specified
**Input**: User description: "Implements community request #740.

## Problem

`--scope` controls which tools a connection advertises (`read`, `write`, `cypher`, `admin`; `crates/service/src/mcp/scope.rs`), but nothing limits **which groups** a write-scope connection may change. Verified against `main`: there is no group-level write check anywhere.

One daemon often holds many co-resident groups with different owners. For example, layer groups hydrated from streams other parties publish (read-only inputs) sit beside the group the local user authors. An agent should only be able to write to the user's own group. Today that needs a client-side proxy, which only protects the connections that go through it.

## Proposed solution

A server-side allow-list, enforced in the daemon:

- **Configuration:** `--writable-groups=<g1,g2,…>` and an equivalent env var (`LCG_WRITABLE_GROUPS`), following the strict-env pattern of ADR-0737. An empty or malformed value fails startup with a clear error. Unset means every group stays writable, as today.
- **One enforcement point:** check in the core request path (`crates/core/src/handlers.rs` `dispatch`) that IPC, standalone MCP (`--mcp-stdio`) and attached MCP (`--mcp-stdio --connect`) all go through. Research must confirm each path really does reach that point. If any doesn't, it needs the same check, and a test proving it.
- **Check the group actually changed,** not every `group_id` a request mentions:
  - an omitted `group_id` resolves to the default group, and that is what gets checked;
  - for `knowledge_add_cross_group_edge`, the check is against the edge's own `group_id`; foreign endpoints are read-only pointers;
  - a tool that writes more than one group needs every written group in the list.
- **Admin writes that change group data are covered:** `knowledge_delete_by_group`, `knowledge_rebuild_from_wal` with `force_clear`, `knowledge_rebind_pointers` (its mutations land in other groups' rows, per ADR-0385), and the other admin mutators Research identifies. Read-only admin tools are not affected.
- **Raw Cypher (`cypher` scope) can't be scoped by group,** because `knowledge_query_cypher` can write anything. When `--writable-groups` is set, write statements through `knowledge_query_cypher` must be refused with a structured error, or `cypher` scope disabled, rather than silently allowed. Decide which in Plan.
- **Error shape:** a rejected call fails with a structured JSON-RPC error that names the group and the setting, and writes nothing: no database change and no WAL event.
- **Startup log:** the effective allow-list is logged at startup.

## Acceptance criteria

1. With `--writable-groups=a`, `knowledge_assert_entity {group_id: "b"}` is rejected with the structured error, and writes no database row and no WAL entry.
2. `knowledge_add_cross_group_edge {group_id: "a", target: {source_group_id: "b", …}}` succeeds, and `{group_id: "b", …}` is rejected.
3. With `--writable-groups=a` and the default group not in the list, a write with `group_id` omitted is rejected.
4. Every write-scope and data-changing admin tool is covered. A table-driven test lists each one, so a newly added write tool that isn't classified fails the test.
5. Raw Cypher behaves as decided in Plan, with a test.
6. All three entry points (IPC, standalone MCP, attached MCP) enforce it, each with a test.
7. Unset, every existing test passes unchanged.
8. Documented in `docs/ipc-mcp-reference.md` under Scopes, and in `docs/configuration.md`.

## Out of scope

- Per-group *read* restrictions.
- Authentication or per-client identity: this is one allow-list per daemon, applying to every connection.
- Changing the existing scope model."

## Background

A single daemon often holds several co-resident groups with different owners: for example, layer groups hydrated from streams that other parties publish (read-only inputs) next to the group the local user authors. An agent connected to that daemon should only be able to change the user's own group.

Today the only control is `--scope`, which decides which *tools* a connection can see (`read`, `write`, `cypher`, `admin`). Nothing restricts *which groups* a write-capable connection may change; no group-level write check exists anywhere. The only workaround is a client-side proxy, which protects only connections that route through it. This feature (community request #740) adds a server-side, daemon-wide allow-list of writable groups so the protection holds for every connection path.

Relevant existing context:
- The scope model and the `cypher` scope's "power scope" caveat are documented under *Scopes* in `docs/ipc-mcp-reference.md`: `knowledge_query_cypher` can perform arbitrary mutations that bypass the structured write tools' invariants.
- Some operations are multi-group by design (ADR-0385: `knowledge_delete_by_group` and `knowledge_rebind_pointers` change rows owned by groups other than the one named in the call).
- The issue refers to a "strict-env pattern" in ADR-0737. That ADR is not present in this checkout's `docs/adr/` (the highest ADR number present is 0707), so it is presumably in flight; the intent (a malformed env value fails startup rather than being silently ignored) is stated in this spec and does not depend on it.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Restrict writes to the operator's own group (Priority: P1)

An operator running a daemon with several groups starts it with `--writable-groups=a` (or `LCG_WRITABLE_GROUPS=a`). Any client, over any connection path, that tries to change data in a group other than `a` is refused, and nothing is written.

**Why this priority**: This is the core guarantee. Without it nothing else has value.

**Independent Test**: Start a daemon with `--writable-groups=a` holding groups `a` and `b`; attempt a write to `b` and confirm it is refused with a structured error and that the database and WAL are unchanged; attempt the same write to `a` and confirm it succeeds.

**Acceptance Scenarios**:

1. **Given** a daemon started with `--writable-groups=a`, **When** a client calls `knowledge_assert_entity {group_id: "b"}`, **Then** the call fails with a structured JSON-RPC error naming group `b` and the `--writable-groups` setting, and no database row and no WAL event is written.
2. **Given** the same daemon, **When** a client calls `knowledge_assert_entity {group_id: "a"}`, **Then** it succeeds as it does today.
3. **Given** a daemon started with `--writable-groups=a` and a default group that is not `a`, **When** a client calls a write tool with `group_id` omitted, **Then** the call is rejected, because the omitted group resolves to the default group and that group is not in the list.
4. **Given** a daemon started without `--writable-groups` and without `LCG_WRITABLE_GROUPS`, **When** any existing client performs any call, **Then** behaviour is identical to today.

---

### User Story 2 - Enforcement holds on every entry point (Priority: P1)

The restriction applies identically whether a client talks to the daemon over its IPC socket, through a standalone MCP stdio process (`--mcp-stdio`), or through an MCP stdio process attached to a running daemon (`--mcp-stdio --connect`).

**Why this priority**: The motivating weakness of the client-side proxy is that it only covers connections going through it. A server-side list that some entry point bypasses defeats the purpose.

**Independent Test**: For each of the three entry points, attempt a write to a non-listed group and confirm rejection with the same structured error and no data change; attempt a write to a listed group and confirm success.

**Acceptance Scenarios**:

1. **Given** the allow-list is set, **When** a write to a non-listed group arrives over IPC, **Then** it is rejected.
2. **Given** the allow-list is set on a standalone MCP process, **When** a write tool call to a non-listed group arrives, **Then** it is rejected.
3. **Given** a daemon with the allow-list set and an attached MCP client, **When** the client calls a write tool for a non-listed group, **Then** it is rejected.

---

### User Story 3 - Only the group that actually changes is checked (Priority: P1)

The check is made against the groups whose data a call actually changes, not every group identifier mentioned in the request.

**Why this priority**: A naive "check every `group_id` in the request" would either block legitimate cross-group links to read-only layer groups or allow multi-group writers to modify groups outside the list. Both defeat the feature.

**Independent Test**: With `--writable-groups=a`, call `knowledge_add_cross_group_edge` once with the edge's `group_id: "a"` and a target whose `source_group_id` is `b`, and once with the edge's `group_id: "b"`; confirm the first succeeds and the second is rejected.

**Acceptance Scenarios**:

1. **Given** `--writable-groups=a`, **When** a client calls `knowledge_add_cross_group_edge {group_id: "a", target: {source_group_id: "b", …}}`, **Then** it succeeds (the foreign endpoint is a read-only pointer).
2. **Given** `--writable-groups=a`, **When** a client calls `knowledge_add_cross_group_edge {group_id: "b", …}`, **Then** it is rejected.
3. **Given** a tool that writes to more than one group, **When** any one of the written groups is not in the list, **Then** the whole call is rejected and nothing is written; it is allowed only if every written group is in the list.

---

### User Story 4 - Data-changing admin operations are covered (Priority: P2)

Admin-scope operations that change group data are subject to the same allow-list, including operations whose effects land in groups other than the one named in the call. Read-only admin operations are unaffected.

**Why this priority**: Without it, any connection with admin scope trivially bypasses the list (for example by purging a protected group). It is P2 only because admin scope is typically held by fewer clients than write scope.

**Independent Test**: With `--writable-groups=a`, call `knowledge_delete_by_group` for `b`, and `knowledge_rebuild_from_wal` with `force_clear`; confirm rejection when the call would change a non-listed group. Confirm a read-only admin tool (for example `knowledge_dump_wal`) still works.

**Acceptance Scenarios**:

1. **Given** `--writable-groups=a`, **When** a client calls `knowledge_delete_by_group` for group `b`, **Then** it is rejected and `b` is unchanged.
2. **Given** `--writable-groups=a`, **When** a client calls `knowledge_rebind_pointers` in a way that would change rows owned by a non-listed group, **Then** it is rejected and nothing is written.
3. **Given** `--writable-groups=a`, **When** a client calls a read-only admin tool, **Then** it behaves as today.

---

### User Story 5 - Raw Cypher cannot be used to bypass the list (Priority: P2)

Because raw Cypher cannot be scoped by group, enabling the allow-list must not leave `knowledge_query_cypher` as a silent loophole. Write statements through it are either refused with a structured error, or the `cypher` scope is unavailable while the list is active. Which of the two is chosen is a Plan-stage decision.

**Why this priority**: Without this, `cypher` scope defeats the whole feature, but it is a narrower population of clients than `write` scope.

**Independent Test**: With `--writable-groups=a` and `cypher` scope otherwise available, attempt a mutating Cypher statement and confirm that it does not execute and the client receives the structured error (or, under the alternative outcome, that the tool is not offered/callable); confirm no database change.

**Acceptance Scenarios**:

1. **Given** `--writable-groups=a`, **When** a client submits a mutating statement through `knowledge_query_cypher`, **Then** it is refused (or the scope is unavailable) and no data changes.
2. **Given** `--writable-groups` is unset, **When** a client uses `knowledge_query_cypher`, **Then** behaviour is unchanged.

---

### User Story 6 - Fail loudly on bad configuration and say what is in effect (Priority: P2)

An operator who sets the allow-list incorrectly learns immediately; an operator who sets it correctly can see in the log what is enforced.

**Why this priority**: A silently ignored allow-list would leave the operator believing data is protected when it is not.

**Independent Test**: Start the daemon with an empty value, with a malformed value, and with a valid value; confirm the first two refuse to start with a clear error and the third logs the effective list.

**Acceptance Scenarios**:

1. **Given** `--writable-groups=` (empty) or an equivalent empty `LCG_WRITABLE_GROUPS`, **When** the daemon starts, **Then** startup fails with a clear error.
2. **Given** a malformed value (for example, empty entries such as `a,,b`), **When** the daemon starts, **Then** startup fails with a clear error naming the problem.
3. **Given** a valid value, **When** the daemon starts, **Then** the effective allow-list is written to the startup log.

---

### Edge Cases

- **Omitted `group_id`**: resolves to the default group; the default group is what is checked (and the default group must be listed explicitly to be writable).
- **Foreign endpoints**: group identifiers that appear only as read-only pointer targets (for example `source_group_id` on a cross-group edge) are not checked.
- **Multi-group writers**: if any one written group is outside the list the entire call is rejected atomically; no partial write and no WAL event for any group.
- **Both flag and env var set**: precedence follows the project's existing convention for other flags that have an env var equivalent (flag wins), and the effective list is what gets logged.
- **A listed group that does not exist yet**: allowed; the list names permissions, not existing data.
- **Whole-database operations** (for example `knowledge_clear_all`, `knowledge_rebuild_from_wal` with `force_clear`): these change every group, so they are permitted only if every group they would change is writable; otherwise they are rejected.
- **Daemon-internal and recovery work** (startup WAL replay, auto-heal index builds, recovery of a degraded database): these are not client write requests against a group and are not blocked by the list.
- **Read-only tools** (any scope) are never affected, including reads of non-listed groups.
- **A newly added write tool that has not been classified**: must be caught by a test rather than silently left unchecked.
- **Rejected calls must be side-effect free**: no database change, no WAL event, and no partial work (for example, no embedding or extraction side effects that persist data).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The daemon MUST accept a `--writable-groups=<g1,g2,…>` option and an equivalent `LCG_WRITABLE_GROUPS` environment variable defining a single, daemon-wide allow-list of group identifiers that clients may change.
- **FR-002**: When neither the option nor the env var is set, every group MUST remain writable and all behaviour MUST be identical to today; all existing tests MUST pass unchanged.
- **FR-003**: An empty or malformed allow-list value (from either source) MUST cause startup to fail with a clear, specific error rather than being ignored or silently treated as "unset".
- **FR-004**: The effective allow-list MUST be logged at startup.
- **FR-005**: The restriction MUST be enforced inside the daemon, at a single point in the core request path, so that IPC, standalone MCP (`--mcp-stdio`) and attached MCP (`--mcp-stdio --connect`) are all subject to it. Any entry path found not to reach that point MUST be given the same check.
- **FR-006**: The check MUST be made against the group(s) whose data the call actually changes, not every group identifier mentioned in the request.
- **FR-007**: A call that omits `group_id` MUST be checked against the default group.
- **FR-008**: For `knowledge_add_cross_group_edge`, the check MUST be made against the edge's own `group_id`; groups referenced only as foreign endpoints MUST NOT be checked.
- **FR-009**: A call that changes more than one group MUST be allowed only if every group it changes is in the allow-list; otherwise it MUST be rejected as a whole.
- **FR-010**: Admin-scope operations that change group data (including `knowledge_delete_by_group`, `knowledge_rebuild_from_wal` with `force_clear`, `knowledge_rebind_pointers` — whose changes land in other groups' rows — and any other data-changing admin operation identified during Research) MUST be subject to the allow-list. Read-only admin operations MUST NOT be affected.
- **FR-011**: When the allow-list is set, `knowledge_query_cypher` MUST NOT be usable as an unrestricted write path: either mutating statements are refused with a structured error, or the `cypher` scope is made unavailable. The Plan stage chooses which; the chosen behavior MUST be documented and tested.
- **FR-012**: A rejected call MUST fail with a structured JSON-RPC error that names the offending group and the `--writable-groups` setting, and MUST write nothing: no database change and no WAL event.
- **FR-013**: Every write-scope tool and every data-changing admin tool MUST be explicitly classified for group-write checking, and a table-driven test MUST enumerate them so that a newly added write tool lacking a classification fails the test.
- **FR-014**: Each of the three entry points (IPC, standalone MCP, attached MCP) MUST have a test demonstrating enforcement.
- **FR-015**: The feature MUST be documented in `docs/ipc-mcp-reference.md` under Scopes and in `docs/configuration.md`, including the raw-Cypher behavior and the rule that the default group must be listed to be writable.

### Key Entities *(if applicable)*

- **Writable-group allow-list**: An optional, daemon-wide set of group identifiers fixed at startup; when absent, no group-level write restriction applies.
- **Written group set**: For a given call, the set of groups whose stored data the call would change. This, not the set of groups mentioned in the request, is what is compared to the allow-list.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With `--writable-groups=a`, a write to any group other than `a` through any of the three entry points is rejected 100% of the time, with zero database rows and zero WAL events written.
- **SC-002**: A cross-group edge authored in a listed group pointing at a non-listed group succeeds; the same edge authored in the non-listed group is rejected.
- **SC-003**: A write with `group_id` omitted is rejected whenever the default group is not in the list.
- **SC-004**: 100% of write-scope and data-changing admin tools are classified, and adding an unclassified write tool causes the test suite to fail.
- **SC-005**: With the allow-list set, no mutation to a non-listed group is achievable through `knowledge_query_cypher`.
- **SC-006**: With the option unset, the pre-existing test suite passes with no modifications.
- **SC-007**: An empty or malformed allow-list prevents startup with an error that identifies the problem; a valid one produces a startup log line showing the effective list.
- **SC-008**: An operator can find the setting, its semantics and the raw-Cypher behavior in `docs/ipc-mcp-reference.md` (Scopes) and `docs/configuration.md`.

## Assumptions

- Group identifiers are matched as exact, case-sensitive strings; no wildcards or patterns.
- The allow-list is read once at startup; changing it requires a restart. There is no runtime reconfiguration.
- When both the flag and the env var are given, the flag takes precedence, consistent with the project's other options that have env equivalents.
- A listed group that does not yet exist is valid; the list grants permission, it does not assert existence.
- Operations that change every group (such as `knowledge_clear_all`) are permitted only if every group they would change is writable. The exact way "every group" is determined is a Plan/Research detail.
- Startup recovery and WAL replay performed by the daemon itself are not subject to the allow-list, since they restore previously accepted state rather than act on a client request.
- The error code and exact message text are Plan-stage details, provided they name the group and the setting.
- The "strict-env pattern" referenced from ADR-0737 means: an invalid env value is a startup error, never silently ignored.

## Out of Scope *(optional)*

- Per-group *read* restrictions.
- Authentication or per-client identity: this is one allow-list per daemon, applying to every connection.
- Changing the existing scope model.
- Runtime reconfiguration of the allow-list.
- Restricting what a *read* tool can learn about non-listed groups.

## Source References *(optional)*

- Issue #740 (community request) and issue #742.
- `docs/ipc-mcp-reference.md` — Scopes section (including the `cypher` power-scope note).
- ADR-0385 — per-group mutation attribution for `delete_by_group` and `rebind_pointers`.
- ADR-0035 — MCP-over-stdio transport.
