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
is an unknown source, and only live episodes can occupy a destination. But superseded revisions are
part of the document, and leaving them under the old path would make `knowledge_delete_by_source` on
the old path delete history and leave lint citations naming the old `doc_id`. They are re-keyed too.
(The spec's wording "only live episodes move" is read as "only live episodes count" for identity and
occupancy.)

**5. Refusals are decided before the first write.** A destination that already holds live episodes is a
merge and is refused, as is a destination `doc_id` that has no live episode but still carries `DocRevision` rows or a `DocSupersession` record, or is named as another document's superseding document, or holds only retired or superseded episodes (a hard delete keeps the revision rows; re-keying the source's rows into it would merge two histories — `destination_has_history`), as are an unknown source, a cross-group identity, a chunk-id collision (checked
group-wide against the post-batch names), an identical identity and batch conflicts (duplicate source,
duplicate destination, a destination that is another move's source). The batch is validated against the
state before the batch, so chains and swaps are refused rather than ordered. Any refusal changes nothing
and writes nothing to the WAL.

The history check is keyed by `doc_id` only. A destination *source path* that only retired or
superseded episodes of another document carry is deliberately not refused: path sharing with earlier
history already happens whenever a new document is ingested at the path of a deleted or retired one
(`source_file` is matched exactly, ADR-0853), so a move does not introduce it, whereas `doc_id` is the
key revision and supersession lineage hang off and two histories under one `doc_id` is a real merge.
Refusing on path-only history would also block the common "delete a note, then rename another onto its
path" case. The consequence — `knowledge_delete_by_source` on that path also removes the old history —
is documented in the reference.

**6. Validate-then-apply, not a transaction.** lbug auto-commits per statement (ADR-0705), and an
explicit transaction would disable the statement-level FTS backstop (`repair_fts_after`), which matters
because `source_description` is FTS-indexed. Atomicity comes from full validation under the
per-document locks and the write lock, then one fixed-valued statement per row, flushed as a single
`wal_flush_chunk`. If an unexpected database error interrupts the apply, the statements that ran are
still flushed so the WAL equals the graph, and the error is returned; re-sending the batch then reports
the finished moves as `unknown_source`. The spike confirmed lbug 0.21.2 accepts `SET` on the ART-indexed
`doc_id` / `name` and the FTS-indexed `source_description`, and a rebuild from the WAL reproduces them.

**7. Locks.** Document ingest takes a per-`(group, doc_id)` lock before reading its baseline. A move
takes the same locks for every old and new `doc_id` in the batch, in sorted order, then the write lock,
and re-plans under them, so a destination claimed by an ingest cannot end in a silent merge. The lock set comes from a pre-plan, so the authoritative re-plan checks that its own lock set is covered (an ingest can add a `doc_id` at a path-only move's source between the two); if it grew, all locks are released and the move retries with the larger set, up to five attempts, then fails with nothing changed.

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
