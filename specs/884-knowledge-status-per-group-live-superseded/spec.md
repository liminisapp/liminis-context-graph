# Feature Specification: knowledge_status per-group live / superseded / pending episode counts and only-superseded entities and edges

**Feature Branch**: `fabrik/issue-884`
**Created**: 2026-11-22
**Status**: Specified
**Input**: User description: "## Problem

ADR-0828 (#828) decided not to prune superseded episodes yet, and defined reopening triggers (for example \"superseded : live episodes exceeds 2 : 1 in any group\"). Nothing exposes those numbers today: `knowledge_status`'s `episode_count` is a total, so the triggers cannot be checked on a real workspace without hand-written Cypher.

## Acceptance criteria

- `knowledge_status` reports, per group, the number of **live**, **superseded** and **pending** (uncommitted document revision) episodes, using the same liveness predicate as belief/search (`EpisodeSupersession::live_at`).
- It also reports, per group, entities supported **only** by superseded episodes (no live `MENTIONS`) and edges whose producing `episodes` are all superseded (ADR-0705 single-producer edges), as defined in ADR-0828 Q1.
- Existing `knowledge_status` fields keep their meaning (additive change; no `PROTOCOL_VERSION` bump). If the counts are too expensive for every `knowledge_status` call, put them behind an opt-in parameter or a lint category and say so in the docs.
- ADR-0828's reopening-triggers section points at the new fields.

## Out of scope

Any pruning or retention policy."

## Background

ADR-0828 (issue #828, a research issue) measured the cost of retaining superseded episodes (episodes replaced by a later document revision, or whose chunk was removed). It decided not to prune yet and recorded *reopening triggers*: the question is reopened if, in a real workspace, any of the following is observed:

1. superseded : live episodes exceeds 2 : 1 in any group;
2. rebuild from WAL takes more than ~2× the live-only equivalent;
3. WAL bytes per live episode exceed ~3× the N=1 figure;
4. a production-scale measurement contradicts the linear cost model.

Trigger 1 needs a per-group live/superseded split, and the ADR's own measurement method also tracked entities with no live `MENTIONS` and edges whose producing episodes are all superseded ("only-superseded"). Today `knowledge_status` reports only a total `episode_count`, so operators and agents cannot evaluate the trigger on a real workspace without hand-written Cypher. ADR-0828's "Follow-ups to file" item 2 asks for exactly this measurement surface. This issue builds only the measurement; it adds no pruning or retention behavior.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Check the 2:1 superseded:live trigger per group (Priority: P1)

An operator or maintainer wants to know, for each group in a workspace, how many episodes are live, superseded, and pending, so they can compute the superseded : live ratio and decide whether ADR-0828 trigger 1 has fired.

**Why this priority**: This is the headline trigger the issue exists to make measurable, and the cheapest part of the surface.

**Independent Test**: Ingest a document, revise it so some chunks are superseded, leave one revision uncommitted, then call `knowledge_status` (with the opt-in if one exists) and compare the per-group counts with a hand-written query using the belief/search liveness rule.

**Acceptance Scenarios**:

1. **Given** a group with 10 live episodes and 25 superseded episodes, **When** the operator requests the status breakdown, **Then** that group reports live = 10, superseded = 25, pending = 0, from which the ratio 2.5 : 1 can be read.
2. **Given** a document revision that is ingested but not yet committed, **When** the status breakdown is requested, **Then** its episodes are counted as pending and are not counted as live or superseded.
3. **Given** a group with no superseded or pending episodes, **When** the breakdown is requested, **Then** the group reports superseded = 0 and pending = 0 (present, not omitted).
4. **Given** several groups with different histories, **When** the breakdown is requested, **Then** each group's counts are reported separately and none are mixed across groups.

---

### User Story 2 - See entities and edges supported only by superseded history (Priority: P2)

A maintainer wants to know how much of the graph (entities and edges) exists only because of superseded episodes, since that is the portion a future prune could actually reclaim and the figure ADR-0828 Q1 uses to size the cost.

**Why this priority**: Gives the "how much could pruning reclaim" number the triggers and any follow-up pruning design rely on, but the triggers can be partially evaluated without it.

**Independent Test**: Using the scenario from ADR-0828's edge cases (oscillation A→B→A and a removed chunk), check that `Globex` is only-superseded while `Acme` is not, and that edge counts follow producers, not fact text.

**Acceptance Scenarios**:

1. **Given** an entity mentioned only by superseded episodes, **When** the breakdown is requested, **Then** it is counted in that group's only-superseded entities.
2. **Given** an entity mentioned by at least one live episode and also by superseded ones, **When** the breakdown is requested, **Then** it is not counted as only-superseded.
3. **Given** an edge whose producing episodes are all superseded, **When** the breakdown is requested, **Then** it is counted in only-superseded edges, even if an identical-text edge with a live producer exists (edges are counted by producer, not by fact text).
4. **Given** an entity or edge in a group whose supporting episodes belong to a different group, **When** the breakdown is requested, **Then** the attribution to a group is documented and deterministic (see Assumptions).

---

### User Story 3 - Existing consumers are unaffected, and cost is controlled (Priority: P1)

An existing client of `knowledge_status` (the Electron app, scripts, the MCP tool) must keep working unchanged, and routine status polling must not become slow because of the new counts.

**Why this priority**: `knowledge_status` is a frequently called status endpoint. Breaking it, or making it expensive, would hurt the app. This is a hard constraint on the feature.

**Independent Test**: Call `knowledge_status` with no new parameters on a large workspace and confirm existing fields are unchanged and latency is not materially worse than before.

**Acceptance Scenarios**:

1. **Given** a client that sends no new parameters, **When** it calls `knowledge_status`, **Then** every existing field keeps its name, type and meaning, and the protocol version is unchanged.
2. **Given** the new counts are gated behind an opt-in, **When** a client omits the opt-in, **Then** it pays no extra query cost and the response contains no breakdown; **When** it sets the opt-in, **Then** the breakdown is returned.
3. **Given** the capabilities/feature registry (ADR-0854), **When** a client checks for support, **Then** it can discover that the breakdown exists without probing.

---

### User Story 4 - ADR-0828 points at the new surface (Priority: P3)

A reader of ADR-0828's reopening triggers should be told how to measure each trigger with the new fields.

**Why this priority**: Documentation closure; low risk, required by the issue.

**Independent Test**: Read ADR-0828's "Reopening triggers" and "Follow-ups" sections and confirm they reference the new fields and mark follow-up 2 as done.

**Acceptance Scenarios**:

1. **Given** the ADR's reopening-triggers section, **When** read, **Then** it names the new `knowledge_status` fields (and the opt-in, if any) as the way to evaluate trigger 1, and removes or updates the statement that "none can be evaluated in production today".

---

### Edge Cases

- **Oscillation A→B→A**: entity support is by `MENTIONS` from live episodes; an entity mentioned by both an old superseded and a current live episode is not only-superseded. Edges are counted by producer episodes, so a superseded duplicate of a live fact is still counted as only-superseded (ADR-0828 edge cases).
- **Chunk removed with no successor** (`chunk_removed`, no `superseded_by`): the episode is superseded, and its otherwise-unsupported entities and edges are only-superseded.
- **Pending revisions**: uncommitted document revision episodes are counted as pending only, never as live or superseded, matching the liveness rule used by belief and search. Entities and edges produced solely by pending episodes are neither live-supported nor only-superseded.
- **Unattributed episodes** (ADR-0292) are never superseded; they count as live.
- **Empty workspace or group with zero episodes**: all counts are 0 and the group is handled without error.
- **Entities shared across groups** (cross-group pointers, ADR-0361/0369): group attribution must be defined and must not double-count.
- **Degraded mode / DB not open**: `knowledge_status` must keep its existing degraded-mode behavior; the breakdown is absent or clearly marked unavailable rather than erroring the whole call.
- **Entity with no MENTIONS at all**: it has no support from any episode, live or superseded, and is not "only supported by superseded episodes".

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_status` MUST be able to report, for each group, the number of live, superseded and pending episodes, with the invariant that the three counts partition the group's episodes and sum to the group's episode total.
- **FR-002**: Episode classification MUST use the same liveness predicate as belief and search (`EpisodeSupersession::live_at`), so the counts agree with what `knowledge_search_*` and belief reads treat as live/superseded; no second definition of liveness may be introduced.
- **FR-003**: "Pending" MUST mean episodes of an uncommitted document revision (ADR-0785) and MUST be reported separately from live and superseded.
- **FR-004**: `knowledge_status` MUST be able to report, for each group, the number of entities supported only by superseded episodes (at least one `MENTIONS` from a superseded episode, and no `MENTIONS` from a live episode), as defined in ADR-0828 Q1.
- **FR-005**: `knowledge_status` MUST be able to report, for each group, the number of edges whose producing episodes (`episodes`) are all superseded (ADR-0705 single-producer edges), counted by producer, not by fact text.
- **FR-006**: The change MUST be additive: all existing `knowledge_status` fields keep their names, types and meanings, and `PROTOCOL_VERSION` is not bumped.
- **FR-007**: The cost of the new counts MUST be assessed; if computing them on every `knowledge_status` call is materially more expensive than the current call, they MUST be returned only on explicit opt-in (a request parameter or a lint category), and the documentation MUST say which and why. A call without the opt-in MUST NOT pay for the new queries.
- **FR-008**: The breakdown MUST be discoverable through the existing capabilities/feature registry mechanism (ADR-0854), so clients can detect support without probing.
- **FR-009**: If a new request parameter is added, it MUST be declared in the MCP-over-stdio tool schema for `knowledge_status`, with tool/registry count tests updated as required, and unknown or absent values keep the current behavior.
- **FR-010**: The output MUST present, per group, the live, superseded, pending, only-superseded-entity and only-superseded-edge counts under stable, documented field names, so the superseded : live ratio for ADR-0828 trigger 1 can be computed by a client from the response alone.
- **FR-011**: Failure to compute the breakdown (for example degraded mode) MUST NOT fail the rest of `knowledge_status`.
- **FR-012**: ADR-0828's "Reopening triggers" section MUST be updated to point at the new fields and opt-in (including removing the "none can be evaluated today" statement), and the corresponding follow-up item MUST be marked as done by this issue. The new behavior MUST be recorded in the user-facing docs and changelog, and a new ADR is only required if a non-obvious decision (for example opt-in vs. always-on) is made; any such ADR is named by this issue's number.
- **FR-013**: The feature MUST NOT delete, mutate, or otherwise change any episode, entity, edge or WAL content; it is read-only.

### Key Entities *(if applicable)*

- **Episode liveness state**: one of live, superseded, pending, derived from the episode's supersession/revision state by the shared predicate.
- **Only-superseded entity**: an entity with `MENTIONS` only from superseded episodes.
- **Only-superseded edge**: an edge whose producing episodes are all superseded.
- **Group**: the existing per-group partition of the graph (the unit of the 2 : 1 trigger).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On a workspace with a known history (for example the ADR-0828 benchmark matrix at N=10 revisions), the reported per-group live and superseded counts equal the counts obtained by a hand-written query using the belief/search liveness rule, with zero discrepancy.
- **SC-002**: Per group, live + superseded + pending equals the group's total episode count in every tested scenario, including with pending revisions and chunk removals.
- **SC-003**: The only-superseded entity and edge counts match ADR-0828's reference numbers for the oscillation and chunk-removed edge cases (`Globex` and the S2 entities counted; `Acme` not counted).
- **SC-004**: An operator can evaluate ADR-0828 trigger 1 for every group from a single `knowledge_status` call, without writing Cypher.
- **SC-005**: A `knowledge_status` call that does not use any new parameter returns every pre-existing field unchanged, and its latency is not measurably worse (within normal run-to-run noise) than before the change.
- **SC-006**: A client can determine whether the workspace's service supports the breakdown from the capabilities response alone.

## Assumptions

- ADR-0828 (open on branch `fabrik/issue-828`) will land on `main`; its measurement definitions (Q1 "only-superseded" definitions, trigger list, and follow-up 2) are the authoritative definitions for the new counts. If it has not merged when this is implemented, the ADR-0828 edit (FR-012) is made to whichever copy is on the branch, or deferred to a trivial follow-up commit once merged.
- Computing live/superseded/pending counts is likely cheap, but the only-superseded entity and edge counts require scanning `MENTIONS` and edge producers and may be costly on large workspaces; Research should measure and decide whether the whole breakdown or only the expensive part is opt-in. The default assumption is that the breakdown is opt-in (parameter on `knowledge_status`), preserving the default call's cost; this is reversible.
- An entity or edge is attributed to the group it belongs to (its own `group_id`); cross-group `MENTIONS` pointers are counted according to a single documented rule so nothing is double-counted.
- Entities or edges with only pending producers are not "only-superseded"; they are simply not live-supported yet.
- Unattributed episodes are live (never superseded) and so are counted as live.
- The field naming and exact response shape are a design decision for the Plan stage; this spec fixes only what must be reported and the invariants.

## Out of Scope *(optional)*

- Any pruning, retention policy, `knowledge_prune_superseded`, or compaction of superseded history (ADR-0828 follow-up 4).
- Measuring ADR-0828 triggers 2 to 4 (rebuild time, WAL bytes per live episode, production-scale model validation); only the counts for trigger 1 and the only-superseded figures are exposed.
- Edge de-duplication on re-extraction and handling of abandoned pending revisions (ADR-0828 follow-up 5).
- Fixing `knowledge_dump_wal` skipping `MENTIONS` edges (ADR-0828 follow-up 1).
- Any `PROTOCOL_VERSION` change.

## Source References *(optional)*

- ADR-0828 (`docs/adr/0828-superseded-episode-retention.md`, on `origin/fabrik/issue-828`): Q1 counts, edge cases, reopening triggers, follow-up 2.
- ADR-0705 (belief time separate from validity), ADR-0707 (document supersession), ADR-0785 (document-mode revisions and commit boundary), ADR-0854 (capabilities and feature registry), ADR-0292 (episode attribution).
