# ADR-0785: Document Mode — Server-Side Chunking, Revision Diff and a Data Commit Boundary

**Status**: Accepted
**Date**: 2026-11-18
**Issue**: #785 (numbered by issue, per the ADR numbering rule), slice 1 of #733
**Builds on**: ADR-0705 (belief time, supersession reasons, split units), ADR-0706 (correction
reads), ADR-0707 (`doc_id`), ADR-0752 (origin), ADR-0292 (`write_path` lineage), ADR-0047 (replay
batching), ADR-0028 (dump column parity). **Consumed by**: #732 (pages as views over evidence),
#596, and the app-side retirement of `canonical-chunker.ts`.

## Context

Chunk mode (`knowledge_process_chunk`) extracts each client-made chunk on its own. It can revise and
remove, but only on explicit instruction: the server never knows a document's full extent, so a
section the client stops sending stays believed for ever, a heading rename orphans the chunk ids under
it, every resent chunk is re-extracted, and the extractor never sees the title or heading path.

Document mode adds a **completeness claim** — "this is document D, in full" — through
`knowledge_process_document`. From it the server can chunk, diff against D's previous revision,
retract by absence and supply document context. The hard part is atomicity: a revision is many
episodes plus many retirements, the WAL has no transaction marker (ADR-0047), replay is
batch-aligned, and every Phase B dedup decision reads the persisted graph.

## Decision

### 1. A hand-rolled, line-oriented chunker (`document/chunker.rs`, `CHUNKER_VERSION = 1`)

No new dependency. Blocks: ATX and single-line setext headings, fenced code (unclosed runs to EOF;
headings inside are text), front matter at byte 0, pipe tables (header + delimiter row), list groups
with continuations, and paragraph runs for everything else. CRLF-aware; every offset is a UTF-8
boundary. A **logical chunk is a section**: a heading plus its blocks up to the next heading of any
level; the preamble has an empty heading path; a body-less section yields no chunk (its heading still
reaches descendants through the heading path and the document map). A section over
`LCG_DOCUMENT_CHUNK_MAX_CHARS` (default 6000, below the 8000-char chunk advisory) is split into units
that exactly tile it — at block boundaries, then list-item/table-row starts, then lines, then
whitespace, then characters; a whitespace-only piece folds into its neighbour.

Invariants, pinned by a deterministic generated-corpus property test: chunks are ordered and
non-overlapping, `doc[byte_start..byte_end] == text`, units tile their chunk, and every non-whitespace
byte lies in a chunk unless it is on a body-less heading's line. **Spans are exact** — nothing is
injected into source text (the heading path and a split table's header go into the context block),
so SC-006 holds by construction. Revision bytes are not stored; the client holds the document.
`pulldown-cmark` was rejected because its event stream omits front matter and link reference
definitions, so "never drop content" could not be proved.

### 2. Matching: hash of the exact section text, then heading path, then position (`document/diff.rs`)

The match key is SHA-256 of the section's exact text, heading line included: **a chunk is
re-extracted iff its own text changed**. A context-only change (title, an ancestor heading, the map)
never re-extracts. Pairing: (1) exact hash as a multiset in occurrence order → `unchanged` (moves and
duplicated boilerplate, never collapsed or double-retired); (2) identical heading path → `changed`;
(3) the leftover old chunk right after the old counterpart of the nearest matched predecessor →
`changed` (renames, heading + body edits); (4) leftovers are `added` / `removed`. A changed chunk
**inherits the old episode `name`** (`"{doc_id}::{12 hex}"`, minted when added), so
`knowledge_get_correction_changes` pairs the versions.

Rejected: **heading-as-metadata** (hash the body only). Renaming `## Accepted` to `## Rejected`
changes how the body must be read — #733's exact failure — so skipping that re-extraction would keep
a stale interpretation believed. **Paragraph-level chunks**: more LLM calls and cascading boundary
churn.

**Measured (SC-009)** with `cargo run -p lcg-core --example document_diff_measure` over this
repository's own markdown history (`docs/`, `ideas/`, `README.md`; the app's 315-pair corpus is not
in this repo — re-measuring there is a follow-up once the app adopts document mode):

