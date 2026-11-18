# Feature Specification: Diagnose and fix `believed_only` having no effect after `knowledge_supersede_document` in the #754 retrieval eval

**Feature Branch**: `fabrik/issue-787`
**Created**: 2026-11-18
**Status**: Specified
**Input**: User description: "## Problem

In the first real run of the #754 retrieval eval (with PR #786 applied), the `lcg-entities-facts-belief` configuration gave **exactly the same results as `lcg-entities-facts` in every cell**, including stale>current (0.600 in both). The only difference between the two is `believed_only: true` on `knowledge_find_relationships`.

The harness ingests 31 notes with `doc_id` set to each note's id, then calls `knowledge_supersede_document` 8 times (`superseding_doc_id` / `superseded_doc_id`, with `effective_at` dates in Feb–Apr 2026; see `crates/eval/fixtures/retrieval/manifest.json` → `supersedes`). Under ADR-0707, each of those calls should end belief in the superseded note's facts at `max(recorded_at, effective_at)`, which is the time of the call. A `believed_only` fact read should then drop the facts that only the superseded notes supported. It drops none.

Testing that is what H1 (\"LCG beats qmd on temporal questions\") was for. It is also the property that the Notes app's stale-page lint (#755) and #732 depend on.

## Find out which of these it is

1. **The harness:** the supersede calls fail silently, or use the wrong parameter names or ids (the call returns `{}` and is never checked). Or the questions' stale notes don't produce facts that differ from the current notes'.
2. **Support from other notes:** each stale fact is also supported by a live episode, such as a meeting note or journal entry that restates the old decision. In that case `believed_only` correctly keeps it, and the fixture needs changing, not the engine.
3. **The engine:** `believed_only` on `knowledge_find_relationships` ignores belief that `doc_id` supersession ended (as opposed to revision or `chunk_removed`), or supersession doesn't reach edge belief.

## Desired outcome

- Name which cause it is, with a reproduction: one superseded note, one fact read with and without `believed_only`.
- If it's the engine, fix it, with a regression test at the RPC level.
- If it's the harness or the fixture, fix that, and make the harness check that each supersede call took effect. For example, count the episodes whose belief ended and fail loudly on 0.
- Re-run the eval from the recorded cassette (`fernhill-gemma4-26b-nothink.cassette.jsonl`, kept in the maintainer's scratch directory; reproduce it with `--record-cassette` if needed) and report the corrected stale>current figures.

## Related
#754 (the eval), #786 (the qmd path fix), ADR-0705 / ADR-0707."

## Background

The #754 retrieval eval compares LCG configurations against qmd on question categories, including stale>current: does retrieval rank the *current* note's answer above a *superseded* note's answer? Its hypothesis H1 is that LCG beats qmd on temporal questions because LCG knows which documents were replaced. The eval ingests 31 notes, each with a caller-supplied `doc_id`, then calls `knowledge_supersede_document` eight times (per the `supersedes` list in the retrieval fixture manifest). Under ADR-0707, each call ends belief in the superseded document's live episodes at `max(recorded_at, effective_at)`. With the fixture's effective dates in Feb–Apr 2026 and the calls made later, that is the time of the call, so belief ends immediately. ADR-0705 defines the belief model that `believed_only` reads rely on.

In the first real run (with the #786 qmd path fix), the `lcg-entities-facts-belief` configuration (same as `lcg-entities-facts` plus `believed_only: true` on `knowledge_find_relationships`) produced results identical to the non-belief configuration in every cell, including stale>current at 0.600 in both. That means either belief-aware reads aren't doing what ADR-0707 promises, or the eval isn't actually exercising it. Until this is known, H1 is untested, and the downstream work that depends on this property (the Notes app's stale-page lint, #755, and #732) rests on an unverified assumption.

Three candidate causes, from the issue:

1. **Harness** — supersede calls fail silently or use wrong parameter names or ids (a failing call may return `{}` and is never checked), or the stale and current notes don't actually produce differing facts.
2. **Fixture (support from other notes)** — each stale fact is also supported by a live episode (for example a meeting note or journal entry restating the old decision), so `believed_only` correctly retains it.
3. **Engine** — `believed_only` on `knowledge_find_relationships` ignores belief ended by `doc_id` supersession (as opposed to revision or `chunk_removed`), or document supersession does not reach edge belief.

This issue is investigative first: determine which cause (possibly more than one) applies, then fix the responsible component and prove the fix.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Name the root cause with a minimal reproduction (Priority: P1)

A maintainer needs to know why `believed_only` made no difference in the eval, with evidence precise enough to trust and re-run: a single superseded note, and one fact read made with and without `believed_only`.

**Why this priority**: Everything else (which component to fix, whether H1 holds) depends on the diagnosis. Without it, any fix is a guess.

**Independent Test**: Follow the reproduction in the findings: ingest one document with a `doc_id`, supersede it with another via `knowledge_supersede_document`, then call `knowledge_find_relationships` with and without `believed_only`. The documented outcome (facts dropped or retained, and why) is observable and matches the stated cause.

**Acceptance Scenarios**:

1. **Given** one ingested note whose facts are supported only by that note, and a recorded supersession of that note, **When** facts are read with `believed_only: true` and without it, **Then** the findings show the observed difference (or lack of one) for those facts, and attribute it to exactly one of: harness, fixture support, or engine (or state clearly if more than one applies).
2. **Given** the eval's actual run state, **When** the eight supersede calls are examined, **Then** the findings state whether each call succeeded (and how many episodes' belief each ended), so cause 1 is confirmed or ruled out with data rather than assumption.
3. **Given** each stale question in the eval, **When** its stale fact's supporting episodes are listed, **Then** the findings state whether any live (non-superseded) episode also supports it, so cause 2 is confirmed or ruled out per question.

---

### User Story 2 - Fix the responsible component and prove it (Priority: P1)

Once the cause is named, the responsible component is fixed so that a superseded document's facts are excluded from `believed_only` reads, and the fix is guarded against regression.

**Why this priority**: This restores the property that H1, #755 and #732 depend on.

**Independent Test**:
- *Engine cause:* an RPC-level regression test ingests a document with `doc_id`, supersedes it, and asserts that `knowledge_find_relationships` with `believed_only: true` omits facts supported only by that document, while the same read without `believed_only` still returns them. The test fails on the pre-fix engine and passes after.
- *Harness/fixture cause:* the eval's belief configuration yields different results from the non-belief configuration on stale questions where the fixture intends a stale fact to be dropped.

**Acceptance Scenarios**:

1. **Given** the cause is in the engine, **When** a regression test at the RPC level runs the scenario above, **Then** it fails against the pre-fix code and passes against the fix.
2. **Given** the cause is in the harness or fixture, **When** the eval is run after the fix, **Then** stale facts that should have been dropped by supersession are excluded from `believed_only` reads, and the fixture/harness change is limited to what the diagnosis requires.
3. **Given** a stale fact that is genuinely also supported by a live episode, **When** read with `believed_only: true`, **Then** it is still returned (correct behavior is preserved, not "fixed away").

---

### User Story 3 - Harness verifies that every supersede call took effect (Priority: P2)

The eval harness must never again silently proceed when a supersession did nothing. After each `knowledge_supersede_document` call, it confirms the call took effect and fails loudly otherwise.

**Why this priority**: The original failure was undetectable from the results alone. A guard makes this class of silent no-op impossible to miss, regardless of which cause is found.

**Independent Test**: Run the harness against a fixture where a supersede call references an unknown document or ends belief for zero episodes; the run aborts with an error naming the offending note ids, rather than completing with misleading numbers.

**Acceptance Scenarios**:

1. **Given** a supersede call that ends belief in zero episodes, **When** the harness checks the outcome, **Then** it fails the run with a message identifying the superseding and superseded document ids.
2. **Given** a supersede call that errors or returns an unexpected empty result, **When** the harness inspects the response, **Then** it fails the run rather than ignoring it.
3. **Given** all eight supersede calls succeed and each ends belief in at least one episode, **When** the harness runs, **Then** it proceeds normally.

---

### User Story 4 - Re-run the eval and report corrected figures (Priority: P1)

After the fix, the eval is re-run from the recorded cassette (`fernhill-gemma4-26b-nothink.cassette.jsonl`, kept in the maintainer's scratch directory; regenerated with `--record-cassette` if unavailable) and the corrected stale>current figures are reported for the belief and non-belief configurations (and qmd for comparison).

**Why this priority**: This is the point of the issue: H1 was untested. Reporting the real numbers closes the loop on #754.

**Independent Test**: Run the eval from the cassette. The report lists stale>current scores for `lcg-entities-facts`, `lcg-entities-facts-belief`, and qmd, and states whether H1 is supported.

**Acceptance Scenarios**:

1. **Given** the fix is applied and the cassette is available, **When** the eval is re-run, **Then** the corrected stale>current figures for each configuration are reported on the issue/PR.
2. **Given** the cassette no longer matches (for example because the fixture changed in a way that alters ingestion), **When** the eval is run, **Then** the cassette is re-recorded with `--record-cassette` and that fact is stated alongside the figures.
3. **Given** the corrected figures, **When** the belief configuration still equals the non-belief configuration on stale>current, **Then** the report explains why (with evidence), rather than silently presenting equal numbers.

---

### Edge Cases

- More than one cause applies at once (for example, a fixture with live-supported stale facts *and* an engine gap). Each is named and fixed separately.
- Some stale notes are legitimately restated by live notes, so a subset of stale facts should survive `believed_only`. The expected per-question outcome must be distinguishable from a bug.
- A supersede call is made with an `effective_at` in the future relative to the call: belief remains until that date (ADR-0707). The harness check must not treat "0 episodes ended yet" as a failure for a future-dated supersession, if the fixture ever contains one. The current fixture's dates are all in the past.
- A superseded document has several episodes (chunks); belief must end for all of them, and the harness count reflects that.
- Facts that reach an edge through multiple producing episodes remain believed while any producing episode is live (ADR-0707 "any live source" rule).
- A supersede call is replayed (idempotent re-record, `already_recorded`): the harness check should not misreport this as a failure.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The investigation MUST determine, with evidence, which of the three candidate causes (harness, fixture support, engine) explains `lcg-entities-facts-belief` matching `lcg-entities-facts`, and report it on the issue or PR.
- **FR-002**: The diagnosis MUST include a minimal reproduction: one superseded note and one fact read, made with and without `believed_only`, showing the observed behavior.
- **FR-003**: The diagnosis MUST establish, for the eval's actual run, whether each of the eight `knowledge_supersede_document` calls succeeded and how many episodes each one ended belief for.
- **FR-004**: The diagnosis MUST establish, per stale question, whether the stale fact is also supported by a live episode.
- **FR-005**: If the cause is in the engine, `knowledge_find_relationships` with `believed_only: true` MUST exclude facts whose only supporting episodes had belief ended by `doc_id` supersession (ADR-0707), as it does for revision- and removal-ended belief, and MUST continue to return them without `believed_only`.
- **FR-006**: If the cause is in the engine, a regression test at the RPC level (through the same method dispatch the eval uses) MUST cover document supersession → `believed_only` fact read, and MUST fail without the fix.
- **FR-007**: If the cause is in the harness or fixture, it MUST be corrected so that the eval genuinely exercises document supersession, without altering engine behavior that is already correct.
- **FR-008**: After every `knowledge_supersede_document` call, the eval harness MUST verify the call took effect (for example by counting episodes whose belief ended) and MUST fail the run loudly, naming the document ids, when a call errors, returns an unexpected empty result, or ended belief for zero episodes (excepting legitimately future-dated or already-recorded cases).
- **FR-009**: The eval MUST be re-run from the recorded cassette (re-recording with `--record-cassette` only if the cassette cannot be used) after the fix, and the corrected stale>current figures for the LCG configurations and qmd MUST be reported.
- **FR-010**: The report MUST state whether the corrected results support H1 ("LCG beats qmd on temporal questions"), including if the result is negative or the belief configuration still ties the non-belief one (with explanation).
- **FR-011**: Existing behavior of reads without `believed_only`, and ingestion/supersession semantics defined by ADR-0705 and ADR-0707, MUST remain unchanged unless the diagnosis shows they are the defect; any such change MUST be documented (ADR if a design decision changes).

### Key Entities

- **Episode**: A unit of ingested content, optionally carrying a caller-supplied `doc_id`. Belief in it can end by revision, removal, or document supersession.
- **Document supersession record**: Durable record that one document replaced another, with `effective_at` and `recorded_at`; belief in the superseded document's episodes ends at `max(recorded_at, effective_at)`.
- **Fact / relationship (edge)**: A relationship produced by one or more episodes; believed while any producing episode is believed.
- **Retrieval eval fixture**: The 31 notes, their `supersedes` list, and the questions (including stale>current) that define expected outcomes.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The root cause is named on the issue or PR, with a reproduction a reader can run, that shows a fact read with and without `believed_only` on one superseded note.
- **SC-002**: After the fix, on a scenario where a superseded document is the sole support for a fact, a `believed_only` read returns 0 such facts while the non-`believed_only` read returns them (verified by an automated test or a recorded eval check).
- **SC-003**: The harness aborts the run (non-zero outcome, with ids in the message) in 100% of cases where a supersede call ended belief for 0 episodes or errored, and does not abort when all eight calls take effect.
- **SC-004**: The eval re-run reports stale>current for each configuration, and `lcg-entities-facts-belief` differs from `lcg-entities-facts` on stale>current where the fixture has stale facts supported only by superseded notes, or an evidence-backed reason is given when it does not.
- **SC-005**: The final report states a clear verdict on H1 (supported, not supported, or inconclusive with reason).

## Assumptions

- The recorded cassette `fernhill-gemma4-26b-nothink.cassette.jsonl` exists in the maintainer's scratch directory; if not, it can be regenerated with `--record-cassette` (which requires the model endpoint the eval was originally run against).
- The eval's supersede calls happen after ingestion at a time later than the fixture's `effective_at` dates, so belief should end at the call time per ADR-0707.
- A fixture change (cause 2) is acceptable provided it keeps the question set meaningful and is documented. It must not be tuned merely to make the belief configuration win.
- Findings are expected to be recorded in the PR description and/or a short note, and need no new feature surface beyond the harness check.
- The default time for `believed_only` reads is "now" unless the read supplies an evaluation time.

## Out of Scope *(optional)*

- Redesigning the belief model, document supersession semantics, or the `believed_only` API (ADR-0705/0707) beyond fixing a demonstrated defect.
- Changes to the qmd configuration or the #786 path fix.
- Building the Notes app stale-page lint (#755) or #732.
- Improving retrieval quality in other categories, or tuning fixtures/prompts to improve scores.
- Changing the LLM or extraction setup used by the eval.

## Source References *(optional)*

- Issue #754 (the retrieval eval), PR #786 (qmd path fix), #755, #732
- `docs/adr/0705-belief-time-separate-from-validity.md`, `docs/adr/0707-document-supersession.md`
- `crates/eval/fixtures/retrieval/manifest.json` (`supersedes`), `questions.jsonl`
