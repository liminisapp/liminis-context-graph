# ADR-0778: Lazy Stamp Restore and Stored-Kind Check When the D1 Stamp Is Absent

**Status**: Accepted
**Date**: 2026-11-17
**Issue**: #778
**Amends**: ADR-0776 (restore was "rebuild only, not lazy"), ADR-0627 (single stamp writer),
ADR-0446 (Decision 4: the sidecar's `identity_types` carve-out)
**Relates to**: ADR-0616, ADR-0637

## Context

`check_identity_set` decided "did the identity set change?" by comparing the group's stamp with the
current identity set, and a missing stamp reads as `∅`. Removing `identity: true` from a group's only
identity type therefore compared `∅` with `∅`: the kind-aware carrier count never ran and
`knowledge_reload_ontology` / `knowledge_add_episode` were accepted while entities still carried
`kind = <type>`. With the stamp present the same change was refused (`-32003`). ADR-0776's restore
fixed this only after `knowledge_rebuild_from_wal`, not on a plain restart. 0.19.0 behaved the same.

## Decision

When the stamp file is **absent** on entry to `check_identity_set` (stamped groups take exactly the
path they always did):

1. **Restore on first resolve.** `restore_stamp_if_absent` runs before the stamp is read, on any
   startup path. It still fills an absent file only: it never overwrites (even a corrupt stamp), and
   restores nothing for a missing sidecar, a sidecar without `identity_types`, or an empty set.
2. **Check the data.** `Conn::distinct_non_default_kinds` lists the group's distinct stored `kind`
   values other than NULL, `''` and `Entity`. Each one not in the current identity set is unioned
   into the changed labels as a removal (`identity_stamp::merge_stored_kinds`), then the existing
   kind-aware carrier count and refusal run unchanged (same `-32003`, same message).

The scan runs even after a successful restore, so stored data wins over a stale or hand-edited
sidecar. The union (rather than "only when nothing else changed") also catches the mixed case: one
identity type stays, another loses its flag. A type removed from the ontology entirely needs no
special case, since its stored kind is simply not in the current set.

If the DB cannot be opened or scanned while the stamp is absent, the result is `unchecked` (not
cached, retried), never a silent accept. A clean scan writes nothing, so a flagless workspace still
writes no stamp file; the price is one group-scoped, unindexed scan per such group on its first
resolve per process. Avoiding it would need a persisted marker, which is out of scope.

## Consequences

- ADR-0776 said restore "happens in `rebuild_from_wal` only, not lazily — a lazy path would add a
  second baseline source". That is reversed: the second baseline source is now safe because real data
  is always consulted and takes priority, so a wrong sidecar cannot make the guard accept a
  kind-reinterpreting change.
- ADR-0627's "`check_identity_set` is the only stamp writer" now also covers the lazy restore
  (alongside the rebuild restore); ADR-0446 Decision 4's `identity_types` carve-out applies to every
  first resolve, not just a rebuild.
- Concurrent first resolutions may race on the restore; `write_stamp` uses a unique temp name and an
  atomic rename and the restore never overwrites, so this is safe.
- Operator-visible: a group with a missing stamp and stale identity-kind data is now refused where it
  was previously accepted.

## Amendment (0.20.1, #919): the stored-kind check is withdrawn

Decision 2 assumed that every stored non-`Entity` kind came from an identity-bearing type. It
doesn't: `knowledge_assert_entity` with an explicit `kind` (ADR-0615) creates that kind whatever
the ontology says. A group whose ontology has no identity type never writes a stamp. After a
restart, the scan therefore read its asserted `Conversation` / `File` entities as an identity
removal and refused every `knowledge_add_episode` with `-32003`. A workspace upgraded from 0.19.1
was refused on its first ingest.

0.20.1 removes Decision 2 from `check_identity_set`. Decision 1 (the lazy restore from the WAL
sidecar's `identity_types`) stays, and it covers the common way a stamp goes missing. An identity
removal made while both the stamp and the sidecar's `identity_types` are missing is again not
detected, as in 0.19.1: #778 is reopened. A proper fix needs to tell an identity-assigned kind
from an asserted one, which today's stored data cannot do. The tests that need Decision 2 are kept
in `crates/core/tests/identity_set_hydrate.rs`, marked `#[ignore]`, as the spec for that fix.
