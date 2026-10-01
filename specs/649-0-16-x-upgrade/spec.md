# Feature Specification: Rebuild FTS indexes built by lbug 0.20 on upgrade to 0.16.x

**Feature Branch**: `fabrik/issue-649`
**Created**: 2026-10-01
**Status**: Specified
**Input**: User description: "0.16.x upgrade: rebuild FTS indexes built by lbug 0.20 (non-ASCII terms undeletable + silently unsearchable) — fixes #646"

## Background

Implements the fix for #646 (community report). **This is a 0.16.x upgrade regression: every database built by lcg 0.15.x or earlier is affected after upgrading to 0.16.x.**

A full-text index written by **lbug 0.20** (lcg ≤ 0.15.x) can't be maintained by **lbug 0.21** (lcg 0.16.x) for terms containing **any non-ASCII character** (`→`, `—`, `–`, `é`, `ü`, `東京`, emoji). Pure-ASCII terms are fine. All three FTS indexes are affected: `edge_name_and_fact`, `node_name_and_summary` and `episode_content`. The storage version is 47 for both lbug versions, so nothing detects the change.

The problem was reproduced deterministically, on clean shutdowns only, using the published v0.15.0 and v0.16.2 binaries on a copy of a real 0.14-era notebook (evidence: `scratchpad/repro646/out/`, A1–S2):

1. **Writes fail.** Deleting or updating any row with an affected term fails with `FTS index '<idx>' is inconsistent: term '<t>' is missing during delete. Drop and recreate the FTS index.` That covers:
   - `knowledge_delete_episode`;
   - re-asserting an entity through `knowledge_assert_entity`;
   - edge `SET name` / `SET fact`;
   - `knowledge_rebuild_from_wal {from_seq: 0, force_clear: true}`. Its `clear_group_for_rebuild` → `purge_groups` deletes rows while the FTS indexes are still live, and `drop_fts_indexes` only runs later, inside the replay.

   On the demo notebook, 30 of 51 episodes, 15 of 246 edges and 1 of 198 entities carry affected terms.
2. **Search is silently wrong.** `QUERY_FTS_INDEX` for `→`, `Zürich` or `東京` against the 0.20-built index returns hits under 0.15 and **zero rows** under 0.16, with no error. ASCII queries are identical.
3. **No self-repair.** 0.16.x startup, `knowledge_build_indices` ("already exists" is swallowed) and `knowledge_recover_full` (`recovery_needed: false`) all leave the stale index in place.
4. **The error's advice is incomplete.** Dropping only the named index just moves the failure to the next one.

**Verified manual workaround:** run `CALL DROP_FTS_INDEX(...)` for all three indexes, then `knowledge_build_indices`. After that, replays complete (1,445 mutations, 0 failed) and non-ASCII searches return results again.

**Upstream:** reported to LadybugDB separately (lbug 0.20 → 0.21 changed how non-ASCII FTS terms are stored without a storage-version change). The upstream report is LadybugDB/ladybug#1092: a pure-lbug repro with the official 0.20.3 and 0.21.0 CLIs and official `fts` extensions on a 3-row table, which confirms that recreating the index under 0.21.0 fixes both symptoms. Its steps can be adapted for this issue's regression test.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Upgrading user gets a working FTS index automatically (Priority: P1)

A user with a database built by lcg 0.15.x or earlier upgrades to 0.16.3. On first start, the stale FTS indexes are detected and rebuilt once, so writes involving non-ASCII content succeed and non-ASCII search returns results again, with no manual steps.

**Why this priority**: Every pre-0.16 database is affected; without this, deletes, updates and full replays fail and non-ASCII search is silently empty.

**Independent Test**: Open a fixture database built by lcg 0.15.0 / lbug 0.20 containing non-ASCII terms in all three FTS-indexed fields, using the new build, then run the write and search operations below.

**Acceptance Scenarios**:

