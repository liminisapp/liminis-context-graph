# Feature Specification: Temporal (2/3): show what a correction changed, and what was written while the old belief held

**Feature Branch**: `fabrik/issue-706`
**Created**: 2026-10-26
**Status**: Specified
**Input**: User description: "Temporal (2/3): show what a correction changed, and what was written while the old belief held"

## Background

#705 (merged) makes corrections end belief without deleting evidence. Episodes are superseded rather than deleted, each extracted edge records the episode that produced it, and whether an edge is believed at a given time is derived from that. Ending belief is half the value. A user who believed something false for a day also needs to see:

1. **What the correction changed**: the facts the old text supported and the facts the new text supports.
2. **What was written into the graph while the old belief held**, so that work built on the false belief can be revised.

Neither question can be answered today with the existing reads. A believed-only read shows the world after the correction. An as-of read shows the world before it. Neither shows the difference, and nothing connects a dropped fact to the work done during the window it was believed.

What #705 provides for this issue:

- An episode records when it was superseded, by what (the replacing episode, if any), and why (`revision` or `chunk_removed`; #707 adds `document_superseded`).
- An extracted edge records its producing episode(s). An edge is believed from its ingest time until its last producing episode is superseded.
- Episodes record `ingested_at` (ADR-0673) and are linked to the entities they mention.

The belief interval of a fact is therefore already derivable. This issue adds two reads over it. Both are derived from existing records and write nothing.

Part 2 of 3. Builds on #705 (done); #707 (explicit document supersession) follows.

Related: ADR-0705 (belief time) · ADR-0673 (`ingested_at`) · #707.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - See what a correction changed (Priority: P1)

When a chunk is corrected, the user wants to see the facts its old text supported and the facts its new text supports, so they can tell what the truth became.

**Why this priority**: "what did the truth become" is the first question after a correction, and the second read (User Story 2) is only useful once the user knows which facts were dropped.

**Independent Test**: ingest a chunk asserting fact F; resubmit the same `chunk_id` so F is replaced by F′; ask what changed for the superseded episode.

**Acceptance Scenarios**:

1. **Given** episode E1 was superseded by E2 (reason `revision`), **When** the user asks what changed, **Then** they get the facts E1 supported that E2 does not, and the facts E2 supports that E1 did not.
2. **Given** an episode superseded with reason `chunk_removed`, **When** the user asks what changed, **Then** they get the facts it supported, with no replacement side.
3. **Given** E1 and E2 both support the same fact, **When** the user asks what changed, **Then** that fact appears on neither the removed nor the added side.
4. **Given** an episode that has not been superseded, **When** the user asks what changed, **Then** the read says there is nothing to compare and returns no facts.

---

### User Story 2 - Find what was written while a dropped belief held (Priority: P2)

Work gets done on a belief before it is corrected. For a fact no longer believed, the user needs to know what was written into the graph during the window in which it was believed, so they can review work that may have leaned on it.

**Why this priority**: it is the reason a short-lived false belief matters at all, but it depends on the user having identified the dropped fact (User Story 1).

**Independent Test**: after a correction, ensure one episode mentioning the corrected fact's entities was ingested inside the belief window and one outside it; ask for affected work on the dropped fact.

**Acceptance Scenarios**:

1. **Given** F was believed during [T1, T2), **When** the user asks for affected work, **Then** they get the episodes ingested during [T1, T2) that mention either of F's endpoint entities, and none ingested outside it.
2. **Given** the list is built from entity overlap rather than recorded dependence, **When** it is returned, **Then** it is presented as candidates to review, not as proof the work used F.
3. **Given** a fact that is still believed, **When** the user asks for affected work, **Then** the read says the belief interval is still open and affected work is not defined, and returns no episodes.

---

### Edge Cases

- **A fact still believed** has no closed belief interval; affected work is not defined for it and the read says so rather than returning an empty list that looks like "nothing affected".
- **A heading rename** supersedes one chunk and creates another with a new `chunk_id`, so a per-episode diff shows only removals for the old chunk. A whole-document diff needs document identity (#707).
- **A fact supported by several episodes** loses belief only when the last is superseded; its interval ends then. Affected work uses that end, not the end of any single supporting episode.
- **A fact whose support was hard-deleted** (not superseded) or a legacy edge with no producing episode has no recorded belief interval; the read says so.
- **A fact with a warrant marker** (direct assertion) is believed until a future retraction primitive exists, so its interval never closes; treated as still believed.
- **A chunk ingested as several split units**: only the final unit records the superseding relationship, and earlier units stay live as part of the same revision. The replacement side of the diff is the revision's facts, not only the final unit's.
- **Chains**: when E2 is itself later superseded by E3, asking about E1 compares E1 with E2 (its direct superseder), not with E3.
- **Window boundaries are half-open**: an episode ingested at exactly the superseding instant T2 is not inside [T1, T2).
- **The fact's own producing episodes** are not reported as affected work; they are the belief, not work done under it.
- **Groups**: both reads stay within the group of the episode or fact asked about (ADR-0368 / ADR-0371).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: A read MUST return, for a superseded episode, the edges it supported and the edges its superseding episode supports, using the episode-to-edge links #705 records, with facts present on both sides excluded from both.
- **FR-002**: A read MUST return, for an edge no longer believed, the episodes ingested within its belief interval that mention either endpoint entity.
- **FR-003**: The affected-work result MUST identify itself as candidates by entity overlap, not dependence.
- **FR-004**: Both reads are new, additive methods with matching MCP `ToolSpec`s in the `read` scope.
- **FR-005**: For an episode superseded without a replacement (`chunk_removed`), the changed-facts read MUST return the removed side and an empty added side, and MUST state that no replacement exists.
- **FR-006**: When asked about an edge whose belief interval is open, has no recorded interval (legacy or support deleted), or is not defined (warranted), the affected-work read MUST say which case applies and MUST NOT return an empty candidate list in its place.
- **FR-007**: Both reads MUST be read-only: they write nothing, including no `valid_at` / `invalid_at`, and require no schema change.
- **FR-008**: Each reported fact and episode MUST carry its UUID so a client can follow up with existing reads.

### Key Entities *(if applicable)*

- **Superseded episode**: an episode with a recorded supersession time, superseding episode (if any) and reason (#705).
- **Belief interval**: [ingest time of the edge, instant its last producing episode was superseded). Derived, never stored.
- **Changed-facts result**: the removed facts (supported by the old episode, not by its replacement) and the added facts (supported by the replacement, not by the old episode).
- **Affected-work candidate**: an episode ingested inside a fact's belief interval that mentions one of the fact's endpoint entities. A candidate, not a dependent.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For a corrected chunk, the changed-facts read returns exactly the facts removed and the facts added.
- **SC-002**: The affected-work read returns an episode ingested inside the belief window that mentions the fact's entities, and excludes one ingested outside it.
- **SC-003**: Every affected-work result is labelled as entity-overlap candidates.
- **SC-004**: Neither read changes any stored state (`valid_at`, `invalid_at`, supersession fields, WAL contents).
- **SC-005**: Both reads give the same answer after `knowledge_rebuild_from_wal`, since they derive only from state #705 already makes rebuild-stable.

## Assumptions

- **Affected work is approximated by entity overlap within the belief window.** Recording which facts an agent actually read is out of scope; an agent audit trail citing edge UUIDs would replace the approximation with exact answers.
- **Two edges are the same fact when they share source entity, target entity, relation name and fact text.** Edges are not deduplicated, so a re-extracted fact is a different row from the original. Matching on all four errs toward over-reporting a reworded fact as one removal plus one addition, which is the safe direction for a review tool. Loosening the match is a later, reversible refinement.
- **The reads take an episode UUID (changed facts) and an edge UUID (affected work) as input.** Looking up the right one from a `chunk_id` or a search is done with existing reads.
- **"Mentions" means the existing episode-to-entity link**, as already used elsewhere in the graph.
- **Candidates are not ranked or truncated by relevance**; they are ordered by ingest time.
- **Reads are opt-in and additive**: no existing read changes behaviour.

## Out of Scope *(optional)*

- Recording which facts an agent read, or an audit trail of edge citations
- Proving dependence of later work on a dropped fact
- Whole-document diffs and rename tracking (#707, which adds document identity)
- Explicit document supersession and in-force-until reporting (#707)
- Notifying users or agents of affected work automatically
- Revising or retracting the affected work itself
- Changing the default behaviour of existing relationship or episode reads

## Source References *(optional)*

- #705 (merged as #708), #707
- ADR-0705 (belief time separate from validity), ADR-0673 (ingest-time transaction start), ADR-0368 / ADR-0371 (group isolation)
