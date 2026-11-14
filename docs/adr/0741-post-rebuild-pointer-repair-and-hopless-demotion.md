# ADR-0741: Rebuild Order Must Not Matter for Cross-Group Pointers — Demote Hop-less Pointers, Rebind After Every Rebuild

**Status**: Accepted
**Date**: 2026-11-13
**Issue**: #741 (implements community report #739)

## Context

A cross-group edge lives in the WAL of the group that **owns** it (`layer` in #739's
reproduction) and points at an entity in another group (`up`). In `cross_group.rs` terms the
pointer's `source_group_id` is `up` — the group pointed *into* — which is **not** the group that
owns the edge. That naming is a trap; this ADR and its tests use `layer`/`up`.

Replaying `layer` before `up` leaves the edge hop-less: the hop writes are `MATCH (…{uuid:…})
… MERGE`, find no endpoint, and no-op (counted in `match_prefixed_no_op`, not a failure). The
`RelatesToNode_` `CREATE` succeeds and replays its cached `attributes` verbatim, so the pointer
still says `bound` with a `resolved_uuid` and a `bound_at_seq` taken from `up`'s position when the
edge was written. Nothing repaired it:

1. `knowledge_rebuild_from_wal` ran `rebind_pointers` only `if reset_detected` (ADR-0387).
2. `rebind_pointers`'s staleness gate (ADR-0392) skips a `Bound` pointer whose `bound_at_seq` is at
   or past the source group's applied position — always true here once `up` has replayed.

`knowledge_status.cross_group_pointers` counts from `binding_state` alone, so it reported `bound`.

## Decision

After **every** successful, non-cancelled, non-dry-run `knowledge_rebuild_from_wal` — full or
incremental, reset or not, foreground (streaming) or background job — run one shared helper,
`handlers::post_rebuild_pointer_repair(state, conn, gid)`, in two steps:

1. **`cross_group::demote_hopless_pointers(conn, gid)`** — for each non-invalidated edge *owned by*
   `gid` (`RelatesToNode_.group_id`), any pointer side that is `Bound` with a `resolved_uuid` but
   whose actual hop (read with `get_relates_to_by_uuids`, the same `OPTIONAL MATCH` the rebind
   diffs against) is absent is rewritten `Unbound`, with `resolved_uuid` and `bound_at_seq`
   cleared. Only hop *presence* is checked; a present hop at a different uuid is an ordinary
   stale pointer the rebind already handles.
2. **`cross_group::rebind_pointers(conn, gid, ts)`** — the existing staleness-gated rebind of
   pointers pointing *into* `gid`, with each mutation bucket flushed to the owning group's writer
   (#383, ADR-0385).

Demote runs first so a self-pointing edge is handled consistently, and so the rebind sees the
demoted state. The helper runs after `set_wal_position`, so the gate sees this replay's
`applied_seq`.

### Why part (2) alone was not enough

The issue hypothesised that rebinding after every rebuild might suffice. It cannot: the ADR-0392
gate skips a `Bound` pointer with `bound_at_seq >= applied_seq` however often it is called.
Fixing that inside the gate would change gate semantics for pointers that are genuinely bound and
current (explicitly out of scope) and would leave `knowledge_status` reporting `bound` for a
missing edge. Demoting to `unbound` rides on ADR-0392's existing guarantee that `unbound` pointers
are always re-resolved, so the gate is unchanged.

### Why not in replay

Replay is a generic Cypher replayer with no knowledge of pointers. The demotion is a post-replay
pass in the handler/`cross_group` layer.

### The demotion is not WAL'd

It is derived local state: re-replaying the same stream re-derives it, no WAL format change is
needed, and appending to the stream being replayed is the hazard documented at
`clear_group_for_rebuild`. `demote_hopless_pointers` drains and discards `Conn`'s recorded-mutation
buffer before returning (on the error path too) so its `SET`s cannot leak into the first `rebind_pointers` bucket and be
flushed to some group's WAL.

### Result shape and failure behaviour

`cross_group_rebind` reuses `RebindCounts` and is non-null on every successful non-dry-run rebuild
(a dry run never replays, so it stays `null`). The demotion and the rebind are independent: a
demotion failure is logged and the rebind still runs. If the rebind itself errors it is logged and
the field is `null` while the rebuild still succeeds, so a caller can tell the repair did not
complete. No
`demoted` counter is added: the demoted pointer is visible as `unbound` in `knowledge_status`
between the rebuilds, and as `bound` in the `up` rebuild's counts once repaired.

### Cancellation

`replay_opts` returns `Ok` on cancel and `ReplayStats` has no cancelled flag, so the streaming path
gates the repair on the service-shutdown flag (`shutdown_cancelled`). A client disconnect alone is
not gated, matching the other post-replay steps (index build, position write), which also run
then. The background-job path has no cancel hook.

## Consequences

- Rebuild order across groups no longer affects the final graph (`layer`→`up` == `up`→`layer`).
- Every rebuild now does one pointer-candidate scan (filtered by `CONTAINS`, cheap when no
  pointers exist) under the write lock, twice (demote + rebind).
- An ordinary rebuild of `up` can now append rebind mutations to `layer`'s WAL stream, where
  before only a reset did. That is the same flush #385 designed, but a setup in which `layer` is a
  received read-only stream should be aware of it.
- A `force_clear` rebuild of `up` with `layer` already built now re-binds `layer`'s pointers
  (the purge flipped them `unbound`, and nothing re-bound them before). A beneficial side effect,
  covered by a test.
- Tests that assumed a manual `knowledge_rebind_pointers` after a rebuild now see the rebuild
  doing that work (`cross_group_incremental_replay.rs` was adapted accordingly).
- Wrong-order replay still counts the hop writes as `match_prefixed_no_op`, which can feed
  `fidelity_warning`; unchanged and out of scope.

## Related

ADR-0369 (pointer model, "unbound = missing hop"), ADR-0378 (per-group positions), ADR-0385
(per-group mutation attribution), ADR-0387 / ADR-0414 (generation and reset detection),
ADR-0392 (staleness gate keys on binding state).
