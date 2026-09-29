# ADR-0616: Ontology-Driven Kind Assignment — Identity-Bearing Entity Types During Extraction

**Status**: Accepted
**Date**: 2026-09-29
**Issue**: #616 (second phase of community report #614; follows #615)
**Amends**: ADR-0615 (its "Extraction stays in the `Entity` namespace" section)

## Context

ADR-0615 made `Entity.kind` part of identity — `(group_id, kind, name)` — but left extraction
assigning the default kind `Entity` to everything, so only asserted entities could stay distinct
from a same-named extracted one. Extracted typing is inconsistent (the same "Aurora" may be
`Technology` in one chunk and `Product` in another), so making *every* extracted type an identity
key would split one thing into several nodes. Kind assignment has to be opt-in per type, decided by
the ontology author.

The governing principle: **identity is decided once, at creation, and never inferred afterwards.**
Two consequences shape the design: a WAL rebuild cannot apply a flag retroactively (replay re-executes
each record's stored `kind` and never calls the extractor), and kind cannot be re-derived from stored
labels (if "Aurora" was extracted as `Person` and as `Technology`, non-identity rules already merged
them and the information to separate them was never recorded).

## Decision

**Format.** `entity_types[].identity: true` (absent = false) on `EntityTypeDef`.
`Ontology::identity_set()` derives the set of normalized identity-bearing names; `content_hash`
appends an `identity:` section only when at least one flag is set, so flagless ontologies hash
byte-identically (no spurious drift). `Merged` (the reserved tombstone kind) and `Unclassified`
cannot be identity-bearing.

**Kind comes from the primary type alone.** `kind = normalize(primary type)` if identity-bearing,
else `Entity`. Never inherited through `parent`, and no label or declaration order is consulted
(an `Rfc` under identity-bearing `Document` gets kind `Entity`). Open mode normalizes only for the
identity-bearing label so `kind ∈ labels` holds; non-identity labels are unchanged.

**Resolution is kind-scoped.** Phase B name lookup and the embedding-dedup candidates use the
entity's own kind (`*_kind` helpers; the default-kind search path is byte-identical to before).
Dedup never compares across kinds.

**Edge endpoints.** Edges carry names only. In-batch, a name maps to at most one entity per kind
(a later same-kind duplicate replaces the earlier, as before); cross-batch resolution
(`Db::resolve_entities_by_name_in_kinds`) is confined to `{Entity} ∪ identity set`, not every
known kind, so extraction still never touches asserted kinds it did not create. **More than one
candidate is ambiguous and the edge is dropped** (`edges_dropped_unresolvable`, distinct log line)
rather than guessed — the #414 / ADR-0615 policy. This is a documented limitation for flagged
groups only. An edge between two same-named entities of different kinds is also dropped as
self-referential by the existing name-only self-loop filter.

**D1 — refuse a reinterpreting change.** On a group's first ontology resolution in a process, its
resolved identity-bearing set is compared with the recorded one. Any label in the symmetric
difference carried by at least one existing entity (full `labels`, so ancestor labels, the kind
label and asserted entities all count) refuses, naming the label and the entity count; a difference
touching only uncarried labels is accepted and the record updated. A refusal modifies nothing,
affects only that group, is cached on the group's ontology entry, and is enforced at
`add_episode` (`Error::IdentitySetChangeRefused`, JSON-RPC `-32003`) and reported in
`knowledge_status.group_identity_refusals`. `resolve_ontology` keeps its signature (34 callers).
Restoring the recorded set takes effect at the next start, since the ontology is cached per
process. If the sets differ but the DB or stamp cannot be read, the entry is not cached and the
check re-runs rather than silently accepting.

**D2 — the stamp is a sidecar, not a `WalPosition` column.** `.lcg/identity-set/<group>.json`
(`{"identity_types": [...]}`), written atomically, only when the set differs from the record — a
flagless workspace never creates one. A `WalPosition` column would be lost on `knowledge_clear_all`,
a DB reset, or a rebuild-from-WAL into a fresh directory, and an absent record reads as the empty
set — wrongly accepting a flip on a group whose entities already carry extracted identity kinds. A
sidecar survives all three, needs no schema migration and no `set_wal_position` signature change
(~90 call sites). `knowledge_clear_all` removes the stamps only when the WAL is deleted too
(`preserve_wal: false`). A stamp that exists but cannot be parsed is treated as unknown and blocks
the check, never as the empty set. Rejected: a DB column plus "unknown ⇒ refuse" (blocks legitimate
rebuilt groups).

**D3 — reprocess never changes kind.** The label rewriters already preserved kind (#615). Because a
reclassification beside an immutable kind leaves two independent leaves (`[Entity, Organization,
Person]`), `find_leaf_type_with_kind` and the restamp phase set the kind label (and its ancestors)
aside so such an entity is idempotent instead of re-planned on every run. `knowledge_reprocess_entity_types`
additionally reports `kind_disagreements` — report-only; merge-or-split is a human decision.

## Consequences

- Flagless ontologies and workspaces behave byte-for-byte as before, including WAL replay.
- The D1 check costs one group-scoped `labels` scan per changed label, only on first resolution
  when the sets differ.
- Flipping a flag on an ancestor type (e.g. `Document`) is refused while any descendant carries
  its label — conservative by design; the error names the label.
- Adding a flag changes the ontology content hash, so the ordinary #451 drift warning also fires;
  it is separate from, and weaker than, the D1 refusal.
- The remedy for a wanted change is re-ingesting the group from source; tooling for that and for
  merge-or-split of reported disagreements is out of scope.
