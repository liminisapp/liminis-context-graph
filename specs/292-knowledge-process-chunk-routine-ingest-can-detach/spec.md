# Feature Specification: `knowledge_process_chunk` must never end belief in (or delete) an episode it cannot attribute to its own prior writes

**Feature Branch**: `fabrik/issue-292`
**Created**: 2026-11-15
**Status**: Draft
**Input**: User description: "

## Background

#284 (PR #286) makes `knowledge_process_chunk` idempotent per `chunk_id`: an unchanged resubmission no-ops, a changed one replaces the prior episodes. The replace path ends in a `DETACH DELETE` of the prior lineage's UUIDs.

To decide what belongs to a `chunk_id`'s lineage, `reconstruct_prior_chunk_text` matches rows whose `Episodic.name` equals the `chunk_id` **and** whose `source_description` ends with `\":{chunk_id}\"` (exactly, or plus a parseable `#i/N` split suffix).

That anchor was added during #284's review, and it fixed a real bug: before it, a `knowledge_add_episode` row whose caller-chosen `name` happened to equal a `chunk_id` would be picked up as prior state, found not to match the incoming `chunk_text`, and silently deleted. ADR-0052 describes that as *\"data loss with no explicit delete action from the caller\"* — the right characterisation.

## Why this issue exists

The anchor narrows that hazard but does not close it, and ADR-0052 accepts the remainder:

