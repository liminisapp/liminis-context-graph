# Feature Specification: D1 identity-set guard detects identity removal when the group's stamp is missing

**Feature Branch**: `fabrik/issue-778`
**Created**: 2026-11-16
**Status**: Specified
**Input**: User description: "## Problem

Found while verifying 0.19.1, after #776. The D1 identity-set guard (ADR-0616, ADR-0776) can't see that identity was **removed** from a type once the group's stamp is missing.

`check_identity_set` (`crates/core/src/app_state.rs`) returns early when the recorded set equals the current set, and a missing stamp reads as `∅`. So once identity is removed from the group's only identity type, both sides are `∅`. The kind-aware carrier count never runs, and `add_episode` is accepted even though existing entities still have `kind = T`.

#776's stamp restore (fix A) covers this only on `knowledge_rebuild_from_wal`, not on a plain restart.

## Repro (0.19.1 release build)

1. Group `up` with an ontology where `Person` has `identity: true`. Ingest one Person entity (stored `kind = Person`), so the stamp is `[\"Person\"]`.
2. Delete `.lcg/identity-set/up.json`, and remove `identity: true` from `Person`.
3. Restart, then call `knowledge_reload_ontology` and `knowledge_add_episode` on `up`. **Both are accepted.**

Same scenario with the stamp present: refused with `-32003`, which is correct. Script: `~/dev/.scratch/lcg/2026-11-16-release-0.19.1/scripts/flip.py`.

0.19.0 behaves the same, so this is not a regression.

## Possible fixes

- **Restore from the sidecar outside rebuilds.** When the stamp is absent and the WAL ontology sidecar has `identity_types`, restore it on first resolve, not only after a rebuild.
- **Check data when the stamp is absent.** If any entity in the group has a `kind` that isn't `Entity` and isn't in the current identity set, treat it as a removal and run the carrier check.

## Acceptance

- The repro above is refused. Cover it both with a 0.19.1-written sidecar and with no sidecar (data-based detection).
- #775's orderings and #776's tests still pass."

## Background

The D1 guard (ADR-0616) protects a group's stored entity `kind` values from silently going stale when the set of identity-bearing types changes. It refuses `knowledge_reload_ontology` and `knowledge_add_episode` (error `-32003`) when an existing entity would be reinterpreted. ADR-0776 (#776) made the carrier count kind-aware and added a restore of the per-group stamp (`.lcg/identity-set/<group>.json`) from the WAL ontology sidecar, but only after `knowledge_rebuild_from_wal`.

The guard compares the stamp (the identity set last recorded for the group) against the current ontology's identity set. A missing stamp reads as the empty set. When identity is removed from the group's only identity-bearing type, the current set is also empty, so the two sets compare equal, the guard concludes "nothing changed", and the carrier check never runs. Existing entities still carry `kind = <removed type>`, yet the change is accepted. With the stamp present the same scenario is correctly refused. This is a pre-existing gap (0.19.0 behaves identically), not a regression, and is limited to the case where the stamp is absent (deleted, lost, or never written for a group whose data predates stamping, or a plain restart that does not go through rebuild).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Identity removal is refused when the stamp is missing and no sidecar exists (Priority: P1)

An operator removes `identity: true` from the only identity-bearing type of a group whose stamp is missing and for which no WAL ontology sidecar records identity types. Existing entities in the group have `kind` equal to that type.

**Why this priority**: This is the case that is detectable only from the stored data, and it is the one with no other safety net. Without it the guard can be bypassed by any group lacking a stamp and sidecar.

**Independent Test**: Ingest an entity of kind `Person` under an identity-bearing `Person`; delete the stamp and any sidecar identity record; remove `identity: true`; restart; call `knowledge_reload_ontology` and `knowledge_add_episode`. Both are refused with `-32003`.

**Acceptance Scenarios**:

1. **Given** a group with entity `kind = Person`, no stamp, no sidecar `identity_types`, and a current ontology with no identity-bearing types, **When** `knowledge_reload_ontology` is called, **Then** it is refused with `-32003`.
2. **Given** the same state, **When** `knowledge_add_episode` is called, **Then** it is refused with `-32003`.
3. **Given** a group whose entities all have `kind = Entity` (or no entities), no stamp, and a current ontology with no identity-bearing types, **When** either call is made, **Then** it is accepted (no false refusal).

---

### User Story 2 - Missing stamp is restored from the WAL ontology sidecar on a plain restart (Priority: P1)

A group has a 0.19.1-written WAL ontology sidecar recording `identity_types`, but its stamp is missing. After a plain restart (no rebuild), the stamp is restored from the sidecar on first resolve, so identity removal is detected by the normal set comparison.

**Why this priority**: It reuses the authoritative recorded set and makes the guard's behaviour consistent between rebuild and restart, so the repro is refused even for groups whose entities are not (or no longer) the evidence.

**Independent Test**: Run the issue's repro with the sidecar intact. After restart, `knowledge_reload_ontology` and `knowledge_add_episode` are both refused with `-32003`, and the stamp file is present again with the sidecar's set.

**Acceptance Scenarios**:

1. **Given** a missing stamp and a sidecar with `identity_types = ["Person"]` and a current ontology without identity on `Person`, **When** the service restarts and `knowledge_reload_ontology` is called, **Then** it is refused with `-32003`.
2. **Given** the same state, **When** `knowledge_add_episode` is called, **Then** it is refused with `-32003`.
3. **Given** a missing stamp and a sidecar whose `identity_types` equals the current identity set, **When** the group is first resolved, **Then** the stamp is restored and subsequent calls are accepted.

---

