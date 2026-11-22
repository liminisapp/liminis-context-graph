# ADR-0884: `knowledge_status` Opt-in Per-group Episode Breakdown

**Status**: Accepted
**Date**: 2026-11-22
**Issue**: #884 (follow-up 2 of ADR-0828)

## Context

ADR-0828 decided not to prune superseded episodes yet and recorded reopening triggers, the first of
which is "superseded : live episodes exceeds 2 : 1 in any group". `knowledge_status` reported only
a total `episode_count`, so no trigger could be checked on a real workspace without hand-written
Cypher. This ADR records the measurement surface. It adds no pruning or retention behavior.

## Decision

- **Opt-in request parameter.** `knowledge_status {"include_episode_breakdown": true}` adds a
  top-level `episode_breakdown`. Only a literal JSON `true` enables it; absent or any other value
  behaves as before and runs no extra query. The response without the flag is unchanged.
- **Whole breakdown opt-in, not just the expensive part.** An always-on episode split still costs a
  full `Episodic` scan on every status poll; `knowledge_status` is polled often. The entity and edge
  counts additionally scan every `MENTIONS` edge and every edge. All of it runs under the shared
  `write_lock` read guard, which blocks writers while it runs, so cost is more than caller latency.
- **Not a `knowledge_lint` category.** A client gets the ADR-0828 trigger from one call, and lint's
  findings/paging model is not bent around counters. Precedent: `include_episodes` (ADR-0857).
- **Shape.** `{as_of_us, groups: [{group_id, live, superseded, pending, only_superseded_entities,
  only_superseded_edges}]}`, groups sorted by `group_id`, every field present even at 0. The
  superseded : live ratio is computed by the client. `as_of_us` is the instant the counts were
  evaluated at. When requested but not computable (database not open, not queryable, scan error)
  `episode_breakdown` is `null` and `episode_breakdown_unavailable` carries a reason; the rest of
  the status call is unaffected. A group appears if it has at least one episode or edge.
- **One liveness definition.** Each episode is classified by `EpisodeSupersession::is_pending`
  (tested first, so a pending episode with `superseded_at` set is pending only), then
  `live_at(now)`, else superseded. Live, superseded and pending partition a group's episodes.
  Classification is in Rust over narrow projected columns; Cypher aggregates would be a third
  hand-written copy of the predicate (beside `live_at` and `LintEpisode::is_live`), and
  `LintSnapshot` is not reused because it holds every row in memory. The row mapping is shared with
  `get_episode_supersession`.
- **Edge rule.** An edge counts when `belief::evaluate(...) == Superseded`, so edge semantics match
  belief reads: hard-deleted producers are ignored, superseded plus pending producers with none
  live count, and pending-only, legacy and producer-less edges do not. Edges are counted by
  producer, not fact text.
- **Entity rule.** An entity counts when it has at least one `MENTIONS` from a superseded episode
  and none from a live episode. Pending mentions are neutral; an entity with no `MENTIONS` is not
  counted.
- **Group attribution.** Episodes by `Episodic.group_id`, entities by `Entity.group_id`, edges by
  `RelatesToNode_.group_id`. Support from an episode in any group counts, so each entity or edge is
  counted exactly once; an entity in group B supported only by a superseded episode in group A is
  counted under B.
- **Discoverability.** `knowledge_capabilities` lists the feature `status_episode_breakdown`.
  `PROTOCOL_VERSION` is unchanged (additive).

## Consequences

- Caveats documented with the field: a future-dated supersession counts as live until its instant
  (hence `as_of_us`); a workspace rebuilt from `knowledge_dump_wal` has no native `MENTIONS`
  (ADR-0828 F1), so `only_superseded_entities` reads 0 there.
- The scan cost at production scale is unmeasured; it is the reason for the opt-in. A `group_ids`
  filter is deliberately not offered yet.

## ADR-0828 text to apply

ADR-0828 was not on `main` when this landed (it lives on the #828 branch), so it is not edited
here. Once it merges, replace the sentence "None can be evaluated in production today because
`knowledge_status` does not split live from superseded (`episode_count` is a total), which is
follow-up 2." in *Reopening triggers* with:

> Trigger 1 is measured by `knowledge_status {"include_episode_breakdown": true}` (#884,
> ADR-0884): per group, `superseded / live` from `episode_breakdown.groups[]`, alongside
> `only_superseded_entities` and `only_superseded_edges`. Triggers 2 to 4 still need a rebuild
> timing, WAL bytes per live episode and a production-scale measurement.

and mark follow-up 2 as *done by #884*.
