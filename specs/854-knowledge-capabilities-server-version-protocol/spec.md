# Feature Specification: `knowledge_capabilities` — server version, protocol version, feature and method lists on the socket

**Feature Branch**: `fabrik/issue-854`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Implements #835.

## Problem
On the IPC socket, neither `knowledge_health_check` nor `knowledge_status` reports the server version or what the server supports. The version is visible only through `--version` and MCP `initialize`. A raw-socket client can't tell what it's talking to or detect features that shipped after the version it pinned. One integrator, pinned ten minor releases back, built workarounds for methods that already existed.

## Change
- Add a new read method, `knowledge_capabilities` (IPC and MCP, read-only, no lock), returning:
  - `server_version`: the crate version;
  - `protocol_version`: an integer, starting at 1, bumped on any breaking change to the socket protocol;
  - `features`: a sorted array of stable feature strings;
  - `methods`: the sorted list of IPC method names the dispatcher accepts.
- Also add `server_version` and `protocol_version` additively to `knowledge_health_check`.
- Add a single source of truth for `features` in code (`crates/core/src/capabilities.rs`). Seed it with features that already exist, for example `process_document`, `chunk_supersede`, `walk`, `origin`, `lint`, `writable_groups`, `hybrid_passage_search`. Document in that file and in the ADR that **later issues add their own feature string when they ship**.
- Derive `methods` from the dispatcher, not a hand-kept list, or add a test that the two match.
- Document it in the IPC reference, and add a CHANGELOG rule to CONTRIBUTING or the release runbook: changes to default scope or response semantics are flagged as breaking for raw-socket clients.

## Acceptance
- `knowledge_capabilities` returns a version equal to `CARGO_PKG_VERSION`, protocol 1, and the seeded features and methods.
- A test fails if a dispatcher method is missing from `methods`.
- The MCP tool count rises by one. Update the tool-count assertions and the docs."

## Background

