# Feature Specification: Prose-introduced facts in graph-question gold answers

**Feature Branch**: `fabrik/issue-907`
**Created**: 2026-11-23
**Status**: Specified
**Input**: User description: "graph-questions: gold answers miss facts the v2 prose states (prose-facts.json + optional_answers)

## Problem

Some v2 graph questions have gold answers that leave out facts the corpus states, so a correct answer is marked wrong.

`graph-questions.jsonl` takes its gold answers from `skeleton.json` only. The LLM-rendered prose added two projects that aren't in the skeleton, both in `notes/journal-04.md`:

| fact in the prose | wording | questions affected |
|---|---|---|
| Marcus Haddad owns *Telemetry clean-up* | \"The telemetry clean-up will continue with Marcus Haddad as owner.\" | g072 (list, owned-by), g109 (join, projects-by-owner-city), i014 (inverse, projects-of-reports) |
| Priya Varga leads *Routing handover* | \"The routing handover, led by Priya Varga, …\" | i013 (inverse, projects-of-reports) |

In the E17 graph-qa run (route runner), each of these questions returned the corpus-stated project as an extra answer and lost precision for it. g109, for example, scores 0.67 for `[Orbit Core, Telemetry clean-up]` against gold `[Orbit Core]`. The extraction got these right: `OWNS Marcus Haddad → Telemetry clean-up`, and `LEADS` plus `OWNS Priya Varga → Routing handover`.

I found these by listing every non-single question where route or walk returned answers outside gold. There are no other prose-introduced answers in the E17 run. Some may remain that no runner has surfaced.

## Change

1. **A committed fixture of prose-introduced facts.** Add `crates/eval/fixtures/retrieval-v2/prose-facts.json`. Each entry has `entity`, `attribute`, `value`, `notes` (the citing notes) and `status`:
   - `stated`: the prose states the fact unambiguously. The generator treats it as a skeleton fact when deriving answers for list, join and inverse questions.
   - `ambiguous`: the wording supports the fact but doesn't settle it (for example, \"led by\" for Owner). The generator adds the value to a new per-question `optional_answers` field.
2. **Scoring:** for `set`-scored questions, an answer in `optional_answers` counts towards neither precision nor recall. Single and history scoring are unchanged.
3. **Initial entries:**
   - `Telemetry clean-up / Owner / Marcus Haddad / journal-04`, status `stated`;
   - `Routing handover / Owner / Priya Varga / journal-04`, status `ambiguous`.
4. **Regenerate** `graph-questions.jsonl` with `retrieval-gen`, and bump the graph-question set version wherever it is recorded, so old and new scores aren't compared unmarked. The question ids, text and splits stay the same. Only `answers` and `optional_answers` change, on g072, g109, i013 and i014.
5. **Docs:** the fixture README and `docs/retrieval-quality-evaluation.md` explain `prose-facts.json` and the scoring rule. CHANGELOG `Unreleased` entry. Regenerate `docs/llms-full.txt`.

## Acceptance

- Regenerating from the committed fixtures reproduces the new `graph-questions.jsonl` byte for byte. The existing reproducibility test covers this, or add one.
- g109's gold is `[Orbit Core, Telemetry clean-up]`, and g072 and i014 include Telemetry clean-up. i013 lists Routing handover under `optional_answers`, not `answers`.
- A unit test shows that an optional answer neither raises nor lowers a set score.
- A diff of the regenerated file against the current one changes only those four questions.
- `lcg-eval graph-qa` against a workspace runs unchanged apart from the scores of those four questions.

## Out of scope

- Editing the corpus prose. That would change the corpus hash and invalidate every v2 cassette.
- A systematic audit of the prose for skeleton contradictions. This issue covers only the facts runners have surfaced.
- Re-running experiments. The maintainer re-scores E17 after this lands."

## Background

The v2 retrieval corpus ("Meridian" notebook) is generated deterministically from `skeleton.json`, then rendered into prose by an LLM. The graph-question gold answers (`graph-questions.jsonl`) are derived from the skeleton alone. The rendering step introduced two project ownership facts that the skeleton does not contain, both in `notes/journal-04.md`. A system that reads the corpus correctly and extracts those facts is therefore penalised: it returns an answer the corpus supports but the gold omits, so precision drops (for example g109 scores 0.67 for `[Orbit Core, Telemetry clean-up]` against gold `[Orbit Core]`).

| Prose fact | Wording | Affected questions |
|---|---|---|
| Marcus Haddad owns *Telemetry clean-up* | "The telemetry clean-up will continue with Marcus Haddad as owner." | g072 (list, owned-by), g109 (join, projects-by-owner-city), i014 (inverse, projects-of-reports) |
| Priya Varga leads *Routing handover* | "The routing handover, led by Priya Varga, …" | i013 (inverse, projects-of-reports) |

