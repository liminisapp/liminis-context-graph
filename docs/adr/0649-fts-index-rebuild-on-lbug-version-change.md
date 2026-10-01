# ADR-0649: Rebuild FTS Indexes When the Building lbug Version Changes

**Status:** Accepted
**Date:** 2026-10-01
**Issue:** #649 (fixes the community report in #646; upstream: LadybugDB/ladybug#1092)

## Context

lbug 0.20 → 0.21 changed how **non-ASCII** terms (`→`, `—`, `é`, `ü`, `東京`, emoji) are stored in a
full-text index, **without bumping the storage version** (47 for both). A database built by lcg
0.15.x or earlier (lbug 0.20) therefore opens cleanly under lcg 0.16.x (lbug 0.21) with all three
FTS indexes (`node_name_and_summary`, `edge_name_and_fact`, `episode_content`) in a state 0.21
cannot maintain:

- **Writes fail.** Deleting or updating any row carrying an affected term fails with
  `FTS index '<idx>' is inconsistent: term '<t>' is missing during delete. Drop and recreate the
  FTS index.` — `knowledge_delete_episode`, re-asserting an entity, edge `SET name/fact`, and
  `knowledge_rebuild_from_wal {from_seq: 0, force_clear: true}` (whose purge deletes rows while the
  indexes are still live).
- **Search is silently wrong.** `QUERY_FTS_INDEX` for a non-ASCII term returns zero rows, no error.
- **Nothing self-repairs.** `create_fts_indexes` swallowed "already exists" (needed for
  idempotence, [#192](../../CHANGELOG.md)), so the stale index survived every startup, and the
  error's advice ("drop and recreate *the* index") only moves the failure to the next of the three.

Nothing in the file format reveals the problem, so detection has to be recorded by us.

## Decision

1. **Marker.** `SchemaState` key `fts_built_by_lbug` holds the `lbug::VERSION` that built the FTS
   indexes (the crate and the `LBUG_VERSION` native-bundle pin move together, so this is the
   running engine; `LBUG_EXTENSION_VERSION` is deliberately not used — it often differs).
2. **Detection and rebuild in `schema::create_fts_indexes`**, the one place every FTS creation goes
   through (`schema::init` ⇒ every `init_schema`: startup, recovery reopen, `drop_lbug_wal`,
   `restore_from_backup`; and `build_indices_and_constraints`). It observes the per-index outcome
   of the `CREATE_FTS_INDEX` calls. If the marker already equals `lbug::VERSION`, nothing more.
   Otherwise:
   - all 3 indexes were just **created** ⇒ built by the running lbug (fresh database, or dropped by
     a replay): write the marker only (FR-008, no rebuild);
   - any **already existed** ⇒ unknown provenance: log on stderr, drop all 3, create all 3, write
     the marker.

   A missing marker is "rebuild", which is exactly every pre-fix database. Because this runs inside
   `init_schema`, it completes synchronously **before any request is processed**, so no write can
   delete through a stale index (FR-005). A rebuild failure propagates like any index-build failure
   (startup is fatal on it, ADR-0036).
3. **Marker last.** The marker is written only after all 3 creates succeed, so a crash anywhere
   leaves it unset and the next open rebuilds again (FR-009). No "building" sentinel is needed:
   marker-last gives the same guarantee with less state. All 3 indexes are always rebuilt together.
4. **Any difference rebuilds, not just known-bad versions.** The rule is "marker ≠ running lbug",
   so every future lbug bump costs one FTS rebuild whether or not that bump is affected. This is
   deliberate: cheap, safe, and it needs no knowledge of which versions are compatible. The cost is
   proportional to corpus size, and the log line says so.
5. **Rebuild DDL is unrecorded (ADR-0015).** Handlers drain `Conn::executed_mutations` into the
   WAL; index DDL and the marker `MERGE` must never reach it. `Conn::query_unrecorded` /
   `exec_params_unrecorded` bypass recording and the backstop below; `ensure_schema_state_table`
   uses the unrecorded path too.
6. **Statement-level backstop.** `Conn::raw_query` / `exec_params` catch
   `error::is_fts_inconsistent_error`; outside an explicit transaction they rebuild all 3 indexes
   (unrecorded), bump `FtsRepairStatus`, log, and **retry that one statement once**. A failed
   autocommit statement is atomic and the caller already holds the write lock, so the retry is safe
   and idempotent. It covers an index touched by a pinned 0.16.0–0.16.2 build after the marker was
   written, or any path that bypasses `init_schema`. The repair count and last-repair timestamp are
   reported as `fts_repair_count` / `fts_last_repair_unix_ms` in `knowledge_status` (`null` when
   degraded).
   - **Not dispatch-level.** Retrying whole handlers would re-run non-idempotent or expensive work
     (an ingest has already spent LLM money; batched passes, ADR-0030, are partially applied).
   - **Not inside transactions.** Index DDL inside an explicit transaction is unsafe. `Conn` tracks
     BEGIN/COMMIT/ROLLBACK in `exec_transaction_control` (every explicit transaction in the repo
     uses it: `group_purge`, `remove_episodes_by_source`, WAL replay `flush_batch`) and propagates
     the error there; the open-time rebuild (Decision 2) and the forced-replay drop (Decision 7) cover those paths.
7. **Forced replay ordering (FR-007).** `clear_group_for_rebuild` drops the FTS indexes **before**
   `purge_groups`, because the purge runs in a transaction (where the backstop cannot act) and used
   to delete through live indexes. The replay's own drop and `indices_built = false` still rebuild
   them afterwards; `create_fts_indexes` marks them through the "all created" branch. The earlier
   drop widens, slightly, the window in which FTS is unavailable to other groups; search auto-heal
   (ADR-0025) recovers it. FR-007's reference to ADR-0030 was a mis-citation (that ADR is about
   canonicalization lock batching); the relevant designs are the replay ADRs 0043/0045/0046/0047.

## Consequences

- First start of 0.16.3 on a pre-0.16 database performs a one-time FTS rebuild, time proportional
  to corpus size, while the socket is bound but before requests are processed (`health_check`
  reports `busy` only for the write lock, ADR-0628 — supervisors must tolerate the delay; the
  stderr line states it).
- Fresh databases never rebuild; a second open never rebuilds.
- Storage version and data are untouched; only derived FTS indexes change (FR-010).
- Users pinned to 0.16.0–0.16.2 do not get the fix and must drop all three indexes by hand and run
  `knowledge_build_indices` (release notes).
- Every future lbug bump forces one rebuild (see Decision 4).

## References

- ADR-0009, ADR-0027 (degraded-mode / autonomous startup recovery), ADR-0015 (drain-and-flush),
  ADR-0025 (auto-heal), ADR-0036 (eager startup build), ADR-0043/0045/0046/0047 (replay)
- Regression fixture: `crates/core/tests/fixtures/fts_lbug020_db/` (built by the published v0.15.0)