A raw-socket client (for example the Electron app's Python bridge, or a third-party integrator) has no way to ask the running service what it is or what it can do. The server version appears only in `--version` and in the MCP `initialize` response; neither is reachable over the Unix socket / named pipe. As a result a client pinned to an older release cannot detect methods or behaviours that shipped later. One integrator pinned ten minor releases back built workarounds for methods that already existed. The reverse problem also exists: a client written against a newer server cannot tell whether the server it connected to is too old to support what it needs.

This issue implements #835. It gives the socket a self-description: a version, a protocol compatibility integer, a stable feature-flag list, and the list of accepted methods.

**Naming note.** The issue text says `knowledge_health_check`, but the existing socket method is `health_check` (the one dispatcher method not prefixed `knowledge_*`; it is not exposed over MCP). This spec treats the issue's "health check" as the existing `health_check` method and does not introduce a new `knowledge_health_check`. The `knowledge_status` method is unchanged.

**Current surface (from docs).** The IPC reference states 58 dispatcher methods (57 `knowledge_*` plus `health_check`) and 57 MCP tools (after #809 added an admin tool). Adding `knowledge_capabilities` makes these 59 and 58 respectively; the reference and the tool-count assertions must move with it.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A raw-socket client discovers what it is talking to (Priority: P1)

An integrator connects to the socket and calls `knowledge_capabilities`. It receives the server's version, the protocol version, the feature strings and the full list of accepted methods, and uses them to decide which calls are safe and whether to use a workaround.

**Why this priority**: This is the core value of the issue; everything else supports or documents it.

**Independent Test**: Start the service, send `knowledge_capabilities` over the socket, and check the four fields.

**Acceptance Scenarios**:

1. **Given** a running service, **When** a client calls `knowledge_capabilities`, **Then** the result contains `server_version` equal to the crate's version, `protocol_version` equal to `1`, a `features` array and a `methods` array.
2. **Given** the result, **When** the client inspects `features` and `methods`, **Then** both are sorted ascending and free of duplicates, and `features` includes the seeded strings (`process_document`, `chunk_supersede`, `walk`, `origin`, `lint`, `writable_groups`, `hybrid_passage_search`).
3. **Given** any method the dispatcher accepts, **When** the client reads `methods`, **Then** that method name appears in it (including `health_check` and `knowledge_capabilities` itself).
4. **Given** the service is busy (a write is held or a rebuild is running), **When** a client calls `knowledge_capabilities`, **Then** it answers promptly without waiting for the write lock.

---

### User Story 2 - A supervisor reads the version from the health check (Priority: P2)

A supervisor that already polls `health_check` for readiness also wants the server and protocol version without a second round-trip.

**Why this priority**: Additive and cheap, and it lets existing pollers learn the version, but `knowledge_capabilities` already covers the need.

**Independent Test**: Call `health_check` in the healthy, busy and degraded states and check that every response carries the two new fields.

**Acceptance Scenarios**:

1. **Given** a healthy service, **When** a client calls `health_check`, **Then** the response contains `server_version` and `protocol_version` with the same values as `knowledge_capabilities`, and every existing field (`ok`, `healthy`, `state`, …) is unchanged.
2. **Given** a busy or degraded service, **When** a client calls `health_check`, **Then** the response still carries both fields alongside the existing `busy`/`degraded` fields.

---

### User Story 3 - An MCP client sees the same capabilities (Priority: P2)

An MCP-over-stdio client lists tools and calls `knowledge_capabilities` as a read-scoped tool.

**Why this priority**: The issue requires MCP parity, but IPC is the motivating transport.

**Independent Test**: List tools in MCP mode and call the new tool with a read-scoped token.

**Acceptance Scenarios**:

1. **Given** an MCP session, **When** the client lists tools, **Then** `knowledge_capabilities` is present and the total count is one more than before.
2. **Given** a session limited to the `read` scope, **When** the client calls `knowledge_capabilities`, **Then** it succeeds and returns the same fields as over IPC.

---

### User Story 4 - Later features register themselves, and drift is caught (Priority: P2)

A maintainer who ships a new capability adds one feature string in one place. If anyone adds a dispatcher method and the advertised `methods` list does not include it, the test suite fails.

**Why this priority**: Without this the list rots and clients lose trust in it.

**Independent Test**: Add a dispatcher method without updating the capabilities source of truth, or remove one from the advertised list, and confirm a test fails.

**Acceptance Scenarios**:

1. **Given** a dispatcher method that is absent from the advertised `methods`, **When** the test suite runs, **Then** a test fails and names the missing method.
2. **Given** a name in `methods` that the dispatcher does not accept, **When** the test suite runs, **Then** a test fails.
3. **Given** the source-of-truth file for features, **When** a maintainer reads it, **Then** it states that later issues add their own feature string when they ship.

---

### User Story 5 - Contributors flag raw-socket-breaking changes (Priority: P3)

A contributor changing a default scope or the semantics of a response field finds a documented rule that such changes are flagged as breaking for raw-socket clients in the CHANGELOG, and that a breaking socket change bumps `protocol_version`.

**Why this priority**: It keeps the protocol integer meaningful, but it is documentation only.

**Independent Test**: Read the contributing/release documentation and the IPC reference.

**Acceptance Scenarios**:

1. **Given** the contributing guide or release runbook, **When** a contributor looks up changelog rules, **Then** it states that changes to default scope or response semantics are marked breaking for raw-socket clients.
2. **Given** the IPC reference, **When** an integrator looks up the new method, **Then** it documents the four fields, the meaning of `protocol_version`, how to treat unknown features, and the new fields on `health_check`.

---

### Edge Cases

- **Degraded mode (DB not loaded):** `knowledge_capabilities` needs no database, so it must still answer (like `health_check` and `knowledge_status`), so a client can learn the version of a degraded server.
- **Unknown method on an old server:** an older server returns its normal unknown-method error for `knowledge_capabilities`. Documentation tells clients to treat this error as "protocol/feature set predates capabilities".
- **Feature strings are append-only:** a feature string, once shipped, is not renamed or removed. A removed capability is a breaking change and bumps `protocol_version`.
- **Writable-groups policy / scope restrictions:** the method reveals only static build information, never workspace data, so it is available under any write policy and the read scope.
- **Version format:** `server_version` is the crate version string exactly as `--version` and MCP `initialize` report it (no `v` prefix, pre-release suffixes preserved).
- **Empty or extra parameters:** `knowledge_capabilities` takes no parameters; unknown parameters are ignored, consistent with other read methods.
- **Methods list vs. feature gating:** `methods` lists every name the dispatcher accepts regardless of whether a particular call would currently be rejected (for example by degraded mode or write policy).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The IPC dispatcher MUST accept a new read-only method `knowledge_capabilities` that takes no required parameters.
- **FR-002**: The `knowledge_capabilities` result MUST contain `server_version`, a string equal to the crate version (`CARGO_PKG_VERSION`).
- **FR-003**: The result MUST contain `protocol_version`, an integer, `1` in this release. It is bumped only on a breaking change to the socket protocol.
- **FR-004**: The result MUST contain `features`, a sorted, duplicate-free array of stable feature strings, seeded with at least: `process_document`, `chunk_supersede`, `walk`, `origin`, `lint`, `writable_groups`, `hybrid_passage_search`.
- **FR-005**: The result MUST contain `methods`, the sorted, duplicate-free list of every IPC method name the dispatcher accepts, including `health_check` and `knowledge_capabilities`.
- **FR-006**: `methods` MUST be derived from the dispatcher's own definition of accepted methods, not maintained as a separate hand-written list that can drift. If derivation is not possible, a test MUST fail whenever the advertised list and the dispatcher differ in either direction.
- **FR-007**: The set of feature strings MUST have a single source of truth in `crates/core/src/capabilities.rs`. That file MUST state that later issues add their own feature string when they ship and that strings are append-only.
- **FR-008**: `knowledge_capabilities` MUST NOT take the write lock and MUST NOT query the database, so it answers during rebuilds and writes and in degraded mode.
- **FR-009**: `health_check` MUST additionally report `server_version` and `protocol_version` in all its states (healthy, busy, degraded), with the same values as `knowledge_capabilities`. No existing field of `health_check` may change name, type or meaning.
- **FR-010**: `knowledge_capabilities` MUST be exposed as an MCP tool in the `read` scope, with a description and input schema, returning the same content as the IPC method.
- **FR-011**: The MCP tool-registry count and per-scope bucket assertions MUST be updated for one additional `read` tool, and every documented count (IPC methods 58→59, MCP tools 57→58, and any README or reference mention) MUST be updated to match.
- **FR-012**: The IPC/MCP reference MUST document `knowledge_capabilities` (fields, sort order, how clients should use `protocol_version` and `features`, the unknown-method fallback for old servers) and the two new `health_check` fields.
- **FR-013**: The contributing guide or release runbook MUST include a CHANGELOG rule that changes to default scope or response semantics are flagged as breaking for raw-socket clients, and that a breaking socket-protocol change bumps `protocol_version`.
- **FR-014**: An ADR MUST record the decision, named per the repository's issue-number convention (`docs/adr/0854-<slug>.md`), added to the ADR index, covering the protocol-version policy and the rule that later issues add their own feature string.
- **FR-015**: A CHANGELOG entry under `[Unreleased]` MUST describe the new method and the additive `health_check` fields.
- **FR-016**: A test MUST assert that `server_version` equals `CARGO_PKG_VERSION`, `protocol_version` is `1`, and every seeded feature appears in `features`.
- **FR-017**: A test MUST fail if any method the dispatcher accepts is missing from `methods`.

### Key Entities *(if applicable)*

- **Capabilities descriptor**: the static, build-time description of a server: version, protocol version, feature strings and accepted methods. Contains no workspace or graph data.
- **Feature string**: a stable, lower_snake_case identifier for a capability. Append-only; added by the issue that ships the capability.
- **Protocol version**: an integer compatibility marker for the socket protocol; distinct from the crate version.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A client connected to the socket can obtain the server version, protocol version, features and methods in a single request, with no database access.
- **SC-002**: `knowledge_capabilities` returns `server_version == CARGO_PKG_VERSION` and `protocol_version == 1` in an automated test.
- **SC-003**: 100% of methods the dispatcher accepts appear in `methods`, enforced by a test that fails when one is missing.
- **SC-004**: `knowledge_capabilities` and `health_check` answer with the correct fields while a write is held and while the service is degraded.
- **SC-005**: Every `health_check` response in every state carries `server_version` and `protocol_version`, and existing `health_check` consumers see no change in their fields.
- **SC-006**: The MCP registry contains exactly one more tool than before, in the `read` scope, and all count assertions and documented counts agree.
- **SC-007**: A contributor can find the breaking-change CHANGELOG rule and the "add your feature string" instruction without reading source code beyond the capabilities file.

## Assumptions

- The issue's `knowledge_health_check` refers to the existing `health_check` method; no new method of that name is created.
- `knowledge_capabilities` is allowed in degraded mode (exempt from the degraded-mode guard), since it needs no database and is most useful exactly when something is wrong.
- `protocol_version` is `1` for this release even though many methods predate it; it marks the baseline from which breaking changes are counted.
- The seeded feature strings correspond to capabilities that already exist in the codebase (document processing, chunk supersession, typed walks, episode origin, notebook lint, writable-groups policy, hybrid passage search). If any of them is not actually shipped, it is dropped from the seed rather than advertised.
- Feature strings are lower_snake_case, appear without a version suffix, and are never removed or renamed once released.
- The MCP tool returns the same payload as the IPC method; MCP's own `initialize` response is unchanged.
- No authentication or per-scope filtering of `features`/`methods` is needed: the content is static and non-sensitive.

## Out of Scope *(optional)*

- Adding feature strings for capabilities that ship in later issues (each issue adds its own).
- Reporting runtime or workspace state (entity counts, ontology, WAL status) — that remains `knowledge_status`.
- Per-method schemas, parameter lists or deprecation metadata in the capabilities response.
- Negotiation or enforcement of protocol versions by the server (clients decide; the server only reports).
- Changes to the Python-side client or the Electron app to consume the new fields.
- Retroactively changing earlier releases or adding a version to other responses beyond `health_check`.

## Source References *(optional)*

- Issue #835 (origin), issue #854 (this work)
- `docs/ipc-mcp-reference.md` — IPC method table, health-check contract, MCP tool registry counts
- `CONTRIBUTING.md` — ADR naming convention, release runbook