The first fact is unambiguous. The second is plausibly Owner but "led by" does not settle it, so it should neither be required nor penalised. These were found by listing every non-single question where the route or walk runner returned answers outside gold in the E17 graph-qa run; no other prose-introduced answers appeared in that run, though some may exist that no runner has surfaced yet. Editing the prose is not an option because it would change the corpus hash and invalidate every v2 cassette.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Unambiguous prose facts count as gold (Priority: P1)

An evaluator scoring a graph-QA run wants answers the corpus states plainly to be treated as correct, so that a correct extraction is not penalised for returning them.

**Why this priority**: This fixes the observed scoring error on g072, g109 and i014 and is the core of the issue.

**Independent Test**: Regenerate the graph questions from the committed fixtures and check that Telemetry clean-up appears in `answers` for g072, g109 and i014, and that a run returning exactly the corrected gold scores full precision and recall.

**Acceptance Scenarios**:

1. **Given** a `stated` prose fact (Telemetry clean-up / Owner / Marcus Haddad), **When** graph questions are regenerated, **Then** g109's gold answers are `[Orbit Core, Telemetry clean-up]`.
2. **Given** the same fact, **When** graph questions are regenerated, **Then** g072 and i014 also include Telemetry clean-up in `answers`.
3. **Given** a run that returned `[Orbit Core, Telemetry clean-up]` for g109, **When** it is scored, **Then** precision and recall are both 1.0.

---

### User Story 2 - Ambiguous prose facts are neutral in scoring (Priority: P1)

An evaluator wants an answer the prose only loosely supports to neither reward nor penalise a system, so that scores do not depend on how a system resolves genuinely ambiguous wording.

**Why this priority**: Without it, i013 still penalises correct extractions or rewards guesswork; it is the second half of the fix and introduces the new scoring concept.

**Independent Test**: Score the same set question twice, once with and once without the optional answer present in the returned list; the precision and recall are identical.

**Acceptance Scenarios**:

1. **Given** an `ambiguous` prose fact (Routing handover / Owner / Priya Varga), **When** graph questions are regenerated, **Then** i013 lists Routing handover under `optional_answers` and not under `answers`.
2. **Given** a set-scored question with an optional answer, **When** a run returns the optional answer in addition to the gold answers, **Then** precision and recall equal those of the same run without it.
3. **Given** the same question, **When** a run omits the optional answer, **Then** recall is not reduced.

---

### User Story 3 - Prose facts are a reviewable, committed fixture (Priority: P2)

A maintainer wants prose-introduced facts recorded in a committed fixture with citations, so the exceptions to "gold = skeleton" are visible and the question set stays reproducible.

**Why this priority**: Makes the change auditable and extensible for later discoveries but does not by itself change scores.

**Independent Test**: Regenerating from the committed fixtures reproduces `graph-questions.jsonl` byte for byte, and the diff against the previous file touches only g072, g109, i013 and i014.

**Acceptance Scenarios**:

1. **Given** the committed fixtures including `prose-facts.json`, **When** `retrieval-gen` is run, **Then** the output matches the committed `graph-questions.jsonl` byte for byte.
2. **Given** the previous and regenerated files, **When** diffed, **Then** only the `answers` and `optional_answers` of g072, g109, i013 and i014 differ; ids, text and splits are unchanged.

---

### User Story 4 - Old and new scores are not silently mixed (Priority: P2)

A maintainer comparing graph-QA results across the change wants to see that the question set changed.

**Why this priority**: Prevents invalid comparisons of E17-era scores against post-change scores.

**Independent Test**: Check that the graph-question set version recorded by the generator and surfaced to consumers differs before and after the change.

**Acceptance Scenarios**:

1. **Given** the regenerated set, **When** its recorded version is read, **Then** it is newer than the pre-change version everywhere that version is recorded.

---

### Edge Cases

