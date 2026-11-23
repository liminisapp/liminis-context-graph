# Feature Specification: Integrator guide, minimal client profile, and a 2 MB configurable process_document cap

**Feature Branch**: `fabrik/issue-862`
**Created**: 2026-11-23
**Status**: Specified
**Input**: User description: "Implements #840.

## Problem
A newcomer to LCG's roughly 60 methods can't tell which small set a client that ingests, searches, shows neighbours and deletes actually needs. One integrator rebuilt features that already existed (`delete_by_source`, `process_document`, `attributes` and `title`, `get_entities_by_source`). The `process_chunk` oversize warning says \"consider splitting oversized input\" instead of pointing at `knowledge_process_document`. `process_document` refuses inputs over 512 KB, but real web pages reach about 600 K characters.

## Change
- **Write an integrator guide** at `docs/integrator-guide.md`, linked from the docs index and the README. It covers:
  - the transport: socket location, newline-delimited JSON-RPC, one request at a time per connection, timeouts;
  - discovering the server with `knowledge_capabilities` (#854);
  - groups and the defaults table (#856);
  - ingest, update, retire and delete side by side (#858, #859);
  - search and neighbours with recommended parameters, including `min_score` and `LCG_SEARCH_MIN_SCORE` (#812) and `include_episodes` (#857);
  - provenance and citations (#853, #860);
  - health checks (#855).
- **Define a minimal client profile** of about 8 methods that are kept compatible across releases. Record the guarantee in an ADR, list the methods in the guide, and add a test that each one stays in the dispatcher and the MCP registry.
- **Fix the pointers to `process_document`.** The `process_chunk` oversize `warning` and its tool description point at `knowledge_process_document`.
- **Raise `process_document`'s cap** (the maintainer's decision). Measure memory and latency on a 2 MB document with the mock providers and record the numbers in the PR. Ship 2 MB as the default unless the measurements argue against it. Make it configurable with `LCG_DOCUMENT_MAX_BYTES`, validated strictly at startup like `LCG_SEARCH_MIN_SCORE` (#812). Inputs over the cap are still **refused** with `document_too_large`, and the error names the cap and the variable. Never truncate.

## Acceptance
- The guide exists and is linked, and the site link check passes.
- The profile test passes.
- A 1.5 MB document is accepted by default, a 3 MB one is refused with the new message, and `LCG_DOCUMENT_MAX_BYTES=4MiB` accepts the 3 MB one.
- The docs, CHANGELOG and `llms-full.txt` are updated."

## Background

LCG exposes roughly 60 `knowledge_*` methods over its Unix-socket JSON-RPC transport and the MCP-over-stdio surface. A client that only needs to ingest, search, show neighbours and delete cannot tell from the reference which small subset that is. One integrator consequently rebuilt functionality that already existed (`delete_by_source`, `process_document`, `attributes` and `title`, `get_entities_by_source`). The sibling issues #853–#860 and #812 have added or are adding the pieces a client needs (capabilities discovery, group defaults, update/retire/delete, provenance and citations, health checks, `min_score`, `include_episodes`), but nothing ties them together for a newcomer.

Two smaller defects came out of the same integrator experience (#840):

- When `knowledge_process_chunk` sees oversized input, its warning and tool description say "consider splitting oversized input" rather than naming `knowledge_process_document`, the method built for that case.
- `knowledge_process_document` refuses input over 512 KB (the current refusal is `document_too_large` and already names the cap in bytes), yet real web pages reach about 600 K characters. The maintainer has decided to raise the default to 2 MB and make it operator-configurable, subject to a measurement of memory and latency on a 2 MB document using the mock providers.

This issue depends on #861 (now merged) and builds on the features delivered by the other issues listed above.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Integrator learns the minimal client surface from one guide (Priority: P1)

A developer building a client that ingests, searches, shows neighbours and deletes opens `docs/integrator-guide.md` (linked from the docs index and the README) and finds, in one place, how to connect, discover the server's abilities, choose groups, ingest/update/retire/delete, search with recommended parameters, read provenance and check health, without reading the full method reference.

**Why this priority**: This is the core of the issue. It prevents integrators from rebuilding existing features.

**Independent Test**: Follow the guide alone against a running service and complete an ingest → search → neighbours → delete round trip using only methods the guide names.

**Acceptance Scenarios**:

1. **Given** the docs index or the README, **When** a reader looks for integration help, **Then** a link to the integrator guide is present and resolves (the site link check passes).
2. **Given** the guide, **When** a reader looks for the transport, **Then** it states the socket location, newline-delimited JSON-RPC framing, that a connection handles one request at a time, and the timeout behaviour.
3. **Given** the guide, **When** a reader looks for ingest, update, retire and delete, **Then** the four operations appear side by side with the method used for each and when to choose which.
4. **Given** the guide, **When** a reader looks for search guidance, **Then** it names `min_score`, the `LCG_SEARCH_MIN_SCORE` default, `include_episodes`, and recommended parameters for search and neighbours.
5. **Given** the guide, **When** a reader looks for health and provenance, **Then** it explains how to discover the server with `knowledge_capabilities`, how to run health checks, and how to obtain provenance and citations.

---

### User Story 2 - Client authors rely on a minimal profile that stays compatible (Priority: P1)

A client author wants a small, explicitly named set of about 8 methods that LCG commits to keeping compatible across releases, so they can build against it without tracking changes to the other ~50 methods.

**Why this priority**: The guide is only trustworthy if the set it recommends is a stable promise, and the promise must be enforced rather than aspirational.

**Independent Test**: The profile test fails if any listed method is removed from the dispatcher or the MCP registry.

**Acceptance Scenarios**:

1. **Given** the guide, **When** a reader looks for the minimal profile, **Then** it lists each method in the profile and states the compatibility guarantee.
2. **Given** an ADR for the guarantee, **When** a maintainer considers changing a profile method, **Then** the ADR states what counts as a breaking change and what process applies.
3. **Given** a change that removes a profile method from the dispatcher or from the MCP tool registry, **When** the test suite runs, **Then** the profile test fails and names the missing method.
4. **Given** the profile list in the guide and the list the test checks, **When** either changes alone, **Then** the discrepancy is detectable (the two cannot silently drift apart).

---

### User Story 3 - Oversized chunks point to the right method (Priority: P2)

A client calls `knowledge_process_chunk` with input that triggers the oversize warning and is told to use `knowledge_process_document` instead of a vague "split it".

**Why this priority**: A small, low-risk fix that removes a direct cause of the integrator's confusion.

**Independent Test**: Trigger the oversize warning and inspect the warning text and the tool description.

**Acceptance Scenarios**:

1. **Given** `knowledge_process_chunk` input over the oversize threshold, **When** the call returns, **Then** the `warning` names `knowledge_process_document`.
2. **Given** the `knowledge_process_chunk` tool description in the MCP registry and the reference docs, **When** a reader looks up oversized input, **Then** it points to `knowledge_process_document`.

---

### User Story 4 - Large web pages can be ingested as documents (Priority: P1)

An integrator ingests a real web page of about 600 K characters, or a larger document up to 2 MB, with `knowledge_process_document`, and an operator can raise or lower the limit for their deployment.

**Why this priority**: The current 512 KB cap rejects realistic input, and the maintainer has decided to change it.

**Independent Test**: Submit 1.5 MB, 3 MB and 3 MB-with-`LCG_DOCUMENT_MAX_BYTES=4MiB` documents and check the outcomes.

**Acceptance Scenarios**:

1. **Given** default configuration, **When** a 1.5 MB document is submitted, **Then** it is accepted and processed.
2. **Given** default configuration, **When** a 3 MB document is submitted, **Then** it is refused with `document_too_large`, and the message names the effective cap and `LCG_DOCUMENT_MAX_BYTES`.
3. **Given** `LCG_DOCUMENT_MAX_BYTES=4MiB`, **When** the same 3 MB document is submitted, **Then** it is accepted.
4. **Given** an invalid `LCG_DOCUMENT_MAX_BYTES` value (for example non-numeric, zero, negative or unparseable), **When** the service starts, **Then** startup fails with a message naming the variable and the offending value, rather than silently falling back.
5. **Given** any input over the cap, **When** it is refused, **Then** nothing from it is ingested (no truncation, no partial write).

---

### User Story 5 - Maintainers can see the cost of the larger cap (Priority: P2)

A reviewer wants evidence that a 2 MB document is safe to accept by default.

**Why this priority**: The maintainer made the 2 MB default conditional on measurements not arguing against it.

**Independent Test**: The PR description contains memory and latency numbers for a 2 MB document run with the mock providers.

**Acceptance Scenarios**:

1. **Given** the PR, **When** a reviewer reads it, **Then** it records peak memory and end-to-end latency for a 2 MB document with the mock providers, and states whether 2 MB is shipped as the default.

---

### Edge Cases

- A document exactly at the cap is accepted; one byte over is refused. The cap is measured in bytes of the content, not characters (a 600 K-character page with multibyte text may be larger in bytes).
- `LCG_DOCUMENT_MAX_BYTES` accepts a plain byte count and the `KiB`/`MiB` suffix form shown in the acceptance (`4MiB`); behaviour for other forms (`4MB`, `4m`, whitespace, decimals) is defined and tested, with invalid forms rejected at startup.
- Any inbound request size limit on the socket or MCP transport must not reject a document below the document cap before the document cap check applies; otherwise the cap in the error message would be misleading.
- A very high configured cap (for example many GiB) is either bounded by a documented upper limit or accepted with a documented warning; the choice is stated in the docs.
- The guide must not describe methods or options from the dependency issues (#853–#860, #812) that have not shipped by the time it is written; where they have shipped, the guide matches their actual behaviour.
- A client that never reads the guide and sends oversized text to `knowledge_process_chunk` still gets the corrected pointer in the warning.
- Reference docs, `docs/llms-full.txt` and the MCP tool descriptions must stay mutually consistent after the changes (the old 512 KB figure appears nowhere).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: A guide exists at `docs/integrator-guide.md` and is linked from the docs index and from the README.
- **FR-002**: The guide describes the transport: socket location, newline-delimited JSON-RPC 2.0 framing, one in-flight request per connection, and timeout behaviour.
- **FR-003**: The guide explains discovering the server with `knowledge_capabilities`.
- **FR-004**: The guide explains groups and includes the group defaults table.
- **FR-005**: The guide presents ingest, update, retire and delete side by side, naming the method for each and when to use it.
- **FR-006**: The guide gives recommended parameters for search and neighbours, including `min_score`, the `LCG_SEARCH_MIN_SCORE` default, and `include_episodes`.
- **FR-007**: The guide explains provenance and citations and how to run health checks.
- **FR-008**: A minimal client profile of about 8 methods is defined, covering at least capability discovery, health, ingest (chunk and document), search, neighbours, and delete by source, and is listed in the guide.
- **FR-009**: An ADR, named by this issue's number per the repository convention, records the compatibility guarantee for the profile: which methods, what counts as a breaking change, and the process for changing one.
- **FR-010**: A test asserts that every profile method remains in the dispatcher and in the MCP tool registry, and fails with a message naming any missing method.
- **FR-011**: The `knowledge_process_chunk` oversize `warning` and its tool description point to `knowledge_process_document`.
- **FR-012**: The default `process_document` size cap is 2 MB unless the measurements in FR-017 argue against it, in which case the PR states the chosen value and why.
- **FR-013**: The cap is configurable through `LCG_DOCUMENT_MAX_BYTES`, parsed strictly at startup in the same manner as `LCG_SEARCH_MIN_SCORE`: an invalid value aborts startup with a message naming the variable and value.
- **FR-014**: `LCG_DOCUMENT_MAX_BYTES` accepts a plain byte count and `KiB`/`MiB` suffixed values (for example `4MiB`).
- **FR-015**: Input over the cap is refused with `document_too_large`; the message names the effective cap and `LCG_DOCUMENT_MAX_BYTES`. Input is never truncated and nothing is ingested from a refused request.
- **FR-016**: The README, configuration reference, IPC/MCP reference, CHANGELOG and `docs/llms-full.txt` are updated to match (new default, new variable, new guide, profile).
- **FR-017**: The PR description records memory and latency measured on a 2 MB document using the mock providers.

### Key Entities *(if applicable)*

- **Minimal client profile**: The named set of about 8 methods with a cross-release compatibility promise, recorded in the ADR and listed in the guide.
- **Document size cap**: The effective maximum byte size accepted by `process_document`, from the default or `LCG_DOCUMENT_MAX_BYTES`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The guide exists, is reachable from the docs index and the README, and the site link check passes.
- **SC-002**: The profile test passes, and deleting any profile method from the dispatcher or MCP registry makes it fail.
- **SC-003**: With default configuration a 1.5 MB document is accepted and a 3 MB document is refused with an error naming the cap and `LCG_DOCUMENT_MAX_BYTES`; with `LCG_DOCUMENT_MAX_BYTES=4MiB` the 3 MB document is accepted.
- **SC-004**: A reader following only the guide can complete ingest, search, neighbours and delete using only methods in the minimal profile.
- **SC-005**: The oversize `process_chunk` warning and tool description name `knowledge_process_document`.
- **SC-006**: The PR records memory and latency numbers for a 2 MB document, and the docs, CHANGELOG and `llms-full.txt` reflect all changes with no remaining reference to a 512 KB document cap.

## Assumptions

- The exact membership of the ~8-method profile is chosen during planning, covering the operations named in FR-008; "about 8" allows a small variance.
- The dependency issues (#812, #853–#860) are merged or being merged; the guide documents the behaviour as shipped on `main` at implementation time.
- The cap is measured in bytes, consistent with the current refusal message.
- `LCG_DOCUMENT_MAX_BYTES` follows the same startup-validation pattern as `LCG_SEARCH_MIN_SCORE`.
- The ADR is named `docs/adr/0862-<slug>.md` per the repository's issue-number convention.
- The 2 MB default holds unless the measurements show a clear problem; the maintainer has already chosen this direction.
- The profile compatibility promise covers method names, required parameters and result shapes, not unspecified internal behaviour; the ADR defines this precisely.

## Out of Scope *(optional)*

- Changing the behaviour of the dependency features (#812, #853–#860) themselves.
- Chunking, streaming or splitting large documents server-side to exceed the cap.
- Truncating oversized input in any form.
- Adding new methods beyond those needed to fill the profile (the profile uses existing methods).
- Client SDKs or generated clients.
- Changing the wire protocol or MCP transport.

## Source References *(optional)*

- Parent discussion: #840
- Related: #812 (`LCG_SEARCH_MIN_SCORE`), #853, #854, #855, #856, #857, #858, #859, #860, #861
- Repository conventions: ADR numbers equal issue numbers; MCP tool registry counts are asserted in its own tests
