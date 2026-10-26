# Proposal: Consolidate the Ingestion Surface in liminis-context-graph

**Status**: Draft for review. Pre-Spec-Kit — this is the argument an ADR would record.
**Date**: 2026-09-05
**Revised**: 2026-10-26, against `main` at `b963de40` (after the Temporal work, #705–#707)
**Belongs to**: `liminis-context-graph` (the surface is lcg's; the fossils are in `liminis`)

Confidence tags: **[V]** verified by direct read at a cited `file:line` · **[I]** inference · **[?]** open.
Citations are to lcg `main` unless prefixed `liminis:` (the app, `shadoworg/liminis-notes` `main` at
`d0d74d1b`).

**What changed since the first draft.**

- **Revision is no longer only a document-mode thing.** The Temporal work shipped revision for chunk
  mode: resubmitting a `chunk_id` supersedes that chunk's earlier episodes, and
  `knowledge_mark_chunk_removed` ends belief in a chunk that is gone (ADR-0705). It also shipped a
  caller-supplied `doc_id` and explicit document-to-document supersession (ADR-0707). The draft's
  §5 asymmetry ("chunk mode is append-only, document mode is revisable") is therefore withdrawn, and
  §5 is rewritten around what document mode *still* adds: a completeness claim.
- **The six phantom methods are gone from the app.** liminis #1175 removed every caller and the MCP
  tools built on them, and added an engine contract test. §4 and §6 are narrowed accordingly.
- **The client-state desync is fixed** (liminis #1285): chunk state is now recorded only after the
  ingest succeeds.
- **Three of the Chunking-hardening defects describe code that never merged.** #288, #291 and #292
  were filed against #284's PR #286, which was closed unmerged. Their hazards reappear, in a
  different shape, in #705's supersession lookup. §3 and §8 say how.
- The 0.15.0 "revision manifest" thread in §8 is superseded by what shipped. Open question 5 is
  rewritten; questions 2 and 6 are reframed.

---

## 1. The claim

Ingestion belongs in lcg, end to end. lcg should accept **either**:

- **Chunk mode** — a sequence of chunks prepared by a client with its own chunking strategy, or
- **Document mode** — a document plus metadata, chunked internally with a markdown-oriented default,

converging on the same stored shape and the same extraction machinery: episodes with rich
attributes and the best entity/relationship extraction achievable *given the context provided*.

Both modes are revisable as of the Temporal work. §5 argues where they still differ: document mode can
carry a **completeness claim**, chunk mode cannot, and that decides which of them can retract by
absence.

## 2. A restoration, not an invention

Server-side chunking existed until the Rust rewrite. **[V]** The liminis #689 Research stage
(2026-04-24) records two chunkers coexisting:

1. `graphiti_service.py:chunk_markdown()` — chonkie `RecursiveChunker`, 512 chars, server-side,
   behind `knowledge_index_document`.
2. `canonical-chunker.ts` — AST/heading-based, `HARD_MAX_TOKENS = 2000`
   (**[V]** `liminis:liminis-app/src/main/canonical-chunker.ts:105`), added later for queue-based
   indexing.

The Rust rewrite implemented the chunk-entry half and kept **neither** server-side chunker. The app
has since removed its calls to the document-level methods the rewrite never implemented (§4), so the
app's chunker is now the only one anywhere.

## 3. One root cause, seven defects — revisited

| Defect | Split-ownership cause | State at `b963de40` |
|---|---|---|
| #292 foreign-episode destruction | `chunk_id` is a *client address*; the server cannot tell which rows a chunk owns | **Reshaped, still open.** #286's `DETACH DELETE` never merged. #705's supersession matches `Episodic.name = chunk_id` within the group and nothing else (**[V]** `db.rs:1480-1496`), so a `knowledge_add_episode` row whose free-form `name` equals a later `chunk_id` is *superseded* rather than deleted. Evidence survives; belief in it ends. |
| #291 unindexed `Episodic.name` scan | `chunk_id` overloaded into `name` because the server has no document model | **Now on the hot path.** The same lookup runs on every `knowledge_process_chunk` call, since `supersede` defaults to `true` (**[V]** `handlers.rs:1091`, `episode.rs:1537-1546`). Bulk ingest of N chunks into one group is O(N²). ADR-0707 records that `doc_id` lookups are scans too. |
| #288 resubmission race | server cannot order or batch work it does not own | **Partly answered.** Phase C is serialized, so of two concurrent resubmits the later commit supersedes the earlier (ADR-0705, "Revision"). Two live duplicates no longer survive. The wasted extraction remains. |
| #284 splitting + idempotency | server reconstructs prior text because the client's `chunk_version_hash` never crosses the wire | **Open, paused.** The protocol now supports client-side splitting (`supersede: false` on non-final units, `same_revision_episode_uuids` on the final one, ADR-0705 "Split units"). The server still has no splitting mechanism, and there is still no unchanged-text no-op. |
| liminis #689 context blindness | server never sees the document a chunk came from | **Unchanged** (below). |
| supersession unsoundness | client computes `ChunkDiff` (added/changed/deleted/**unchanged**) and discards it at the wire | **Mostly fixed by #705, but the app does not use it.** Changed → resubmit supersedes; deleted → `knowledge_mark_chunk_removed`. The app still sends deletions as `knowledge_delete_chunk_episode` (**[V]** `liminis:indexing-queue.ts:1672`), which destroys the episodes and leaves their edges `support_deleted` (ADR-0705 "Hard deletion"). "Unchanged" still never crosses the wire (§5). |
| client-state desync | `chunkState.setEntry` committed at *enqueue*, before the server confirmed | **Fixed** (liminis #1285): state is recorded only after a confirmed ingest (**[V]** `liminis:indexing-queue.ts:1244-1270`), which is what ADR-0705's split-unit protocol requires of clients. |

**[V]** The document signal still reaches the server and is still discarded.
`source_description = "{source_file}:{chunk_id}"` (`handlers.rs:1079`), and the app's `chunk_id` is
`doc_id::heading_path::chunk_index` (`liminis:canonical-chunker.ts:418`). The heading path physically
arrives. Nothing parses it or embeds it, and the extractor never sees it. The extraction
user-message is still only `<CURRENT_MESSAGE>{body}</CURRENT_MESSAGE>` + `<ENTITIES>` +
`<REFERENCE_TIME>` (`crates/core/src/prompts/mod.rs:228-241`). The app does not send `doc_id` either:
its `processChunk` passes `source_file`, `chunk_text`, `chunk_id`, `reference_time`, `attributes` and
`group_id` only (**[V]** `liminis:indexing-queue.ts:1735-1742`), so ADR-0707's document
supersession is unreachable from the app today.

## 4. The six unimplemented methods — resolved on the client, one fix left in lcg

**[V]** All six are still absent from lcg's dispatch table (`crates/core/src/handlers.rs:156-218`)
and from lcg's own MCP registry (`crates/service/src/mcp/tools.rs`). The first draft's complaint
was that the **app** advertised all six to agents as MCP tools and called them from live paths.
liminis #1175 (closed 2026-10-03) removed every caller and every tool built on them, and added a
static contract test that checks each socket method the app sends against a checked-in list of
methods the pinned engine implements (**[V]**
`liminis:liminis-app/src/main/__tests__/context-graph-engine-contract.test.ts`,
`liminis:…/__tests__/fixtures/engine-methods.json`). Legacy file-level queue entries that targeted
`knowledge_process_document` are now dropped rather than retried forever
(**[V]** `liminis:indexing-queue.ts:1686-1695`).

| Method | Live callers (liminis), 2026-09-05 | Now |
|---|---|---|
| `knowledge_index_document` | `closer-agent.ts`, `knowledge-writer-provider.ts`, `ipc/context-graph-handlers.ts` | removed (#1175) |
| `knowledge_process_document` | `indexing-queue.ts`, `knowledge-writer-provider.ts` | removed (#1175) |
| `knowledge_list_sources` | `knowledge-reader-provider.ts` | removed (#1175) |
| `knowledge_preview_chunks` | `knowledge-reader-provider.ts` | removed (#1175) |
| `knowledge_suggest_duplicates` | `knowledge-reader-provider.ts` | removed (#1175) |
| `knowledge_entity_edge_analysis` | `knowledge-reader-provider.ts` | removed (#1175) |

So the broken agent surface is closed. What is left is the reverse: the capability these names stood
for (document in, chunks out, sources enumerable) is now simply **missing**, rather than missing
*and* advertised.

**The error-code defect is still open in lcg.** An unknown method still returns the generic -32000
with the text `"Method not found: <name>"` (**[V]** `handlers.rs:218`, mapped at `handlers.rs:105`).
The app still detects it by substring, for the one optional method it keeps calling,
`knowledge_recover_autonomous` (**[V]** `liminis:knowledge-reader-provider.ts:1-11`,
`liminis:ipc/__tests__/context-graph-handlers.test.ts:2424-2427`). **Fix regardless of this
proposal's outcome: return -32601 for an unknown method, and keep the `"Method not found: "` message
text verbatim.** Changing the code and the string together would reclassify real failures for any
client still substring-matching during the window between the engine change and the client update.
Changing the code alone needs no client coordination.

## 5. Where the two modes cannot be equivalent

**What shipped.** Chunk mode is revisable at chunk granularity, through explicit per-chunk operations:

- **Correction:** resubmitting a `chunk_id` supersedes that chunk's earlier live episodes in the
  group once the new ingest has fully succeeded. Belief ends at the commit instant; `valid_at` /
  `invalid_at` are never written (ADR-0705, "Revision"; **[V]** `episode.rs:1532-1548`).
- **Removal:** `knowledge_mark_chunk_removed {chunk_id, group_ids}` supersedes a chunk's live episodes
  with reason `chunk_removed` (ADR-0705, "Removal"; **[V]** `handlers.rs:2284`).
- **Document replacement:** an optional caller-supplied `doc_id` on episodes (**[V]**
  `handlers.rs:1119`, `schema.rs:88`) and `knowledge_supersede_document` (**[V]** `handlers.rs:2357`)
  record that document A replaces document B from an effective date. That ends belief at
  `max(recorded_at, effective_at)` and reports "in force until" on belief reads (ADR-0707).
- **Reads:** `believed_only` / `as_of` on the edge reads (ADR-0705), plus
  `knowledge_get_correction_changes` and `knowledge_get_affected_work` for what a correction changed
  and what was written while the old belief held (ADR-0706).

The draft's proposal that chunk mode stay append-only is therefore moot, and it was the wrong call
anyway: the app needs chunk-level correction now, and it got it without a document model.

**What chunk mode still cannot do: retract by absence.** Every chunk-mode revision is an *explicit*
statement by the client: "this chunk changed", "this chunk is gone". The server never infers either.
That is sound, but it makes the client the only party that knows a document's full extent:

- **[V]** The app still sends only *changed* chunks; unchanged ones are filtered by
  `ChunkStateStore.diffChunks()` and never enqueued (`liminis:indexing-queue.ts:1178-1194`). A server
  receiving chunks still cannot distinguish "unchanged", "deleted but the removal is still queued",
  and "re-addressed under a new `chunk_id`".
- A removal the client never sends leaves the old chunk believed indefinitely. With the app as it is,
  a removal arrives as a hard delete (§3), which is worse: the evidence goes, and its edges become
  `support_deleted`.
- A re-addressed chunk (a heading rename changes the `chunk_id`) shows up as one new chunk plus one
  removal, and only if the client sends the removal.

**What document mode still adds: the completeness claim.** A document-mode ingest says *this is
document D, in full, at time T*. That is the precondition for retraction by absence: "F is no longer
asserted by D" is only meaningful over a scope known to be complete. With it, the server can:

- compute added / changed / removed / unchanged chunks itself, from bytes it holds, and apply ADR-0705's
  supersede and `chunk_removed` operations without the client sending them;
- choose a chunk-matching strategy with both revisions in hand (below), rather than inheriting the
  client's cache key;
- record the revision as a fact about D, not as an inference from which chunks happened to arrive.

**The guarantee must be recorded on the data, not only in the API.** The same content ingested two
ways now carries different *retraction* semantics: under document mode, absence from a later revision
ends belief; under chunk mode, it does not. A reader asking "why is F still believed?" must be able to
tell which guarantee applied, or the answer is uninterpretable on a mixed-mode graph. **[I]** The
natural carrier is a server-minted revision identity on the episode, present only when the episode
was observed under a completeness claim. `doc_id` alone does not do this job: since ADR-0707 it is
caller-supplied and means only "these episodes belong to document D", which a chunk-mode client can
assert without claiming completeness.

**[I] Against over-claiming:** document mode does **not** make chunk churn disappear. Measured over
315 real revision pairs, 9.8% orphan at least one `chunk_id`, driven by heading renames. Since the
heading sits *inside* the chunk text, a rename still changes chunk content. What changes is that the
server holds *both* revisions and can choose a matching strategy (paragraph-level, content similarity,
heading-as-metadata). Client addressing offers no choice: a changed address is simply a different
chunk. Matching becomes a server-side decision with full information rather than an artifact of
someone else's cache key.

## 6. Disposition of the six

The app no longer calls any of them (§4), so this is now a question of which *capabilities* lcg should
add, not of which broken calls to repair. There is no deprecation sequence left to run on the app side.

**Implement (3)**

- **`knowledge_process_document`** — the document-mode entry point. It chunks internally, ingests
  through the same Phase C as `process_chunk`, applies ADR-0705 supersession and removal itself from
  the server-side diff, mints the revision identity (§5), and returns per-chunk results. It streams
  progress through the existing `_progress_token` mechanism (ADR-0005). It requires a `doc_id`,
  which makes ADR-0707's document supersession reachable for document-mode callers.
- **`knowledge_list_sources`** — the read counterpart to document ingest. `doc_id` is now a real
  column (**[V]** `schema.rs:88`), so "list the documents in a group, with their latest revision" is
  a group-scoped scan away. It needs no new storage. **[I]** It should list by `doc_id`, not by
  `source_description`.
- **`knowledge_preview_chunks`** — **load-bearing, not a nicety.** If lcg owns chunking, any preview
  UI must ask the server what the chunks are, or it will show something different from what is
  stored. This is what lets the app's chunker be *deleted* rather than kept in sync (§7).

**Do not revive (3)**

- **`knowledge_index_document`** — two names for one operation was a Python-era accident, and the app
  no longer calls it, so no alias is needed. `process_document` pairs with `process_chunk`.
- **`knowledge_suggest_duplicates`** — not ingestion, and largely subsumed: LLM-verified extraction
  dedup (ADR-0652) and the corrections pipeline (`validate_corrections` / `apply_corrections` /
  `merge_entities`) cover what it was for. Re-file against that work if a gap is found.
- **`knowledge_entity_edge_analysis`** — **[?]** still no one has stated what it did that
  `knowledge_status`, `list_entities`, `list_relationships` and `get_entity_neighbors` do not. Leave it
  dead unless the gap can be named.

## 7. Chunker ownership

Porting `canonical-chunker.ts` is the main cost: mdast + micromark + `gpt-tokenizer` become a Rust
markdown AST (comrak/pulldown-cmark) plus a tokenizer, with matching semantics.

Two chunkers that disagree is a new bug class. The resolution is to **not have two**: make lcg's
chunker canonical and have any preview call `knowledge_preview_chunks`. The app's local chunker is
itself a rewrite-era workaround for a server capability that went missing.

**The fidelity question is downstream of a cheaper one: is a re-index additive or destructive?** The
Temporal work has settled this. A re-ingest **supersedes; it never deletes** (ADR-0705): the old
episodes stay, belief in them ends, and the full history stays queryable with `as_of`. Re-indexing
with a different chunker is therefore *additive* by construction, which the first draft named as the
condition under which the port need not be behaviour-identical. A different chunker produces a new
revision whose chunks differ from the old one's, and that is what an episodic graph should do with a
re-observation anyway.

Two costs remain:

- **[I] Belief churn on cut-over.** Re-indexing a corpus with a new chunker supersedes every episode
  whose chunk boundaries moved, and re-runs extraction on them. **[V]** Extraction is
  nondeterministic (`docs/history/extraction-eval-2026-07.md:61`), so the believed graph after the
  cut-over is a *re*-extraction, not a re-labelling: some facts will be worded differently or missing.
  `knowledge_get_correction_changes` (ADR-0706) makes that difference visible per episode. That is a
  cost to plan for, not a reason for behaviour-identity.
- **[I] Unchanged-text no-op.** Without one, every document-mode ingest re-extracts every chunk.
  Document mode needs to skip unchanged chunks server-side, by content hash, before extraction; this is
  the half of #284 that is still worth doing.

## 8. Interactions with work in flight

- **Temporal (#705–#707, shipping in 0.18.0).** This is now the foundation document mode builds on,
  not a parallel thread. Document mode should be specified as a *producer* of ADR-0705's operations
  (supersede by resubmission, `chunk_removed`) and a *user* of ADR-0707's `doc_id`, so that both modes
  converge on one belief model. The 2026-09-06 manifest discussion is overtaken by events. No
  manifest protocol shipped; chunk mode got explicit per-chunk operations instead, and that is
  sufficient for it. The record half of that discussion survives in a smaller form: "what did D
  consist of at T?" is answerable from the episodes carrying `doc_id` and their supersession columns,
  provided document mode mints a revision identity (§5).
- **Chunking hardening (#282, #284, #288, #291, #292, #293 — all open).** Re-scope against `main`
  before working any of them, because three were written against PR #286's unmerged code:
  - #284's splitting becomes document mode's *internal chunking strategy*, using ADR-0705's
    split-unit protocol (`supersede: false` / `same_revision_episode_uuids`) server-side.
  - #291 is more urgent than when filed: the unindexed `name` scan now runs on every
    `process_chunk` call (§3). The `NameIndex`-style accelerator it proposes (ADR-0038) applies
    unchanged, and should cover `doc_id` lookups too.
  - #292's foreign-row hazard now ends belief instead of deleting, which is recoverable in the
    sense that the evidence stays, but no API un-supersedes. **[I]** Scoping the supersession lookup to
    episodes that `process_chunk` itself wrote would close it.
  - #288 is largely answered by Phase C's commit ordering (§3).
  - #282 and #293: lcg now returns an advisory `warning` above `LCG_CHUNK_TEXT_ADVISORY_MAX_CHARS`
    (default 8,000, #407; **[V]** `handlers.rs:995-1031`) but still has no IPC-layer size cap.
    #293's cap becomes a document-size cap in document mode.
- **The app (liminis).** It is a step behind the engine. It still hard-deletes removed chunks, does not
  send `doc_id`, and therefore gets neither chunk removal nor document supersession. **[?]** No
  liminis issue tracks adopting ADR-0705/0707. That adoption is cheap and independent of this
  proposal, and should come first: it is the chunk-mode baseline that document mode would later
  replace for the app.
- **liminis #689 (contextual retrieval, still open).** Stops being an architecture debate. In document
  mode the context is present, so Paths A/B/C/D collapse to an internal quality choice. Two findings
  carry over: `bge-base-en-v1.5`'s 512-token limit against `HARD_MAX_TOKENS = 2000` means chunks over
  ~412 tokens are embedded from a truncated prefix; and **[V]**
  `CREATE_FTS_INDEX('Episodic','episode_content')` exists (`schema.rs:909`) but nothing queries it.
  Every `QUERY_FTS_INDEX` targets `Entity` or `RelatesToNode_` (`db.rs:2186`, `db.rs:2215`), and
  `knowledge_search_passages` is vector-only (`db.rs:2365`).
- **#596 ("lcg does it all").** **[V]** It explicitly puts "the producer/ingestion side (document
  chunking …)" out of scope, because for the zen/orac consumers that runs in a separate producer.
  **[I]** That is a second client with its own chunker, which strengthens the case for one canonical
  chunker in lcg, but #596 does not depend on this proposal.

## 9. Risks

- **Wire contract.** lcg is public with tagged releases. Additive methods are safe;
  `knowledge_process_chunk` stays as chunk mode, with its ADR-0705 revision semantics unchanged.
- **Long-running document ingest.** N extractions in one call. Progress streaming exists. ADR-0705
  gives a per-chunk answer to partial failure: supersession is the *last* step of a chunk's Phase C,
  so a failed chunk leaves its old episodes live. **[?]** What a *document* is after a partial
  failure (a revision with some chunks new and some old) still needs defining (§10, question 2).
- **Re-index.** Additive by construction now (§7). The cost is belief churn and re-extraction, which
  should be stated in release notes rather than discovered.
- **Scan cost.** Every chunk and document lookup is an unindexed scan (§3, ADR-0707 "Consequences").
  Document mode multiplies the lookups per call. #291's accelerator is close to a prerequisite.
- **Scope.** This proposal now touches the Chunking-hardening milestone and the app's Temporal
  adoption. It is a reframing, not a competing plan, but it should be settled before more specs are
  written against the current shape.

## 10. Open questions

1. ~~Behaviour-identical chunker port, or free to differ with a re-index?~~ **Answered (§7):**
   re-ingest supersedes rather than deletes (ADR-0705), so the port is free to differ. What remains
   is planning the belief churn of the cut-over.
2. **Partial-failure semantics for a document.** Per chunk it is defined (old episodes stay live). Per
   document: does a revision with failed chunks count as complete for retraction by absence? **[I]**
   It should not: removals inferred from a partial revision would retract facts the document still
   asserts. Apply removals only once every chunk of the revision has committed.
3. Does `knowledge_entity_edge_analysis` have a purpose worth keeping? (§6)
4. Non-markdown content: design the content-type seam now, or markdown-only with a fallback later?
5. **Revision identity.** Server-minted per document-mode ingest, carried on each episode, and the
   marker that a completeness claim applied (§5). Is a new episode column the right carrier, or a
   per-revision record alongside ADR-0707's `DocSupersession`? **[?]**
6. **Mode transition — still undefined, now cheaper.** A document first ingested in chunk mode, later
   in document mode. The draft's two bad answers still apply in principle: treat the first
   document-mode ingest as a first observation and the chunk-mode episodes stay believed forever, or
   adopt them into revision 1. **[I]** The app's `chunk_id` already starts with the document path
   (`liminis:canonical-chunker.ts:418`), so the first document-mode ingest of D can find D's
   chunk-mode episodes by `name` prefix and supersede them with reason `revision`. The old facts lose
   belief at the instant the new ones gain it, with no matching needed. This is the realistic
   migration path for the app, so it should be specified rather than discovered.
