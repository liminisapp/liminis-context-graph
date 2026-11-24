# Feature Specification: knowledge_walk — a frontier entity can be the next hop's answer

**Feature Branch**: `fabrik/issue-912`
**Created**: 2026-11-23
**Status**: Specified
**Input**: User description: "knowledge_walk: a frontier entity can't be the next hop's answer (visited set drops sibling edges)

## Problem

`knowledge_walk` drops a correct answer when that entity was already reached on the previous hop through a different edge.

**Cause.** `candidates()` in `crates/core/src/walk.rs` skips every edge whose far end is in `visited`, and `visited` includes the current frontier. If hop 1 reaches both a supplier and one of its projects, hop 2 can't take the edge `project -SUPPLIED_BY-> supplier`: both ends are in the frontier, so the far end counts as visited.

**Observed (E17 graph-qa, g116).** \"For which projects is Yara Nakamura the account manager at the supplier?\" Gold: Helix Core, Juniper Core.
- Plan: hop 1 `CONTACT_FOR | WORKS_WITH` (out), hop 2 `SUPPLIED_BY | DEPENDS_ON` (in), answer_type Project.
- Hop 1's frontier holds Sable Supply and Tern Supply (CONTACT_FOR), and also Juniper Core over an unrelated WORKS_WITH edge.
- Hop 2 needs `Juniper Core -SUPPLIED_BY-> Sable Supply`. That edge is excluded because Juniper Core is visited, so the walk returns only Helix Core and route scores 0.67.
- With hop 1 limited to `CONTACT_FOR`, Juniper Core isn't visited and hop 2 returns both projects.

The planner can't reliably avoid this. Listing 2 or 3 relation types per hop, as the planner prompt asks, makes it more likely that the answer is reached early by a sibling edge.

**A second, latent bug in the same place.** When both ends of an edge are in the frontier, the code always treats the source as the current entity. For a hop with direction `in`, the edge is then scored as the wrong orientation.

## Change