| | |
|---|---|
| revision pairs | 677 |
| chunks in new revisions | 8255 |
| re-extracted (changed + added) | 1097 (13.3%) — the other 7158 are skipped by hash |
| pairs with an added or removed chunk | 111 |
| pairs that orphan a heading-path address | 135 (19.9%), 264 orphaned addresses |
| heading-change-only pairs | 0 in this corpus |

A heading-path-addressed scheme orphans 264 chunk ids across those 135 pairs (each one left believed
unless the client remembers to remove it); document mode retires every removed chunk by absence. For
the heading-rename-only case the corpus has no instance, so it is pinned by tests instead: renaming a
section with a subsection re-extracts **1** chunk (the renamed section) where address-keyed ids
orphan **2**, and renaming a body-less heading re-extracts **0**.

### 3. Revision identity: episode columns plus a `DocRevision` record

Six nullable `Episodic` columns — `revision_id`, `revision_committed_at`, `byte_start`, `byte_end`,
`heading_path STRING[]`, `chunk_hash` — follow the `origin`/`write_path` precedent: probe-then-ALTER,
no backfill, storage version unchanged, logged params, dump columns appended last. Document-mode
episodes are written with their own `CREATE` template, so the chunk-mode statement and its WAL lines
are byte-identical (FR-019); a chunk-mode episode never carries a revision (FR-003) even with a
`doc_id`. `write_path = 'document'` is not chunk-eligible (ADR-0292 §2), so a chunk-mode revise by
name leaves document rows alone.

A `DocRevision` node table (uuid = server-minted UUIDv4 revision id, `group_id`, `doc_id`, `title`,
`source_path`, `origin`, `effective_at`, `committed_at`, `reference_time`, `attributes`,
`content_hash`, `byte_len`, `chunker_version`, ordered `chunk_names`, `previous_revision_id`) holds
what has no episode to live on: the metadata, removal-only revisions and the manifest whose order the
positional pairing needs. It copies `DocSupersession`'s plumbing: dump, replay (`MERGE … ON CREATE`),
group purge, and `GROUP_SCOPED_NODE_TABLES`. `effective_at` is recorded only — never `valid_at`,
never belief time.

### 4. The commit boundary is data: pending episodes plus one commit step

1. Added/changed units are written one at a time through the normal `add_episode_with_opts`
   (`SupersedePolicy::None`), so chunk 2's dedup sees chunk 1's entities. Each is **pending**:
   stamped with the new `revision_id`, `revision_committed_at` NULL.
2. Only if every unit succeeded and the call was not cancelled, one commit step under the write
   lock, in one explicit lbug transaction, one commit instant `t` and one WAL chunk:
   `DocRevision` row → `revision_committed_at = t` on the new episodes → `revision` supersession of
   changed chunks' old episodes (`superseded_by` = the new final unit) → `chunk_removed` for removed
   chunks → `revision` for stale and mode-transition episodes. Every statement is uuid-keyed and
   guarded, so replay is idempotent.

**Belief gating** (`belief.rs`): a revision-stamped episode is evidence only from its
`revision_committed_at`. A pending producer is skipped; an edge whose only surviving producers are
pending reads `not_yet_ingested`, and its interval is `no_record` / `pending`. Old beliefs end at
exactly `t`, new ones begin at `t`: no gap, no overlap. `revision_unit_uuids` (correction reads) and
the lint snapshot use the same predicate.

| Failure point | Live graph | Rebuilt from WAL | Belief for D |
|---|---|---|---|
| cap, empty document, origin conflict | nothing written | — | before |
| extractor/DB error or cancel on unit k | units 1..k−1 pending | same | before |
| crash before the commit step | pending rows | pending rows (or a prefix) | before |
| crash inside the commit, before `COMMIT` | rolled back | commit chunk absent | before |
| `COMMIT` done, crash before the WAL flush | after | before (graph ahead of WAL, as chunk mode) | never mixed |
| torn WAL tail inside the commit chunk | after | revision + stamp first, some supersessions missing | overlap until the next ingest |

