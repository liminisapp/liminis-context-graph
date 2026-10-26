# ADR-0706: Correction Reads — What a Correction Changed, and What Was Written While the Old Belief Held

**Status**: Accepted
**Date**: 2026-10-26
**Issue**: #706 (part 2 of 3; builds on #705 / ADR-0705, #707 follows)

## Context

ADR-0705 made corrections end *belief* without deleting evidence. That answers "is this fact
believed?" but not the two questions a user has after a correction:

1. **What did the correction change?** A believed-only read shows the world after the correction, an
   as-of read shows it before. Neither shows the difference.
2. **What was written while the old belief held?** Work may have leaned on a fact that was later
   dropped. Nothing connects a dropped fact to the window in which it was believed.

Everything needed is already recorded: an episode's `superseded_at` / `superseded_by` /
`supersession_reason`, an edge's producing `episodes` and `ingested_at`, and the episode-to-entity
`MENTIONS` link.

## Decision

Add two **read-only, additive** methods, each with an MCP `ToolSpec` in the `read` scope. They write
nothing (no `valid_at` / `invalid_at`, no WAL entry, no schema change), so they inherit ADR-0705's
rebuild stability (WAL replay, dump, checkpoint restore).

- `knowledge_get_correction_changes {episode_uuid}` — the facts the episode supported that its direct
  superseder does not (`removed`) and the reverse (`added`). A fact on both sides appears on neither.
  Each removed fact carries `still_believed`, true when another live episode outside the replacement
  still supports the edge (its belief interval is open), so "removed from the replacement" is not
  mistaken for "dropped from belief".
  `added` means only "supported by the replacement and not by the old episode": such a fact may already
  have been believed through another live episode before the correction.
- `knowledge_get_affected_work {edge_uuid}` — for an edge no longer believed, the episodes ingested in
  its belief interval `[T1, T2)` that mention either endpoint entity.

**Fact identity** is `(source entity uuid, target entity uuid, relation name, fact text)`, with `name`
being `RelatesToNode_.name` rather than the ontology-reclassifiable `relation_type` (ADR-0033/0037).
Edges are not deduplicated, so a re-extracted fact is a different row; matching on all four errs
toward reporting a reworded fact as one removal plus one addition, the safe direction for a review
tool. Entity merges that change endpoint uuids over-report the same way.

**The replacement side of a split chunk is the whole revision.** `superseded_by` records only the
final unit; earlier units (`supersede: false`) stay live with no stored link, and persisting one would
need a schema change. The read derives them statelessly: episodes of the same group and name ingested
at or before the supersession instant `T` and not superseded at or before `T`, plus the final unit.
The old generation was superseded at exactly `T` and older generations before it, so both are
excluded; later generations were ingested after `T`. **Known limit:** a free-form
`knowledge_add_episode` episode that reuses a chunk id as its `name` and sits in that window is swept
into the replacement set.

**The belief interval is derived, never stored** (`belief::belief_interval`), from the same records as
`belief::evaluate`: `T1` is the edge's `ingested_at`; `T2` is the last supersession instant among its
surviving producing episodes. Its states are named, never collapsed into an empty list (FR-006):

| `interval.state` | cause | meaning |
|---|---|---|
| `closed` | — | every surviving producer superseded; candidates are returned |
| `open` | `still_believed` | a producer is live; affected work is not defined |
| `no_record` | `legacy` / `support_deleted` | no producing episode, or all hard-deleted |
| `not_defined` | `warranted` | a direct assertion; believed until a retraction primitive exists |

If some producers were hard-deleted but others were superseded, `T2` ignores the missing ones and the
response sets `derived_from_surviving_support: true`.

**Time is compared Rust-side in microseconds.** The whole-second read-back would collapse a correction
and the edges it produced onto one instant (ADR-0705), and a timestamp parameter in Cypher risks the
same loss. Candidates are fetched through `MENTIONS` with `ingested_at` read in microseconds and
filtered to `[T1, T2)` in Rust, so an episode ingested at exactly `T2` — the correcting episode — is
outside the window.

**Affected work is candidates, not dependents.** The list comes from entity overlap, not recorded
dependence (that would need an audit trail of which facts an agent read). Every response, including
the empty and non-closed ones, carries `basis: "entity_overlap"` and a note saying so, and the list is
keyed `candidates` — never `episode_uuids` or `producing_episode_uuids`, which mean different things
(ADR-0012, ADR-0705). The fact's own producing episodes are excluded: they are the belief, not work
done under it. So are the units of the revision that superseded them (recovered as in
`revision_unit_uuids`): a split revision's earlier units are ingested before its final unit closes
the interval, but they are the correction, not work done under the old belief. Candidates are ordered by ingest time and neither ranked nor truncated; a hub entity
over a long window returns a long list.

**Edges are found by producing episode** with a per-episode `list_contains(rn.episodes, $uuid)` query
scoped to the group (probed against lbug 0.21.1). The group is taken from the loaded record, as in the
other uuid-keyed reads, so neither method takes `group_ids` (ADR-0368 / ADR-0371).

## Consequences

- **Additive.** No existing read changes behaviour; the Python client is untouched.
- **Registry**: 47 → 49 MCP tools, `read` bucket 15 → 17.
- **Per-episode only.** A heading rename supersedes one chunk and creates another under a new
  `chunk_id`, so a per-episode diff shows only removals for the old chunk. Whole-document diffs need
  document identity (#707).
- **Approximate affected work** until an agent audit trail exists. The response says so each time.
- **Cost.** Each call is a handful of uuid- or group-keyed lookups; the changed-facts read runs one
  list-membership query per revision unit.
