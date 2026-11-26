# Feature Specification: Bump lbug 0.21.2 → 0.23.0 to fix Windows CHECKPOINT corruption

**Feature Branch**: `fabrik/issue-930`
**Created**: 2026-11-26
**Status**: Specified
**Input**: User description: "Fixes #719. Supersedes Dependabot #750 (0.21.2 → 0.22.3, which does not contain the fix).

## Problem
On Windows, lbug's CHECKPOINT can silently corrupt some STRING and INT8/UINT8 column data (#719, LadybugDB/ladybug#1447). MSVC's x64 AVX2 vectorized `std::minmax_element` returns a wrong min/max for some byte arrays, so CHECKPOINT wrongly stores a column chunk as CONSTANT. The prebuilt lbug 0.21.2 bundle that Windows builds link is affected. macOS and Linux are not.

The upstream fix is LadybugDB/ladybug#1593: it avoids the MSVC vectorized minmax and guards CONSTANT with a bitwise check. It first shipped in **lbug 0.23.0** (released 2026-11-25; crate published on crates.io; Windows x86_64/arm64 bundles in the release; extensions on the CDN at `v0.23.0/`). The 0.22.x line does not contain it.

## Change
Move lcg from lbug `=0.21.2` to `=0.23.0`, following the #699 / #619 bump pattern:
- **Pins:** the `lbug` crate pin in `Cargo.toml` and `LBUG_VERSION` in `.cargo/config.toml` (they move together; see CLAUDE.md). Also update `LBUG_CRATE_VERSION_VERIFIED_AGAINST` and the version-to-directory notes in `crates/core/src/lbug_extension_home.rs`, the README dependency table, and any other `0.21.2` references outside the CHANGELOG and historical docs.
- **Extensions:** `LBUG_EXTENSION_VERSION` goes from `0.21.0` to `0.23.0`. The 0.23.0 CMake build defines `LBUG_EXTENSION_VERSION=\"0.23.0\"`, so the CDN directory is `v0.23.0/`; confirm this at runtime. Re-stage the extensions with `scripts/stage-lbug-extensions.sh` and re-pin `LBUG_EXTENSION_HASHES` for every platform. Add an ADR-0593 addendum and an ADR-0559 note.
- **Storage format:** lbug 0.23.0 is storage version **53** (0.21.2 is 47; 0.22.x is 49). Existing user databases must open and migrate correctly. Add a storage-v47 migration test in the style of `crates/core/tests/storage_v42_migration.rs`, using a fixture built by the current 0.21.2 binary. Confirm that the FTS-rebuild-on-lbug-version-change path (ADR-0649) fires as intended and that the HNSW/vector indexes survive.
- **Upstream behaviour changes to audit:** read the v0.23.0 release notes and check lcg against at least these:
  - `debug_enable_multi_writes` is renamed to `enable_multi_writes` and now defaults to true (#1435).
  - CHECKPOINT is now treated as a write, so read-only databases refuse it (#1459).
  - Several transaction, checkpoint and WAL changes (#1361, #1358, #1375, #1507, #1516, #1520).
  - Windows path changes (#1496, #1534).
  - The `lbug_cli` and C API changes, if lcg uses them.

  Adapt the code or pin the old behaviour wherever lcg depends on the previous semantics. Record the decisions.

## Acceptance
- On macOS: the full suite passes and the real-corpus and upgrade tests are green.
- Windows CI (`.github/workflows/windows.yml`) is green.
- Run on a real Windows machine: the #719 repro passes. Write STRING and INT8/UINT8 data that previously collapsed to CONSTANT, CHECKPOINT, reopen, and every value must read back exactly. Row-asserting vector and FTS pairing runs pass, as in #619.
- A database created by lcg on lbug 0.21.2 (storage 47) opens under the new binary, migrates, and serves correct reads and searches, including FTS with non-ASCII terms and vector search.
- CHANGELOG: #719 is removed from Known issues and noted as fixed. The storage-version change is stated. `llms-full.txt` and docs are updated where they name the lbug version.
- Close Dependabot #750 as superseded once this merges."

## Background

On Windows, lbug's CHECKPOINT can silently corrupt some STRING and INT8/UINT8 column data (#719, upstream LadybugDB/ladybug#1447). The cause is MSVC's x64 AVX2 vectorized `std::minmax_element` returning a wrong min/max for some byte arrays, so CHECKPOINT wrongly stores a column chunk as CONSTANT and values are lost without any error. The prebuilt lbug 0.21.2 bundle that Windows builds link is affected; macOS and Linux are not.

The upstream fix (LadybugDB/ladybug#1593) avoids the MSVC vectorized minmax and guards CONSTANT detection with a bitwise check. It first shipped in lbug 0.23.0 (released 2026-11-25), which has a published crate, Windows x86_64/arm64 bundles, and extensions on the CDN under `v0.23.0/`. The 0.22.x line does not contain the fix, so Dependabot's #750 (0.21.2 → 0.22.3) is not sufficient and is superseded by this work.

The 0.21.2 → 0.23.0 jump crosses several storage-format versions (47 → 53; 0.22.x is 49), and the release carries upstream behaviour changes (multi-writes default, CHECKPOINT-as-write, transaction/checkpoint/WAL changes, Windows path handling, CLI/C API changes) that lcg may depend on. The bump therefore needs an audit and an upgrade-path proof, not only a version change. It follows the pattern of the earlier lbug bumps (#699, #619).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Windows data survives CHECKPOINT (Priority: P1)

A Windows user of lcg writes STRING and INT8/UINT8 data, the database checkpoints, and the service later reopens. Every value reads back exactly as written, with no silent corruption.

**Why this priority**: This is the reason for the bump. Silent data loss is the most severe class of defect for a knowledge store, and it affects every Windows deployment.

**Independent Test**: On a real Windows machine, run the #719 repro: write STRING and INT8/UINT8 data that previously collapsed to CONSTANT, CHECKPOINT, reopen, and compare every value to what was written.

**Acceptance Scenarios**:

1. **Given** a Windows build linked against lbug 0.23.0 and data that previously triggered the CONSTANT mis-detection, **When** CHECKPOINT runs and the database is reopened, **Then** every STRING and INT8/UINT8 value reads back exactly as written.
2. **Given** the same Windows build, **When** the row-asserting vector and FTS pairing runs (as done for #619) execute, **Then** they pass.
3. **Given** the Windows CI workflow, **When** it runs on the bump branch, **Then** it is green.

---

### User Story 2 - Existing databases upgrade safely (Priority: P1)

A user whose database was created by lcg on lbug 0.21.2 (storage version 47) upgrades to the new binary. The database opens, migrates to storage version 53, and serves correct reads and searches, including FTS with non-ASCII terms and vector search.

**Why this priority**: Every existing user hits this path on upgrade. A failed or lossy migration is as damaging as the bug being fixed.

**Independent Test**: Build a fixture database with the current 0.21.2 binary, check it in, and run a migration test (in the style of `storage_v42_migration.rs`) under the new binary that opens it and asserts content, FTS and vector search results.

**Acceptance Scenarios**:

1. **Given** a storage-v47 database fixture produced by the 0.21.2 binary, **When** the 0.23.0 build opens it, **Then** it opens, migrates, and every previously stored entity, episode and relationship reads back correctly.
2. **Given** the migrated database, **When** an FTS query with non-ASCII terms is run, **Then** it returns the expected rows.
3. **Given** the migrated database, **When** a vector search is run, **Then** the HNSW/vector indexes are intact and return the expected rows.
4. **Given** the lbug version differs from the one that built the FTS index, **When** the database opens, **Then** the FTS-rebuild-on-version-change path (ADR-0649) fires as intended.

---

### User Story 3 - Upstream behaviour changes are audited and recorded (Priority: P2)

A maintainer reviews the v0.23.0 release notes against lcg's use of lbug and has a recorded decision for each behaviour change that could affect lcg, either adapting the code or pinning the previous behaviour.

**Why this priority**: Silent semantic drift (for example a new multi-writes default, or CHECKPOINT refused on read-only databases) could cause regressions that tests do not catch. It is secondary to the fix and the migration itself, but must land in the same change.

**Independent Test**: The audit record covers each listed change (#1435, #1459, #1361, #1358, #1375, #1507, #1516, #1520, #1496, #1534, plus CLI/C API changes if used), with a decision and rationale for each. Where lcg relied on old semantics, a test or configuration pins the intended behaviour.

**Acceptance Scenarios**:

1. **Given** the v0.23.0 release notes, **When** the audit is complete, **Then** each listed change has a recorded decision (adapted, pinned, or not applicable) with its reason.
2. **Given** lcg opening a read-only database, **When** it attempts CHECKPOINT, **Then** the behaviour is the intended one under the new "CHECKPOINT is a write" rule.
3. **Given** the multi-writes setting rename and new default, **When** lcg configures lbug, **Then** it uses the new setting name and the chosen value is documented.

---

### User Story 4 - Extensions and pins are consistent with 0.23.0 (Priority: P2)

Builds fetch, stage and verify the lbug extensions for 0.23.0, and every version pin in the repository agrees.

**Why this priority**: A mismatch between the crate, the native bundle and the extension binaries breaks builds or runtime extension loading (the v0.9.0 release was broken by exactly this class of skew).

**Independent Test**: Run `scripts/stage-lbug-extensions.sh` for every platform, confirm the hashes are pinned, confirm extensions load at runtime from the `v0.23.0/` directory, and grep the repository for stale `0.21.2` references.

**Acceptance Scenarios**:

1. **Given** the bumped pins, **When** the build runs, **Then** the `lbug` crate pin and `LBUG_VERSION` are both `0.23.0` and the extension version constant is `0.23.0`.
2. **Given** the staging script, **When** it runs for every supported platform, **Then** `LBUG_EXTENSION_HASHES` has a verified hash for each platform's 0.23.0 extension bytes.
3. **Given** a running service, **When** it loads extensions, **Then** it finds them under the `v0.23.0` directory (confirmed at runtime, not assumed).

---

### Edge Cases

- A database already migrated by a 0.22.x build (storage 49) is opened by the new binary; this is not a supported lcg state but must not corrupt data.
- A database that was last checkpointed on Windows under 0.21.2 may already contain silently corrupted CONSTANT chunks. The bump prevents new corruption but does not repair existing damage; this must be stated in release notes.
- A read-only or degraded-mode open where lcg or its recovery path issues CHECKPOINT (or WAL-drop and reopen) now behaves differently because CHECKPOINT is a write.
- WAL replay and recovery (`knowledge_recover`, resume from the episode cursor) behave correctly against the changed WAL/transaction semantics.
- Windows path handling changes (#1496, #1534) affect workspace paths containing spaces, drive letters, UNC paths or non-ASCII characters.
- Cross-version skew: the crate pin, `LBUG_VERSION` and the native bundle disagree. This must remain impossible to ship.
- A new binary opens a database that cannot be downgraded afterward (storage 53 is not readable by 0.21.2); users need to be told.
- The FTS index is stale or missing after migration; the rebuild path must not leave search silently empty.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The `lbug` crate pin is `=0.23.0`, and `LBUG_VERSION` in the cargo config is `0.23.0`; the two always agree.
- **FR-002**: Every other reference to the old version outside the CHANGELOG and historical docs (including the verified-against crate-version constant, the version-to-directory notes, the README dependency table, and documentation that names the lbug version) is updated to 0.23.0.
- **FR-003**: The lbug extension version is `0.23.0`, extensions are staged for every supported platform with `scripts/stage-lbug-extensions.sh`, and the pinned extension hashes cover every platform.
- **FR-004**: Extension loading at runtime resolves to the `v0.23.0` directory, verified by an actual run rather than by assumption.
- **FR-005**: An ADR-0593 addendum and an ADR-0559 note record the extension version/hash change, and an ADR (named per the repository's issue-number convention) or equivalent record documents the audit decisions from FR-009.
- **FR-006**: A storage-v47 migration test, in the style of `storage_v42_migration.rs` and using a fixture built by the current 0.21.2 binary, proves that a 0.21.2 database opens under 0.23.0, migrates, and returns correct reads.
- **FR-007**: The migration test also asserts that FTS search (including non-ASCII terms) and vector search return correct rows on the migrated database, and that HNSW/vector indexes survive.
- **FR-008**: The FTS-rebuild-on-lbug-version-change path (ADR-0649) is confirmed to fire on the 0.21.2 → 0.23.0 change, with a test or recorded evidence.
- **FR-009**: The v0.23.0 release notes are audited against lcg for at least: multi-writes rename and default (#1435), CHECKPOINT as a write and read-only refusal (#1459), transaction/checkpoint/WAL changes (#1361, #1358, #1375, #1507, #1516, #1520), Windows path changes (#1496, #1534), and any `lbug_cli` or C API changes lcg uses. A decision is recorded for each.
- **FR-010**: Wherever lcg depended on previous semantics, the code is adapted or the old behaviour is pinned explicitly, and the multi-writes setting uses the new name with a documented value.
- **FR-011**: A Windows regression check reproduces #719 (STRING and INT8/UINT8 data that previously collapsed to CONSTANT, CHECKPOINT, reopen, exact read-back) and is run on a real Windows machine; the row-asserting vector and FTS pairing runs also pass there.
- **FR-012**: The existing suite passes on macOS, including the real-corpus and upgrade tests, and Windows CI (`.github/workflows/windows.yml`) is green.
- **FR-013**: The CHANGELOG entry is added as a fragment under `changelog.d/` (not by editing `CHANGELOG.md`). It notes that #719 is fixed, removes #719 from Known issues, states the storage-version change (47 → 53) and that downgrade is not supported, and notes that databases already corrupted on Windows under 0.21.2 are not repaired.
- **FR-014**: `llms-full.txt` and docs that name the lbug version are updated where they are sourced (the generated `docs/llms-full.txt` is not committed).
- **FR-015**: After this change merges, Dependabot PR #750 is closed as superseded.

### Key Entities

- **lbug pin set**: the crate pin, `LBUG_VERSION`, the verified-against constant, the extension version and the extension hash file; these move together.
- **Storage version**: the on-disk format version of a database (47 for 0.21.2, 49 for 0.22.x, 53 for 0.23.0).
- **Migration fixture**: a database produced by the 0.21.2 binary and used to prove the upgrade path.
- **Audit record**: the written decision for each upstream behaviour change in the 0.23.0 release.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On a real Windows machine, 100% of STRING and INT8/UINT8 values in the #719 repro read back exactly after CHECKPOINT and reopen.
- **SC-002**: The full test suite passes on macOS with zero failures, including the real-corpus and upgrade tests.
- **SC-003**: The Windows CI workflow is green on the bump branch.
- **SC-004**: A database created on lbug 0.21.2 (storage 47) opens under the new binary and loses zero entities, episodes or relationships; FTS (including non-ASCII terms) and vector searches return the expected results.
- **SC-005**: A repository search finds no remaining stale `0.21.2` references outside the CHANGELOG, fragments and historical docs, and the crate pin, `LBUG_VERSION` and extension version all read 0.23.0.
- **SC-006**: Every upstream change listed in FR-009 has a recorded decision.
- **SC-007**: Release notes state that #719 is fixed, the storage-version change, and that downgrade is unsupported.

## Assumptions

- The pinned version is exactly `0.23.0`, not a later patch; the 0.22.x line is skipped because it lacks the fix.
- The CDN directory for 0.23.0 extensions is `v0.23.0/`; this is expected from the 0.23.0 build definition but is confirmed at runtime (FR-004).
- A migrated database cannot be opened by older lcg builds; documenting this is sufficient and no downgrade path is built.
- Existing Windows databases that were already corrupted under 0.21.2 are not repaired by this change.
- Following the established bump pattern (#699 / #619) is acceptable for process and file layout.
- The real-Windows-machine run is performed by a maintainer with Windows access, since automated stages cannot do it. It is a validation step, not code.
- Windows CI is the automated stand-in for the Windows checks that can run in CI.
- Per repository convention, the changelog entry is a `changelog.d/` fragment and the ADR number is the issue number (930).

## Out of Scope *(optional)*

- Upgrading to any lbug version other than 0.23.0 (including the 0.22.x line).
- Detecting or repairing data already corrupted by the Windows CHECKPOINT bug on 0.21.2.
- A downgrade path from storage 53 back to earlier storage versions.
- Adopting new 0.23.0 features beyond what is needed to keep lcg's existing behaviour correct.
- Changes to the IPC protocol or MCP tool surface, unless the audit shows an upstream change forces one (which would then be called out).
- Replacing or restructuring the extension bundling approach (ADR-0559 / ADR-0593 mechanisms stay as they are, with updated versions and hashes).

## Source References *(optional)*

- Issue #719 (Windows CHECKPOINT corruption) and Dependabot PR #750 (superseded)
- LadybugDB/ladybug#1447 (bug) and #1593 (fix), first shipped in lbug 0.23.0
- LadybugDB/ladybug 0.23.0 release notes: #1435, #1459, #1361, #1358, #1375, #1507, #1516, #1520, #1496, #1534
- Prior bump pattern: #699 (0.21.1 → 0.21.2) and #619
- ADR-0559 (bundle lbug extensions), ADR-0593 (pin lbug extension bytes), ADR-0649 (FTS index rebuild on lbug version change)
- `crates/core/tests/storage_v42_migration.rs` (migration test style)