> The `\":{chunk_id}\"`-suffix anchor narrows, but does not eliminate, the foreign-row risk: a `knowledge_add_episode` caller whose `name` equals a `chunk_id` *and* whose own `source_description` happens to end in `\":{chunk_id}\"` too (e.g. a caller mirroring this handler's own `\"{source_file}:{chunk_id}\"` convention, such as `\"notes:chunk-42\"`) still gets pulled into that chunk_id's reconstructed lineage and is a candidate for deletion on a subsequent replace.

The reason this is worth its own issue rather than an accepted tradeoff: **the surviving case is not adversarial.** `source_description = \"notes:chunk-42\"` with `name = \"chunk-42\"` is a caller following the same convention this handler writes. `Episodic.name` has no namespace separating `knowledge_process_chunk` lineages from `knowledge_add_episode` callers, so a well-behaved integrator — exactly the kind of third-party client that surfaced #202 — can construct this collision by imitation.

The consequence is unchanged from the bug the anchor was introduced to fix: a routine ingest call destroys an episode the caller never asked to delete.

Worth distinguishing from a case that is *not* a problem: `handle_delete_chunk_episode` still matches and deletes by `name` alone, and that is fine — it is an explicit, caller-invoked deletion whose contract has always been \"delete everything with this name.\" The issue here is specifically **routine ingestion** deleting data as a side effect.

## Proposed fix — make the ambiguous case fail safe, not fail destructive

Closing the hole completely needs a provenance marker distinguishing this handler's own writes from any other writer's, which is a schema change and out of scope per #284's Research Constraint 4. This issue does not ask for that.

Instead: when the reconstructed lineage contains **any** row that this handler cannot positively attribute to its own prior writes, do not delete. Either

- proceed as a fresh ingest (accepting a duplicate rather than destroying data), or
- return an actionable error naming the colliding UUID and `source_description`, so the caller can rename.

Losing idempotency on a rare collision is a cheap trade. Deleting a caller's episode is not. The asymmetry should be resolved in favour of never deleting something we are not sure we wrote.

## Scope note

This does not revisit #284's idempotency semantics, the split mechanism, or the delete-after-success ordering — all of which are correct as implemented. It changes only what happens when lineage attribution is *ambiguous*: today the replace proceeds, and it should not.

Related follow-ups already filed off #284: #288 (per-`(group_id, chunk_id)` locking for the concurrent-resubmission race) and #291 (unindexed `Episodic.name` scan on the lookup). This is the third and last of the review findings on PR #286.

## Acceptance

- A test in which a `knowledge_add_episode` row with `name = \"<chunk_id>\"` and `source_description = \"notes:<chunk_id>\"` exists, then `knowledge_process_chunk` is called for that `chunk_id` with different `chunk_text`: the foreign row **survives**, and the outcome is either a fresh ingest or a named error — never a silent delete.
- ADR-0052 updated: the residual-collision paragraph is replaced by the fail-safe behaviour, or, if the behaviour is deliberately kept, the risk is stated as a top-level Consequence rather than a qualification inside the `source_file`-independence discussion.

## References

- #284 / PR #286 — the idempotency + splitting implementation
- ADR-0052 `docs/adr/0052-chunk-splitting-and-chunk-id-idempotency.md` — Consequences, the residual-collision paragraph
- `crates/core/src/handlers.rs` — `reconstruct_prior_chunk_text`, the `Replace { uuids_to_delete }` path
- Review finding 3 on PR #286: https://github.com/verveguy/liminis-context-graph/pull/286#pullrequestreview-4818317902
"

## Background

**The mechanism this issue was written against is not what `main` does today.** PR #286 (#284) was closed without merging; `reconstruct_prior_chunk_text`, the `Replace { uuids_to_delete }` path, the `":{chunk_id}"` source-description anchor and the chunk-splitting ADR cited above do not exist on `main`. (The ADR-0052 number cited in the issue is now a different, unrelated ADR; the chunk-idempotency ADR lived on the abandoned branch.) The idempotency need was instead met by belief-time supersession (#705, ADR-0705): resubmitting a `chunk_id` through `knowledge_process_chunk` now ends belief in ("supersedes") the chunk's earlier live episodes in that group once the new ingest has fully succeeded — a soft, reversible mark that retains the evidence — not a `DETACH DELETE`.

**The underlying hazard survives, and in one respect is wider.** Which earlier episodes count as "the chunk's lineage" is decided by `Episodic.name` equal to the `chunk_id`, within the group, and nothing else. There is no `source_description` anchor at all. So any `knowledge_add_episode` row whose free-form `name` happens to equal a `chunk_id` — whether or not its `source_description` mirrors this handler's `"{source_file}:{chunk_id}"` convention — has its belief ended by a routine `knowledge_process_chunk` call. That is the very bug #284's anchor was introduced to fix, now unguarded. The consequence is milder than deletion (the row persists and can be read as historical belief), but the caller's episode silently drops out of current-belief reads as a side effect of a call that never named it. ADR-0705 itself notes `knowledge_add_episode.name` is free-form and that `knowledge_process_chunk` is the only superseding entry point, but does not address a name collision.

**Why it matters (unchanged from the original report).** The collision is not adversarial. `name = "chunk-42"`, `source_description = "notes:chunk-42"` is a well-behaved integrator imitating the handler's own convention, and `Episodic.name` has no namespace separating the two kinds of writer. The goal is the same asymmetry the issue states: losing idempotency on a rare collision is cheap; ending belief in (or deleting) data the caller never asked to touch is not. **Routine ingestion must never alter an episode it is not sure it wrote.**

**Boundary the original issue draws, kept.** Explicit, caller-named operations that act "by name" (`knowledge_delete_chunk_episode`, and `knowledge_mark_chunk_removed`, which supersedes by `chunk_id` with reason `chunk_removed`) have a by-name contract and are not the problem; the problem is a *routine ingest* acting on rows as a side effect.

**A constraint the acceptance test exposes.** `knowledge_process_chunk(source_file="notes", chunk_id="chunk-42")` writes exactly `name="chunk-42"`, `source_description="notes:chunk-42"` — byte-for-byte what the mirroring `knowledge_add_episode` caller writes. With the data the episode currently carries, the two are indistinguishable by name and `source_description` alone, so "the foreign row survives" (the issue's acceptance case) cannot be met together with "this handler's own prior writes are still superseded" unless the handler can positively tell its own writes apart. The original issue ruled out a provenance marker (a schema change) on the strength of #284's Research Constraint 4; that constraint belonged to a PR that never merged, and the schema has since grown several columns (origin, doc_id, supersession columns). Whether to relax that is an open question below.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A foreign episode survives a routine chunk ingest (Priority: P1)

An integrator has written an episode with `knowledge_add_episode`, choosing `name = "<chunk_id>"` and `source_description = "notes:<chunk_id>"` (imitating this handler's convention). Later a `knowledge_process_chunk` call arrives for that same `chunk_id` in the same group with different `chunk_text`. The integrator's episode must be neither deleted nor have its belief ended.

**Why this priority**: This is the data-loss/belief-loss bug itself and the issue's primary acceptance case.

**Independent Test**: Write the foreign episode via `knowledge_add_episode`, call `knowledge_process_chunk` for the same `chunk_id` and group with different text, then read the foreign episode back: it still exists and is still live (not superseded).

**Acceptance Scenarios**:

1. **Given** a `knowledge_add_episode` row with `name = X` and `source_description = "notes:X"`, **When** `knowledge_process_chunk` is called for chunk `X` in the same group with different text, **Then** the foreign row still exists, is not superseded, and the call's outcome is a fresh ingest or a named error — never a silent delete or supersession.
2. **Given** a `knowledge_add_episode` row with `name = X` and a `source_description` that does *not* mirror the convention (e.g. `"manual note"`), **When** `knowledge_process_chunk` is called for chunk `X`, **Then** the same protection holds.
3. **Given** the guarded outcome is a fresh ingest, **When** the call returns, **Then** the response tells the caller that prior same-named episodes were left alone, naming their UUIDs, so the skipped replacement is visible rather than silent.

---

### User Story 2 - Legitimate resubmission keeps working (Priority: P1)

An ingest pipeline resubmits a changed chunk it previously ingested via `knowledge_process_chunk`, including as a multi-unit revision (`supersede: false` on non-final units, `same_revision_episode_uuids` on the final one). The earlier episodes it wrote are superseded exactly as today.

**Why this priority**: The fail-safe must not regress the revision semantics of #705, which Liminis Notes depends on for whole-notebook re-indexing.

**Independent Test**: Ingest chunk `X` via `knowledge_process_chunk`, resubmit changed text for `X`; confirm the first episode is superseded by the second, with no foreign rows present.

**Acceptance Scenarios**:

1. **Given** chunk `X` previously ingested only via `knowledge_process_chunk`, **When** it is resubmitted with changed text, **Then** the earlier episode(s) are superseded and the response lists their UUIDs, as before.
2. **Given** a multi-unit revision, **When** the final unit lists the earlier units' UUIDs as `same_revision_episode_uuids`, **Then** those stay live and only genuinely earlier-revision own episodes are superseded.

---

### User Story 3 - Mixed lineage degrades safely (Priority: P2)

A `chunk_id` has both its own earlier episodes and a foreign same-named episode in the group. A resubmission must not touch the foreign one, and must not let its presence turn the whole call destructive.

**Why this priority**: A fail-safe that handles only the pure-collision case but mishandles a mixed lineage would leave a hole.

**Independent Test**: Create own episode for `X` via `knowledge_process_chunk` and a foreign same-named episode via `knowledge_add_episode`, resubmit `X`; confirm the foreign row is untouched.

**Acceptance Scenarios**:

1. **Given** a lineage containing at least one row not attributable to this handler's own prior writes, **When** a resubmission arrives, **Then** that row is untouched and the outcome follows the single fail-safe rule chosen for the ambiguous case (see FR-002).

---

### User Story 4 - The decision is documented (Priority: P2)

A maintainer reading the ADRs learns that routine chunk ingest never alters episodes it cannot attribute to itself, and what the remaining limits are.

**Why this priority**: The issue's second acceptance criterion; it also prevents the stale, abandoned-branch ADR reference from persisting.

**Independent Test**: Read the ADR; the fail-safe behaviour (or any retained residual risk) is a top-level statement, not a qualification buried elsewhere.

**Acceptance Scenarios**:

1. **Given** the change is merged, **When** a maintainer reads the supersession ADR trail, **Then** the fail-safe behaviour, its outcome choice and any residual limit are recorded as a top-level Consequence.

---

### Edge Cases

- **Own and foreign rows share name, group and `source_description` byte-for-byte** (the `notes:chunk-42` case): must be resolved by whatever attribution rule the open questions settle; it must not default to "treat as own".
- **Rows written before this change** (legacy episodes with no way of proving who wrote them): behaviour must be stated, since Liminis Notes re-indexes existing corpora and must not be silently stuck with duplicate-forever lineages.
- **Foreign row in a different group**: never touched; supersession is already group-scoped and that must hold.
- **Foreign row already superseded**: nothing to protect; it is already excluded from "live" lineage.
- **Failed ingest**: unchanged — nothing is superseded when any step fails.
- **Concurrent resubmissions** (#288) and **unindexed `name` scan** (#291, resolved by native indexes): unrelated and not revisited.
- **Foreign row with a `doc_id` or non-primary origin**: existing origin/doc guards (#707, #752) continue to apply independently.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: A `knowledge_process_chunk` call MUST NOT delete, supersede, or otherwise alter any `Episodic` row it cannot positively attribute to its own prior writes.
- **FR-002**: When the chunk's same-named lineage contains one or more rows that cannot be positively attributed to this handler's own prior writes, the call MUST resolve the ambiguity in a single, defined, non-destructive way (a fresh ingest that leaves those rows alone, or a named error — see Open Question 1), and MUST NOT apply it destructively to the unambiguous rows without that being a deliberate, documented behaviour.
- **FR-003**: If the outcome is a fresh ingest, the response MUST report which same-named episodes were deliberately left alone (UUIDs and their `source_description`), so the skipped replacement is observable.
- **FR-004**: If the outcome is an error, it MUST name the colliding episode UUID(s) and `source_description`, MUST be returned before anything is written, and MUST tell the caller how to resolve the collision (e.g. rename).
- **FR-005**: A resubmission whose lineage consists solely of episodes this handler wrote MUST behave exactly as today (earlier live episodes superseded after a fully successful ingest; `supersede: false` and `same_revision_episode_uuids` honoured).
- **FR-006**: The protection MUST hold both when the foreign row's `source_description` mirrors the `"{source_file}:{chunk_id}"` convention and when it does not.
- **FR-007**: The protection MUST hold when the incoming `chunk_text` is identical to, or different from, the foreign row's content.
- **FR-008**: Explicit by-name operations (`knowledge_delete_chunk_episode`, `knowledge_mark_chunk_removed`) MUST keep their current contracts.
- **FR-009**: The behaviour for rows written before this change MUST be defined and documented (see Open Question 2).
- **FR-010**: The change MUST NOT alter the response shape for the unambiguous case beyond fields explicitly added by this spec, so existing clients (Liminis Notes, MCP callers) are unaffected.
- **FR-011**: An ADR named `docs/adr/0292-<slug>.md` (per the repository's issue-numbered ADR convention) MUST record the fail-safe rule, the outcome chosen, how "own writes" are recognised, and any residual limit as a top-level Consequence.
- **FR-012**: Tests MUST cover: the issue's `notes:<chunk_id>` collision; a non-mirrored `source_description` collision; a mixed own+foreign lineage; the unchanged own-only resubmission; the multi-unit revision; and a legacy-row case.

### Key Entities

- **Episodic row**: `name` (the `chunk_id` for chunk ingests, free-form for `knowledge_add_episode`), `source_description`, group, supersession state, origin, optional `doc_id`.
- **Chunk lineage**: the set of live episodes in a group sharing a `name` equal to a `chunk_id`; the set a revision supersedes today.
- **Own write**: an episode that this handler can positively show it wrote; the definition of "positively" is the subject of Open Question 2.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In the issue's collision scenario, 100% of foreign episodes remain present and live (not superseded) after any number of `knowledge_process_chunk` calls for the colliding `chunk_id`.
- **SC-002**: Zero silent outcomes: every ambiguous-lineage call either fails with a message naming the colliding episode(s) or reports the left-alone episodes in its response.
- **SC-003**: Own-only resubmission behaviour is unchanged: the existing revision and supersession test suites pass without modification of their expectations.
- **SC-004**: A re-index of an existing corpus does not leave chunks permanently un-superseded solely because their earlier episodes predate this change (subject to the answer to Open Question 2).

## Assumptions

- The repository's `main` behaviour (#705 supersession keyed on `name` within the group) is the baseline; the closed #284 mechanism is not being resurrected, and the issue's "do not revisit idempotency, splitting or delete-after-success" scope note is read as "do not revisit #705's supersession, split-unit protocol or supersede-last ordering".
- "Fail safe" is a stricter requirement than the issue's original framing in one respect: it also covers un-anchored same-name collisions, because `main` has no anchor.
- A fresh ingest accompanied by an explicit report in the response is preferred over a hard error, because Liminis Notes' bulk re-indexing would otherwise stall on a single colliding chunk; challengeable via Open Question 1.
- The ADR number is `0292` per the repository's issue-numbered-ADR rule; the issue's "update ADR-0052" refers to an ADR that no longer exists under that name, and the relevant standing ADR is ADR-0705.
- Group scoping of supersession (ADR-0368/0371) is unchanged.

## Out of Scope *(optional)*

- Per-`(group_id, chunk_id)` locking for concurrent resubmission (#288).
- Episodic `name` lookup performance (#291).
- Changing explicit by-name operations (`knowledge_delete_chunk_episode`, `knowledge_mark_chunk_removed`).
- Namespacing or renaming `knowledge_add_episode` names, or migrating existing data.
- Redesigning chunk splitting or revision semantics.

## Source References *(optional)*

- Issue #284 / PR #286 (closed, unmerged) and review finding 3 on that PR.
- #705 / ADR-0705 (belief time; revision supersedes by `name` within the group), #707 / ADR-0707 (document supersession), #752 / ADR-0752 (episode origin), #291 / ADR-0291 (native indexes on `Episodic.name`/`doc_id`), ADR-0283 (issue-numbered ADR precedent).