1. **Exclusion set.** On hop 2 and later, exclude far ends that are the start entities or entities from earlier hops. Don't exclude entities in the current frontier. Hop 1 is unchanged: the start entities are excluded. Edges back to the start, and to any earlier hop, stay excluded, so walks still don't cycle back.
2. **Both orientations.** An edge between two frontier entities becomes a candidate in both orientations, each with its own `direction_matched`.
3. **One slot per edge.** `rank()` keeps only the better-ranked orientation of each edge before the beam cut, so it takes one beam slot and its fact is reported once.
4. **Docs.** Update the module doc in `walk.rs`, the `knowledge_walk` candidate rule in `docs/ipc-mcp-reference.md` (\"cycles never revisit\"), and the `candidate_count` description there.

The completion hop is unaffected. It runs only when no frontier entity has `answer_type`, so an edge between two frontier entities can't complete it.

## Measured

The maintainer applied this change locally, as above, and ran graph-qa on the E17 graph with E17's cached v5 plans:
- Of 144 questions, only g116's walk output changed (two new reader calls), going from 0 to 1.0 for both walk and route. No other question's score changed.
- Route on the E13 set: 0.964 → 0.972. Join: 0.833 → 0.905.

A one-question gain is too small to separate from noise. This is filed as a correctness bug, not as a measured improvement.

## Acceptance

- A unit test on the in-memory graph: start `a`; `a -WORKS_WITH-> p` (Project); `a -CONTACT_FOR-> s`; `p -SUPPLIED_BY-> s`.
  - Hops: `[WORKS_WITH, CONTACT_FOR]` out, then `[SUPPLIED_BY]` in. List mode, answer_type Project.
  - Hop 2 returns `p -SUPPLIED_BY-> s` once, with `direction_matched: true`, and no edge back to `a`.
  - On current `main`, hop 2 dead-ends (`dead_end_at: 2`), so the test fails there.
- A test that an edge between two frontier entities appears once in the hop's output and takes one beam slot.
- `visited_nodes_are_not_revisited` still passes. Edges back to the start, and to entities of earlier hops, stay excluded on a 3-hop walk (add a test).
- Existing walk, `knowledge_walk` handler and graph-qa tests pass.
- CHANGELOG `Unreleased` entry. Regenerate `docs/llms-full.txt`.

## Out of scope

- Re-running graph-qa. The maintainer does that after this lands.
- Planner prompt changes. A v6 rule was tried separately and not adopted.
- g110, the other remaining join miss. It's an extraction typing error, not a walk bug: one supplier was typed Project instead of Organization, so the answer-type filter accepted it and the completion hop never ran."

## Background

`knowledge_walk` follows a plan of hops (each with relation types and a direction) outward from start entities, keeping a beam of candidate edges per hop. Its candidate generation excludes any edge whose far end is already "visited" so a walk never cycles back. The visited set currently includes the *current frontier* (the entities reached on the previous hop), not only the start entities and entities from strictly earlier hops.

This is wrong when two entities reached on the same hop are connected to each other, and the next hop's correct answer is an edge between them. In E17 graph-qa question g116 ("For which projects is Yara Nakamura the account manager at the supplier?"; gold: Helix Core, Juniper Core), hop 1 (`CONTACT_FOR | WORKS_WITH`, out) reaches suppliers Sable Supply and Tern Supply, and also Juniper Core via an unrelated `WORKS_WITH` edge. Hop 2 (`SUPPLIED_BY | DEPENDS_ON`, in) needs `Juniper Core -SUPPLIED_BY-> Sable Supply`, but that edge is dropped because Juniper Core is "visited". The walk returns only Helix Core (route score 0.67). Restricting hop 1 to `CONTACT_FOR` alone would avoid it, but the planner cannot reliably do that: listing two or three relation types per hop, as its prompt asks, makes early sibling reach more likely.

A second, latent defect lives in the same code: when both ends of an edge are in the frontier, the source is always treated as the current entity. For an `in`-direction hop, that edge is scored with the wrong orientation (wrong `direction_matched`).

The maintainer prototyped the change locally and measured it on graph-qa with E17's cached v5 plans: of 144 questions only g116's walk output changed (0 → 1.0 for both walk and route); route on the E13 set moved 0.964 → 0.972 and join 0.833 → 0.905. A one-question gain is too small to separate from noise, so this is filed as a correctness bug, not a measured improvement.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A frontier entity can be the next hop's answer (Priority: P1)

A caller (or the graph-qa planner) issues a multi-hop walk where hop 1's relation types reach both an intermediate entity and, via a sibling edge, an entity that is itself linked to that intermediate entity. Hop 2 must be able to follow the edge between those two frontier entities and return it.

**Why this priority**: This is the reported correctness bug; a correct answer is silently dropped and the walk dead-ends.

**Independent Test**: In-memory graph: start `a`; `a -WORKS_WITH-> p` (Project); `a -CONTACT_FOR-> s`; `p -SUPPLIED_BY-> s`. Walk with hops `[WORKS_WITH, CONTACT_FOR]` (out) then `[SUPPLIED_BY]` (in), list mode, answer_type Project. Hop 2 returns `p -SUPPLIED_BY-> s`.

**Acceptance Scenarios**:

1. **Given** the graph above, **When** the walk runs, **Then** hop 2 returns `p -SUPPLIED_BY-> s` exactly once with `direction_matched: true`, the walk does not dead-end, and no edge back to `a` appears.
2. **Given** the same graph on current `main`, **When** the walk runs, **Then** it dead-ends at hop 2 (`dead_end_at: 2`) — the regression test must fail before the change and pass after.

---

### User Story 2 - An edge between two frontier entities is scored correctly and counted once (Priority: P1)

When both ends of an edge are in the current frontier, either end could be the "current" entity. The edge is a candidate in both orientations, each scored against the hop's direction, and only the better-ranked orientation survives into the beam so the edge occupies one slot and its fact is reported once.

**Why this priority**: Without it, fixing the exclusion would either mis-score `in`-direction hops (latent bug) or double-report the edge and consume two beam slots, distorting beam cuts.

**Independent Test**: A hop where an edge connects two frontier entities, with a beam width that would otherwise be exhausted by a duplicate; assert the edge appears once in the hop's output and consumes one beam slot, and that `direction_matched` reflects the hop's direction for the surviving orientation.

**Acceptance Scenarios**:

1. **Given** an edge between two frontier entities and an `in`-direction hop, **When** candidates are generated, **Then** both orientations are candidates, each with its own `direction_matched`.
2. **Given** both orientations are candidates, **When** the beam is ranked and cut, **Then** only the better-ranked orientation remains, the edge takes one beam slot, and its fact is reported once.

---

### User Story 3 - Walks still never cycle back to the start or earlier hops (Priority: P1)

Relaxing the exclusion for the current frontier must not reintroduce revisiting. Edges whose far end is a start entity or an entity reached on an earlier hop remain excluded; hop 1 continues to exclude start entities.

**Why this priority**: Guards against the regression the visited set exists to prevent.

**Independent Test**: A 3-hop walk where hop 3 has candidate edges back to the start entity and to a hop-1 entity; assert they are excluded. Existing `visited_nodes_are_not_revisited` continues to pass.

**Acceptance Scenarios**:

1. **Given** a 3-hop walk with edges leading back to the start and to an entity of an earlier hop, **When** hop 3 runs, **Then** those edges are not candidates.
2. **Given** hop 1, **When** candidates are generated, **Then** edges whose far end is a start entity are excluded as before.

---

### Edge Cases

- Both orientations of a frontier-to-frontier edge tie in rank: exactly one survives, deterministically.
- A frontier-to-frontier edge matches the hop's direction in only one orientation: the matched orientation is preferred by ranking and the reported `direction_matched` is that orientation's.
- A frontier entity linked to itself (self-loop): no new behavior is required beyond not crashing or double-counting.
- The completion hop runs only when no frontier entity has `answer_type`, so a frontier-to-frontier edge can never complete it; its behavior is unchanged.
- `candidate_count` (the unfiltered candidates after exclusion) now reflects the relaxed exclusion and may be higher than before for the same walk.
- History mode and last-hop live-edge filtering behave as before.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: On hop 1, candidate generation MUST continue to exclude edges whose far end is a start entity.
- **FR-002**: On hop 2 and later, candidate generation MUST exclude edges whose far end is a start entity or an entity reached on an earlier hop, and MUST NOT exclude edges whose far end is in the current frontier.
- **FR-003**: An edge whose both ends are in the current frontier MUST be generated as a candidate in both orientations, each with its own `direction_matched` computed against the hop's direction.
- **FR-004**: Ranking MUST keep only the better-ranked orientation of each such edge before the beam cut, so the edge consumes one beam slot and its fact is reported once in the hop's output.
- **FR-005**: For the scenario in User Story 1, hop 2 MUST return `p -SUPPLIED_BY-> s` once with `direction_matched: true`, MUST NOT return an edge back to the start entity, and MUST NOT report a dead end.
- **FR-006**: Walks MUST still never revisit start entities or entities from earlier hops (verified on a 3-hop walk), and the existing `visited_nodes_are_not_revisited` test MUST still pass.
- **FR-007**: Completion-hop behavior MUST be unchanged.
- **FR-008**: Existing walk, `knowledge_walk` handler and graph-qa tests MUST continue to pass.
- **FR-009**: The module documentation for the walk, the `knowledge_walk` candidate rule in `docs/ipc-mcp-reference.md` (currently "cycles never revisit"), and that document's `candidate_count` description MUST be updated to describe the new exclusion rule (start entities and earlier hops excluded; current frontier not excluded; frontier-to-frontier edges deduplicated to one orientation).
- **FR-010**: A changelog fragment MUST be added under `changelog.d/` per repository convention (not an edit to `CHANGELOG.md`); `docs/llms-full.txt` is generated at publish time and MUST NOT be committed.
- **FR-011**: The change MUST NOT alter the `knowledge_walk` request/response schema or the MCP tool registry.

### Key Entities

- **Frontier**: The set of entities reached by the immediately preceding hop; the "current entities" for the next hop.
- **Visited / excluded set**: Start entities plus entities reached on hops before the current frontier; far ends in this set are not candidates.
- **Candidate edge / orientation**: An edge considered for a hop, with the entity treated as "current" (near end) and a `direction_matched` flag relative to the hop's direction.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The User Story 1 unit test fails on current `main` (`dead_end_at: 2`) and passes after the change.
- **SC-002**: A test shows a frontier-to-frontier edge appears exactly once in a hop's output and takes exactly one beam slot.
- **SC-003**: A 3-hop test shows edges back to the start and to an earlier-hop entity remain excluded, and `visited_nodes_are_not_revisited` still passes.
- **SC-004**: All existing walk, `knowledge_walk` handler and graph-qa tests pass unchanged.
- **SC-005**: The documented candidate rule and `candidate_count` description match the implemented behavior.
- **SC-006**: (Informational, run by the maintainer after landing) re-running graph-qa on the E17 graph with cached v5 plans changes only g116's walk output, matching the prior local measurement.

## Assumptions

- "Better-ranked orientation" means the orientation that would sort first under the walk's existing ranking order (which already considers `direction_matched`); no new ranking criteria are introduced, and ties resolve deterministically.
- The exclusion applies uniformly regardless of walk mode (list or history); only live-edge filtering differs by mode as today.
- The relaxed exclusion may change `candidate_count` values in responses; this is a documented, intended consequence and not a schema change.
- Spec-Kit-formatted spec lives at `specs/912-*/spec.md`, produced by the engine; the changelog entry follows the `changelog.d/` fragment convention.

## Out of Scope *(optional)*

- Re-running graph-qa (the maintainer does this after merge).
- Planner prompt changes (a v6 rule was tried separately and not adopted).
- g110, the other remaining join miss: an extraction typing error (a supplier typed Project instead of Organization, so the answer-type filter accepted it and the completion hop never ran), not a walk bug.

## Source References *(optional)*

- `crates/core/src/walk.rs` — `candidates()`, `rank()`
- `docs/ipc-mcp-reference.md` — `knowledge_walk` candidate rule and `candidate_count` description
- `changelog.d/README.md` — changelog fragment format
- E17 graph-qa question g116
