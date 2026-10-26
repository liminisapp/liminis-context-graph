# ADR-0707: Document Supersession — A New Document Explicitly Replaces an Old One

**Status**: Accepted
**Date**: 2026-10-26
**Issue**: #707 (part 3 of 3; builds on ADR-0705)

## Context

ADR-0705 let an *episode* be superseded (corrected or removed), ending belief without touching
validity. A document is sometimes replaced rather than edited, and that differs from a correction in
one essential way: the old decision **was true** until it was replaced. Belief ends when the
supersession is *recorded*; validity ends at the *effective date*, which can be earlier (retroactive)
or later (future) than the recording. Documents existed in the graph only as free-text
`source_description` strings, so a supersession had nothing to point at.

## Decision

**Documents become addressable through an optional `doc_id` on episodes.** It is caller-supplied,
unique within a group, never defaulted from `source_file`, and absent on legacy episodes (which cannot
take part; legacy graphs are re-ingested). Omitting it changes nothing for existing callers.

**The relation is a durable record, not per-episode columns.** A new `DocSupersession` node table,
keyed `group_id ␟ superseded_doc_id`, holds `superseding_doc_id`, `effective_at` (where "in force"
ends) and `recorded_at` (when the graph learned). Keying by the superseded document makes "at most
one superseding document per group" structural and turns idempotent re-record and conflict detection
into a point lookup. Episode columns alone cannot represent a document with no live episodes, cannot
carry a per-link date through a chain, and cannot be traversed for cycle detection. The record is
created with `MERGE … ON CREATE SET`, so a replayed statement never overwrites the first record.

**Belief reuses ADR-0705's episode columns.** Recording supersedes B's live episodes
(`superseded_by_kind = document`, `supersession_reason = document_superseded`, `superseded_by` = A's
`doc_id`) with `superseded_at = max(recorded_at, effective_at)`. A future effective date therefore
leaves B believed until it is reached with no change to `belief::evaluate`, whose `s > t` test already
treats a future `superseded_at` as live. `effective_at` is stored only on the record — one source of
truth, joined by `(group_id, doc_id)` — and `invalid_at` is never written.

**A pending document supersession does not shield an episode from correction.** "Live" for the
revision and removal paths becomes `superseded_at IS NULL OR (superseded_by_kind = 'document' AND
superseded_at > now)`, and the supersede statements use the same guard. A correction of B before its
effective date ends belief at the correction and the episode becomes `corrected` (the statement clears
`superseded_by` / `superseded_by_kind`). After that the kind is no longer `document`, so a replay of
the same statement matches nothing and stays idempotent.

**In force is derived and reported, never filtered.** `belief::in_force` answers per evaluated time,
independently of belief: a producing episode is in force unless its document was superseded with an
effective date at or before `t`; an episode superseded any other way was *corrected* and makes no
validity claim. An edge is in force iff any producing episode is (mirroring belief's "any live source"
rule); `null` when every present producing episode was corrected. `supersession` (`cause`,
`superseded_at`, and for documents `superseded_by_document`, `in_force_until`, `recorded_at`) reports
the stored state, independent of `t`, so a fact is classifiable as "corrected" or "in force until T"
from any read. Both fields appear only on opted-in belief reads; the record lookup runs only when a
document-superseded episode is present, so default reads are unchanged.

**Validation is Rust-side, under the write lock, before any mutation.** Same document, unknown
document (no episode with that `doc_id` in the group), conflicting existing record, and cycle (walk
the superseding document's chain of superseders, ADR-0012) are rejected with nothing written and no
WAL entry. The record and episode statements share one WAL flush, so the WAL never holds half a
supersession. An identical re-record is a no-op (`already_recorded`).

**Durability follows ADR-0673 / ADR-0705.** The record's timestamp params are registered as timestamp
parameters, `doc_id` is appended last in the Episodic dump columns (ADR-0028), the record table is
dumped by compaction, and `knowledge_delete_by_group` purges it so a re-ingested document meets no
stale record.

## Consequences

- A document's supersession is permanent; withdrawing one is a later primitive.
- Existence and uniqueness lookups on `doc_id` are scans (no index), as chunk lookups are.
- `effective_at` is supplied by the caller as an absolute timestamp; extracting it from text, an
  in-force-only filter, and fact-level supersession are out of scope.
