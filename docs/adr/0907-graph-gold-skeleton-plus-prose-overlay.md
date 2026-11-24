# ADR-0907: Graph-Question Gold Is the Skeleton Plus a Prose Overlay; Optional Answers Are Score-Neutral

**Status**: Accepted
**Date**: 2026-11-23
**Issue**: #907

## Context

The v2 corpus is generated from `skeleton.json` and then rendered into prose by an LLM (ADR-0788).
The graph-question gold (ADR-0821) was derived from the skeleton alone. The rendering introduced two
project owners the skeleton does not contain, both in `notes/journal-04.md`: Marcus Haddad owns
*Telemetry clean-up* ("will continue with Marcus Haddad as owner") and Priya Varga leads *Routing
handover* ("led by"). A system that extracted them correctly returned an answer the corpus supports
but the gold omitted, and lost precision on g072, g109, i013 and i014 (g109: 0.67).

Editing the prose is not an option: it would change the corpus hash and invalidate every v2
cassette (ADR-0044).

## Decision

1. **Gold = skeleton + `prose-facts.json`.** A committed, hand-edited fixture lists the exceptions
   (`entity`, `attribute`, `value`, `notes`, `status`). `retrieval-gen` reads it from `--out`; a
   missing file means no overlay, an invalid one is an error. Only `Owner` is accepted, and the
   cited notes and owner must exist in the skeleton, and each cited note's rendered text must name
   the entity and the value, so a typo fails loudly instead of silently changing nothing. Names
   compare through `scorer::norm`: a repeated `(entity, attribute)`, an entity that already has a
   skeleton Owner, or one that equals a skeleton entity only after normalising is rejected, since
   each would put a project under two owners or skip the kind check. A misspelling that
   matches nothing is indistinguishable from a new project and is not caught.
2. **`stated` is gold, `ambiguous` is optional.** A `stated` fact counts as a skeleton fact for the
   owned-by, projects-by-owner-city and projects-of-reports questions. An `ambiguous` fact goes to a
   new additive `optional_answers` field (omitted when empty, so 140 of 144 lines are byte-identical
   and old consumers still parse the file).
3. **Optional answers are score-neutral in `set` scoring.** A returned answer matching an optional
   answer and no gold answer is dropped before precision; recall is over gold only. Gold wins when
   both match, because gold matching is containment-based. Optional answers themselves match by
   normalised equality, so a fragment of one is a wrong answer. `single` and `ordered` are
   unchanged. The graph-qa report rows list `optional_answers` where there are some.
4. **The overlay is applied inside those three families, not merged into the skeleton facts or
   project list.** Merging would add items to other families (owner-manager, supplier-*), shift
   the shared `Random(788)` shuffle and reselect later questions. As built, it never creates,
   removes or reselects a question: only `answers` and `optional_answers` change.
5. **`hops` stay skeleton-derived.** They describe the skeleton's facts; the prose facts are cited
   by `prose-facts.json` itself. This keeps the diff to answers only.
6. **Version bump: `questions_version` `meridian-questions-3` to `-4`.** It is the only recorded
   question-set version. The retrieval `questions.jsonl` is unchanged, `questions_hash` and the corpus
   hash are untouched, and `skeleton_hash` (and the copies in `render.json`, `review.json` and
   `questions.meta.json`, which are mechanical) change with the version string. The graph-qa report
   records no question-set version, so old and new scores are distinguished by the fixture and docs
   only; a report-level marker is a possible follow-up. Because the gold also depends on
   `prose-facts.json`, `questions.meta.json` records its `prose_facts_hash` and
   `retrieval-gen --check` fails when the file changes without it, forcing the version bump and
   the new hash to be a deliberate step. The check cannot see the old version, so it enforces the
   hash, not the bump itself.

## Consequences

- The E13 reference-parity test has a deliberate, narrow exception for g072 and g109.
- Further prose-introduced facts need an entry, a regenerate and a version bump. Supporting another
  attribute is a deliberate change to the generator.
- No systematic audit of the prose for skeleton contradictions was done; only facts surfaced by the
  E17 run are covered.