**Self-heal.** The baseline groups live committed episodes by `name`; for a name with episodes from
more than one committed revision, all but the latest are *stale* and superseded at the next commit,
and a removal whose `chunk_removed` was torn away is simply re-classified `removed`. So the torn-tail
overlap — the same class every multi-line `with_chunk` already has — is repaired by the next ingest of
the document, without extraction. It is not prevented.

**Abandoned attempts are not cleaned up.** Their pending episodes are never believed and stay visible
to default (non-belief) reads, like superseded episodes (ADR-0705 "Reads"); readers see
`revision_id` without `revision_committed_at`. They do **not** count toward ADR-0752's per-`doc_id`
origin consistency (`origins_for_doc` skips pending rows), so a failed first ingest cannot fix a
document's origin and a retry with a different origin is accepted. The cost is a narrow race: while a
document ingest is still pending, a concurrent chunk-mode write to the same `doc_id` with another
origin is not refused. The corroboration reads in `origin.rs` / `origin_reads.rs` do not yet consult
the commit stamp. Garbage collection of abandoned rows is a follow-up.

Rejected: supersede-at-the-end without gating (a failure leaves old and new both believed); one
lock-held transaction for the whole document (Phase B makes LLM calls, readers would block for
minutes, and within-batch entity resolution does not exist); a compensating abort (does not survive a
crash); a WAL framing marker (replay is batch-aligned and cannot see it).

### 5. Diff policy

- **No-op**: nothing added, changed, removed, stale or in transition, and the whole-document hash
  equals the previous revision's → `status: "unchanged"`, zero extraction calls, nothing written
  (FR-021). A pure reorder commits a new revision (new manifest) with zero extraction.
- **Mode transition (FR-023)**: live chunk-mode or pre-marker episodes with the same `doc_id` are
  superseded (`revision`, no `superseded_by`) at every commit and reported as
  `mode_transition_superseded_episode_uuids`. No `chunk_id`-prefix adoption: it would be a guess.
- **Split units (FR-017)** are all written `SupersedePolicy::None` because supersession is deferred
  to the commit, which supersedes every old unit together. That gives the split-unit protocol's
  guarantees without its per-call `supersede` / `same_revision_episode_uuids` flags.
- **Serialization (FR-022)**: a per-`(group_id, doc_id)` async lock, taken before the baseline read
  and held through the commit. It lives in a process-wide map keyed also by the `AppState` pointer
  rather than as an `AppState` field, so the ~50 test fixtures that build `AppState` literally are
  unaffected. Chunk-mode writes to D are not excluded; the guarded statements and the stale rule
  absorb them.
- **Empty document**: zero chunks (empty, whitespace-only, headings-only) is refused with
  `empty_document`; a whole document is retired with the existing operations
  (`knowledge_supersede_document`, `knowledge_mark_chunk_removed`, `knowledge_delete_by_source`).
- **Cap**: `LCG_DOCUMENT_MAX_BYTES` (default 524288) on UTF-8 bytes, before anything else; the error
  names the cap. Invalid values fall back to the defaults with a warning.

### 6. Document context (`document/context.rs`)

A new `ExtractOptions.document_context` carries a rendered `<DOCUMENT_CONTEXT>` block — title,
source path, origin, heading path, part k of n for split sections, a split table's header, and a
document map windowed to 40 headings with the current section marked — prepended to each user prompt
ahead of the source-text tag. The "context, not source text: do not extract from it" instruction is
inside the block, so system prompts are unchanged. `None` everywhere but document mode, and the
cassette key gains a `document_context` field only when present: chunk-mode prompts and keys are
byte-identical (a golden test pins the key). Facts are cited only to the unit's own episode. Folding
the block into `custom_instructions` was rejected: it follows the message and is ambiguous with
ontology instructions.

## Consequences

- Two new methods (registry 52 → 54): `knowledge_process_document` (write) and
  `knowledge_preview_chunks` (read, the same chunking, no DB or WAL access).
- Cut-over from the app's chunker re-extracts chunks whose boundaries differ; the churn is additive
  and visible through `knowledge_get_correction_changes` because names are stable thereafter.
- Extraction within one call is sequential; a bounded-concurrency knob is a follow-up.
- Slice 2 (#733's interpretation pass and document-scoped reconciliation) builds on the revisions and
  manifests recorded here.
