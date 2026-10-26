# ADR-0705: Belief Time Is Separate From Validity — Corrections Supersede Episodes

**Status**: Accepted
**Date**: 2026-10-26
**Issue**: #705 (part 1 of 3; #706 and #707 build on it)

## Context

ADR-0673 gave the graph the *start* of transaction time (`ingested_at`). Nothing ended it. When a
document chunk was corrected (the same `chunk_id` resubmitted), `knowledge_process_chunk` was pure
append, so a fact the correction removed stayed believed indefinitely. When a chunk disappeared, the
usual client move was `knowledge_delete_chunk_episode`, which `DETACH DELETE`s the episodes: the edges
they produced survived and stayed believed, while the record of where they came from was destroyed.

There was also no link from a fact to the episode that produced it. `RelatesToNode_.episodes` was
never written, and `Episodic.entity_edges`, despite its name, holds *entity* UUIDs. The
`episode_uuids` returned for an edge is inferred at read time from episodes mentioning *either
endpoint* (ADR-0012), so a corrected fact still looked supported by any unrelated episode that
mentioned "Alice".

A correction must not be confused with a change in the world. The removed fact was never true, so
writing `invalid_at` would falsely claim it was true until the correction.

## Decision

**Correction ends belief; it never touches validity.** No supersession path writes `valid_at` or
`invalid_at` (FR-008, SC-006). Belief time (transaction time) and validity (the world's clock) stay
separate columns with separate meanings.

**Episodes are superseded, not deleted.** `Episodic` gains four nullable, additive columns —
`superseded_at TIMESTAMP`, `superseded_by STRING`, `superseded_by_kind STRING` (`episode`; `document`
is reserved for #707) and `supersession_reason STRING` (`revision`, `chunk_removed`; #707 adds
`document_superseded`). A fifth documented divergence from graphiti's `kuzu_driver.py` after
ADR-0673's `ingested_at`: nullable, probe-then-`ALTER` migrated, so graphiti-shaped statements still
bind.

**Edges record their producing episode.** Extraction writes `RelatesToNode_.episodes = [ep_uuid]`
(exactly one, since edges are not deduplicated). `RelatesToNode_.valid_at_source`
(`text` / `reference_time`) records whether the edge's `valid_at` came from the text or silently
defaulted. `RelatesToNode_.warrant` (`assertion`, `cross_group`) marks the edges that legitimately
have no producing episode; they are believed until a future retraction primitive retracts them.
`producing_episode_uuids` on the read annotation is provenance. The ADR-0012 `episode_uuids` field is
unchanged — ADRs are immutable — and stays the either-endpoint inference.

**Belief is derived, never written onto edges.** An edge is believed at `t` iff it was ingested at or
before `t` and either it has a warrant, or at least one episode in `episodes` was not superseded at or
before `t`. An edge with neither episodes nor warrant is a **legacy** row: believed, and counted in
`knowledge_status.legacy_edge_count`. Legacy edges cannot be soundly backfilled — endpoint
co-occurrence is not provenance (#404) — so legacy graphs are re-ingested rather than migrated. The
interval is half-open: an edge superseded at `T2` is believed on `[T1, T2)`.
`RelatesToNode_.expired_at` (graphiti's transaction-end column) is deliberately not reused: writing it
would fan a per-edge write out of every supersession and blur belief with validity.

**One instant per commit.** Phase C of `add_episode` mints a single service-clock stamp up front. It is
the new episode's and every new edge's `ingested_at` *and* the `superseded_at` of the episodes the
ingest supersedes, so the old facts lose belief at the very instant the new ones gain it: no gap and
no overlap (SC-003). ADR-0673's "stamp only when empty" rule allows the pre-stamp.

**Revision (FR-003).** `knowledge_process_chunk` is the only entry point that supersedes
(`knowledge_add_episode.name` is free-form). Supersession is the **last** step of Phase C, after the
new episode, its edges and MENTIONS exist, and any earlier failure returns first, so a failed
re-ingest leaves the old episode live (belief ends late, never wrongly). The statements are
`MATCH (ep:Episodic) WHERE ep.uuid IN $uuids AND ep.superseded_at IS NULL SET …`: uuid-keyed,
fixed-valued and guarded, so tail replay over an overlapping seq range (ADR-0026) is idempotent and
a second supersession can never overwrite the first. They land in the same `wal_flush_chunk` as the
new episode, so the pair is atomic in the WAL. Scoped to the call's `group_id` (ADR-0368/0371).
Because Phase C is serialized and extraction is lock-free, "the chunk's other live episodes" is
commit-ordered: of two concurrent resubmits, the one that commits second supersedes the first.

**Split units.** The service has no chunk-splitting mechanism, so the protocol stays stateless.
`supersede: false` on every non-final unit of a chunk sent as several calls under one `chunk_id`;
`same_revision_episode_uuids` on the final unit lists the earlier units' episodes, which stay live. If
a unit fails the final call never happens and the old episodes stay live. A client that records a
chunk as ingested before the server confirms will never resend a failed correction; clients should
commit chunk state only after the ingest succeeds.

**Removal (FR-004).** `knowledge_mark_chunk_removed {chunk_id, group_ids}` (write scope; `group_ids`
required, as for the delete, #406) supersedes the chunk's live episodes with reason `chunk_removed`
and no `superseded_by`. Per-group attribution and WAL flush mirror `remove_episodes_by_chunk_id`
(ADR-0378/0385/0402). `knowledge_delete_chunk_episode` keeps its explicit-deletion contract.

**Assertions never overwrite extracted edges (FR-010, resolves #405).** `resolve_and_update_edge`
treats a matched edge with a non-empty `episodes` as no match, so a separate `assertion`-warranted
edge is created. Today the extracted `"A → B"` naming keeps the two from matching by accident; this
makes it structural. The corrections copies (`same_as`, endpoint rewrite) carry `episodes`,
`warrant` and `valid_at_source` into the re-created row — otherwise a merge would turn a supported
extracted edge into a believed-forever legacy row.

**Hard deletion.** `knowledge_delete_episode`, `delete_by_source` and the chunk delete still destroy
episodes. An edge whose `episodes` all fail to resolve is `support_deleted` and unbelieved at every
`t`: believing a fact whose evidence the client deleted is the bug this ADR fixes.

**Reads (FR-009).** `believed_only` and `as_of` are accepted on `find_relationships`,
`list_relationships`, `get_entity_neighbors`, `get_edges_by_group` and `get_edges_by_uuids`; either one
adds a `belief` object (`state`, `believed`, `producing_episode_uuids`, `warrant`, `valid_at_source`)
to each edge. `as_of` alone implies the filter; `believed_only: false` with an `as_of` annotates
without filtering. Evaluation is Rust-side (`belief.rs`): two batched uuid-keyed lookups (edge rows,
episode supersession) feed a pure function, rather than a correlated Cypher join — ADR-0012 records
lbug cycle support as unverified, and no existing read does a correlated lookup — which also avoids
editing a dozen hand-written RETURN lists. Capped reads over-fetch ×3 and truncate, so a filtered read
can return fewer than `num_results`. **Default reads are unchanged** (no extra queries, same shape);
making belief the default is a later, documented behaviour change (precedent: #406 in 0.13.2).
Timestamps are compared at microsecond precision; the whole-second read-back would collapse a
correction and the edges it ingested onto one instant.

**Durable, not derived.** The new columns are bound params of the CREATE/SET statements and are kept in
the WAL (not in `DERIVED_PARAM_KEYS`); `superseded_at` is in `TIMESTAMP_PARAM_NAMES`. The dump
templates and page queries append the columns last (ADR-0028), so compaction preserves belief state
(SC-004). An unset string column dumps as `""` and reads back as absent.

## Consequences

- Believed-only reads drop the corrected-away fact and the superseded duplicates that re-ingesting
  changed chunks used to accumulate (SC-005); the full history stays queryable with `as_of`.
- Search and passage reads (`search_passages`, `get_episodes`) are unchanged: superseded episode text
  is still returned by default.
- Edges are not deduplicated. If dedup is adopted later, `episodes` holds several entries and the
  "any live episode" rule already covers it.
- Not covered: inferring correction vs change, fact-level retraction from contradicting sources,
  recording what agents read, explicit document supersession (#707), and what-changed/affected-work
  (#706).