1. **Given** a database built by lcg 0.15.0 / lbug 0.20 with non-ASCII terms in all three FTS-indexed fields, **When** it is opened with the new build, **Then** the one-time FTS rebuild runs and is logged on stderr.
2. **Given** that rebuilt database, **When** a `→` edge, a `—` episode and a non-ASCII entity are deleted or updated, **Then** each operation succeeds.
3. **Given** that rebuilt database, **When** `knowledge_rebuild_from_wal {from_seq: 0, force_clear: true}` runs, **Then** it completes.
4. **Given** that rebuilt database, **When** `QUERY_FTS_INDEX` is run for `→`, `Zürich` and `東京`, **Then** the expected rows are returned.

---

### User Story 2 - Rebuild happens once and only when needed (Priority: P1)

The rebuild is one-time. Databases that already carry a matching marker, and fresh databases, never pay for it.

**Why this priority**: The rebuild time is proportional to corpus size, so repeating it or running it needlessly would be a regression of its own.

**Independent Test**: Open a fixture twice; open a freshly created database.

**Acceptance Scenarios**:

1. **Given** a database that has been rebuilt once, **When** it is opened a second time, **Then** no rebuild runs.
2. **Given** a database freshly created under 0.16.3+, **When** it is opened, **Then** no rebuild runs, because the marker was written at index creation.
3. **Given** a rebuild that is interrupted partway (for example a crash), **When** the database is next opened, **Then** the marker is unset and the rebuild runs again.

---

### User Story 3 - Residual inconsistency self-heals (Priority: P2)

If any operation still hits `FTS index … is inconsistent`, the system rebuilds all three FTS indexes and retries once rather than failing the host's operation outright.

**Why this priority**: A backstop for any stale index the marker does not catch (for example one touched by a pinned 0.16.0–0.16.2 build after the marker was written).

**Independent Test**: A test that forces the inconsistency error and checks that the retry succeeds.

**Acceptance Scenarios**:

1. **Given** an operation that raises the `FTS index … is inconsistent` error, **When** it runs, **Then** all three FTS indexes are rebuilt (not only the one named) and the operation is retried once and succeeds.
2. **Given** a backstop rebuild occurred, **When** the host inspects `knowledge_status` or the logs, **Then** the event is reported.

---

### User Story 4 - Full replay never deletes through a live FTS index (Priority: P2)

`knowledge_rebuild_from_wal` with `force_clear` drops (or first rebuilds) the FTS indexes before `purge_groups` deletes rows.

**Why this priority**: Today the purge deletes through live indexes and fails on affected terms before the replay's own `drop_fts_indexes` runs.

**Independent Test**: Run `knowledge_rebuild_from_wal {from_seq: 0, force_clear: true}` against a database whose FTS indexes still carry stale non-ASCII terms.

**Acceptance Scenarios**:

1. **Given** a database with stale FTS indexes, **When** a forced full rebuild from the WAL runs, **Then** it completes with no failed mutations.

---

### User Story 5 - Pinned and upgrading users know what to do (Priority: P3)

Users moving from 0.15 to 0.16, or pinned to 0.16.0–0.16.2, find an upgrade note explaining the one-time rebuild and the manual workaround.

**Why this priority**: Pinned users won't receive the automatic fix and need the manual steps.

**Independent Test**: Read the CHANGELOG/release notes for 0.16.3.

**Acceptance Scenarios**:

1. **Given** the 0.16.3 release notes, **When** a 0.15 → 0.16 user reads them, **Then** they state that the first start of 0.16.3 rebuilds the FTS indexes once and document the manual workaround for 0.16.0–0.16.2.

---

### Edge Cases