### User Story 3 - Existing guard behaviour is preserved (Priority: P1)

**Why this priority**: The fix must not regress the work in #775/#776.

**Independent Test**: The existing #775 hydrate-ordering, #776 stamp-restore, kind-aware identity, and sidecar identity-set tests pass unchanged.

**Acceptance Scenarios**:

1. **Given** the existing #775 and #776 test suites, **When** run after the change, **Then** all pass without modification of their expectations.
2. **Given** a group with the stamp present, **When** identity is removed from a type with carriers, **Then** it is still refused with `-32003`.
3. **Given** a group rebuilt from its WAL into a fresh root with an unchanged ontology, **When** its first `add_episode` is made, **Then** it is still accepted.

---

### Edge Cases

- Stamp missing, sidecar present but without `identity_types` (older writer): fall back to data-based detection.
- Stamp missing, sidecar `identity_types` disagrees with data (e.g. entities with kind `Person` but sidecar says `[]`): the guard must not accept a removal that the stored data shows would reinterpret existing entities.
- Stamp missing and entities carry kinds that are legitimately non-identity under the current ontology (e.g. the current identity set still contains that kind): not a removal; accepted.
- Stamp missing, group empty (no entities): nothing to protect; accepted.
- Stamp missing and a type was removed from the ontology entirely (not just its identity flag) while entities still carry its kind: treated as a removal and refused, consistent with the stamped behaviour.
- Group with a missing stamp where the current identity set is non-empty and differs from the sidecar: existing add/remove logic applies after restore.
- Restoring the stamp must not overwrite an existing stamp, and must not itself trigger a refusal on a group whose ontology is unchanged.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: When a group's stamp is absent and the WAL ontology sidecar records `identity_types`, the system MUST restore the stamp from the sidecar on first resolve of the group, on any startup path and not only after `knowledge_rebuild_from_wal`.
- **FR-002**: Once the stamp is restored per FR-001, the guard MUST compare it against the current identity set and apply the existing kind-aware carrier check, so removing identity from a type with carriers is refused with `-32003` on both `knowledge_reload_ontology` and `knowledge_add_episode`.
- **FR-003**: When the stamp is absent and no sidecar `identity_types` is available, the system MUST determine from the group's stored entities whether identity was removed: if any entity has a `kind` that is neither `Entity` nor in the current identity set, it MUST treat that as a removal and run the carrier check for that kind.
- **FR-004**: The data-based detection in FR-003 MUST NOT refuse a group whose entities all have `kind = Entity` or whose stored kinds are all in the current identity set, and MUST NOT refuse an empty group.
- **FR-005**: The data-based detection MUST be a bounded, group-scoped check, executed only when the stamp is absent, so that groups with a stamp incur no additional cost.
- **FR-006**: Restoring the stamp (FR-001) MUST NOT overwrite an existing stamp and MUST NOT cause a false refusal when the current ontology's identity set equals the restored set.
- **FR-007**: The refusal for the previously-accepted repro MUST use the same error code (`-32003`) and message shape as the existing stamped-case refusal.
- **FR-008**: The behaviour of the existing stamp-present path, the `knowledge_rebuild_from_wal` stamp restore (#776), and the #775 hydrate orderings MUST be unchanged.
- **FR-009**: Automated tests MUST cover the issue's repro (a) with a 0.19.1-written sidecar and (b) with no sidecar (data-based detection), for both `knowledge_reload_ontology` and `knowledge_add_episode`, plus the no-false-refusal cases in FR-004.

### Key Entities *(if applicable)*

- **Identity-set stamp**: per-group record (`.lcg/identity-set/<group>.json`) of the identity-bearing types last accepted for the group.
- **WAL ontology sidecar**: durable record of the ontology, including `identity_types`, written alongside the WAL.
- **Carrier**: an existing entity whose stored `kind` would differ under the new identity set (ADR-0776).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The issue's repro, run with the sidecar intact, results in both `knowledge_reload_ontology` and `knowledge_add_episode` being refused with `-32003`.
- **SC-002**: The same repro with the stamp and the sidecar identity record both removed is also refused with `-32003`.
- **SC-003**: No false refusals: groups with no entities, only `Entity`-kind entities, or kinds still in the identity set are accepted when the stamp is missing.
- **SC-004**: All pre-existing #775 and #776 tests pass unchanged.
- **SC-005**: Groups with a stamp present show no additional queries compared with the current behaviour.

## Assumptions

- A missing stamp with a sidecar recording `identity_types` is treated as authoritative for the previously accepted set, consistent with ADR-0776's rebuild restore.
- Where the sidecar and stored data disagree, stored data takes priority for refusal (never accept a change that would leave existing entities with a stale kind).
- Restoring or evaluating on first resolve of the group is acceptable; no eager scan of all groups at startup is required.
- The fix reuses the existing kind-aware carrier rule and refusal error; no new error code or protocol change.
- 0.19.0/0.19.1 behaviour is a pre-existing gap, so no data migration is needed.

## Out of Scope *(optional)*

- Changing the kind-aware carrier rule itself (ADR-0776) or the added-identity path.
- Rewriting or backfilling stored entity `kind` values.
- Writing stamps from WAL replay in general (beyond the restore on first resolve).
- Changes to the IPC protocol or MCP tool registry.

## Source References *(optional)*

- ADR-0616 (ontology-driven kind assignment, D1 guard), ADR-0776 (kind-aware identity-set check and stamp restore), ADR-0446 (WAL ontology sidecar)
- Issues #775, #776; repro script `~/dev/.scratch/lcg/2026-11-16-release-0.19.1/scripts/flip.py`
