# ADR-0903: Move a document by re-keying its episodes

**Status**: Accepted
**Date**: 2026-11-23
**Issue**: #903 (numbered by issue, per the ADR numbering rule)
**Builds on**: ADR-0752 Decision 8 (fixed-valued, uuid-keyed updates), ADR-0853 (episode source
identity), ADR-0707 (document supersession), ADR-0785 (document mode), ADR-0705 (auto-commit
statements).

## Context

A note app renames notes and reorganises folders routinely. The only way to express that was to remove
the document and ingest it again, which extracts every fact a second time at full LLM cost although not
a word changed. A reorganisation of about 900 meeting documents would re-extract all of them. A
document's identity is in the path in several places, but everything derived from its *content* is
still correct after a rename.

## Decision

**1. A move re-keys existing episodes; it never extracts.** `knowledge_move_document` rewrites identity
columns on the document's episodes and calls no extractor, LLM or embedder. Entities, relationships,
embeddings, belief history and `origin` are untouched.

**2. The identity columns are more than the issue lists.** `knowledge_delete_by_source` matches the
stored `source_file` (ADR-0853), so rewriting only `doc_id` and `source_description` would leave a
delete on the old path still matching. A move therefore rewrites `doc_id`, `source_file`,
`source_description` (recomposed as `<source_file>:<chunk_id>`; a raw `add_episode` description is the
whole new source) and, only with an explicit `chunk_id_prefix`, `name` and `chunk_id` together. A legacy
row with no stored `source_file` gets one written when the move names a path. Document-mode chunk names
(`<doc_id>::<hex>`) are opaque baseline keys, so with no prefix they stay.

**3. State keyed by `doc_id` outside `Episodic` moves too.** `DocRevision` rows (`doc_id`,
`source_path`, `chunk_names`) are what the next `knowledge_process_document` at the new `doc_id` reads
as its baseline; without re-keying it would find none and re-extract, which is the cost this feature
removes. `DocSupersession` records and `Episodic.superseded_by` are re-keyed on both sides so
"supersession links continue to resolve". A `DocSupersession` primary key cannot be `SET`, so that
record is deleted and merged under the new key (both fixed-valued).

**4. Live episodes identify a document; its history moves with it.** An identity with no live episode
is an unknown source. But superseded revisions are part of the document, and leaving them under the old
path would make `knowledge_delete_by_source` on the old path delete history and leave lint citations
naming the old `doc_id`. They are re-keyed too. (The spec's wording "only live episodes move" is read as
"only live episodes identify the document". What a *destination* may hold is decision 5, and there
liveness does not matter: history counts as something under it.)

**5. The destination must be empty; refusals are decided before the first write.** A move's
destination must have **no episodes or document state of any kind** in the group. One function,
`destination_footprints`, answers that for every move — whether it names a `doc_id`, a source path or
both — and the pure planner only receives its result. The footprint is every `Episodic` row of any
liveness matching the destination `doc_id`, the destination path or a chunk id under the rewritten
prefix, plus the destination `doc_id`'s `DocRevision` rows, its `DocSupersession` record on either side
and the episodes whose `superseded_by` names it. The moving document's own rows are set aside. A live
episode in the footprint is `occupied_destination` (a merge, not a move); anything else is
`destination_has_history`. Reading the footprint can fail with a database error, which is an error, not
a refusal. This replaced a series of one-case checks (revision rows, then a supersession record, then
retired chunks, then the superseding side, then path-only history): each fix had left the next special
case, because "what does the destination hold?" had no single answer. A path or `doc_id` that still has
history is cleared with `knowledge_delete_by_source` first.

The other refusals are an unknown source, a cross-group identity, a chunk-id collision *between* moves
of one batch (an existing chunk id is already under the destination), an identical identity and batch
conflicts (duplicate source, duplicate destination, a destination that is another move's source). The
batch is validated against the state before the batch, so chains and swaps are refused rather than
ordered. Any refusal changes nothing and writes nothing to the WAL.

**6. Validate-then-apply, not a transaction.** lbug auto-commits per statement (ADR-0705), and an
explicit transaction would disable the statement-level FTS backstop (`repair_fts_after`), which matters
because `source_description` is FTS-indexed. Atomicity comes from full validation under the
per-document locks and the write lock, then one fixed-valued statement per row, flushed as a single
`wal_flush_chunk`. If an unexpected database error interrupts the apply, the statements that ran are
still flushed so the WAL equals the graph, and the error is returned; re-sending the batch then reports
the finished moves as `unknown_source`. The spike confirmed lbug 0.21.2 accepts `SET` on the ART-indexed
`doc_id` / `name` and the FTS-indexed `source_description`, and a rebuild from the WAL reproduces them.

**7. Locks.** Document ingest takes a per-`(group, doc_id)` lock before it reads its baseline and keeps
it until it has committed; only its commit step runs under the global write lock, so the write lock
alone does not serialize a move against an ingest in flight. A move therefore takes the same
document locks for every `doc_id` it reads or writes (old and new, in sorted order), then the write
lock, and takes the destination footprint and applies the plan under both. An ingest into the
destination then either committed before the move — the footprint sees it and the move is refused — or
starts after it and finds the moved document at that `doc_id`; and a move waits for an ingest in
flight on its source and re-keys what it committed. Nothing can land in between. The lock set comes from
a pre-plan (a path-only move learns its `doc_id`s from the episodes), so the authoritative re-plan checks
that its own lock set is covered (an ingest can add a `doc_id` at a path-only move's source between the
two); if it grew, all locks are released and the move retries with the larger set, up to five attempts,
then fails with nothing changed. That loop exists to discover the lock set, not to repair a read-lock
gap, and cannot be dropped while the ingest baseline is read outside the write lock.

**8. Write scope, `GroupRequired`.** A rename is a routine consumer operation, unlike
`knowledge_stamp_origin`, a one-off adoption tool. `--writable-groups` therefore applies to `group_id`.

**9. Inbound links are not rewritten.** `attributes.links` on other pages is free-form JSON on other
documents' episodes. A move touches only the document it moves; consumers re-send the pages that link
to the moved `doc_id`.

## Consequences

- A folder rename is one call whose cost does not depend on document size.
- One statement per episode: a 900-document move is a few thousand statements in one WAL chunk, which
  replay batches by template.
- `knowledge_capabilities` advertises `move_document`; MCP tools 59 → 60, IPC methods 60 → 61.
- Planning is a linear scan of the group's episodes per move (matching the source and the
  destination footprint), done twice per call — once under the read lock to find the lock set, once
  under the write lock — and again if the lock set grows. Measured on the pure planner in a **debug**
  build: 900 moves of 10 episodes each in a group of 69,000 episodes took about 2.5 s for the
  footprints and 1.0 s for the plan per pass (release is expected to be much faster; not measured).
  If that ever matters in practice, index the snapshot by `doc_id` and stored `source_file` once and
  look up per move; the legacy-row rule in `episode_matches_source` (rows with no stored
  `source_file`) must keep its linear fallback, which is why this is not done up front.
