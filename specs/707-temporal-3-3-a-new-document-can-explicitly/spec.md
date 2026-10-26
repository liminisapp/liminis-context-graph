# Feature Specification: Temporal (3/3): a new document can explicitly supersede an old one

**Feature Branch**: `fabrik/issue-707`
**Created**: 2026-10-26
**Status**: Specified
**Input**: User description: "Temporal (3/3): a new document can explicitly supersede an old one"

## Background

A document is sometimes replaced rather than edited — a decision record from last year superseded by a new one. That is a relation between **two documents**, not a revision of one, and it differs from a correction (#705) in one essential way: the old decision **was true** until it was replaced.

| | Correction (#705) | Supersession (this issue) |
|---|---|---|
| Was the fact ever true? | No | Yes, until the effective date |
| Belief ends | when the correction is ingested | when the supersession is **recorded** |
| Validity ends | never touched | at the **effective date** — derived, not stamped |
| Work done on it meanwhile | suspect | correct at the time |

The two time axes stay separate here too. If a decision took effect on 1 January but the superseding document was only recorded on 1 March, the graph *did* believe the old decision during January and February — it had not learned otherwise — even though the old decision was no longer *in force*.

Today, old and new decisions are both believed indefinitely. #705 (merged, ADR-0705) supplied the substrate this builds on: episodes can be superseded (not deleted), carrying when/by-what/why, with `document` and `document_superseded` reserved for this issue; edges record their producing episode; belief at time *t* is derived, and reads accept believed-only and as-of filters. What it lacks is any way to talk about a **document**: documents exist in the graph only as free-text `source_description` strings, so there is nothing for a supersession to point at. This issue makes documents addressable and adds the explicit supersession relation.

Supersession is never inferred. Two documents on the same topic are not related unless a caller says so.

Part 3 of 3. Blocked by #705 (now merged). Related: #706 · ADR-0673 · ADR-0705.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A new decision document supersedes an old one (Priority: P1)

A user records that document A supersedes document B, effective on a given date. B's facts stop being believed from then on, unless A asserts them too, and reads can tell "in force until a date" apart from "corrected".

**Why this priority**: decision records are replaced routinely, and today the old and new decisions are both believed indefinitely.

**Independent Test**: ingest documents B and A; record that A supersedes B effective T_eff, at T_rec; query belief and validity at several times.

**Acceptance Scenarios**:

1. **Given** A supersedes B, recorded at T_rec, **When** querying believed-now, **Then** B's facts are not believed, unless A also asserts them.
2. **Given** a supersession recorded at T_rec with effective date T_eff earlier than T_rec, **When** querying as of some t in [T_eff, T_rec), **Then** B's facts *are* believed as of t (the graph had not yet learned), and are reported as no longer in force as of t.
3. **Given** a superseded fact and a corrected fact, **When** both are read, **Then** they are distinguishable: "in force until T_eff" versus "corrected in a revision".
4. **Given** two documents on the same topic, **When** no supersession has been recorded, **Then** nothing is superseded. Supersession is never inferred.

---

### User Story 2 - Supersession survives restart and rebuild (Priority: P2)

An operator rebuilds the graph from the WAL (or restores from a checkpoint, or recovers a WAL tail). The supersession relations and the belief state they produced come back identical.

**Why this priority**: belief state that only holds until the next rebuild is not trustworthy; but the capability is useful before this is proven, so it ranks below the core behaviour.

**Independent Test**: record a supersession, snapshot belief/in-force answers at several times, run `knowledge_rebuild_from_wal`, compare.

**Acceptance Scenarios**:

1. **Given** a recorded supersession, **When** the graph is rebuilt from the WAL, **Then** every episode's supersession state and every as-of belief and in-force answer is identical.
2. **Given** a supersession recorded in a WAL range replayed twice (overlapping tail recovery), **When** replay completes, **Then** the result is the same as a single replay and the first supersession timestamps are not overwritten.

---

### Edge Cases

- **Chains**: A supersedes B, which superseded C. Each relation keeps its own effective date; C's facts are reported in force until the date B took effect, B's until the date A took effect.
- **The superseding document is later itself corrected or superseded.** B stays superseded: a recorded supersession is not undone by later changes to A. Further changes to A follow the #705 rules for A's own episodes (and a new supersession of A by A2 is simply the next link in a chain).
- **T_eff in the future**: the old decision is believed and in force until then; it ceases to be believed only once T_eff is reached (see FR-003, Assumptions).
- **T_eff earlier than T_rec**: the retroactive case in scenario 2 — B was believed by the graph from its ingest until T_rec, but reported not in force from T_eff.
- **A fact both documents assert** stays believed through A's own episode; only B's copy loses belief.
- **Groups**: supersession never crosses a group (ADR-0368 / ADR-0371). A and B are named within one group; documents with the same `doc_id` in another group are untouched.
- **Episodes ingested before `doc_id` existed** have none and cannot take part; legacy graphs are re-ingested.
- **Episodes of B ingested after the supersession is recorded**: the supersession covers the episodes live when it is recorded; it does not silently mark later ingests.
- **Unknown, identical-document, or cyclic requests**: A and B the same document, A or B unknown in the group, or a relation that would make B supersede something that (transitively) supersedes B are rejected, not recorded.
- **Re-recording**: recording the identical supersession again is a no-op; recording a different superseding document or effective date for an already-superseded B is rejected rather than silently overwriting the first record.
- **B has no live episodes** (e.g. all already corrected away): the relation can still be recorded; there is simply nothing further to end.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Every episode ingested from a document MUST be able to carry a `doc_id`: the stable identity of the logical document it came from (path-valued today; a later, stabler identity becomes a value change, not a migration). Ingest paths accept `doc_id` as an optional input; an episode ingested without one has none and cannot take part in supersession.
- **FR-002**: A new, additive write method MUST record "document A supersedes document B, effective T_eff", within a single group. It is always explicit; the graph never infers supersession.
- **FR-003**: Recording a supersession MUST supersede B's live episodes in that group with reason `document_superseded` (#705 FR-002), by-kind `document`, referring to A, stamped with the service clock at the moment of recording. When T_eff is later than the recording instant, B MUST remain believed (and in force) until T_eff, and cease to be believed at T_eff.
- **FR-004**: Reads MUST be able to report a superseded document's facts as in force until T_eff, derived from the supersession record. `invalid_at` is never written.
- **FR-005**: The supersession record MUST be durable in the WAL and stable across rebuild, tail recovery and checkpoint restore — the ADR-0673 / ADR-0705 pattern.
- **FR-006**: The relation MUST be rejected, without side effects, when A and B are the same document, when either is unknown in the group, or when it would create a cycle. Recording an identical relation again MUST be a no-op; a conflicting relation for an already-superseded B MUST be rejected.
- **FR-007**: The belief annotation on relationship reads MUST distinguish a fact whose episode was superseded by revision or removal ("corrected") from one superseded by a document ("in force until T_eff"), and MUST report, for an as-of time *t*, whether the fact is in force at *t* independently of whether it is believed at *t*. Believed-only filtering (#705 FR-009) is unchanged: it filters on belief, not on being in force.
- **FR-008**: No supersession path may write `valid_at` or `invalid_at`.
- **FR-009**: Supersession MUST NOT cross groups: it reads and changes only episodes of the call's group.
- **FR-010**: The new method MUST be reachable on every transport that exposes write methods, including MCP-over-stdio, and MUST NOT change the behaviour of existing methods when `doc_id` is omitted.

### Key Entities *(if applicable)*

- **Document**: the logical source of one or more episodes, identified by `doc_id` within a group. Previously only a free-text `source_description`.
- **Document supersession record**: a durable statement that document A supersedes document B within a group, effective T_eff, recorded at T_rec. The source of the "in force until T_eff" answer.
- **Episode (`Episodic`)**: gains `doc_id`; its #705 supersession fields are populated with kind `document` and reason `document_superseded`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: After a supersession is recorded, B's facts are absent from believed-only reads unless A also asserts them.
- **SC-002**: As-of reads between T_eff and T_rec show B's facts believed and not in force.
- **SC-003**: No supersession path writes `invalid_at`.
- **SC-004**: `knowledge_rebuild_from_wal` reproduces identical supersession and belief state.
- **SC-005**: Across a corpus with both corrected and superseded facts, every returned fact can be classified as "corrected" or "in force until T_eff" from the read alone.
- **SC-006**: Rejected requests (FR-006) leave belief state and the WAL unchanged.

## Assumptions

- **#705 is the substrate.** Its supersession fields, producing-episode links, warrant markers and belief reads (ADR-0705) are in place; this issue adds the document layer and does not alter their meaning.
- **Belief in the future-effective case ends at T_eff.** The issue's edge case (old decision believed and in force until a future T_eff) is read as overriding a literal stamp of the recording instant: the recording instant is still kept as the time the graph learned of it, but belief does not end before the decision takes effect.
- **`doc_id` is supplied by the caller**, not derived from `source_description`. It is optional on ingest so existing clients keep working; protocol compatibility with the Electron app is preserved.
- **"In force" is reported, not filtered.** No new filter is added; callers who want only in-force facts can use the reported state. A later change can add one.
- **The effective date is supplied with the supersession** as an absolute timestamp with timezone; extracting it from document text is a possible later step. #705 FR-011 records whether an edge's own `valid_at` was extracted or defaulted, which matters most for decision records.
- **Both documents must already be known in the group** (have at least one episode with that `doc_id`), so a mistyped identity is caught rather than silently recorded.
- **A document has at most one superseding document** per group (a linear history; chains are the way to express successive replacement).
- **A recorded supersession is not undone** by later corrections to A. Withdrawing one is a separate, later primitive.

## Out of Scope *(optional)*

- Inferring supersession from topic similarity, titles, or text
- Extracting the effective date from document text
- Undoing or withdrawing a recorded supersession
- Backfilling `doc_id` onto legacy episodes (legacy graphs are re-ingested)
- A new in-force-only read filter, and making belief filtering the default
- Fact-level (rather than document-level) supersession
- What changed and affected work (#706)

## Source References *(optional)*

- #705 (PR #708) · #706 · #404 · #405
- ADR-0673 (ingest-time transaction start), ADR-0705 (belief time separate from validity), ADR-0368 / ADR-0371 (group isolation)
- Prior art: the bitemporal pattern (SQL:2011 system-time vs. application-time) — the two axes here are the same split
