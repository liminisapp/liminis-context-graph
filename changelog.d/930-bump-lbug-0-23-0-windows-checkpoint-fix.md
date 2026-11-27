### Upgrade notes

- **lbug 0.21.2 → 0.23.0: the on-disk storage version goes from 47 to 53 (#930).** An existing
  database opens under the new binary with no operator step; the rewrite to storage 53 happens at
  its first checkpoint. **The change is one-way:** a database that a 0.23.0 build has checkpointed
  cannot be opened by an older lcg, so take a backup (or keep the WAL) before upgrading if you may
  need to roll back. The full-text indexes are rebuilt once on the first open (the lbug version
  differs from the one that built them, ADR-0649). The pin skips lbug 0.22.x, which does not contain
  the Windows fix. No IPC or MCP change; raw-socket clients are unaffected.
- **Databases already damaged on Windows are not repaired.** The upgrade prevents new corruption
  from #719; a graph that was checkpointed on Windows under lbug 0.21.2 or earlier may already hold
  wrongly collapsed STRING or INT8/UINT8 columns. `knowledge_rebuild_from_wal` restores a graph from
  lcg's own WAL.

### Fixed

- **Windows: lbug's CHECKPOINT no longer silently corrupts STRING and INT8/UINT8 data (#719,
  #930).** lbug 0.23.0 includes the upstream fix (LadybugDB/ladybug#1593) for MSVC's vectorized
  `std::minmax_element` returning a wrong min/max, which made CHECKPOINT store a column chunk as
  CONSTANT. This removes #719 from Known issues. macOS and Linux were never affected.

### Changed

- **lbug is pinned to `=0.23.0`, the bundled extensions to `v0.23.0/` (#930).** lcg keeps lbug's
  single-writer mode explicitly: 0.23.0 made multi-writer the engine default
  (LadybugDB/ladybug#1435), and lcg serialises its own writes, so `enable_multi_writes` is pinned to
  `false` (ADR-0930). It likewise pins `throw_on_wal_replay_failure`, `enable_checksums` and
  `auto_checkpoint` to `true`: left to the 0.23.0 engine default, an lbug WAL with a damaged
  record body opened successfully and silently replayed only up to the damage, instead of
  entering degraded mode. **A WAL whose header is damaged is not detected:** lbug 0.23.0 discards
  it silently on open, with or without these pins, so there is no error and no degraded mode, and
  the loss shows only as missing recent data. lcg's own WAL is the recovery path
  (`knowledge_recover` / `knowledge_rebuild_from_wal`; ADR-0930).
  Startup also recognises lbug's new `Corrupted WAL record` wording as recoverable. Raw-row
  conversion failures that used to panic inside the lbug crate now surface as ordinary database
  errors.
