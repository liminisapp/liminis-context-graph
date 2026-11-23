# Feature Specification: knowledge_capabilities feature strings for the 0.19.2 changes that shipped without one

**Feature Branch**: `fabrik/issue-885`
**Created**: 2026-11-23
**Status**: Specified
**Input**: User description: "## Problem

`knowledge_capabilities` (#854, ADR-0854) advertises feature strings so clients can detect behaviour, and its module docs make adding one a contributor duty when a feature ships. Several 0.19.2 changes merged without one, so a client cannot detect them except by version-sniffing. On `main` today `FEATURES` is: `chunk_supersede`, `entity_provenance`, `hybrid_passage_search`, `lint`, `origin`, `process_document`, `walk`, `writable_groups` (`walk_provenance` and `unchanged_noop` arrive with #860 and #858).

Candidates with no string (from the 0.19.2 merges):

- `LCG_SEARCH_MIN_SCORE` configurable default `min_score` for passage search (#812, PR #813)
- `health_check` `ready_for_reads` (#809/#855, PR #869)
- test-providers mode: built-in deterministic embedder and extractor (#851, PR #870)
- exact source matching plus stored `source_file` / `chunk_id` (#853, PR #868), a behaviour change clients may need to detect
- hybrid passage `legs` field (#871, PR #873)
- implicit default-group write warnings (#856, PR #874), if clients can act on it
- `knowledge_reembed_missing` (PR #826) and the zero-vector record in `knowledge_status` (PR #810); the method is already visible in `methods`, so decide whether a string adds anything

## Acceptance criteria

- Each change above either gets a `lower_snake_case` feature string (sorted, append-only, per the module docs and the existing unit test) or a one-line note in the PR saying why `methods` or the response shape already makes it detectable.
- `docs/ipc-mcp-reference.md` lists every feature string with the issue that introduced it.
- CONTRIBUTING.md's release runbook gains a check: \"every user-visible change in this release has a feature string or a stated reason it doesn't\".

## Out of scope

`PROTOCOL_VERSION` (no breaking change involved)."

## Background

`knowledge_capabilities` (#854, ADR-0854) is how a raw-socket client asks the running service what it supports. Its `features` list is the only way to detect a capability without comparing version numbers, and the module documentation makes adding a string a contributor duty when a capability ships. That duty is not enforced by anything, and several 0.19.2 changes merged without a string. A client that wants to know "does this server report `legs` on hybrid passage results?" or "does `health_check` carry `ready_for_reads`?" today has to guess from `server_version`, which is exactly what the registry exists to avoid.

Current state of `main`: the registry already carries `chunk_supersede`, `entity_provenance`, `hybrid_passage_search`, `lint`, `origin`, `process_document`, `status_episode_breakdown` (#884), `unchanged_noop` (#858), `walk`, `walk_provenance` (#860) and `writable_groups`. The strings in the issue's list (`walk_provenance`, `unchanged_noop`) have since landed, so this issue covers only the remaining changes below.

Precedent for the decision rule: features that add a response field or a behaviour with no new method (`entity_provenance`, `status_episode_breakdown`, `walk_provenance`, `unchanged_noop`) were each given a string, while a brand-new method is already visible in the `methods` list. This spec follows that rule.

Candidate changes and the intended disposition (each is a default the Plan stage may confirm or adjust against the code, see Assumptions):

| Change | Issue / PR | Client-detectable today? | Disposition |
|---|---|---|---|
| Configurable default `min_score` via `LCG_SEARCH_MIN_SCORE` | #812 / #813 | No: operator setting, changes the default floor only | Gets a string |
| `health_check` `ready_for_reads` | #809, #855 / #869 | Only by probing a live response | Gets a string |
| Test-providers mode (built-in deterministic embedder and extractor) | #851 / #870 | Only by running it | Gets a string |
| Exact source matching | #853 / #868 | No: behaviour change | Gets a string |
| Stored `source_file` / `chunk_id` | #853 / #868 | Only by probing a response | Gets a string (separate from exact matching, since a client may need one without the other) |
| Hybrid passage `legs` field | #871 / #873 | Only by probing a response | Gets a string |
| Implicit default-group write warnings | #856 / #874 | Only by probing a write response | Gets a string (clients can act on the warning by supplying an explicit `group_id`) |
| `knowledge_reembed_missing` | #809 / #826 | Yes: appears in `methods` | No string; reason recorded |
| Zero-vector record in `knowledge_status` | #808 / #810 | Only by probing a status response | Gets a string |

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Client detects 0.19.2 behaviour without version-sniffing (Priority: P1)

A client author (for example the Liminis app, or a third-party agent) calls `knowledge_capabilities` and checks `features` for each 0.19.2 capability it wants to use or depend on, instead of comparing `server_version`.

**Why this priority**: This is the whole point of the issue: the registry is only trustworthy if every user-visible change appears in it.

**Independent Test**: Call `knowledge_capabilities` on a build containing this change and assert every new string is present, the list is sorted, duplicate-free and `lower_snake_case`.

**Acceptance Scenarios**:

1. **Given** a server built from this change, **When** a client calls `knowledge_capabilities`, **Then** `features` contains one string for each change in the table above marked "Gets a string", in sorted order.
2. **Given** a server built before this change, **When** a client calls `knowledge_capabilities`, **Then** the new strings are absent, and the client treats that as "capability not present" (existing "ignore strings you do not recognise" contract is unchanged).
3. **Given** the pre-existing strings, **When** this change ships, **Then** none is renamed or removed.

---

### User Story 2 - Reader finds every feature string and its origin in the reference (Priority: P1)

A client author or maintainer opens `docs/ipc-mcp-reference.md` and finds a list of every feature string in `FEATURES`, each with the issue that introduced it and a one-line meaning, so they know what to test for and where the full contract is documented.

**Why this priority**: A string is only useful if its meaning is discoverable; today the docs show an example list and no per-string description.

**Independent Test**: Compare the strings in the registry with those in the reference table; they must match exactly.

**Acceptance Scenarios**:

1. **Given** the reference document, **When** a reader looks up any string returned by `knowledge_capabilities`, **Then** it appears in the list with the introducing issue number.
2. **Given** a change that is deliberately not advertised (`knowledge_reembed_missing`), **When** a reader looks in the reference, **Then** the reason it needs no string is stated.
3. **Given** a contributor adds a string to the registry without documenting it, **When** the test suite runs, **Then** it fails, pointing at the undocumented string.

---

### User Story 3 - Release manager is prompted to check feature coverage (Priority: P2)

A maintainer preparing a release follows the CONTRIBUTING.md runbook and, as an explicit step, verifies that every user-visible change in the release has a feature string or a stated reason it does not.

**Why this priority**: Nothing enforces the contributor duty today; the gap that produced this issue will recur unless the release process checks for it. It does not affect runtime behaviour, so it ranks below the registry and reference.

**Independent Test**: Read the runbook; the check is present as a numbered or bulleted step in the release procedure and states the pass condition verbatim.

**Acceptance Scenarios**:

1. **Given** the release runbook, **When** a maintainer reaches the pre-tag checks, **Then** they find a step requiring that every user-visible change in the release has a feature string or a stated reason it does not.
2. **Given** a release that omits a string for a user-visible change, **When** the maintainer runs the step, **Then** the step tells them what to do (add the string, or record the reason in the release PR).

---

### Edge Cases

- A change that is detectable only through a response-shape field which is `null` or absent in some states (degraded mode, empty database): the string still advertises that the *server supports* it; it does not promise the field is populated on every call.
- Test-providers mode is selected at run time, while `knowledge_capabilities` is static build information. The string advertises that this build supports the mode, not that it is currently active.
- `LCG_SEARCH_MIN_SCORE` is an operator setting. The string advertises support for the setting; it does not expose the configured value.
- Two related changes (exact source matching, stored `source_file`/`chunk_id`) came from one issue; they get distinct strings because a client can depend on one independently of the other.
- Counts that tests or docs hard-code for the feature list or the example response (the documented example in the reference, any test asserting list length) must be updated together with the registry.
- Existing string names, including those added by #858, #860 and #884, remain exactly as released.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The feature registry MUST gain a `lower_snake_case` string for the `LCG_SEARCH_MIN_SCORE` configurable passage-search default (#812).
- **FR-002**: The feature registry MUST gain a string for `health_check`'s `ready_for_reads` field (#809/#855).
- **FR-003**: The feature registry MUST gain a string for test-providers mode (#851).
- **FR-004**: The feature registry MUST gain a string for exact source matching (#853).
- **FR-005**: The feature registry MUST gain a string for the stored `source_file` / `chunk_id` values (#853), distinct from FR-004.
- **FR-006**: The feature registry MUST gain a string for the hybrid passage `legs` field (#871).
- **FR-007**: The feature registry MUST gain a string for implicit default-group write warnings (#856).
- **FR-008**: The feature registry MUST gain a string for the zero-vector record in `knowledge_status` (#810).
- **FR-009**: `knowledge_reembed_missing` MUST NOT receive a string, and the reason (it is already listed in `methods`) MUST be stated in the PR description and in the reference document.
- **FR-010**: Any item in FR-001 to FR-008 that the Plan stage finds, on reading the code, to be genuinely detectable from `methods` or an always-present response field MAY instead be left without a string, provided a one-line reason is recorded in the PR description and in the reference document for that item.
- **FR-011**: New strings MUST follow the existing registry rules: `lower_snake_case`, no version suffix, kept sorted, unique, and never renamed or removed once released. The existing registry unit test MUST continue to pass unmodified in intent.
- **FR-012**: `knowledge_capabilities` MUST return the new strings in `features` on both IPC and MCP surfaces, with `PROTOCOL_VERSION` unchanged.
- **FR-013**: `docs/ipc-mcp-reference.md` MUST list every string in the registry (new and pre-existing) with the issue that introduced it and a one-line meaning, and its example `knowledge_capabilities` response MUST show the current feature list.
- **FR-014**: A test MUST fail when a string in the registry is missing from that reference list.
- **FR-015**: The CONTRIBUTING.md release runbook MUST gain a step stating: every user-visible change in this release has a feature string or a stated reason it doesn't.
- **FR-016**: The runtime behaviour of every feature listed above MUST remain unchanged; this issue only advertises and documents them.
- **FR-017**: Any other document that duplicates the feature list or the method/feature counts (for example generated or mirrored documentation) MUST be kept consistent with the registry.

### Key Entities *(if applicable)*

- **Feature string**: A stable, sorted, append-only `lower_snake_case` identifier in the `knowledge_capabilities` `features` array that tells a client a capability exists.
- **Feature reference list**: The table in `docs/ipc-mcp-reference.md` mapping each string to its introducing issue and meaning.
- **Release runbook check**: The CONTRIBUTING.md step that audits a release for feature-string coverage.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Every candidate change in the issue ends in exactly one of two states, recorded in the PR description: it has a feature string, or it has a stated reason it does not; none is left unaddressed.
- **SC-002**: A client can detect each advertised 0.19.2 capability with a single membership test on `features`, with no version comparison.
- **SC-003**: The set of strings in the registry equals the set of strings in the reference list (0 undocumented, 0 stale), and this is enforced by an automated test.
- **SC-004**: The release runbook contains the feature-coverage check, so a future release cannot skip it without ignoring a written step.
- **SC-005**: No existing feature string is changed or removed, `PROTOCOL_VERSION` is still 1, and no feature's runtime behaviour differs from before.

## Assumptions

- The dispositions in the Background table are defaults: a response-shape field or behaviour change with no new method gets a string (matching `entity_provenance`, `status_episode_breakdown`, `walk_provenance`, `unchanged_noop`); a new method already visible in `methods` does not.
- Exact string names are chosen at the Plan stage, following the existing naming style (short noun phrases, no version suffix). Names are fixed forever once released, so the Plan stage should choose them deliberately.
- The strings advertise build-level support, not run-time state (see Edge Cases).
- "Why it doesn't need a string" notes go in the PR description as the issue requires, and are also recorded in the reference document so the reasoning survives the PR.
- Issue numbers for the pre-existing strings are recoverable from history, ADRs and specs; where one cannot be established, the reference states the closest known origin rather than leaving the cell blank.
- The #858, #860 and #884 strings are already merged on `main` and need no work beyond being listed in the reference.
- No ADR is required: this applies the existing ADR-0854 registry rules. If one is wanted, it is named for issue 885 per repository convention.
- This is a small, clearly scoped change and is delivered as a single PR.

## Out of Scope *(optional)*

- `PROTOCOL_VERSION` (no breaking change involved).
- Changing the runtime behaviour of any listed feature.
- Automating or enforcing the "every user-visible change has a feature string" rule at merge time (the runbook step is a manual check; a CI-level rule would be a separate issue).
- Exposing run-time state (active providers, configured `min_score`) through `knowledge_capabilities`.
- Renaming or removing any existing feature string.

## Source References *(optional)*

- Issue #854 and ADR-0854 (`docs/adr/0854-knowledge-capabilities-protocol-version-and-feature-registry.md`): the registry and its append-only rules.
- `crates/core/src/capabilities.rs` module documentation ("Adding a feature").
- `docs/ipc-mcp-reference.md`, "Capabilities" section.
- CONTRIBUTING.md, "Release runbook (maintainers)".
- Source issues and PRs: #812/#813, #809/#855/#869, #851/#870, #853/#868, #871/#873, #856/#874, #826, #810.
