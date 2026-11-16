# Feature Specification: D1 identity-set check — kind-aware carrier count and stamp restore on hydrate

**Feature Branch**: `fabrik/issue-776`
**Created**: 2026-11-16
**Status**: Specified
**Input**: User description: "## Problem

Community report: #775. Target release: **0.19.1** (patch).

A group rebuilt from its WAL into a **fresh workspace root** is locked by the D1 identity-set guard (#616, ADR-0616) on its first `add_episode`. The ontology's identity-bearing set hasn't changed. From then on every `add_episode` for the group fails with -32003 (`IdentitySetChangeRefused`), and `knowledge_reload_ontology` returns the same refusal.

Root cause, verified on `main` (c8dce25f):
- The stamp `{workspace_root}/.lcg/identity-set/<group>.json` is written only by `AppState::check_identity_set` (`crates/core/src/app_state.rs` ~L465–510), never by WAL replay. A host that keeps only the WAL (an ephemeral container, or a fresh clone that hydrates from a git-hosted WAL) has no stamp, so the recorded set reads as `{}`.
- Replay recreates entities whose stored `e.kind` already equals the identity type. The first resolve therefore sees every identity type as *added*.
- `Db::count_entities_carrying_label` (`crates/core/src/db.rs` ~L2560) counts every entity with `$label IN e.labels`. It never asks whether any entity's `kind` would actually change, so the \"change\" is refused.
- The doc comment in `crates/core/src/identity_stamp.rs` (L10–14) claims the stamp survives \"a rebuild-from-WAL into a fresh directory\". That holds only when the workspace root persists.
- The purge handler comment (`handlers.rs` ~L2878) already describes this failure mode: identity-kind entities with no stamp mean \"the next restart would refuse the group though its ontology never changed\".

## Fix

### B (required): make the D1 check kind-aware

Count only carriers whose interpretation would actually change:
- **Adding** `identity` to type T: refuse only if some entity in the group carries label T but has `kind` ≠ T. That is, it was created when T was not identity-bearing.
- **Removing** `identity` from type T: refuse only if some entity has `kind == T`.

If the sets differ and nothing would be reinterpreted, accept and write the stamp, as the accepted path does today. A missing stamp with consistent data then passes and self-heals. A real flag flip on an established group is still refused.

Specify/Plan must pin down the exact predicate in the presence of the type hierarchy. An entity of an identity subtype carries its ancestors' labels, including T, while its `kind` is the subtype. Cases to resolve:
- T and its subtype S are both identity-bearing;
- identity is added to T while S already has it;
- an entity whose `kind` is a non-identity default (`Entity` / NULL / the extractor's fallback).

The rule is: an entity counts as a carrier only if its `kind` would differ had it been extracted under the new set. Write that rule down in ADR-0616 (amend it, or add a short follow-up ADR) and test each case.

### A (required): restore the stamp on hydrate

When a group is replayed from the WAL and has no stamp, restore it from the WAL. Preferred: record the identity-bearing set in the WAL ontology sidecar (`crates/core/src/ontology_sidecar.rs`, `.wal-ontology.json`, which already records the ontology hash) at stamp-write time, and write the stamp from it during rebuild/hydrate if it's absent. Never overwrite an existing stamp. Older sidecars without the field are fine: with no field there's nothing to restore, and B covers the case.

If A turns out to be disproportionate (sidecar format or compatibility concerns), Plan may propose deferring it to a follow-up issue. B is the part that fixes #775.

## Acceptance criteria

1. **Integration test reproducing #775.** Ingest into group G, whose ontology has an `identity: true` type with entities of that type. Copy only the WAL into a new, empty workspace root (no `.lcg/identity-set`). Start, hydrate/rebuild, then call `add_episode` for G. It must succeed, and the stamp must exist afterwards. Cover all three orderings from #775's table: hydrate → reload; install → hydrate → add_episode; install → reload → hydrate → add_episode.
2. **A genuine flip is still refused.** On an established group (stamp present, entities with identity kinds), adding identity to a type whose existing entities have a different kind is refused. So is removing identity from a type that existing entities have as their kind. The existing `reload_ontology.rs` US2 tests and `identity_kinds.rs` keep passing.
3. **A flip with the stamp lost is still refused.** Same as 2 but with the stamp deleted: it's refused when data is inconsistent with the new set, and accepted when it's consistent.
4. Subtype/ancestor cases from the B section have tests.
5. With A: after a rebuild into a fresh root, the stamp equals the set recorded in the sidecar, and an existing stamp is never overwritten.
6. Correct the doc comments in `identity_stamp.rs` and `count_entities_carrying_label`.
7. CHANGELOG `Unreleased` entry under Fixed that references #775.

## Out of scope

- Changing how `kind` is assigned at extraction, or re-deriving kinds.
- Any storage-version bump or lbug change (0.19.1 stays on lbug 0.21.2, storage 47).
- The lbug 0.22 upgrade (#718/#750).

## Constraints

- Keep the carrier count a single group-scoped query per changed label, run only when the sets differ, as today.
- The no-`workspace_root` path stays a no-op."

## Background

The D1 identity-set guard (#616, ADR-0616) exists to stop a group's identity-bearing type set from changing underneath entities that were created under a different set. It compares the resolved set against a per-group recorded stamp and refuses (`-32003`, `IdentitySetChangeRefused`) when the difference touches any label carried by an existing entity.

Community report #775: a group rebuilt from its WAL into a fresh workspace root is refused on its first `add_episode`, although the ontology never changed, and stays refused (`knowledge_reload_ontology` returns the same refusal). Two defects combine:

1. **Missing stamp after WAL-only hydrate.** The stamp is written only when the guard accepts a changed set — never by WAL replay. A host that persists only the WAL (ephemeral container, fresh clone hydrating from a git-hosted WAL) has no stamp, so the recorded set reads as empty and every identity type looks newly *added*.
2. **Carrier count ignores `kind`.** The check counts every entity carrying the label, without asking whether any entity's `kind` would actually change. Replayed entities already have `kind` equal to the identity type, so nothing would be reinterpreted, yet the change is refused.

ADR-0616 (D2) and the doc comment in the stamp module claim the stamp survives "a rebuild-from-WAL into a fresh directory"; this holds only when the workspace root persists. The purge handler already documents the failure mode ("the next restart would refuse the group though its ontology never changed").

Fix B (kind-aware check) is what resolves #775. Fix A (restoring the stamp on hydrate) makes the recorded set durable alongside the WAL so the guard keeps its protective value on rebuilt groups. Target release: 0.19.1 (patch).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A group rebuilt from its WAL accepts new episodes (Priority: P1)

An operator hydrates a group into a fresh workspace root from only its WAL (no stamp present). The ontology's identity-bearing set is unchanged and existing entities already have the matching kinds. Ingestion and ontology reload must work and the group must self-heal its stamp.

**Why this priority**: This is the reported outage — a rebuilt group is permanently unwritable.

**Independent Test**: Ingest into group G with an `identity: true` type and entities of that type; copy only the WAL into an empty workspace root; hydrate; call `add_episode` for G. It succeeds and the stamp now exists.

**Acceptance Scenarios**:

1. **Given** a WAL-only fresh root with consistent identity-kind entities, **When** the ontology is resolved then `knowledge_reload_ontology` is called (hydrate → reload), **Then** no refusal occurs and the stamp is written.
2. **Given** the same state, **When** the ontology is installed, the group hydrated, then `add_episode` is called (install → hydrate → add_episode), **Then** the episode is accepted and the stamp exists afterwards.
3. **Given** the same state, **When** the ontology is installed, reloaded, the group hydrated, then `add_episode` is called (install → reload → hydrate → add_episode), **Then** the episode is accepted and the stamp exists afterwards.

---

### User Story 2 - A genuine identity flip on an established group is still refused (Priority: P1)

The guard must keep protecting against real reinterpretation: adding or removing `identity` on a type where existing entities would be interpreted differently remains refused, whether or not the stamp is present.

**Why this priority**: Loosening the guard must not reopen the duplicate/mis-split hazard that #616 closed.

**Independent Test**: On a group with entities, flip the flag in each direction, with the stamp present and with the stamp deleted.

**Acceptance Scenarios**:

1. **Given** an established group (stamp present) with entities of type T whose `kind` ≠ T, **When** `identity` is added to T, **Then** the change is refused naming the label and the count of affected entities.
2. **Given** an established group with entities whose `kind` is T, **When** `identity` is removed from T, **Then** the change is refused.
3. **Given** the stamp has been deleted, **When** the same flips are attempted, **Then** each is refused if existing data is inconsistent with the new set, and accepted (stamp written) if consistent.
4. **Given** the existing US2 tests in `reload_ontology.rs` and the tests in `identity_kinds.rs`, **When** the suite runs, **Then** they still pass.

---

### User Story 3 - The identity-bearing set is restored with the WAL (Priority: P2)

When a group is rebuilt from its WAL and no stamp exists, the recorded identity-bearing set travels with the WAL (in the WAL ontology sidecar) and is restored as the stamp, so the guard on a rebuilt group has the same baseline as the original.

**Why this priority**: Fix B alone resolves #775; restoring the stamp preserves the guard's full strength (e.g. detecting a flip made while the stamp was lost) but is secondary and may be deferred if disproportionate.

**Independent Test**: Rebuild a group into a fresh root from a WAL whose sidecar records an identity set; verify the stamp equals the recorded set. Repeat with a pre-existing, different stamp and verify it is untouched. Repeat with an older sidecar lacking the field and verify nothing is restored and no error occurs.

**Acceptance Scenarios**:

1. **Given** a sidecar recording identity set S and no stamp, **When** the group is rebuilt/hydrated, **Then** the stamp is written equal to S.
2. **Given** an existing stamp, **When** the group is rebuilt/hydrated, **Then** the stamp is never overwritten.
3. **Given** an older sidecar without the identity-set field, **When** the group is rebuilt, **Then** nothing is restored and no error occurs.

---

### Edge Cases

- **Subtype and ancestor labels**: an entity of an identity subtype S carries ancestor labels (including T) while its `kind` is S. Adding identity to T when S is already identity-bearing must not count such entities as carriers, since their `kind` would not change. Where T and S are both identity-bearing, entities already have a kind matching what extraction under the set would assign.
- **Identity added to T while S already has it**: entities of S are unaffected; entities that carry T but whose `kind` would differ under the new set (e.g. an entity directly typed T with `kind` `Entity`) are carriers.
- **Non-identity default kind** (`Entity`, NULL, or the extractor's fallback): an entity carrying T with such a kind is a carrier when adding identity to T (its kind would change); it is not a carrier when removing identity from T.
- **Removing identity from a type with subtype entities**: the kind-based rule applies to entities whose `kind` equals the removed type; entities with a subtype kind are not carriers of the removed type.
- **Mixed groups**: one changed label with no reinterpreted entities and another with some — the group is refused naming only the label(s) that are genuinely carried.
- **Stamp exists but is unparseable**: still treated as unknown and blocks the check, never as the empty set (unchanged).
- **DB or stamp unreadable while sets differ**: the entry is not cached and the check re-runs (unchanged).
- **No `workspace_root`**: the guard remains a no-op.
- **Sets equal**: no carrier query runs (unchanged).
- **Already-refused group** after this fix: a group refused under the old logic is accepted on the next start/reload if the data is consistent.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: When the resolved identity-bearing set differs from the recorded set, the D1 check MUST count a label as carried only by entities whose `kind` would differ had they been extracted under the new set; the label MUST NOT be refused when no such entity exists.
- **FR-002**: For a label **added** to the identity set, an entity MUST count as a carrier only if it carries that label (including via subtype ancestry) and its stored `kind` differs from the kind it would receive under the new set.
- **FR-003**: For a label **removed** from the identity set, an entity MUST count as a carrier only if its stored `kind` equals that label.
- **FR-004**: The predicate MUST be consistent with the extractor's existing kind-assignment behavior for entities that carry multiple identity-bearing labels (T and subtype S both identity-bearing), including entities whose `kind` is a non-identity default (`Entity`, NULL, or the extractor's fallback); the exact rule MUST be documented and each case tested.
- **FR-005**: When the sets differ and no entity would be reinterpreted, the check MUST accept the change, write the stamp as the accepted path does today, and leave the group fully writable (no refusal at `add_episode` or `knowledge_reload_ontology`).
- **FR-006**: A genuine flip (at least one entity that would be reinterpreted) MUST still refuse with `IdentitySetChangeRefused` (`-32003`), naming the label and the number of affected entities, regardless of whether the stamp is present.
- **FR-007**: The refusal's scope and reporting MUST be unchanged: it modifies nothing, affects only the one group, is cached on the group's ontology entry, and is reported in `knowledge_status.group_identity_refusals`.
- **FR-008**: The carrier count MUST remain a single group-scoped query per changed label, run only when the sets differ.
- **FR-009**: With no `workspace_root`, the guard MUST remain a no-op.
- **FR-010**: The WAL ontology sidecar MUST record the group's identity-bearing set when the stamp is written, alongside the existing ontology hash.
- **FR-011**: When a group is rebuilt/hydrated from the WAL and no stamp exists, the stamp MUST be written from the set recorded in the sidecar.
- **FR-012**: An existing stamp MUST never be overwritten by restore.
- **FR-013**: A sidecar without the identity-set field MUST be accepted without error and restore nothing; the kind-aware check (FR-001–FR-005) covers that case.
- **FR-014**: Behavior MUST be correct under all three orderings: hydrate → reload; install → hydrate → add_episode; install → reload → hydrate → add_episode.
- **FR-015**: ADR-0616 MUST be amended (or a short follow-up ADR added, named per the repo's issue-number rule) to record the kind-aware carrier rule, including the type-hierarchy cases, and to correct D2's claim that the stamp survives a rebuild into a fresh directory. The ADR's current consequence "flipping a flag on an ancestor type is refused while any descendant carries its label" MUST be updated to reflect the kind-aware rule.
- **FR-016**: The doc comments in the stamp module and on the carrier-count function MUST be corrected to describe actual behavior (the stamp survives only when the workspace root persists; the count is kind-aware).
- **FR-017**: `CHANGELOG.md` `Unreleased` MUST gain an entry under **Fixed** referencing #775.
- **FR-018**: If restoring the stamp from the WAL (FR-010–FR-013) proves disproportionate during Plan (sidecar format or compatibility concerns), it MAY be deferred to a follow-up issue; FR-001–FR-009, FR-014 and FR-016–FR-017 are required either way.

### Key Entities

- **Identity-bearing set**: the set of entity types flagged `identity: true` for a group; it determines which `kind` an extracted entity receives.
- **Identity-set stamp**: per-group record of the last accepted identity-bearing set, stored under the workspace root; absent reads as the empty set.
- **WAL ontology sidecar**: per-WAL record that already holds the ontology hash and is to additionally hold the identity-bearing set.
- **Carrier**: an existing entity whose interpretation (its `kind`) would change under the new identity-bearing set.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In an integration test, a group rebuilt from only its WAL into an empty workspace root accepts `add_episode` and writes its stamp, in all three orderings listed in FR-014.
- **SC-002**: Adding identity to a type with at least one entity of a different kind, and removing identity from a type with at least one entity of that kind, are each refused with `-32003`, both with the stamp present and with it deleted.
- **SC-003**: With the stamp deleted and data consistent with the resolved set, the check accepts and the stamp is recreated; zero refusals for an unchanged ontology.
- **SC-004**: Each subtype/ancestor case in the Edge Cases (T and S both identity-bearing; identity added to T while S already has it; non-identity default kind) has a passing test.
- **SC-005**: After a rebuild into a fresh root (if restore is in scope), the stamp equals the sidecar's recorded set, an existing stamp is byte-for-byte unchanged, and an older sidecar rebuilds without error.
- **SC-006**: Existing tests in `reload_ontology.rs` (US2) and `identity_kinds.rs` pass unchanged, and the carrier count remains one query per changed label.
- **SC-007**: No storage-version or lbug change ships: 0.19.1 remains on lbug 0.21.2 and storage 47.

## Assumptions

- The kind a given entity "would receive under the new set" follows the extractor's existing kind-assignment logic (ADR-0616, including its handling of multiple identity-bearing labels); this spec does not change that logic. Research/Plan will confirm the exact rule from the code and encode it in the predicate.
- Entities replayed from the WAL retain their stored `kind` verbatim (replay never re-runs the extractor), which is why a consistent group appears consistent to the kind-aware check.
- A missing stamp is still read as the empty set; the kind-aware count, not a change to that reading, is what prevents false refusals.
- Adding an optional field to the sidecar is backward-compatible (older readers ignore it; newer readers tolerate its absence).
- The existing remedy for a genuinely wanted flip (re-ingest the group from source) is unchanged.

## Out of Scope

- Changing how `kind` is assigned at extraction, or re-deriving kinds for existing entities.
- Any storage-version bump or lbug change (0.19.1 stays on lbug 0.21.2, storage 47).
- The lbug 0.22 upgrade (#718/#750).
- Tooling for merge-or-split of kind disagreements.

## Source References

- Community report #775; ADR-0616 (D1/D2), ADR-0627 (refusal lifecycle)
- Issue #616 (identity-bearing types), #615 (kind as identity)
