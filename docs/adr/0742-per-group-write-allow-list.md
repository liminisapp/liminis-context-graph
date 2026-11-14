# ADR-0742: Per-Group Write Allow-List (`--writable-groups`) Enforced in the Daemon

**Status**: Accepted
**Date**: 2026-11-13
**Issue**: #742 (implements community request #740)

## Context

`--scope` filters which *tools* an MCP connection sees. Nothing limited *which groups* a
write-capable connection could change, although one daemon routinely holds groups with different
owners (read-only layer groups hydrated from other parties' streams next to the user's own group).
A client-side proxy only protects connections that go through it, and the IPC socket has no scope
concept at all.

## Decision

**A daemon-wide allow-list, enforced in `handlers::handle()`.** `--writable-groups=<g1,g2,…>` /
`LCG_WRITABLE_GROUPS` (flag wins) is parsed strictly by `write_policy::WritableGroups`: an empty
value, an empty entry (`a,,b`) or an entry with whitespace is a startup error, never "unset"
(ADR-0737's stance). It is resolved once in `async_main` before migration and the socket bind and
logged. IPC, standalone MCP and attached MCP all call `handlers::dispatch` → `handle()`, so one
check covers all three; startup recovery, auto-heal and WAL replay never go through it.

**The guard runs before any handler work** (right after the degraded-mode guard), so a refusal
performs no embedding, extraction, DB write or WAL event. It uses the handlers' own extraction
helpers (`extract_required_group_id(s)`, `group_id_param`) so resolution cannot drift from what a
handler writes.

**Carrier: a process-global `OnceLock` plus `dispatch_with_policy`, not an `AppState` field.**
`AppState` has ~82 struct-literal construction sites; a new field would churn them all (a class of
change that has repeatedly broken CI). `dispatch` is a thin wrapper reading the global; tests call
`dispatch_with_policy` with an explicit list, so they never mutate global state or the environment.

**Classification table.** `write_policy::CLASSIFICATION` names every method `handle()` dispatches:
read-only, exempt (with the reason), or how its written-group set is derived. A method missing
from it is refused (`unclassified_method`) whenever a list is set, and a service-crate test walks
`tools::registry()` and fails if any tool is unclassified or its class contradicts its scope
(FR-013).

**Check the group that changes.** `add_cross_group_edge` is checked against the edge's own
`group_id`; foreign `source_group_id`s are read-only pointers. Multi-group writers need every
group listed. `delete_by_group` and `rebuild_from_wal` (every call: replay writes the group, and a
detected generation reset auto-forces a clear) also need every group that owns a pointer row into
the targets, because ADR-0385 shows the purge/replay rebinds those foreign-owned rows.
`rebind_pointers` needs the pointer owners only. `delete_episode` looks up the episode's group.
`clear_all` and `apply_corrections` (no group in the request; name resolution spans all groups)
require **every group present**, i.e. DB groups ∪ WAL directories; `preserve_wal` does not change
that, since DB rows are wiped either way.

**Raw Cypher is refused outright** (`raw_cypher_disabled`), reads included. The IPC socket and
attached MCP have no scopes, so "disable `cypher` scope" cannot protect them, and
`wal::looks_like_mutation` is a WAL-logging filter that misses `CALL`, `COPY`, `ALTER`, `LOAD`,
`ATTACH`, so it is not a security boundary. A read-only Cypher mode is a possible follow-up.

**`dry_run` has no special case**: a dry run on a non-listed group is refused, rather than
trusting every handler's `dry_run` to be non-mutating.

**Recovery:** `knowledge_recover` is exempt only while the DB is degraded (it must stay reachable,
ADR-0009). On a healthy DB the handler does not refuse, and its strategies replace or rewind every
group, so the guard requires every present group to be listed (like `clear_all`).
`knowledge_recover_full` is a no-op on a healthy DB and stays exempt.

**Exempt:** `strip_wal_embeddings`, `build_indices`,
`prepare_checkpoint`, `dump_wal`, `wal_mark_*` and `close` touch only WAL/checkpoint files, indices
or process state.

**Error:** `Error::WriteRefused` → JSON-RPC `-32004` with `data {reason, group_ids, setting, env,
writable_groups}`; MCP and attached paths already pass `data` through. `--writable-groups` with
`--connect` is a CLI error (the attached process owns no state to enforce with); an inherited
`LCG_WRITABLE_GROUPS` there logs a warning.

## Consequences

- Purging or rebuilding a listed group is refused while a non-listed group holds a pointer into it.
- **Accepted TOCTOU:** pointer owners and episode groups are read before the write lock is taken.
  Only daemon-internal replay can create a new pointer owned by a non-listed group in that window,
  and the allow-list governs client requests, not restored state.
- `apply_corrections` fails closed unless all groups are listed; per-entry classification is a
  possible follow-up.
- A new write tool must be added to `CLASSIFICATION` or its registry test fails.