- A `stated` fact whose entity does not occur in a given question's derivation (for example a different attribute or relation): the question's answers are unchanged.
- A `stated` fact that duplicates a skeleton fact: no duplicate answer is added.
- An `ambiguous` value that is also already a gold answer through the skeleton: it stays in `answers` and is not duplicated into `optional_answers`.
- Questions with scoring `single` or `history`: no `optional_answers` effect; scoring is unchanged. Questions with no optional answers carry an empty or absent field consistently with how the file is serialized.
- A prose-facts entry with an unknown `status`, or one that cites a note not in the corpus: generation fails with a clear error rather than silently ignoring it.
- A runner that returns the same optional answer twice: it is ignored both times, not counted as a duplicate.
- Consumers of `graph-questions.jsonl` that predate `optional_answers` must still parse the file (the new field is additive).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: A committed fixture `prose-facts.json` under the v2 retrieval fixtures holds prose-introduced facts, each with `entity`, `attribute`, `value`, `notes` (citing notes) and `status` of `stated` or `ambiguous`.
- **FR-002**: The question generator reads `prose-facts.json`. For list, join and inverse questions it treats each `stated` fact as if it were a skeleton fact when deriving `answers`.
- **FR-003**: For each `ambiguous` fact, the generator adds its derived value to a per-question `optional_answers` field on the questions it would otherwise affect, and not to `answers`.
- **FR-004**: For `set`-scored questions, an answer in `optional_answers` counts towards neither precision nor recall, whether or not the run returns it.
- **FR-005**: Scoring for `single` and `history` questions is unchanged.
- **FR-006**: `prose-facts.json` initially contains `Telemetry clean-up / Owner / Marcus Haddad / journal-04` with status `stated` and `Routing handover / Owner / Priya Varga / journal-04` with status `ambiguous`.
- **FR-007**: `graph-questions.jsonl` is regenerated with `retrieval-gen`; question ids, text and splits are unchanged, and only `answers` and `optional_answers` change, on g072, g109, i013 and i014.
- **FR-008**: After regeneration, g109's `answers` are `[Orbit Core, Telemetry clean-up]`; g072 and i014 `answers` include Telemetry clean-up; i013 lists Routing handover under `optional_answers` and not `answers`.
- **FR-009**: The graph-question set version is bumped wherever it is recorded, so results from the old and new sets are distinguishable.
- **FR-010**: Regenerating from the committed fixtures reproduces `graph-questions.jsonl` byte for byte, covered by the existing reproducibility test or a new one.
- **FR-011**: A unit test demonstrates that adding or removing an optional answer in a returned set does not change the set score.
- **FR-012**: The v2 fixture README and `docs/retrieval-quality-evaluation.md` describe `prose-facts.json`, the `stated`/`ambiguous` distinction and the optional-answer scoring rule.
- **FR-013**: The CHANGELOG `Unreleased` section records the change, and `docs/llms-full.txt` is regenerated.
- **FR-014**: Generation fails with a clear error for an invalid `status` or a malformed entry in `prose-facts.json`.
- **FR-015**: `lcg-eval graph-qa` runs unchanged against a workspace; only the scores of the four affected questions differ from before.

### Key Entities *(if applicable)*

- **Prose fact**: A fact the LLM-rendered prose states that the skeleton does not, with entity, attribute, value, citing notes and a status (`stated` or `ambiguous`).
- **Optional answer**: A per-question value that is acceptable but not required in a set-scored answer; excluded from precision and recall.
- **Graph question**: A record in `graph-questions.jsonl` with gold `answers`, a scoring mode (`single`, `set`, `history`) and now an additive `optional_answers` field.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Regenerating from the committed fixtures yields `graph-questions.jsonl` identical byte for byte to the committed file.
- **SC-002**: A diff of the regenerated file against the previous one changes exactly four questions (g072, g109, i013, i014), and only their `answers`/`optional_answers`.
- **SC-003**: A run returning the corrected gold for g109 (`[Orbit Core, Telemetry clean-up]`) scores precision 1.0 and recall 1.0, where it previously scored 0.67 precision.
- **SC-004**: For i013, a run returning Routing handover and a run omitting it score identically.
- **SC-005**: The graph-question set version differs between the pre-change and post-change sets in every place it is recorded.
- **SC-006**: All graph-qa scores other than the four affected questions are unchanged against the same workspace.

## Assumptions

- The E17 graph-qa run's list of prose-introduced answers is complete for what runners have surfaced; the initial fixture covers only those two facts.
- "Wherever it is recorded" for the set version means the existing questions-version field(s) in the scenario/skeleton/meta artifacts and anything derived from them; the exact locations are determined during research.
- The `optional_answers` field is additive, so older consumers continue to parse the file; only the scorer needs to understand it.
- Bumping the version does not by itself require editing the corpus prose, so the corpus hash and existing v2 cassettes are unaffected.
- Ambiguous facts apply to the same question families (list, join, inverse) as stated facts.

## Out of Scope *(optional)*

- Editing the corpus prose (it would change the corpus hash and invalidate every v2 cassette).
- A systematic audit of the prose for skeleton contradictions; this issue covers only facts runners have surfaced.
- Re-running experiments; the maintainer re-scores E17 after this lands.

## Source References *(optional)*

- `crates/eval/fixtures/retrieval-v2/notes/journal-04.md` (the two prose facts)
- `crates/eval/fixtures/retrieval-v2/graph-questions.jsonl`, `skeleton.json`, `README.md`
- `docs/retrieval-quality-evaluation.md`