- Marker missing: treated as "rebuild", because every pre-fix database lacks the marker.
- Marker present but different from the running lbug version: rebuild.
- Crash midway through the rebuild: the marker is unset, so the next open rebuilds again.
- Rebuild must complete before any write path can delete through a stale index.
- Dropping only the index named in the error moves the failure to the next index, so all three are always rebuilt together.
- Pure-ASCII data: indexes are unaffected, but the one-time rebuild still runs for a database without the marker.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST persist which lbug version built the FTS indexes as a `SchemaState` marker (`crates/core/src/schema.rs`, following the #615 `entity_kind_lookup_key_v2` pattern). The key name is left to the spec; for example `fts_built_by_lbug`.
- **FR-002**: On open, if the marker is missing or differs from the running lbug version, the system MUST drop and recreate all three FTS indexes (`edge_name_and_fact`, `node_name_and_summary`, `episode_content`; statements at `schema.rs` ~654), then write the marker.
- **FR-003**: A missing marker MUST be treated as "rebuild".
- **FR-004**: The rebuild MUST be logged clearly on stderr, stating that it is one-time, that its time is proportional to corpus size, and that it is needed to recover non-ASCII search.
- **FR-005**: The rebuild MUST complete before any write path can delete through a stale index. Whether it runs synchronously at open or as part of `build_indices_once` / the startup index build (`app_state.rs:937`) is decided in Research.
- **FR-006**: If any operation hits `FTS index … is inconsistent`, the system MUST rebuild all three FTS indexes and retry the operation once, and MUST report the event in `knowledge_status` or the logs rather than failing the host's operation outright.
- **FR-007**: `knowledge_rebuild_from_wal` with `force_clear` MUST drop the FTS indexes before `purge_groups` deletes rows, or rebuild them first, so that a full replay never deletes through a live FTS index. This is to be verified against ADR-0030 and the replay batching design.
- **FR-008**: Fresh databases created under 0.16.3+ MUST write the marker at index creation and MUST NOT rebuild on open.
- **FR-009**: The rebuild MUST be idempotent and safe to interrupt; a crash midway MUST leave the marker unset so the next open rebuilds again.
- **FR-010**: The fix MUST NOT change the storage version or any data; only derived FTS indexes are rebuilt.
- **FR-011**: The CHANGELOG/release notes MUST include an upgrade note for 0.15 → 0.16 users stating that the first start of 0.16.3 rebuilds the FTS indexes once, and documenting the manual workaround (`CALL DROP_FTS_INDEX(...)` for all three indexes, then `knowledge_build_indices`) for anyone pinned to 0.16.0–0.16.2.

### Key Entities

- **FTS indexes**: `edge_name_and_fact`, `node_name_and_summary` and `episode_content`; derived data, safe to drop and recreate.
- **`SchemaState` marker**: records the lbug version that built the FTS indexes.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Regression fixture: a database built by lcg 0.15.0 / lbug 0.20 with non-ASCII terms in all three FTS-indexed fields (following the `storage_v42_db` fixture pattern), once opened with the new build, supports deleting or updating a `→` edge, a `—` episode and a non-ASCII entity; `knowledge_rebuild_from_wal {from_seq: 0, force_clear: true}` completes; and `QUERY_FTS_INDEX` for `→`, `Zürich` and `東京` returns the expected rows.
- **SC-002**: A second open of a rebuilt database does not rebuild.
- **SC-003**: Fresh databases never trigger the rebuild.
- **SC-004**: A test that forces the inconsistency error shows the backstop rebuilds and the retry succeeds.
- **SC-005**: The upgrade note for 0.15 → 0.16 users and the pinned-user workaround are present in the CHANGELOG/release notes.
- **SC-006**: The change ships as patch release 0.16.3.

## Assumptions

- The upstream behaviour change (lbug 0.20 → 0.21 storing non-ASCII FTS terms differently without a storage-version bump) is outside this repo's control; recreating the index under 0.21 fixes it (confirmed by LadybugDB/ladybug#1092).
- The upstream repro steps in LadybugDB/ladybug#1092 can be adapted for the regression test.
- The change is patch-sized (0.16.3).

## Out of Scope

- Fixing the underlying behaviour in lbug itself (tracked upstream in LadybugDB/ladybug#1092).
- Any storage-version change or data migration.

## Source References

- #646 (community report)
- LadybugDB/ladybug#1092 (upstream report)
- `scratchpad/repro646/out/` (repro evidence, A1–S2)
- `crates/core/src/schema.rs` (~654 FTS statements; #615 `entity_kind_lookup_key_v2` pattern), `app_state.rs:937` (`build_indices_once`)
- ADR-0030 (replay batching design)
