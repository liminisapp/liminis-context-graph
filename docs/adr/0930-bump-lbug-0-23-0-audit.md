# ADR-0930: Bump lbug 0.21.2 → 0.23.0 — Audit of Upstream Changes and Decisions

**Status**: Accepted
**Date**: 2026-11-27
**Issue**: #930 (implements #719; supersedes Dependabot #750)

## Context

On Windows, lbug's CHECKPOINT could silently corrupt STRING and INT8/UINT8 column data (#719,
LadybugDB/ladybug#1447): MSVC's AVX2 `std::minmax_element` returned a wrong min/max, so a column
chunk was stored as CONSTANT. The fix (LadybugDB/ladybug#1593) first shipped in lbug 0.23.0; the
0.22.x line does not contain it, so Dependabot's 0.21.2 → 0.22.3 bump (#750) was not enough.
0.23.0 is storage version 53 (0.21.2 is 47, 0.22.x is 49) and carries behaviour changes, so the bump
needed an audit and an upgrade-path proof rather than only a version change. This ADR records the
decisions. Extension bytes: see the addenda to [ADR-0559](0559-bundle-lbug-extensions.md) and
[ADR-0593](0593-pin-lbug-extension-bytes.md).

## Decision

Pin lbug `=0.23.0` (crate, `LBUG_VERSION`, `LBUG_CRATE_VERSION_VERIFIED_AGAINST`,
`LBUG_EXTENSION_VERSION`, `LBUG_EXTENSION_HASHES` move together) and adapt as follows.

### Crate API changes (compile-time)

| Change in the 0.23.0 crate | Decision |
|---|---|
| `QueryResult` yields `Result<Vec<Value>, ConversionError>` instead of `Vec<Value>`; a row that cannot be converted (decimal, interval, type mismatch) used to panic inside the crate. | **Adapted.** One choke point, `db::collect_rows`, maps `ConversionError` into `Error::Lbug`; every raw-`QueryResult` read in `Conn` goes through it. `Conn::query_cypher_raw` keeps its `Vec<Value>` item type (it now materialises eagerly) so the test and bench call sites are unchanged. A conversion failure is now an ordinary error, not a panic. |
| `lbug::Error` is `#[non_exhaustive]` with a new `ConversionError` variant. | No exhaustive match on it exists in lcg; `Error::Lbug(#[from] lbug::Error)` is unchanged. Nothing to do. |
| `SystemConfig` fields became `Option`; an unset field now defers to the **engine** default instead of the crate's old baked-in value (0.21.2: `auto_checkpoint`, `throw_on_wal_replay_failure`, `enable_checksums` all `true`, `enable_multi_writes` `false`). | **Pinned** — see the next section. This is the one change that mattered: the engine default for WAL-replay failure is *not* "throw". |
| `build.rs` is byte-identical to 0.21.2. | ADR-0550 / ADR-0690 OpenSSL linkage and prefetch are unchanged. |

### Pinned `SystemConfig` values

`LbugTuning::system_config()` now sets four values explicitly, restoring what lcg ran under
through 0.21.2 (each documented at the call site):

| Setting | Pinned | Why |
|---|---|---|
| `enable_multi_writes` | `false` | 0.23.0's engine default is multi-writer (#1435). lcg serialises writes itself (ADR-0015); a unit test checks a second concurrent write transaction is refused. |
| `throw_on_wal_replay_failure` | `true` | **Measured on 0.23.0 with the engine default:** a lbug WAL with one flipped byte (or truncated, or zeroed header) opened *successfully* and replayed only up to the damage — 99 of 200 rows survived, no error. lcg's degraded mode and WAL recovery (ADR-0009/0025/0026) need the open to fail so the operator is told. With the pin, every one of those cases fails the open. |
| `enable_checksums` | `true` | The WAL checksum is what makes a flipped byte detectable. |
| `auto_checkpoint` | `true` | The WAL must keep folding into the database file. |

### Release-note audit

| Upstream | Decision | Reason |
|---|---|---|
| #1435 `debug_enable_multi_writes` → `enable_multi_writes`, default now **true** | **Pinned: `enable_multi_writes(false)`** in `LbugTuning::system_config()`, with a unit test that two concurrent write transactions are refused. | lcg serialises writes itself (`RwLock` + WAL drain, ADR-0015). Multi-writer would add risk and no benefit to a bump whose purpose is the Windows fix. The Rust crate already used the name `enable_multi_writes`, so there was no rename to make. Accepting multi-writer is a possible follow-up with its own concurrency tests. |
| #1459 CHECKPOINT is parsed as a write; read-only databases refuse it | **Not applicable.** | lcg never issues `CHECKPOINT` and never sets `read_only`. Auto-checkpoint on a normal database is unchanged. `drop_lbug_wal` and recovery reopen through `Db::open`. |
| #1361 a committed write no longer fails when its automatic checkpoint fails | **Noted, no change.** | Behaviour change on the write path in lcg's favour. |
| #1358 a checksum-setting mismatch throws instead of dropping the WAL | **Pinned** (`enable_checksums(true)`, above). | lcg now sets it explicitly, matching what its existing WALs were written with. |
| #1375 a connection stays usable after a refused BEGIN | **Noted, no change.** | Neutral to positive. |
| #1507, #1516, #1520 COPY commit gating, BEGIN lock order, torn WAL tail reads | **Adapted.** | Two behaviour changes surfaced in `degraded_startup` and `fts_lbug020_migration`: (1) a WAL whose *header* is garbage is now silently discarded on open (the file is deleted, no error), where 0.21.2 failed with "Corrupted wal file"; the engine does not expose a knob for it, so `test_degraded_mode_from_corrupt_wal` now builds a real WAL and damages a record body, which fails the open under the pins above. (2) Corrupt record bodies can surface as `Corrupted WAL record: …`, a wording the service's startup recoverability check (`crates/service/src/main.rs`) did not match; it does now. |
| New in 0.23.0: a stale pre-0.21 FTS index entry is reported as `Detected a stale string primary-key hash index entry (issue #1092) …` instead of `FTS index '<idx>' is inconsistent` | **Adapted.** | `is_fts_inconsistent_error` matches the new wording, so the ADR-0649 statement-level backstop still rebuilds all three FTS indexes and retries. Without it `fts_lbug020_migration` failed 3 of 6. |
| #1496, #1534 Windows path-separator normalisation; relative paths resolved via UTF-8 | **Covered by Windows CI and the maintainer's real-machine run.** | The #719 regression test (below) opens its database under a path containing a space and a non-ASCII character. |
| #1378, #1379, #1380, #1513 checkpoint-crash recovery and primary-key fixes | **Noted only.** | Not used by lcg directly. |
| C API / `lbug_cli` changes | **Not applicable.** | lcg uses the Rust crate only. |

## Verification

- **Upgrade path (FR-006/007/008):** `crates/core/tests/storage_v47_migration.rs`, using a fixture
  written by the unchanged 0.21.2 binary (`tests/fixtures/storage_v47_db/`, storage 47). It asserts
  zero loss (4 entities, 2 episodes, 2 relationships), content including non-ASCII values,
  non-ASCII FTS on all three indexed tables, HNSW vector search on entities and edges (a stored
  embedding returns its own row at ~zero distance), the `fts_built_by_lbug` marker moving from
  `0.21.2` to `lbug::VERSION` (ADR-0649), no recorded FTS rebuild (ADR-0015), writes to non-ASCII
  rows succeeding without the backstop firing, and everything holding after a clean close and
  reopen (which checkpoints into storage 53).
- **Extensions:** the first `v0.23.0/` files on the CDN (Last-Modified 2026-11-23) were not the
  release build and their vector extension failed `CREATE_VECTOR_INDEX` against the 0.23.0 core
  (`mutex lock failed: Invalid argument`). The v47 migration test and a fresh index both fail on
  those bytes. Upstream republished from the release build (LadybugDB/ladybug#1707); the pins in
  `LBUG_EXTENSION_HASHES` are the republished bytes. Details in the ADR-0593 addendum.
- **macOS arm64 (debug), 2026-11-27, against the republished extensions:** `cargo test -p lcg-core`
  2723 passed / 0 failed (23 ignored, of which `real_corpus_e2e` and `real_corpus_replay_perf` were
  then run with `--ignored`: 2/2 and 3/3 passed); `cargo test -p lcg-service -p lcg-eval` 955
  passed / 0 failed; `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.
  The first full run (before the pins above and the classifier change) failed
  `test_degraded_mode_from_corrupt_wal` and 3 of 6 `fts_lbug020_migration` tests — both are the
  adaptations recorded in the audit table.
- **Windows:** `crates/core/tests/windows_checkpoint_719.rs` is the #719 repro; the maintainer's
  real-machine run and the vector/FTS pairing runs (as in #619) remain a manual step.

## Consequences

- A WAL whose *header* is unreadable is dropped silently on open: lbug does not expose a setting
  for it. lcg's own application WAL is the durable record (ADR-0015), and `knowledge_recover` /
  `knowledge_rebuild_from_wal` remain the repair path if the tail of lbug's WAL is lost this way.
- Storage 47 → 53 is **one-way**. The changelog fragment says so.
- Databases already damaged on Windows under 0.21.2 are not repaired.
- A database already migrated by a 0.22.x build (storage 49) is not a supported lcg state.
- The extension directory is no longer shared across a minor: 0.23.0 resolves to `v0.23.0/`.
