# Feature Specification: Publish JSON Schemas for the responses consumers read, validated in lcg CI

**Feature Branch**: `fabrik/issue-852`
**Created**: 2026-11-22
**Status**: Specified
**Input**: User description: "## Problem

Consumers know lcg's method **names** (Liminis Notes pins them in `engine-methods.json` and diffs the `handlers.rs` dispatch on each bump) but not the **shapes** of the responses. Responses are built ad hoc with `serde_json::json!`, so consumers type them as guesses.

This cost a real bug. Liminis Notes' Notebook Health read `knowledge_lint` finding `details` by guessing key names, and every guess was wrong against 0.19.1:
- the stale page is `details.derived_document`, which the app didn't check, so Refresh never worked;
- the stale cause is a `causes` map, not a `cause` string;
- the weak-support kind is `finding.reason`, not `details.support`, which is a tally object.

No lcg test could catch this, and the consumer's only check was a slow real-engine suite that wasn't running (shadoworg/liminis-notes#1359, fixed in shadoworg/liminis-notes#1362).

## Proposal

Publish a **JSON Schema for each response that consumers read**, versioned with the release:

- Start with the methods consumers depend on most: `knowledge_lint` (including the per-category `details` shapes), `knowledge_status` (including `wal_groups`), `knowledge_search_passages`, `knowledge_find_entities` / `knowledge_find_relationships`, `knowledge_process_chunk`, `health_check`.
- Generate the schemas from typed response structs (for example `schemars`) or write them by hand, but **test in lcg that real responses validate** against them, so a shape change fails lcg's own CI first.
- Ship them as a release asset (for example `contract/*.schema.json`) alongside the binaries, so consumers fetch them at the pinned version exactly as they fetch the binary.
- Changes to a published schema go in the release notes. Removing or renaming a field is a breaking change.

## Acceptance

- The release includes the schemas, and lcg CI fails when a response stops validating.
- A consumer can validate a recorded response, or generate types from the schemas, at a pinned version, with no running engine.

## Consumer follow-up

Liminis Notes checks its response types against the schemas at the pinned version in a fast unit test (the same pattern as `engine-methods.json`) and drops the guess-the-key readers."

## Background

lcg exposes its API as newline-delimited JSON-RPC methods (and an MCP-over-stdio mirror). Consumers can discover method *names* (Liminis Notes pins them in `engine-methods.json` and diffs the dispatch table on each version bump), but the *shape* of each response is documented nowhere machine-readable. Responses are assembled ad hoc, so the only source of truth is the code that builds them, and consumers end up guessing key names.

That guessing already caused a shipped bug. Against lcg 0.19.1, Liminis Notes' Notebook Health read `knowledge_lint` finding details with the wrong keys: it missed `details.derived_document` (so Refresh never worked), treated the stale cause as a `cause` string when it is a `causes` map, and read the weak-support kind from `details.support` (a tally object) rather than `finding.reason`. No lcg test could catch this, and the consumer's only protection was a slow real-engine suite that was not running (shadoworg/liminis-notes#1359, fixed in #1362).

The goal is to make the response shapes an explicit, versioned, machine-readable contract that lcg itself enforces: a response that drifts from its published schema must fail lcg's own CI before a release can ship, and consumers can check their types against the schemas offline at a pinned version.

Related recent work: #857 (entity provenance and the `uuids` filter on episodes) changed response shapes for `knowledge_find_entities` / `knowledge_get_episodes`; this issue builds on the post-#857 shapes, which is why it waits on #857.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - lcg CI catches response-shape drift (Priority: P1)

A maintainer changes how a covered method builds its response (renames a field, changes a type, drops a key). lcg's own test suite fails because the real response no longer validates against the published schema, so the drift is caught before release rather than by a downstream consumer.

**Why this priority**: This is the root cause of the incident: nothing in lcg verified response shape. Without enforcement, published schemas would silently rot and be worse than none.

**Independent Test**: Run the validation tests against the current code (pass), then deliberately rename a field in one covered response and re-run (the corresponding test fails, naming the method and the offending path).

**Acceptance Scenarios**:

1. **Given** a covered method's schema and a real response produced by the engine, **When** the test suite runs, **Then** the response validates against the schema and the test passes.
2. **Given** a change that renames or removes a required field, or changes a field's type, in a covered response, **When** the test suite runs, **Then** at least one test fails and identifies the method and the path that no longer validates.
3. **Given** a response that exercises each `knowledge_lint` finding category, **When** it is validated, **Then** each category's `details` shape (including `derived_document`, the `causes` map, and the weak-support tally plus `finding.reason`) is checked, not just the generic envelope.

---

### User Story 2 - Schemas ship with each release (Priority: P1)

A consumer pins an lcg version. At that version, they fetch the schema files from the release alongside the binary, with no running engine and no source checkout needed.

**Why this priority**: The contract only helps consumers if they can obtain it at the exact version they run, the same way they obtain the binary.

**Independent Test**: Inspect a release (or a release dry-run/plan) and confirm the schema files are present as downloadable assets for that tag, and that their content matches the tagged source.

**Acceptance Scenarios**:

1. **Given** a tagged release, **When** a consumer lists the release assets, **Then** one schema file per covered method response is available, along with a way to know which version they belong to.
2. **Given** two different releases, **When** a consumer fetches schemas from each, **Then** each set reflects the response shapes of that release.
3. **Given** the schema files committed in the repository, **When** CI runs, **Then** CI fails if the committed files differ from what the source of truth produces (no stale or hand-drifted copies are published).

---

### User Story 3 - Consumer validates recorded responses offline (Priority: P2)

A consumer (e.g. Liminis Notes) downloads the schemas at its pinned version and, in a fast unit test with no engine running, validates recorded sample responses against them or generates types from them, replacing guess-the-key readers.

**Why this priority**: This is the downstream payoff and the second half of the acceptance criteria, but it depends on Stories 1 and 2 delivering trustworthy, fetchable schemas.

**Independent Test**: With only the schema files and a recorded response JSON, run a stock JSON Schema validator and confirm valid responses pass and a response with a wrong key fails.

**Acceptance Scenarios**:

1. **Given** the schema file for a method and a recorded real response, **When** a standard JSON Schema validator is run, **Then** it validates with no reference to a running engine or the lcg source.
2. **Given** a schema file, **When** a standard type generator for the consumer's language is run on it, **Then** it produces usable types without hand-editing (schemas are self-contained, with no external references to unpublished files).

---

### User Story 4 - Schema changes are visible in release notes (Priority: P3)

A consumer reading release notes can tell when a published schema changed and whether the change is breaking.

**Why this priority**: Consumers need a signal to know when to re-check their types, but the mechanism is documentation/process on top of the core contract.

**Independent Test**: A release that changes a schema has a corresponding changelog entry; contributor documentation states the rule that removing or renaming a field is a breaking change.

**Acceptance Scenarios**:

1. **Given** a change to any published schema, **When** the change is prepared for release, **Then** the changelog/release notes mention the schema change, and mark removed or renamed fields as breaking.
2. **Given** the contributor documentation, **When** a contributor adds a field to a covered response, **Then** the documented process tells them to update the schema, add or adjust validating tests, and note the change.

---

### Edge Cases

- A response contains fields the schema does not yet describe (additive change): the documented policy decides whether additional properties are permitted; adding an optional field must not be a breaking change for consumers.
- Optional or conditional response parts (e.g. `include_episodes` provenance from #857, degraded-mode responses, `wal_groups` present only when applicable) must be modeled as optional, not required, so valid real responses are not rejected.
- `knowledge_lint` findings span multiple categories with different `details` shapes; the schema must discriminate per category rather than accepting any object.
- Error responses (JSON-RPC errors) and streamed progress lines (`_progress_token`) are not part of the per-method result schema.
- Empty results (no entities, no findings, empty graph) must validate.
- Tests must cover a response that was generated by the real engine code path, not a hand-written fixture that could itself drift from the code.
- The MCP-over-stdio surface wraps the same results; the schemas describe the underlying method result, not the MCP envelope.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: lcg MUST publish a JSON Schema document for the result of each of these methods: `knowledge_lint`, `knowledge_status`, `knowledge_search_passages`, `knowledge_find_entities`, `knowledge_find_relationships`, `knowledge_process_chunk`, and `health_check`.
- **FR-002**: The `knowledge_lint` schema MUST describe the per-category finding `details` shapes (including the stale-page `derived_document`, the `causes` map, and the weak-support tally with the kind carried in `finding.reason`), not only a generic finding envelope.
- **FR-003**: The `knowledge_status` schema MUST describe `wal_groups` and the other status fields consumers read.
- **FR-004**: The schemas MUST be valid JSON Schema documents using a single, documented draft, and MUST be self-contained (no references to files that are not published alongside them).
- **FR-005**: The lcg test suite MUST validate real responses, produced by the engine's actual response-building code for each covered method, against the corresponding published schema, and MUST fail when a response does not validate.
- **FR-006**: The validation tests MUST exercise each `knowledge_lint` finding category and the optional/conditional response parts (e.g. entity provenance from #857) so that those shapes are covered, not just the common case.
- **FR-007**: The published schema files MUST be kept in sync with their source of truth by a check that fails CI when the committed files are stale or hand-edited out of sync.
- **FR-008**: Each release MUST include the schema files as downloadable release assets alongside the binaries, so a consumer can fetch them at a pinned version by the same mechanism as the binary.
- **FR-009**: The schema set MUST identify the lcg version it belongs to, so a consumer can confirm the schemas match the pinned binary.
- **FR-010**: The project MUST document a compatibility policy: removing or renaming a field, or narrowing its type, in a published schema is a breaking change; adding an optional field is not.
- **FR-011**: Any change to a published schema MUST be recorded in the release notes/changelog, with breaking changes called out.
- **FR-012**: Documentation MUST explain to consumers how to fetch the schemas at a pinned version and validate a recorded response or generate types from them without a running engine, and MUST explain to contributors how to update schemas when changing a covered response.
- **FR-013**: The schemas MUST describe only the method's successful result payload, not JSON-RPC error envelopes, streamed progress lines, or the MCP wrapper.
- **FR-014**: Adding the contract MUST NOT change the wire format of any existing response: existing consumers must see byte-for-byte compatible responses (aside from any shape discrepancy the work uncovers, which is documented and treated as a schema correction, not a silent wire change).

### Key Entities *(if applicable)*

- **Response schema**: A versioned JSON Schema describing the result of one JSON-RPC method.
- **Schema set / contract**: The collection of response schemas for a release, identified by the lcg version and published as release assets.
- **Compatibility policy**: The rule defining which schema changes are breaking and how they are announced.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of the seven listed methods have a published schema, and each is validated against real engine responses in the automated test suite.
- **SC-002**: Introducing a field rename, removal, or type change in any covered response causes the lcg test suite to fail before merge, demonstrated for at least one change per covered method category (including each `knowledge_lint` category).
- **SC-003**: Every tagged release lists the schema files among its assets, and the committed schema files match the generated ones at that tag (zero drift).
- **SC-004**: A consumer can validate a recorded response of any covered method using only the downloaded schema and a standard JSON Schema validator, with no engine running, in a unit-test time scale (seconds).
- **SC-005**: Every release that changes a published schema has a corresponding release-notes entry, with breaking changes explicitly labelled.
- **SC-006**: The shapes that caused the original incident (`derived_document`, `causes`, `finding.reason`) are each asserted by a schema check that fails if they change.

## Assumptions

- Covered methods are exactly the seven listed; other methods can be added later using the same mechanism.
- The schemas are generated from typed response definitions in lcg (or an equivalent single source of truth) rather than maintained fully by hand, because that makes drift checkable; the exact mechanism is left to Research/Plan.
- The schemas are committed under a top-level `contract/` directory with names like `<method>.schema.json`, as suggested in the issue, and attached to the GitHub release as assets by the existing release workflow.
- The schema set is identified by the lcg release version (for example via a manifest/index file or a version field), rather than being versioned independently.
- Additional properties are tolerated by default in published schemas so that additive changes remain non-breaking for consumers validating recorded responses; fields that are always present are marked required.
- The post-#857 response shapes (entity provenance, `uuids` filter) are the baseline; this work starts after #857 has merged.
- The consumer-side work in Liminis Notes (unit test against pinned schemas, dropping guess-the-key readers) is tracked separately in that repository.

## Out of Scope *(optional)*

- Schemas for request parameters / input shapes, and for the remaining methods beyond the seven listed.
- Changing existing response formats, or refactoring all handlers to typed responses beyond what is needed for the covered methods.
- A schema registry, hosted schema URLs, or runtime response validation inside the service.
- The Liminis Notes follow-up (consumer-side tests, removing guess-the-key readers).
- Schema description of JSON-RPC error objects, progress streaming, and the MCP envelope.
- Generating client SDKs/types on behalf of consumers.

## Source References *(optional)*

- Issue #852; Liminis Notes issues shadoworg/liminis-notes#1359 and #1362 (the incident and its fix).
- #857 (entity provenance and `uuids` filter), a dependency for the baseline shapes.
- `docs/ipc-mcp-reference.md` (human-readable method reference that the schemas complement), `docs/release-process.md`, and `CHANGELOG.md`.
