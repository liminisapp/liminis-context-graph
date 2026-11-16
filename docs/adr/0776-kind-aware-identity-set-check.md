# ADR-0776: Kind-Aware D1 Identity-Set Check and Stamp Restore from the WAL Sidecar

**Status**: Accepted
**Date**: 2026-11-16
**Issue**: #776 (implements community report #775)
**Amends**: ADR-0616 (D1's carrier count; D2's rebuild-into-a-fresh-directory claim), ADR-0627
(single stamp writer), ADR-0446 (Decision 4: the WAL ontology sidecar is never read)
**Relates to**: ADR-0615, ADR-0637

## Context

The D1 guard (ADR-0616) refuses a change to a group's identity-bearing set when an existing entity
"carries" a changed label. Community report #775: a group rebuilt from its WAL into a **fresh
workspace root** is refused on its first `add_episode` — and on `knowledge_reload_ontology` — though
its ontology never changed. Two defects combine:

1. The stamp `.lcg/identity-set/<group>.json` is written only by `check_identity_set`, never by WAL
   replay. A host that keeps only the WAL has no stamp, so the recorded set reads as `∅` and every
   identity type looks newly *added*. ADR-0616 D2 claimed the stamp survives "a rebuild-from-WAL
   into a fresh directory"; that holds only while the workspace root persists.
2. The carrier count matched every entity whose `labels` contained the type. Replay restores
   entities whose `kind` already equals the identity type, so nothing would be reinterpreted — yet
   they were counted.

## Decision

### Kind-aware carrier rule

An entity is a **carrier** only if its `kind` would differ had it been extracted under the new set.
Kind is decided from the entity's *primary (leaf) type alone* (ADR-0616; `Ontology::identity_kind`):
ancestors never confer kind. The predicate follows (`Db::count_identity_carriers`, still one
group-scoped query per changed label, run only when the sets differ):

- **Removing identity from T** — carrier iff `e.kind = T`. Ancestry plays no part.
- **Adding identity to T** — carrier iff all of:
  - `T IN e.labels`;
  - `e.kind IS NULL OR e.kind = '' OR e.kind <> T` (NULL and empty count as "not T" — pre-#615 and
    old-WAL rows; a bare `<>` would drop them);
  - none of `D(T)` is in `e.labels`, where `D(T)` is every type in the **new** ontology whose
    ancestors include T (`Ontology::descendants_of`). Carrying no descendant label is the proxy for
    "T is the entity's leaf type". The exclusion is a chain of bound `NOT ($dN IN e.labels)` terms —
    only the `IN e.labels` construct already used in the repo, no list functions.
- **Adding identity to an `extract: false` type** — never a carrier: such a type never receives an
  extracted kind (ADR-0637), so the count is 0 and no query runs. Removal stays kind-based whatever
  the type's `extract` flag.

| Entity (adding identity to T) | Carrier? |
|---|---|
| leaf T, kind T (replayed, consistent) | no |
| leaf T, kind `Entity` / NULL / empty | yes |
| leaf S under T, kind S or `Entity` (S identity-bearing or not) | no |
| T and S both being added | S is judged on its own; T excludes S-leaf entities |

| Entity (removing identity from T) | Carrier? |
|---|---|
| kind T | yes |
| leaf T, kind `Entity` | no |
| kind S under T | no |

**Unknown or older hierarchy.** `D(T)` comes from the new ontology. An entity stamped under an older
hierarchy may carry descendant labels the new map does not know; such an entity has no *known*
descendant label and is counted — the check errs towards refusing.

**Consequences for ADR-0616.** "Flipping a flag on an ancestor type is refused while any descendant
carries its label" no longer holds: entities of descendant types keep their kind and are not
carriers. A genuine flip — an entity whose kind would change — is still refused, with the stamp
present or deleted, naming the label and the count; refusal scope, caching and
`group_identity_refusals` reporting are unchanged. A missing stamp with consistent data now passes
and self-heals (the accepted path writes the stamp). A group refused under the old logic is accepted
on its next start or reload if its data is consistent.

### Stamp restore on hydrate

The WAL ontology sidecar (`<wal>/<group>/.wal-ontology.json`, ADR-0446 Decision 4) gains an optional
`identity_types: [..]` — the sorted identity-bearing set — populated only by
`write_wal_ontology_sidecar` (the workspace and group drift sidecars leave it absent). It is
rewritten on every successful `add_episode`; because the guard blocks changes, it equals the set the
group's entities were created under.

After a rebuild-from-WAL (both the streaming and background paths of `knowledge_rebuild_from_wal`),
`identity_stamp::restore_stamp_if_absent` writes the stamp from the sidecar, **before**
`clear_group_drift` (which on a `Resolving` slot runs the D1 check and must see the restored
baseline). It never overwrites: any existing stamp file, even an unparseable one, is left alone. An
older sidecar without the field, a missing sidecar, or an empty recorded set restores nothing and
raises nothing (a flagless group has no stamp file). The kind-aware check covers all of those.

This adds a second stamp writer to ADR-0627's "`check_identity_set` is the only writer"; the restore
only ever fills an absent file. Restore happens in `rebuild_from_wal` only, not lazily during
resolution — a lazy path would add a second baseline source to every resolve.

The sidecar is therefore no longer "write-only" (ADR-0446 Decision 4): its `identity_types` is the one
field a rebuild consumes. Its vocabulary fields remain documentation only and never govern a
consumer's extraction, validation or reprocessing. If the restored set differs from the consumer's
resolved set, the kind-aware check decides against the real replayed data — so a flip made while the
stamp was lost is still caught.

### Trust

A hand-edited or stale `identity_types` can only fill an absent stamp, and the kind-aware check still
runs against the real data, so the exposure is bounded.

## Consequences

- #775 is fixed in all three reported orderings (hydrate → reload; install → hydrate → add_episode;
  install → reload → hydrate → add_episode), with or without the sidecar.
- No storage-version, lbug or IPC change; `-32003` and `group_identity_refusals` are unchanged.
- The doc comments in `identity_stamp.rs` and on the carrier count are corrected; the stamp survives
  only while the workspace root persists, and the sidecar carries the set otherwise.
- `.wal-ontology.json` is no longer purely informational; omitting it loses the restore baseline but
  degrades gracefully.
