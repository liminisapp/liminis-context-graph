# Feature Specification: Align live-episode predicate and caps between find_entities include_episodes and knowledge_walk include_provenance

**Feature Branch**: `fabrik/issue-883`
**Created**: 2026-11-22
**Status**: Specified
**Input**: User description: "## Problem

Two provenance features added in 0.19.2 disagree on which episodes count as \"live\" and on how many they return:

| | `knowledge_find_entities` `include_episodes` (#857, ADR-0857) | `knowledge_walk` `include_provenance` (#860) |
|---|---|---|
| Liveness | `EpisodeOriginInfo::is_live` (the `support` predicate): does **not** exclude a pending document revision | `EpisodeSupersession::live_at(now)` (the belief/search predicate): excludes superseded, pending-revision and deleted episodes |
| Cap | `ENTITY_EPISODE_CAP = 50` per entity | `MAX_PROVENANCE_EPISODES = 20` per edge |

So while a `knowledge_process_document` revision is pending (written, not yet committed, ADR-0785), the same episode can be cited by one method and not the other. ADR-0857 notes the difference but does not justify it.

## Acceptance criteria

- `find_entities` `include_episodes` uses the same liveness predicate as `knowledge_walk` provenance and belief/search (`live_at`), so a pending-revision episode is never reported as a live mention. A test covers a pending revision: excluded from `episode_uuids` before commit, included after.
- Decide whether entity `support`/`corroboration` should also exclude pending revisions; if so change it, if not record why in an ADR-0857 addendum.
- The two caps are either aligned or the reference docs (`docs/ipc-mcp-reference.md`) state each and why they differ.
- No change to responses when the flags are off.

## Out of scope

`get_entity_neighbors` / `get_entities_by_source`, which include superseded episodes (documented in ADR-0857, \"Neighbors parity reads as live subset\")."

## Background

Release 0.19.2 added two provenance features that tell a client which episodes back an entity or an edge: `include_episodes` on `knowledge_find_entities` (#857, ADR-0857) and `include_provenance` on `knowledge_walk` (#860). They were built separately and use different notions of a "live" episode:

- `find_entities` uses the entity-support liveness check, which excludes superseded and deleted episodes but does **not** exclude an episode belonging to a document revision that has been written but not yet committed (ADR-0785, the `knowledge_process_document` commit boundary).
- `knowledge_walk` provenance uses the same liveness predicate as belief and search, which also excludes pending-revision episodes.

During a pending revision, one method can cite an episode as a current mention while the other (and search/belief) treats it as not yet live. A client combining the two methods sees inconsistent evidence, and the pending, uncommitted content leaks through `find_entities`. The two methods also cap differently (50 episodes per entity vs 20 per edge), and ADR-0857 records the liveness difference without justifying it.

This issue makes the liveness definition consistent across the user-facing provenance surfaces, settles whether entity support and corroboration counts should follow it, and makes the cap difference either disappear or be explicitly documented.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Pending revisions are never reported as live mentions by find_entities (Priority: P1)

A client calls `knowledge_find_entities` with `include_episodes` while a document revision is pending (written, not yet committed). The episodes of that pending revision must not appear in the returned episode list. Once the revision commits, its episodes appear, exactly as the other provenance and search surfaces treat them.

**Why this priority**: This is the correctness defect. Uncommitted content is cited as live evidence, and the two methods disagree about the same episode.

**Independent Test**: Ingest a document, start a revision of it without committing, call `find_entities` with `include_episodes` for an entity mentioned by the revision, and check the episode list. Commit the revision and call again.

**Acceptance Scenarios**:

1. **Given** an entity mentioned by an episode of a pending (uncommitted) document revision, **When** `knowledge_find_entities` is called with `include_episodes`, **Then** that episode's identifier is absent from the entity's episode list.
2. **Given** the same entity after the revision is committed, **When** `knowledge_find_entities` is called with `include_episodes`, **Then** that episode's identifier is present.
3. **Given** an episode that is cited by `knowledge_walk` `include_provenance` for an edge, **When** `find_entities` with `include_episodes` is called for the endpoint entity, **Then** the two methods agree on whether that episode is live (including during a pending revision).
4. **Given** superseded or deleted episodes, **When** `find_entities` is called with `include_episodes`, **Then** they remain excluded as before.

---

### User Story 2 - Documented decision on entity support and corroboration (Priority: P2)

A maintainer or client author needs to know whether the entity `support` and `corroboration` figures count pending-revision episodes. The question is explicitly decided: either those figures also exclude pending revisions, or they deliberately keep counting them and the reason is recorded as an addendum to ADR-0857.

**Why this priority**: Once the episode list excludes pending revisions, a `support` count that includes them can disagree with the list next to it. The decision prevents an undocumented new inconsistency, but it does not block the P1 fix.

**Independent Test**: With a pending revision, compare an entity's `support`/`corroboration` with its episode list, and confirm the behaviour matches what the ADR-0857 addendum (or updated ADR text) states.

**Acceptance Scenarios**:

1. **Given** the decision is to exclude pending revisions from support, **When** an entity has episodes only from a pending revision, **Then** its support and corroboration do not count them, and a test covers this.
2. **Given** the decision is to keep counting them, **When** the work is delivered, **Then** an ADR-0857 addendum states the reason and the documented behaviour matches the code.

---

### User Story 3 - Caps are aligned or their difference is documented (Priority: P3)

A client developer reading the IPC/MCP reference can see how many episodes each provenance feature returns and why. Either the per-entity and per-edge limits are the same, or the reference states each limit and the reason they differ.

**Why this priority**: Documentation or a constant alignment; no correctness impact.

**Independent Test**: Read the reference; confirm the stated limits match the behaviour of both methods.

**Acceptance Scenarios**:

1. **Given** the caps are aligned, **When** the reference is read, **Then** it states the single limit for both methods.
2. **Given** the caps stay different, **When** the reference is read, **Then** it states each limit and why they differ (per entity vs per edge).

---

### Edge Cases

- An entity whose only episodes belong to a pending revision: its episode list is empty until commit.
- A pending revision that replaces an earlier live revision: the earlier revision's episodes remain live until the commit boundary, per belief/search behaviour; the list must match what search treats as live at the same moment.
- Truncation: if the cap is applied, it is applied after liveness filtering, so excluded episodes never consume cap slots.
- Flags off: `find_entities` without `include_episodes` and `knowledge_walk` without `include_provenance` return byte-for-byte the same responses as before.
- Liveness is evaluated at request time, consistent with `knowledge_walk`.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_find_entities` with `include_episodes` MUST determine episode liveness with the same predicate used by `knowledge_walk` `include_provenance` and by belief/search, so superseded, deleted and pending-revision episodes are never reported.
- **FR-002**: An episode of a pending (uncommitted) document revision MUST be absent from an entity's episode list before commit and present after commit.
- **FR-003**: A test MUST cover the pending-revision case in FR-002 (excluded before commit, included after).
- **FR-004**: The project MUST make and record an explicit decision on whether entity `support` and `corroboration` exclude pending-revision episodes. If they do, the behaviour is changed and tested; if not, the reason is recorded in an addendum to ADR-0857.
- **FR-005**: The per-entity episode cap of `find_entities` and the per-edge episode cap of `knowledge_walk` MUST either be aligned to one value, or `docs/ipc-mcp-reference.md` MUST state each cap and why they differ.
- **FR-006**: When the cap truncates, it MUST apply to the live episodes after the liveness filter, with ordering unchanged.
- **FR-007**: Responses MUST be unchanged when `include_episodes` and `include_provenance` are not set.
- **FR-008**: ADR-0857's statement of the liveness predicate and the reference documentation MUST be updated (or an addendum added) so they describe the shipped behaviour and no longer record the liveness mismatch as intended.

### Key Entities *(if applicable)*

- **Live episode**: An episode that is not superseded, not deleted, and not part of an uncommitted document revision.
- **Pending revision**: A document revision whose episodes have been written but not yet committed (ADR-0785).
- **Entity support / corroboration**: Existing per-entity evidence figures derived from live episodes (distinct documents).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For any set of episodes, including a pending revision, `find_entities` `include_episodes` and `knowledge_walk` `include_provenance` classify every episode as live or not-live identically.
- **SC-002**: A test shows a pending-revision episode is absent before commit and present after commit, and it passes.
- **SC-003**: The ADR-0857 addendum or updated text states the entity support/corroboration decision, and the code behaves as stated.
- **SC-004**: The reference documentation states the episode limit(s) of both methods, and they match the code.
- **SC-005**: Existing tests for responses with the flags off pass unchanged.

## Assumptions

- "Live" for user-facing provenance means the belief/search definition (`live_at`), since that is what a client sees elsewhere; no consumer is expected to rely on pending-revision episodes appearing in `find_entities`.
- The behaviour change to `find_entities` provenance is acceptable without a new capability flag: the field shipped in 0.19.2 and the change only removes uncommitted episodes.
- Whether `support`/`corroboration` change is left to be decided during research/planning (FR-004); either outcome satisfies this spec as long as it is tested or recorded, and the final behaviour must be consistent with the episode list or the difference explained.
- The caps may reasonably differ because one is per entity and one is per edge; keeping them different with a documented rationale is acceptable, and so is aligning them.
- The ADR is amended by an addendum (same ADR, issue-numbered naming not required for an addendum to an existing ADR).

## Out of Scope *(optional)*

- `get_entity_neighbors` and `get_entities_by_source`, which include superseded episodes (ADR-0857, "Neighbors parity reads as live subset").
- Changing any response shape or any behaviour when the provenance flags are off.
- Request-tunable caps.

## Source References *(optional)*

- ADR-0857 (find_entities episode provenance), ADR-0785 (document-mode revisions and commit boundary)
- Issues #857, #860
