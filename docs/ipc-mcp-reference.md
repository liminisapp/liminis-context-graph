---
layout: default
title: IPC & MCP Reference
---

# IPC & MCP Reference

`liminis-context-graph` serves the same graph over **two transport surfaces**, both routed
through the same core dispatch in
[`crates/core/src/handlers.rs`](https://github.com/liminisapp/liminis-context-graph/blob/main/crates/core/src/handlers.rs) —
no graph logic is duplicated between them:

- **JSON-RPC 2.0 over a local socket** (default). Newline-delimited requests/responses over `.lcg/service.sock` — a Unix domain socket on macOS/Linux, a named pipe on Windows (see [Windows: named pipe](#windows-named-pipe)).
- **[Model Context Protocol](https://modelcontextprotocol.io) over stdin/stdout** (`--mcp-stdio`). Any MCP client — Claude Code, Claude Desktop, other agents — can query and mutate the graph directly.

### Windows: named pipe

On Windows the service serves the identical protocol over a named pipe, because `AF_UNIX` is not
usable from Rust's tokio, Node or CPython there (issue #581). `LCG_SOCKET_PATH` keeps its meaning —
it still names the workspace's endpoint — and maps to a pipe:

- **Discovery.** At bind the service writes the pipe name to `.lcg/service.endpoint` (the socket
  path with its extension replaced). Clients should read that file.
- **Name.** `\\.\pipe\lcg-<16 hex digits>`: FNV-1a 64-bit over the UTF-8 bytes of the absolute
  socket path, lower-cased, with `/` replaced by `\`. Stable per workspace, so a client that cannot
  read the file can compute it. A `LCG_SOCKET_PATH` (or `--connect`) that is already a
  `\\.\pipe\…` name is used as-is.
- **Access.** Only the user the service runs as (and SYSTEM) can connect; remote clients are
  refused, and a second process cannot bind the same pipe name.
- **Signals.** Closing the console, logoff/shutdown, Ctrl+Break and Ctrl+C all trigger the same
  graceful shutdown SIGTERM/SIGINT do on Unix.

A pipe opens like a file, so a client needs no socket library:

```python
import json
endpoint = open(".lcg/service.endpoint").read().strip()      # \\.\pipe\lcg-…
with open(endpoint, "r+b", buffering=0) as pipe:
    pipe.write((json.dumps({"jsonrpc": "2.0", "id": 1, "method": "health_check", "params": {}}) + "\n").encode())
    print(json.loads(pipe.readline()))
```

In Node, `net.createConnection(endpoint)` accepts the pipe name directly.

## IPC methods (55)

The socket dispatch handles **55 methods**: 54 `knowledge_*` methods plus `health_check`.
`health_check` is the one method not prefixed `knowledge_*`, and it is the reason the IPC
surface (55) and the MCP tool registry (54, below) differ by exactly one — `health_check` is
not exposed as an MCP tool.

| Category | Methods |
|----------|---------|
| Health | `health_check` |
| Status | `knowledge_status`, `knowledge_rebuild_status` |
| Ingestion | `knowledge_process_chunk`, `knowledge_process_document`, `knowledge_add_episode` |
| Document chunking (read-only) | `knowledge_preview_chunks` |
| Direct assertion | `knowledge_assert_entity`, `knowledge_assert_relationship` |
| Search | `knowledge_find_entities`, `knowledge_resolve_entity`, `knowledge_find_relationships`, `knowledge_search_passages`, `knowledge_query_cypher` |
| Graph reads | `knowledge_get_episodes`, `knowledge_get_nodes_by_group`, `knowledge_get_edges_by_group`, `knowledge_get_edges_by_uuids`, `knowledge_list_entities`, `knowledge_list_relationships`, `knowledge_get_entity_neighbors`, `knowledge_get_entities_by_source` |
| Correction reads | `knowledge_get_correction_changes`, `knowledge_get_affected_work` |
| Notebook health | `knowledge_lint` |
| Deletion | `knowledge_delete_episode`, `knowledge_delete_by_source`, `knowledge_delete_chunk_episode`, `knowledge_delete_by_group`, `knowledge_clear_all` |
| Supersession | `knowledge_mark_chunk_removed`, `knowledge_supersede_document` |
| Episode origin | `knowledge_stamp_origin` |
| Curation | `knowledge_merge_entities`, `knowledge_validate_corrections`, `knowledge_apply_corrections`, `knowledge_reprocess_entity_types` |
| Ontology | `knowledge_reload_ontology` |
| Relation typing | `knowledge_canonicalize_relations`, `knowledge_backfill_relation_types` (deprecated), `knowledge_reprocess_relation_types` |
| Semantic search maintenance | `knowledge_backfill_summary_embeddings` |
| Cross-group pointers | `knowledge_add_cross_group_edge`, `knowledge_rebind_pointers` |
| WAL administration | `knowledge_dump_wal`, `knowledge_strip_wal_embeddings`, `knowledge_prepare_checkpoint`, `knowledge_wal_mark_create`, `knowledge_wal_mark_list`, `knowledge_wal_mark_delete`, `knowledge_rebuild_from_wal`, `knowledge_build_indices` |
| Recovery / lifecycle | `knowledge_recover`, `knowledge_recover_full`, `knowledge_close` |

For request/response shapes and parameter details, the dispatch `match` arms in
[`handlers.rs`](https://github.com/liminisapp/liminis-context-graph/blob/main/crates/core/src/handlers.rs)
and their handler functions are the source of truth — this page is the method index, not a
copy of each handler's parameter parsing.

`knowledge_status`'s WAL fields — including `wal.hydration_status` (issue #456), which
distinguishes a genuinely empty group from one whose WAL holds unapplied content — are documented
field-by-field in [Operations: `knowledge_status` health fields](operations.md#knowledge_status-health-fields)
rather than here.

Every long-running method above (the six WAL/recovery/reclassification operations most likely
to run for a while) accepts a `_progress_token` and streams `{"type":"progress",...}` frames
before the terminal result — see [Progress notifications](#progress-notifications) below.

### Readiness

A successful connection to `.lcg/service.sock` is **not** evidence the service is ready. The
socket is bound before the database opens — deliberately, so `health_check` and recovery IPC stay
reachable during degraded-mode recovery ([ADR-0009](adr/0009-degraded-mode-startup-recovery.md))
— and issue #378's WAL-root migration also runs in that same pre-open window, after the bind.
(Legacy `.graphiti/`→`.lcg/` workspace migration runs earlier still, before the socket is even
bound.) The process's own accept loop only starts once startup work has fully resolved, so a
request sent immediately after `connect()` queues in the kernel rather than racing the migration
with stale state — the real risk is a client that treats `connect()` succeeding as readiness by
itself and acts on that assumption (e.g. inspecting on-disk state) without waiting for a
`health_check` round-trip.

The correct readiness signal is a `health_check` round-trip reporting `"healthy"`:
`handle_health_check` only returns `healthy` once `Db::open()` has succeeded, which is after
migration has completed. Poll `health_check` until it reports `healthy` (or `knowledge_status`
until `connected` and `queryable` are both `true` and `initializing` is `false` —
`knowledge_status` has no `healthy` field of its own) before sending real work; see
[Operations: Self-healing and degraded mode](operations.md#self-healing-and-degraded-mode)
for the full rationale. A `degraded` response after startup has otherwise settled is a legitimate
outcome (e.g. unrecovered corruption) — not something to retry indefinitely.

A `busy` response (see [Health check contract](#health-check-contract)) is **not** a readiness
signal: it means the service is alive but a write is holding or waiting for the write lock — most
importantly a long `knowledge_rebuild_from_wal`. Pollers keep waiting on `busy` (gate on
`result.healthy == true`) and must not treat it as failure.

### Health check contract

`health_check` never waits on the write lock, so it answers promptly even during a long WAL
rebuild ([ADR-0628](adr/0628-health-check-busy-state.md), #628). The response's `state` is one of:

| `state` | `ok` | `healthy` | Meaning | Supervisor action |
|---|---|---|---|---|
| `healthy` | `true` | `true` | Alive and ready: DB open, no write in progress or pending. | Ready — send work. |
| `busy` | `true` | `false` | Alive, **not ready**: a write is pending or in progress. | Alive — keep waiting; do not restart. |
| `degraded` | `false` | `false` | Alive but unusable: DB not loaded. `reason` says why. | Recover (`knowledge_recover`) or investigate. |
| *(no answer)* | — | — | Dead or wedged. | Restart. |

`busy` carries an `activity` field. While a `knowledge_rebuild_from_wal` job is running:

```json
{"ok": true, "healthy": false, "state": "busy", "activity": "rebuilding",
 "job_id": "…", "progress": {"mutations_replayed": 48210, "wal_files_processed": 3,
                             "wal_files_total": 12, "elapsed_seconds": 91.4}}
```

`progress` uses the same values as `knowledge_rebuild_status`. For any other writer the response
is `{"ok": true, "healthy": false, "state": "busy", "activity": "writing"}` with no `job_id` or
`progress`. The busy path never connects to or queries the database.

Notes:

- `busy` means "a write is pending **or** in progress": a writer queued behind readers also makes
  the lock unavailable, so an otherwise idle service can briefly report `busy`.
- Batched passes ([ADR-0030](adr/0030-batched-write-lock-for-long-running-passes.md)) release the
  lock between batches, so during one the state alternates between `busy` and `healthy`.
- A streaming rebuild (`_progress_token`) is not registered as a job: it reports
  `activity: "writing"` without `progress`.
- Startup work (legacy and WAL-root migration, startup recovery) runs before the accept loop, so a
  probe sent then queues in the kernel and gets no answer until startup finishes.
- `healthy` can be reported in the short window after a rebuild job is registered but before it
  takes the write lock.
- `busy` has `healthy: false` on purpose: existing pollers that gate on `healthy` keep waiting.
  A liveness probe that only needs "is the process alive" should accept `ok: true`.

## MCP-over-stdio transport

`liminis-context-graph --mcp-stdio` starts a native [Model Context Protocol](https://modelcontextprotocol.io)
server over stdin/stdout, using the official Rust SDK (`rmcp`). Any MCP client (Claude Code,
Claude Desktop, other agents) can query and mutate the knowledge graph directly — no Electron
app, no Node, no custom JSON-RPC client required. This is an *additional* external-facing
surface; the Unix-socket JSON-RPC protocol above is unchanged.

Every MCP tool is derived from the `knowledge_*` dispatch methods above — tool names match the
IPC method names verbatim, and each `tools/call` is translated into an `IpcRequest` and routed
straight through the same core dispatch the socket service uses. Tool descriptions and JSON
schemas are maintained in the
[`ToolSpec` registry in `crates/service/src/mcp/tools.rs`](https://github.com/liminisapp/liminis-context-graph/blob/main/crates/service/src/mcp/tools.rs) —
that file is the canonical source for per-tool descriptions; they are not duplicated here.

### Flags

| Flag | Description |
|------|-------------|
| `--mcp-stdio` | Starts the MCP server over stdin/stdout instead of binding the Unix socket. |
| `--scope=<list>` | Comma-separated list of scopes to advertise in `tools/list` (default `all`). See [Scopes](#scopes) below. |
| `--connect <path>` | Attached mode: forward every `tools/call` as JSON-RPC over the given Unix socket to an already-running service, instead of opening the database directly. By default the socket is dialled lazily, on the first `tools/call` — see [DB-access modes](#db-access-modes) below. |
| `--connect-eager` | Attached mode only: dial `--connect`'s socket at startup and exit immediately if it's unreachable, restoring the behavior attached mode had before issue #575. No effect without `--connect`. |
| `--allow-remote-close` | Attached mode only: advertise and allow `knowledge_close`, forwarding the shutdown to the remote service. No effect in standalone mode (no `--connect`). |

### DB-access modes

- **Standalone (default, no `--connect`)**: the MCP process opens the `.lcg` database directly,
  reusing the same startup and self-recovery path as the socket service ([ADR 0009](adr/0009-degraded-mode-startup-recovery.md)).
  Zero-dependency — works with no other process running.
- **Attached (`--connect <socket-path>`)**: the MCP process never opens the database; it
  forwards each call over the given socket to a service that already has it open. Use this to
  add MCP access to a workspace where another socket-service instance is already running,
  without contending for lbug's single-writer lock.
  - **Lazy connect by default (issue #575).** The socket is not dialled during startup —
    `initialize` and `tools/list` succeed immediately, serving the tool set from the compiled-in
    registry, regardless of whether the daemon at `--connect`'s path is up. The first
    `tools/call` triggers a dial; if it fails, that call returns a tool result with `isError:
    true` naming the socket path and suggesting you start the daemon, but the MCP process itself
    stays up and the client's session is unaffected — no protocol error, no exit. This avoids
    two failure modes a client can otherwise get stuck in for the client's own cached-failure
    window: registering a native MCP server before its daemon has started (first install,
    post-reboot), and the client caching a failed attach for several minutes even after the
    daemon comes up. Pass `--connect-eager` to restore the pre-#575 behavior of dialling at
    startup and exiting immediately (before `initialize`) if the socket is unreachable — useful
    if you rely on that fail-fast behavior as an external health check (e.g. a supervisor that
    restarts the front until the daemon is ready).
  - **Idle timeout.** `LCG_ATTACHED_CALL_TIMEOUT_MS` (default 30s) is a **per-read-line** idle
    timeout, not a whole-call timeout: it resets on every line read off the socket, including
    `{"type":"progress"}` lines. A call that keeps emitting progress is never bounded by it, no
    matter how long the call runs in total — only genuine silence (no output at all for the full
    timeout window) trips it. If the remote stops responding mid-call (e.g. it crashes), the
    attached client fails that call with a clean timeout error rather than blocking forever.
  - **Reconnect and retry.** If the connection to the remote breaks, the client transparently
    re-dials the same socket path rather than staying wedged. If the break is detected while
    writing the outgoing request — treated as safe to retry, since the write failing is the
    client's best available signal that the request didn't get through — the client
    automatically retries that request exactly once over the freshly-dialed connection. If the
    break is detected only after the request was fully written — while waiting for or reading
    the response — the call is **not** retried automatically, since the remote's execution
    status is unknown and blind retry could double-apply a non-idempotent write (e.g.
    `knowledge_add_episode`); that call fails with a clear "connection lost mid-call" error, but
    the connection is marked dead so the *next* call reconnects fresh. If a reconnect attempt
    itself fails (no listener at that path), the call fails with a clear, descriptive error —
    never a hang — and a later call will try reconnecting again. See
    [ADR-0040](adr/0040-attached-mode-reconnect-retry-boundary.md) for the full rationale.

### Scopes

Scopes are additive and composable (e.g. `--scope=read,admin`). `tools/list` advertises the
union of all active scopes.

| Scope | Methods |
|-------|---------|
| `read` | `knowledge_status`, `knowledge_find_entities`, `knowledge_resolve_entity`, `knowledge_find_relationships`, `knowledge_get_episodes`, `knowledge_get_nodes_by_group`, `knowledge_get_edges_by_group`, `knowledge_get_edges_by_uuids`, `knowledge_search_passages`, `knowledge_list_entities`, `knowledge_list_relationships`, `knowledge_get_entity_neighbors`, `knowledge_get_entities_by_source`, `knowledge_rebuild_status`, `knowledge_validate_corrections`, `knowledge_get_correction_changes`, `knowledge_get_affected_work`, `knowledge_lint`, `knowledge_preview_chunks` |
| `write` | `knowledge_process_chunk`, `knowledge_process_document`, `knowledge_add_episode`, `knowledge_delete_episode`, `knowledge_delete_by_source`, `knowledge_delete_chunk_episode`, `knowledge_mark_chunk_removed`, `knowledge_supersede_document`, `knowledge_clear_all`, `knowledge_apply_corrections`, `knowledge_merge_entities`, `knowledge_reprocess_entity_types`, `knowledge_canonicalize_relations`, `knowledge_backfill_relation_types`, `knowledge_reprocess_relation_types`, `knowledge_add_cross_group_edge`, `knowledge_assert_entity`, `knowledge_assert_relationship` |
| `cypher` | `knowledge_query_cypher` |
| `admin` | `knowledge_dump_wal`, `knowledge_strip_wal_embeddings`, `knowledge_prepare_checkpoint`, `knowledge_wal_mark_create`, `knowledge_wal_mark_list`, `knowledge_wal_mark_delete`, `knowledge_rebuild_from_wal`, `knowledge_recover`, `knowledge_recover_full`, `knowledge_close`, `knowledge_build_indices`, `knowledge_rebind_pointers`, `knowledge_delete_by_group`, `knowledge_backfill_summary_embeddings`, `knowledge_reload_ontology`, `knowledge_stamp_origin` |
| `all` | every scope above (default) |

**`cypher` is a power scope, not bundled into anything else.** `knowledge_query_cypher` executes
raw Cypher with no param interpolation or value coercion — despite being a "query" method, it can
perform arbitrary mutations, and it bypasses the WAL-ordering and embedding invariants that the
structured write tools maintain. It is never implicitly included in `read`, `write`, or `admin`;
operators must opt in explicitly (or via `all`).

#### Per-group write allow-list

`--scope` decides which *tools* a connection sees; it does not limit *which groups* a
write-capable connection may change. When one daemon holds groups with different owners — say
layer groups hydrated from streams other parties publish, beside the group the local user
authors — start it with `--writable-groups=<g1,g2,…>` (or `LCG_WRITABLE_GROUPS`; see
[Configuration](configuration.md)). The list is enforced **inside the daemon**, at the one point
every request reaches, so it holds for the IPC socket (which has no scopes at all), standalone
`--mcp-stdio` and attached `--mcp-stdio --connect` alike. It is one list per daemon: there is no
per-client identity, and per-group *read* restriction is out of scope. Unset, nothing changes.

The check is made against the groups a call **actually changes**, not every group it mentions:

- An omitted (or `null`/non-string) `group_id` resolves to the default group, `liminis` — **the
  default group must be listed to stay writable.** `group_id: ""` is a distinct group that can
  never be listed, so it is always refused.
- `knowledge_add_cross_group_edge` is checked against the edge's own `group_id`. The
  `source_group_id` of a foreign endpoint is a read-only pointer and is not checked: with
  `--writable-groups=a`, `{group_id: "a", target: {source_group_id: "b", …}}` succeeds and
  `{group_id: "b", …}` is refused.
- A call that changes several groups needs **every** one listed, otherwise it is refused whole
  and nothing is written.
- The check runs before any handler work, so a refused call does no embedding or extraction and
  writes no database row and no WAL event. Reads of any group, and read-only tools, are never
  affected. `dry_run` gets no special case: a dry run on a non-listed group is refused too.
- Daemon-internal work — startup WAL replay, auto-heal index builds, degraded-mode recovery — is
  not a client request and is not blocked.

| Tool(s) | Group(s) that must be listed |
|---------|------------------------------|
| `knowledge_add_episode`, `knowledge_process_chunk`, `knowledge_process_document`, `knowledge_assert_entity`, `knowledge_assert_relationship`, `knowledge_merge_entities`, `knowledge_add_cross_group_edge`, `knowledge_reprocess_entity_types`, `knowledge_reprocess_relation_types` | `group_id` (default group if omitted) |
| `knowledge_supersede_document`, `knowledge_stamp_origin`, `knowledge_canonicalize_relations`, `knowledge_backfill_relation_types`, `knowledge_backfill_summary_embeddings`, `knowledge_reload_ontology` | the required `group_id` |
| `knowledge_delete_by_source`, `knowledge_delete_chunk_episode`, `knowledge_mark_chunk_removed` | every entry of `group_ids` |
| `knowledge_delete_episode` | the group that owns the episode (looked up; a missing episode is a no-op) |
| `knowledge_delete_by_group`, `knowledge_rebuild_from_wal` (every call, not just `force_clear`) | the target group(s) **and** every group owning a cross-group pointer row into them (ADR-0385: the purge/replay rebinds those rows) |
| `knowledge_rebind_pointers` | every group that owns a pointer row into `source_group_id` (those rows are what it rewrites; `source_group_id` itself is not written) |
| `knowledge_clear_all`, `knowledge_apply_corrections` | **every group present** (DB groups plus WAL directories). `apply_corrections` carries no group in its request and resolves names across all groups, so it fails closed |
| `knowledge_query_cypher` | refused entirely — see below |
| `knowledge_recover` | **while the database is degraded: exempt**, because a degraded database must stay recoverable ([ADR-0009](adr/0009-degraded-mode-startup-recovery.md)). **While it is healthy: every group present must be listed**, like `knowledge_clear_all`, because its strategies (`restore_from_backup`, `rebuild_from_workspace_wal`, `drop_lbug_wal`) replace or rewind every group's data |
| `knowledge_recover_full` | exempt: a no-op on a healthy database, and on a degraded one it only restores previously accepted state |
| `knowledge_strip_wal_embeddings`, `knowledge_build_indices`, `knowledge_prepare_checkpoint`, `knowledge_dump_wal`, `knowledge_wal_mark_*`, `knowledge_close` | exempt: they touch only WAL/checkpoint files, indices or process state, never group rows |

A consequence worth knowing: purging or rebuilding a listed group is refused while a non-listed
group still holds a pointer into it — list the owner too, or remove the pointer first.
(`force_clear` has been group-scoped since #378, not whole-database; only `knowledge_clear_all`
and `knowledge_recover`'s `rebuild_from_workspace_wal` span every group.)

**Raw Cypher is refused while a list is set.** `knowledge_query_cypher` cannot be scoped by group
— any statement can write anywhere — and the IPC socket and attached MCP have no scopes to turn
off, so the daemon refuses the method outright, reads included, with reason `raw_cypher_disabled`.
Use the structured `knowledge_*` read tools instead. It is refused rather than filtered because
no keyword scan can reliably tell a read from a write in Cypher (`CALL` procedures, `COPY`,
`LOAD`, `ALTER`, …). With the list unset, `cypher` scope is unchanged.

**Error shape.** A refused call fails with JSON-RPC code `-32004` and structured `data`; over MCP
the same object arrives as the tool error's `data`:

```json
{"code": -32004,
 "message": "knowledge_assert_entity refused: group 'b' not writable: not in --writable-groups (LCG_WRITABLE_GROUPS); writable groups: [a]",
 "data": {"reason": "group_not_writable", "group_ids": ["b"],
          "setting": "--writable-groups", "env": "LCG_WRITABLE_GROUPS",
          "writable_groups": ["a"]}}
```

`reason` is `group_not_writable`, `all_groups_required` (a whole-database operation with unlisted
groups present), `raw_cypher_disabled`, or `unclassified_method` (a method with no write
classification is refused rather than assumed safe). An attached MCP process cannot enforce the
list itself and rejects `--writable-groups` alongside `--connect`; set it on the daemon. See
[ADR-0742](adr/0742-per-group-write-allow-list.md).

**`knowledge_close` in attached mode is a footgun without `--allow-remote-close`.** In
**standalone** mode, `knowledge_close` is always advertised under `admin` scope and shuts down
only this MCP process's own DB connection. In **attached** mode, calling it would shut down the
*running remote service*. Without `--allow-remote-close`, `knowledge_close` is omitted from
`tools/list` entirely in attached mode (not merely rejected when called). Pass
`--allow-remote-close` only when you specifically intend this MCP connection to be able to stop
the remote service.

**Recovery and export live under `admin`.** `knowledge_rebuild_from_wal` (rebuild one group's data
from its own WAL directory — `group_id`, default `"liminis"`), `knowledge_dump_wal`
(snapshot/export the graph into a fresh compacted WAL directory), `knowledge_strip_wal_embeddings`
(rewrite existing WAL files in place to remove the embedding-vector `params` fields a pre-0.14
WAL still carries — see [Operations](operations.md#wal-administration)), `knowledge_wal_mark_create` /
`_list` / `_delete` (name a retained WAL position within one group's stream, without a full
snapshot — each also takes `group_id`, default `"liminis"`, and `_list` reports only that one
group's marks, never an aggregate across groups), and `knowledge_recover` / `knowledge_recover_full`
are all `admin`-scope tools — an attached client only sees them when launched with `--scope=admin`
(or `all`). If a mutation goes wrong, this is the recovery path. See [Operations](operations.md)
for the recovery model in full. Note the WAL replays **forward-only**, so take periodic
`knowledge_dump_wal` snapshots, or a lighter-weight `knowledge_wal_mark_create` named position, if
you want restore points before large or destructive operations — a mark does not survive
`knowledge_dump_wal`, since dump_wal renumbers sequence numbers and a copied mark's `seq` would be
meaningless against the new numbering.

**`knowledge_rebuild_from_wal` refuses to run against a non-empty group, unless you ask it not
to.** Since issue #378, one instance holds an independent WAL directory and applied position per
`group_id`; `knowledge_rebuild_from_wal {group_id, ...}` targets exactly one of them and never
disturbs another group's data or position. A `from_seq: 0` (default) full rebuild against a group
that already contains data fails fast with an explicit error rather than silently emitting a
duplicate-primary-key failure for every existing `Entity`/`Episodic`/`RelatesToNode_` row in that
group — the native write path uses `CREATE`, not `MERGE`, for those labels. Pass `force_clear:
true` to have the call clear *that group's* data automatically before replaying (the same
group-scoped purge `knowledge_delete_by_group` uses — this does **not** delete or reopen the
database file, unlike the pre-378 whole-database `force_clear` behavior), or clear it yourself
first with `knowledge_delete_by_group {group_ids: [group_id]}`. `dry_run: true` always fails fast
on a non-empty group regardless of `force_clear`, since a dry run must never mutate the database —
this lets a preview surface the problem before you commit to a real rebuild. None of this applies
to an incremental `from_seq > 0` resume, which intentionally targets a group that already has
state.

**Rebuild order across groups does not matter, and every rebuild reports `cross_group_rebind`
(issue #741).** A cross-group edge lives in the WAL of the group that *owns* it (say `layer`) but
points at an entity in another group (say `up`). Rebuilding `layer` before `up` used to leave that
edge permanently missing: its hop writes found no endpoint and did nothing, yet the pointer kept the
`bound` state cached in its WAL attributes. Now, after **every** successful rebuild — full or
incremental, reset-triggered or not, foreground or background job — the service (1) demotes any
`bound` pointer on an edge owned by the rebuilt group whose hop was not actually created to
`unbound` (local derived state, not written to any WAL), so `knowledge_status.cross_group_pointers`
reports the gap instead of claiming `bound`, and (2) runs the staleness-gated `rebind_pointers`
for pointers pointing *into* the rebuilt group, flushing each resulting mutation to the group that
owns the edge. Rebuilding `layer` then `up` therefore ends in the same graph as `up` then `layer`,
with no manual `knowledge_rebind_pointers` call. The result's `cross_group_rebind` carries the same
counts `knowledge_rebind_pointers` reports (zeros when nothing needed attention); it is `null` only
on a `dry_run`, or if the repair itself errored (logged, and the rebuild still succeeds).

**`to_seq` bounds replay from the other end: `from_seq <= seq <= to_seq`.** Pass an inclusive
upper bound to exclude a mutation (and everything after it) from a rebuild — e.g. a WAL-recorded
mutation that corrupted the graph. Omit `to_seq` for today's unbounded behavior (replay to the
end of the WAL); it must not be less than `from_seq`, or the call is rejected before any WAL line
is read or the database is touched. A bounded rebuild is **not durable**: WAL entries past
`to_seq` stay on disk, unapplied, not truncated or archived — a later unbounded rebuild, or a
`from_seq` resume that covers the excluded range, reapplies them, including a previously-excluded
bad mutation. `to_seq` bounds an endpoint; it does not add reverse/undo semantics to the
forward-only replay noted above.

### Search evidence and the similarity floor (`find_entities`, `find_relationships`)

Hybrid search fuses its candidate lists with Reciprocal Rank Fusion, which is rank-only: the top
hit of a gibberish query scores almost the same as the top hit of a perfect one, and every query
fills its top-k. To let a caller tell a real match from filler, each result of
`knowledge_find_entities` and `knowledge_find_relationships` carries an additive `search` object
(issue #629):

| Field | On | Meaning |
|-------|----|---------|
| `rrf_score` | both | fused, rank-based score — ordering only, **not** a relevance measure |
| `text_match` | both | `true` iff the full-text (BM25) path retrieved the item |
| `bm25_score` | both | raw BM25 score, or `null` |
| `name_similarity`, `summary_similarity` | entities | cosine similarity (`1 - distance`) on that vector path, or `null` |
| `fact_similarity` | relationships | cosine similarity (`1 - distance`) on the fact-vector path, or `null` |

A field is `null` when that retrieval path did not return the item (`text_match` is then `false`).
Values are raw and unnormalised; cosine similarity can be negative.

Both tools also accept an optional **`min_similarity`** (number). Vector candidates whose cosine
similarity is below it are dropped *before* fusion, independently per vector path, so they never
affect the ranking and are not reported for an item they did not qualify for. Full-text matches
always stay eligible, so an exact-name query still returns its match while a gibberish query
returns nothing. With the `kind` filter the floor applies on top of the kind path's bounded
nearest-neighbour probe.

How it differs from `knowledge_search_passages`' `min_score`: the value is clamped to 0–1 the same
way (a non-number is an invalid-params error), but `min_similarity` has **no default** — omitted
or `null` means no floor and results are identical to before, apart from the added `search`
object — and it acts before fusion and per path rather than as a post-filter on a single list.

### Hybrid passage search (`knowledge_search_passages`, issue #789)

`knowledge_search_passages` ranks episode passages in one of two modes, chosen per request with
`mode`:

| `mode` | Ranking | `score` | `similarity` |
|---|---|---|---|
| `vector` | Vector cosine, then belief-aware re-ranking by default (see `belief_aware`); with `belief_aware: false` it is cosine only, exactly the behaviour before #789. | Cosine similarity (`1 - distance`); a demoted passage's score is lowered to just below the passage it ranks under. | Not present. |
| `hybrid` | BM25 over episode text (the existing `episode_content` full-text index) and the vector ranking, fused with **weighted** Reciprocal Rank Fusion (`weight/(rank+60)` per list; the lexical list has weight 0.4, the vector list 1.0), then belief-aware re-ranking (see `belief_aware`). | The **fused rank score** (at most about 0.023). It orders results; it is not a cosine and not comparable across queries. | Cosine similarity where the vector retriever matched the passage; absent for a passage only the lexical retriever found. |

- **`mode`** is optional. Omitted, the service default applies: the `LCG_PASSAGE_SEARCH_MODE`
  environment variable (`vector` or `hybrid`; an unrecognised value warns on stderr and falls back
  to `vector`), else `vector`. An unknown or non-string `mode` is an invalid-params error. Callers
  that need plain cosine scores or a strict similarity floor should send `mode: "vector"` **and**
  `belief_aware: false` explicitly: `mode: "vector"` alone is belief-aware by default (since #806),
  which lowers a demoted passage's `score`. Sending both also makes them unaffected if the service
  default or `LCG_PASSAGE_BELIEF_AWARE` changes.
- **`belief_aware`** (boolean; default `true` in both modes since #806; omitted or `null` means
  "use the default"): after fusion and
  before truncating to `num_results`, two rules re-rank the candidate set (up to `3 × num_results`).
  A `derived` or `answer` passage that cites a `primary`/`external` note by `[[wikilink]]` (matched
  on that note's `doc_id`, within the same group) ranks below that note's best-ranked passage, so a
  page restating a fact does not outrank its source ([ADR-0752](adr/0752-episode-origin-and-corroboration.md)).
  A passage whose episode is no longer believed (a revision, or a superseded document;
  [ADR-0705](adr/0705-belief-time-separate-from-validity.md),
  [ADR-0707](adr/0707-document-supersession.md)) ranks below its successor. A rule applies only when
  the passage it ranks under is itself in the candidate set. A removed chunk has no successor and a
  document supersession with a future effective instant is still believed, so neither is demoted.
  Demoted passages are never dropped, and a demoted passage's `score` is lowered to just below the
  passage it ranks under, so **`score` can differ as well as the order**. In `vector` mode the same
  rules run on the vector ranking (`score` stays a cosine otherwise). `belief_aware: false` turns
  the rules off and returns, byte-for-byte, what the same request returned before #806 (cosine
  order and scores in `vector`; weighted RRF without the rules in `hybrid`). The workspace
  switch `LCG_PASSAGE_BELIEF_AWARE` (`off`, `0` or `false` disable; `on`, `1` or `true` enable;
  case-insensitive; anything else warns once on stderr and is ignored) sets the default for
  requests that omit `belief_aware`; an explicit request value always wins. With the switch off,
  `hybrid` gives weighted RRF without the rules, which is not the pre-#806 hybrid default (rules on).
  A non-boolean value is an invalid-params error. The removed `top_rank_bonus` parameter is ignored.
- **`min_score`** (default `LCG_SEARCH_MIN_SCORE`, else 0.5; clamped 0–1) floors the *vector
  retriever's* cosine similarity before fusion, as `min_similarity` does for entity search. A
  passage found only by the lexical retriever is **not** excluded by it. This is a deliberate change
  in meaning from `vector` mode, where `min_score` filters the single list. When a request omits
  `min_score` (or sends `null` or a non-number), the operator-configured default applies; an
  explicit number, including `0.0`, always wins. The effective default is reported by
  `knowledge_status` as `search_min_score_default` (additive, top-level, present in every status
  response shape). `LCG_SEARCH_MIN_SCORE` is validated once at startup: an empty, non-numeric, NaN
  or infinite value fails startup, and a finite out-of-range value is clamped to 0–1.
- **Fallbacks.** A query with no lexical tokens (punctuation or stop words only), or no lexical
  match, returns the vector ranking. If the episode full-text index is missing or inconsistent
  (for example right after an lbug upgrade, before the index is rebuilt) hybrid search silently
  falls back to vector-only, and the service logs one line on stderr per process. Query text is
  reduced to alphanumeric tokens before it reaches the index, so no query can cause a parse error.
- **Filters.** `group_ids` and `origins` ([ADR-0752](adr/0752-episode-origin-and-corroboration.md))
  are applied inside both the lexical and the vector query, so a passage excluded by either cannot
  enter through the other path.
- **Superseded episodes** are still returned in both modes; `belief_aware` only moves them below
  their successor.

The full-text index also covers an episode's `source` and `source_description`, so a query term in
a file name or description can match. Which mode is better is measured per category on the
retrieval eval (`lcg-passages` is the shipped default; `lcg-passages-raw`,
`lcg-passages-belief`, `lcg-passages-hybrid` and `lcg-passages-hybrid-belief` are opt-in; see
[retrieval-quality-evaluation.md](retrieval-quality-evaluation.md) and
[ADR-0789](adr/0789-hybrid-passage-search-rrf-top-rank-bonus.md)).

### Optional rerank and expansion stages on `knowledge_search_passages` (issue #796)

`knowledge_search_passages` accepts three **optional** parameters. Omit them all and the call behaves
exactly as before (same results, order, response shape and latency).

| Param | Type | Meaning |
|-------|------|---------|
| `rerank` | boolean | Rescore the top candidates with a local cross-encoder, blended with belief and recency. Overrides the workspace default (`LCG_RERANK`, default off). Latency (default model, Core ML, one maintainer machine): about 1.1 s p50 at 25 candidates and 1.9 s at 50 (the default), well over the 150 ms target; p95 not measured. The earlier ~587 ms figure was a Python/MPS run. See ADR-0796. |
| `rerank_candidates` | integer 1–50 | Candidates rescored (default 50, which is also the maximum; `LCG_RERANK_CANDIDATES`). Validated only when a stage runs (`rerank` / `expand` effective for the call), where a non-integer, negative or `null` value is an error; a call that runs no stage ignores it, so the default path is unchanged. An out-of-range integer is clamped to 1–50 and a warning is logged. Results beyond it follow in first-stage order, with no duplicates. |
| `expand` | boolean | Rewrite the query with a local LLM into lexical, semantic and hypothetical-answer variants and RRF-fuse their results (before rerank). Never a default; expected to cost on the order of a second (not yet measured). |

A non-boolean, non-null `rerank`/`expand` is an invalid-params error on every call, including one that would otherwise run no stage; omit the key or send `null` for "not specified". When a stage is requested the response
gains a block for it — `rerank: {applied, candidates, reason?, elapsed_ms?}` and/or
`expansion: {applied, variants, reason?, elapsed_ms?}` (`elapsed_ms` is the stage's wall-clock time, present
when the stage did work) — and the search **never fails** because a stage could not
run: it returns the un-staged results with `applied: false` and a `reason` (`not_configured`,
`unavailable`, `no_candidates`, `no_variants`). Each reranked passage gains `rerank_score` (cross-encoder
logit) and `belief` (`current` or `superseded`; omitted when unknown). `score` stays the first-stage
score (cosine in `vector` mode, the fused rank score in `hybrid` mode); only the order changes. The
first stage follows the call's `mode` and `belief_aware` (belief-aware is the default since #806; see above), except that a hypothetical-answer
expansion variant is always searched by vector. With `expand: true`, a passage the original query found
keeps the `score` computed against that query; a passage found only by an expansion variant carries that
variant's `score`, and `min_score` is applied to each variant's own query, so such a passage can have a
lower similarity to the user's query than `min_score`. `queries` (#802) compose with the stages: with `rerank` and/or `expand`, the user's own query runs the `queries` weighted-RRF pipeline as its first stage (LLM-expansion variants are single queries, searched as before), and the cross-encoder then reorders the fused result. The `belief_aware` ordering (a superseded passage
below its successor) is applied to each first-stage list, not to the fused result, so with `expand: true`
and **without** `rerank` it is not guaranteed; `expand: true` together with `rerank: true` ranks a
replacement above the note it supersedes when both are among the reranked candidates (`rerank_candidates`). See [Passage rerank and query expansion](configuration.md#passage-rerank-and-query-expansion).

### group_ids semantics: omitted vs. empty

Every read tool that accepts `group_ids` treats an **omitted or `null` `group_ids` uniformly as
"all groups"** (issue #413) — this holds across all 10 read tools: `knowledge_find_entities`,
`knowledge_find_relationships`, `knowledge_search_passages`, `knowledge_list_entities`,
`knowledge_list_relationships`, `knowledge_get_entity_neighbors`, `knowledge_get_entities_by_source`,
`knowledge_get_nodes_by_group`, `knowledge_get_edges_by_group`, `knowledge_get_episodes`.

**An explicit `group_ids: []` does not mean the same thing on every tool**, and this split is
deliberate, not an inconsistency to be fixed here:

- On `knowledge_find_entities`, `knowledge_find_relationships`, `knowledge_get_nodes_by_group`,
  and `knowledge_get_edges_by_group`, an explicit `group_ids: []` is preserved as "exactly these
  groups" — i.e. **zero rows**, a filter matching nothing.
- On the other six read tools (`knowledge_search_passages`, `knowledge_list_entities`,
  `knowledge_list_relationships`, `knowledge_get_entity_neighbors`,
  `knowledge_get_entities_by_source`, `knowledge_get_episodes`), an explicit `group_ids: []` collapses to the same
  behavior as omitting it — **all groups**.

If you need "zero rows" as a filter result, use one of the four tools in the first list, or check
the tool's own `ToolSpec` description in
[`crates/service/src/mcp/tools.rs`](https://github.com/liminisapp/liminis-context-graph/blob/main/crates/service/src/mcp/tools.rs)
before relying on `[]` to mean "nothing" — on the other six it doesn't.

This section is the single, central statement of that contract; individual tool entries on this
page don't restate it.

### Time: event time vs ingest time (`reference_time`, `ingested_at`)

The graph keeps two separately stored clocks, so a record can be both *old in the world* and
*new to the graph* — a months-old conversation ingested today, or a document chunk dated by its
content rather than its file mtime.

| Clock | Key | Set by | Meaning |
|---|---|---|---|
| **Event time** | `reference_time` (request) → `valid_at` / `created_at` (stored) | the caller; defaults to *now* | When the content happened. `knowledge_process_chunk` validates it as RFC-3339 and writes it to the episode's `created_at` and `valid_at` and to the `created_at` of the entities and edges it extracts; time-ordered reads (episode ordering, paging cursors) key on it. |
| **Ingest time** | `ingested_at` (response only) | the service clock — **never caller-supplied** | When this service first wrote the record. |

`ingested_at` is on episodes, entities and edges, in the same timestamp format and timezone
convention as `created_at` (`YYYY-MM-DD HH:MM:SS` on reads). It is an additive key: it appears on
every read that returns these records (`knowledge_get_episodes`; `find_entities`,
`resolve_entity`, `list_entities`, `get_nodes_by_group`, `get_entities_by_source`,
`get_entity_neighbors`; `find_relationships`, `list_relationships`, `get_edges_by_group`,
`get_edges_by_uuids`) and is accepted in the `fields` projection of `get_episodes` and
`list_entities`. Any `ingested_at` passed in a request is ignored.

- **First write wins.** The value is set once. A later episode that matches or updates an existing
  entity or edge, `knowledge_merge_entities` (the surviving entity keeps its own value), and
  re-asserting an existing entity or edge do not change it. A record re-created under a new uuid by
  a correction (`same_as`, endpoint rewrite) carries the original edge's `ingested_at`: the graph
  learned that fact when it was first written.
- **Direct assertions** (`assert_entity`, `assert_relationship`) get the service clock at the call,
  independent of any `valid_at` supplied.
- **Replay never restamps.** `knowledge_rebuild_from_wal`, WAL-tail and startup recovery,
  checkpoint restore and `knowledge_dump_wal` compaction all carry the recorded value through.
- `created_at` / `valid_at` mean exactly what they did before. (Edge reads still leave `created_at`
  empty, as before; only `ingested_at` was added.)

**Upgrading an existing graph.** Records written before this feature have no stored ingest time, so
on the first start of an upgraded service each one is backfilled once, from the best available
signal, in this order:

1. the `ts` of the WAL entry that created the record (the first native `CREATE` line for its uuid
   under the group WAL directories) — a true ingest time;
2. otherwise the record's own `created_at`.

Signal 2 is **approximate**: for a record written before this feature, `created_at` was the
event time, so a backfilled value can predate the real ingest. It is used where the WAL no longer
holds a creating line — typically after `knowledge_dump_wal` compaction (a dumped line's `ts` is
the compaction time, which says nothing about when the record was learned, so it is never used) or
after WAL files were pruned. The backfill is idempotent and never rewrites a record that already
has an ingest time. See [ADR-0673](adr/0673-ingest-time-separate-from-event-time.md).

### Belief time: corrections supersede, they do not delete (issue #705)

Alongside event time and ingest time (above) the graph records **when it stopped believing**
something. Belief is *transaction time*; it is deliberately separate from validity
(`valid_at` / `invalid_at`, the world's clock). When a document is corrected, the removed fact was
never true, so no supersession path ever writes `valid_at` or `invalid_at`: **correction ends belief
and leaves validity alone.** See [ADR-0705](adr/0705-belief-time-separate-from-validity.md).

**Corrections.** Resubmitting a `chunk_id` to `knowledge_process_chunk` supersedes that chunk's
earlier live episodes **in the same `group_id`**, at the new episode's ingest time, and only once the
new ingest has fully succeeded. A failed re-ingest leaves the old episode live. The result gains
`superseded_episode_uuids`. Nothing is deleted. The same `chunk_id` in another group is never touched.
`knowledge_add_episode` never supersedes (its `name` is free-form).

A revision supersedes only episodes `knowledge_process_chunk` can attribute to itself (issue #292,
[ADR-0292](adr/0292-chunk-ingest-never-supersedes-unattributed-episodes.md)). Every episode records
which write path produced it (`Episodic.write_path`: `chunk` for `knowledge_process_chunk`, `episode`
for `knowledge_add_episode` and every other path; not returned by episode reads). A live same-named
episode written by another path — for example a `knowledge_add_episode` whose `name` equals a
`chunk_id` and whose `source_description` mirrors `"{source_file}:{chunk_id}"` — is **left alone**:
it stays live, the call still succeeds as a normal ingest, a `WARN` naming its UUID and
`source_description` is logged, and the result gains `left_alone_episode_uuids` (present only when
non-empty). **Legacy exposure:** episodes written before the marker existed (NULL `write_path`) are
still treated as the chunk's own, so a pre-upgrade `knowledge_add_episode` row whose `name` equals a
`chunk_id` can still be superseded; so can rows written by raw Cypher or other writers that leave the
column NULL. The explicit by-name operations (`knowledge_mark_chunk_removed`,
`knowledge_delete_chunk_episode`) are unchanged and still act on every same-named episode.

To submit one chunk as several calls under one `chunk_id`, send `supersede: false` on every unit but
the last, and pass the earlier units' `episode_uuid`s as `same_revision_episode_uuids` on the last:
they stay live, the older episodes are superseded. **Record a chunk as ingested only after the call
succeeds** — a client that records it first will never resend a failed correction.

**Removal.** `knowledge_mark_chunk_removed {chunk_id, group_ids}` records that a chunk no longer
exists in its document: its live episodes are superseded with reason `chunk_removed`, and nothing is
deleted. `group_ids` is required and non-empty, as for the delete. It returns
`{superseded_count, superseded_uuids, superseded_at}` and is idempotent (a repeat reports 0).
`knowledge_delete_chunk_episode` is unchanged and still destroys the episodes — use it only for
explicit deletion. After a heading rename, ingest the renamed chunks first, then report the old ids
removed; no fact present in both revisions is unbelieved at any instant.

**Episode fields.** Episodes gain `superseded_at` (null while live), `superseded_by` (the superseding
episode, absent for `chunk_removed`), `superseded_by_kind` (`episode`) and `supersession_reason`
(`revision`, `chunk_removed`). They appear on `knowledge_get_episodes` only when set and are accepted
in its `fields` projection. `get_episodes` and `search_passages` still return superseded episodes by
default.

**Edge provenance.** Each extracted edge records the episode that produced it. That is
`producing_episode_uuids` in the `belief` object below — **not** the `episode_uuids` field on edge
reads, which remains the either-endpoint inference (episodes mentioning the source or target) and is
not evidence for that specific relationship. Edges with no producing episode carry a `warrant`
(`assertion` for `knowledge_assert_relationship`, `cross_group` for `knowledge_add_cross_group_edge`)
and are believed until retracted. An extracted edge's `valid_at_source` is `text` when the extractor
read the date from the text and `reference_time` when it defaulted. A direct assertion never
overwrites an extracted edge's fact; it creates a separate edge.

**Reading belief.** `knowledge_find_relationships`, `knowledge_list_relationships`,
`knowledge_get_entity_neighbors`, `knowledge_get_edges_by_group` and `knowledge_get_edges_by_uuids`
accept two optional parameters:

| Parameter | Meaning |
|---|---|
| `believed_only` (boolean) | Drop edges the graph did not believe at `as_of` (default now). |
| `as_of` (RFC 3339) | Evaluate belief at this time. Implies `believed_only` unless that is explicitly `false`, which annotates without filtering. |

Either one adds a `belief` object to each edge: `state` (`believed`, `superseded`, `legacy`,
`support_deleted`, `not_yet_ingested`), `believed`, `producing_episode_uuids`, and `warrant` /
`valid_at_source` when set. An edge is believed at `t` iff it was ingested at or before `t` and either
carries a warrant or has at least one producing episode not superseded at or before `t`; a fact is
therefore believed on `[T1, T2)` when ingested at `T1` and superseded at `T2`, and stays believed
while any one live source still states it. Capped reads (`find_relationships`,
`list_relationships`, `get_entity_neighbors`) over-fetch and filter afterwards, so a filtered read can
return fewer than `num_results`. **Without these parameters every read is unchanged** and includes
superseded edges — making belief the default is a later, documented change.

**Legacy edges.** An edge with neither a producing episode nor a warrant predates this feature. It is
treated as believed and reported as `legacy`; `knowledge_status` returns `legacy_edge_count`. They
cannot be soundly backfilled (endpoint co-occurrence is not provenance, #404), so re-ingest legacy
graphs. Separately, an edge whose producing episodes were all hard-deleted
(`knowledge_delete_episode`, `delete_by_source`, `delete_chunk_episode`) is `support_deleted` and
unbelieved at every time.

### Reviewing a correction: what changed, and what was written under the old belief (issue #706)

Ending belief is half the job. After a correction you also want to see what it changed, and review
work that may have leaned on the fact you dropped. Both reads below are **read-only** (no `valid_at` /
`invalid_at`, no WAL entry, no schema change) and give the same answers after a rebuild. See
[ADR-0706](adr/0706-correction-diff-and-affected-work-reads.md).

The workflow: find the superseded episode (`knowledge_get_episodes` shows `superseded_at`,
`superseded_by` and `supersession_reason`), run the changed-facts read, then run the affected-work
read on each dropped fact.

**`knowledge_get_correction_changes {episode_uuid}`** (MCP `read`). Compares a superseded episode with
its direct superseder (E1 → E2 even if E2 was later superseded by E3):

| Field | Meaning |
|---|---|
| `status` | `superseded`, or `not_superseded` (nothing to compare; both lists empty) |
| `episode` | `uuid`, `name`, `group_id`, `ingested_at`, `superseded_at`, `reason` |
| `replacement` | `{superseded_by, unit_episode_uuids}`, or `null` for `chunk_removed` (no replacement exists) |
| `removed` / `added` (+ `*_count`) | Facts the old episode supports that the replacement does not / the replacement supports that the old episode does not, each with `uuid`, `source_node_uuid`, `target_node_uuid`, `name`, `fact`, `episodes`. Each `removed` fact also carries `still_believed`: `true` when another live episode outside the replacement still supports the edge, so it left the replacement but is not dropped from belief (`knowledge_get_affected_work` on it returns `open`). An `added` fact means only that the old episode did not support it: it may already have been believed through another live episode before the correction |

A fact present on both sides appears on neither. Two edges are the same fact when they share source
entity, target entity, relation name and fact text, so a **reworded fact is one removal plus one
addition**. For a chunk ingested as several units, the replacement side is the whole revision, not only
its final unit; the units are recovered from episodes of the same group and `name`, so a free-form
`knowledge_add_episode` episode that reuses a chunk id as its name in that window is swept in. A heading
rename supersedes one chunk and creates another, so a per-episode diff shows only removals for the old
chunk — a whole-document diff needs document identity (#707).

**`knowledge_get_affected_work {edge_uuid}`** (MCP `read`). For an edge no longer believed, lists the
episodes ingested inside its belief interval `[start, end)` that mention either endpoint entity,
ordered by ingest time:

| Field | Meaning |
|---|---|
| `interval.state` | `closed` (candidates returned), `open` (still believed; affected work is not defined), `no_record` (`cause`: `legacy` or `support_deleted`) or `not_defined` (`cause`: `warranted`) |
| `interval.start` / `end` | Microsecond RFC 3339; `end` is when the last producing episode was superseded. `derived_from_surviving_support` is true when some producers were hard-deleted and ignored |
| `basis` | Always `entity_overlap` |
| `candidates` (+ `count`) | `uuid`, `name`, `ingested_at`, `source_description`, `matched_entity_uuids` |

**Candidates are not dependents.** The list is built from entity overlap, not recorded dependence, so
review it rather than treating it as proof the work used the fact. The window is half-open (an episode
ingested at exactly the superseding instant is outside it), the fact's own producing episodes and the
units of the revision that superseded them are excluded, and the list is neither ranked nor truncated — a hub entity over a long window returns many
episodes. Any state other than `closed` returns no candidates *and says why*, so an empty list never
means "nothing affected" by accident. Both reads take a uuid, derive the group from the record, and
return an error for a missing or unknown uuid.

### Document supersession: a new document replaces an old one (issue #707)

A document is sometimes *replaced* rather than edited — last year's decision record superseded by a
new one. That is a relation between two documents, and it differs from a correction in one way: the
old decision **was true** until it was replaced. So the old facts stop being *believed* when the
supersession is recorded, but are reported as *in force* until the date the new document took
effect. The two axes stay separate. See [ADR-0707](adr/0707-document-supersession.md).

**Document identity.** `knowledge_process_chunk` and `knowledge_add_episode` accept an optional
`doc_id`: the stable identity of the logical document an episode came from, unique within its
`group_id` (path-valued today). It is stored on the episode and returned by `knowledge_get_episodes`
when set. It is never defaulted from `source_file`; an episode without one cannot take part in
supersession, and omitting it changes nothing for existing callers. Graphs written before this
release have none — re-ingest them.

**Recording a supersession.** `knowledge_supersede_document {group_id, superseding_doc_id,
superseded_doc_id, effective_at}` (write scope) records "A supersedes B, effective `effective_at`"
within one group. `effective_at` is an RFC 3339 timestamp **with a timezone offset**; it may be in the
past (the retroactive case) or the future. Supersession is always explicit — the graph never infers it
from topic similarity. The call:

- writes one durable record per superseded document (a document has at most one superseding
  document; express successive replacement as a chain), and
- supersedes B's live episodes in that group with `superseded_by_kind` `document`, reason
  `document_superseded`, `superseded_by` A's `doc_id`, at the service-clock recording instant — or at
  `effective_at` when that is later, so a decision with a future effective date stays believed and in
  force until then. A correction to B's chunk before that date still ends belief at the correction.

It returns `{success, already_recorded, group_id, superseding_doc_id, superseded_doc_id,
effective_at, recorded_at, superseded_count, superseded_uuids}`. Episodes of B ingested *after* the
recording are not marked. Facts A also asserts stay believed through A's own episode. A recorded
supersession is not undone by later corrections to A.

It is rejected with no side effects (nothing written, WAL untouched) when A and B are the same
document, when either is unknown in the group (no episode carries that `doc_id`), when the relation
would form a cycle, when `effective_at` has no timezone, or when B is already superseded by a
*different* document or effective date. Recording the identical relation again is a no-op
(`already_recorded: true`). `valid_at` / `invalid_at` are never written. The record is in the WAL, is
carried by dump compaction and checkpoint restore, and is removed by `knowledge_delete_by_group`.

**Reading it.** On opted-in relationship reads (`believed_only` / `as_of`, above) the `belief` object
gains:

| Field | Meaning |
|---|---|
| `in_force` | Whether the fact was in force at the evaluated time, **independently of `believed`**. `false` once the superseding document's effective date has been reached; `true` otherwise; `null` for a fact that was corrected (it was never true, so it makes no validity claim). An edge is in force while any producing episode is. |
| `supersession` | Present once the fact's producing episodes are all superseded; not time-dependent, so it classifies the fact from any read. `cause` is `corrected` (revision or removal) or `document_superseded`. For the latter it also carries `superseded_by_document`, `in_force_until` (the effective date) and `recorded_at`; `superseded_at` is when belief ended. |

So between the effective date and the recording date an as-of read shows the old fact `believed` with
`in_force: false` — the graph had not yet learned otherwise. Believed-only filtering is unchanged: it
filters on belief, not on being in force, and there is no in-force-only filter yet.

### Document mode: send the whole document, the server does the diff (issue #785)

`knowledge_process_chunk` makes you chunk a document yourself and tell the server what changed. A
section you stop sending stays believed for ever, a heading rename orphans the chunk ids under it,
every chunk you resend is re-extracted, and the extractor never sees the document's title or the
chunk's heading path. `knowledge_process_document` removes all four costs: you send the **whole
document** every time, and each call is a **completeness claim** — "this is document `doc_id`, in
full". See [ADR-0785](adr/0785-document-mode-revisions-and-commit-boundary.md).

| Param | |
|---|---|
| `doc_id` | **Required.** The document's identity within the group; every call diffs against this document's last committed revision. |
| `content` | **Required.** The whole document (markdown; other text is chunked best-effort). |
| `group_id` | Default `liminis`. |
| `title`, `source_path` | Given to the extractor as context and recorded on the revision; `source_path` (or `doc_id`) prefixes each episode's `source_description`. |
| `origin` | As for `knowledge_process_chunk` (issue #752); applies to every episode, and a different origin for a known `doc_id` is refused with `origin_conflict:`. |
| `effective_at` | Recorded on the revision only — never `valid_at`, never belief time. |
| `reference_time` | Extraction reference time, as in chunk mode; defaults to now. |
| `attributes` | JSON object recorded on the revision record and on every episode the call extracts. An unchanged section's episodes are not rewritten, so they keep the `attributes` they were extracted with; the latest revision record holds the document's current value. |

**What the server does.** It chunks the document by section (a heading plus its body up to the
next heading of any level; text before the first heading is the preamble), splitting a section over
`LCG_DOCUMENT_CHUNK_MAX_CHARS` into units. It then matches each section against the previous
revision by the hash of its exact text: **unchanged sections are skipped without extraction**, so
re-sending an identical document with the same metadata makes zero extraction calls and writes
nothing (`status: "unchanged"`). Moving a section is free. Editing a section re-extracts only that section. Renaming a
heading re-extracts only the section whose heading line changed, never its subsections, because the
new heading can change how the body reads. Each extraction receives a `<DOCUMENT_CONTEXT>` block
(title, source path, origin, the section's heading path and a document map) marked as context, not
source text. Facts are cited only to their own section's episode.

**Commit.** New and changed sections are written *pending*. Only when every one of them has
succeeded does one commit step supersede the changed sections' old episodes (reason `revision`) and
retire the sections that are gone (reason `chunk_removed`); you never send removals. **A failed or
cancelled call retires nothing**: the previous revision stays fully believed, and its pending
episodes are never believed (they read `not_yet_ingested` on belief reads). Retry with the same
content and the diff runs against the last *committed* revision. The first document-mode ingest of a
`doc_id` also supersedes that document's live chunk-mode episodes. Concurrent calls for one
document are serialized.

**Response.**

| Field | |
|---|---|
| `status` | `committed`, or `unchanged` for a byte-identical resubmission with the same `title`, `source_path`, `effective_at` and `attributes`. Identical content with different metadata commits a new revision with zero extraction calls, which records the new metadata and changes no belief. So does identical content whose stored spans are behind the latest revision (a crash cut the previous commit's re-stamps): the resend re-stamps them. |
| `revision_id`, `previous_revision_id`, `committed_at`, `chunker_version` | The server-minted revision and its predecessor. |
| `extraction_calls` | Units extracted by this call (0 when nothing changed). |
| `chunks[]` | Per section: `index`, `name` (stable across revisions), `heading_path`, `byte_start`, `byte_end`, `content_hash`, `classification` (`added` / `changed` / `unchanged`), `units`, `episode_uuids`, `previous_episode_uuids`. |
| `removed[]` | Sections gone from the document: `name`, `heading_path`, `episode_uuids`. |
| `superseded_episode_uuids`, `retired_episode_uuids` | Superseded with `revision`, retired with `chunk_removed`. |
| `mode_transition_superseded_episode_uuids`, `stale_superseded_episode_uuids` | Chunk-mode episodes of the document replaced on transition; overlap left by an interrupted earlier commit, repaired now. |
| `nodes_extracted`, `edges_extracted`, `duration_seconds` | Totals. |

With `_progress_token`, progress lines stream before the result: `{"type":"progress","phase":
"chunked"|"extracting"|"committing", "chunk_index", "chunk_count", "classification", "unit",
"units", "extracted", "total"}`.

**Episode fields.** A document-mode episode read by `knowledge_get_episodes` also carries
`revision_id`, `revision_committed_at` (absent while pending), `byte_start` / `byte_end` (its span
in the content of revision `revision_id`: `content[byte_start..byte_end]` is the episode's text),
`heading_path` and `chunk_hash`. A live episode always names the latest committed revision: an
unchanged section is not re-extracted, but each commit re-stamps its episodes with the new
`revision_id`, span and `heading_path`, so the span is valid against the document you last sent. A
superseded episode keeps the span of the last revision it was live in. Episodes from every other path carry none of these, so chunk-mode reads are unchanged.
Its `write_path` is `document`, so a chunk-mode revise of the same name never touches it.

**Limits.** A document over `LCG_DOCUMENT_MAX_BYTES` (default 512 KiB) is refused with
`document_too_large` before any work. An empty, whitespace-only or headings-only document is refused
with `empty_document`. To retire a whole document use `knowledge_supersede_document`,
`knowledge_mark_chunk_removed` or `knowledge_delete_by_source`.

**`knowledge_preview_chunks`** (read scope) takes `content` and returns exactly the chunking
`knowledge_process_document` would use — `chunks[]` with `heading_path`, `byte_start`, `byte_end`,
`content_hash`, `char_count` and each unit's span, plus `document_map` — touching neither the graph
nor the WAL. A client never needs its own chunker.

**Cut-over.** A client moving from its own chunker to document mode sees each section re-extracted
once (different boundaries are a new revision). The change is additive under ADR-0705 and visible
through `knowledge_get_correction_changes`; section names are stable from then on.

### Episode origin: derived pages never corroborate themselves (issue #752)

Systems that keep an "LLM wiki" write agent-authored summary, topic and synthesis pages back into the
same notebook that holds the human's notes. Indexed like any other note, such a page returns a fact
the agent *derived* from note A as a second, apparently independent episode supporting it: support
counts inflate, retrieval re-reads the agent's own pages as evidence, and an error in a page
propagates. **Origin** is the field that lets a client say which is which. See
[ADR-0752](adr/0752-episode-origin-and-corroboration.md).

**Vocabulary.** `knowledge_process_chunk` and `knowledge_add_episode` accept an optional `origin`:

| Origin | Meaning | Counts as corroboration |
|---|---|---|
| `primary` | a human-written note (**the default**) | yes |
| `external` | clipped or imported third-party material | yes |
| `derived` | an agent-written page or a consolidation | no |
| `answer` | a filed Q&A response | no |

The set is closed: any other value is rejected before anything is written (no episode, no WAL entry),
with an error naming the allowed values. An episode with no `origin` is `primary`, so a client that
never sends the field sees no change. The value is stored on the episode, written to the WAL, carried
by dump compaction, and restored by `knowledge_rebuild_from_wal`. A WAL written before this feature
replays with every episode `primary`. `knowledge_get_episodes` reports the origin of an episode that
has one, and `knowledge_search_passages` reports it on every passage (`primary` when none was
recorded).

> **Origin is a claim, not an authenticated attribute.** LCG records and enforces what the caller
> declares; it does not detect a misdeclared origin.

**Support, not a head count.** Relationship and entity reads (`knowledge_find_relationships`,
`knowledge_find_entities`) carry two additive fields:

```json
"support": {"primary": 2, "external": 1, "derived": 3, "answer": 0},
"corroboration": 3
```

`support` counts **distinct source documents** per origin: a `doc_id` is one document, however many
chunks it was split into, and an episode with no `doc_id` counts as its own document. `corroboration`
is `primary + external`; a `derived` or `answer` episode is a *citation* (it stays linked for
provenance) and never adds to it. Counts are over episodes that are live now, so a corrected episode
stops contributing. On `knowledge_find_relationships`, when a belief query (`as_of`) is given, liveness
(for both `support` and the `origins` filter) is evaluated at that `as_of` instant instead, so it agrees
with the `belief` block. For a relationship, `support` is derived from the edge's producing `episodes` and
those of its **same-fact siblings** — edges with the same endpoints and relation name. (Ingest does
not deduplicate edges, [ADR-0705](adr/0705-belief-time-separate-from-validity.md), so a page that
restates a fact produces a second edge; the grouping happens at read time.) It never comes from the
either-endpoint `episode_uuids` inference. Supply a `doc_id` for notes split into chunks: without one,
each chunk counts as a separate document.

**Filtering.** `knowledge_find_relationships`, `knowledge_find_entities` and
`knowledge_search_passages` accept `origins`, a non-empty array drawn from the vocabulary; an empty
list or unknown value is rejected. A relationship matches if one of *its own* live producing episodes
has a listed origin, an entity if one of its live mentioning episodes does, a passage if its episode
does. Asserted facts, entities with no episode at all, and pre-#705 edges count as `primary`. Omitted,
nothing changes. `origins: ["primary", "external"]` is what an agent should send while refreshing its
own page. The filter runs after retrieval (relationships and entities over-fetch ×3; passages
oversample the nearest-neighbour probe; in hybrid mode the filter also runs inside the full-text query), so a selective filter can return fewer than `num_results`.

**Derived content never overrides primary content.** A `derived` or `answer` submission:

- never rewrites the summary of, or merges by embedding or LLM verdict into, an entity that has a live
  `primary` or `external` mention. An exact or identical-normalized name match still *links* the
  episode to the entity (provenance) but leaves its summary alone; an embedding or judged match keeps
  the extracted entity separate. An entity with no mentioning episode (`knowledge_assert_entity`) is not
  protected. The guard is best-effort against a concurrent primary ingest (the ADR-0029 trade-off).
- never invalidates a fact: ingest does not do that for any origin, so a derived page saying
  "X uses MySQL" is stored as its own derived-only fact beside the primary "X uses Postgres", which
  stays valid and believed. The conflict is visible: the new fact has `corroboration: 0` and is
  excluded by `origins: ["primary", "external"]`.
- is **refused** — error message prefixed `origin_conflict:`, code `-32000`, nothing written, no WAL
  entry — when it would end belief in a `primary` or `external` episode: a `knowledge_process_chunk`
  whose `chunk_id` has a live primary or external episode (whatever `supersede` says); a
  `knowledge_mark_chunk_removed` with `origin: "derived"` or `"answer"` on such a chunk (the removal
  path takes an optional `origin`; omitted means `primary`); or a `knowledge_supersede_document` whose
  superseding document is derived and whose superseded document is primary or external. Primary
  content may supersede derived content freely by `chunk_id` revision (no `doc_id` involved) or by
  `knowledge_supersede_document`.
- is refused when its `doc_id` already carries a different origin: one document has one origin. This
  applies in both directions, so a `primary` submission cannot take over a `derived` `doc_id` by reusing
  it. To replace an agent-written document with a human-written one, use a new `doc_id` and
  `knowledge_supersede_document`, or re-stamp the old document with `knowledge_stamp_origin`.

A rebuild cannot re-open a protected fact: these decisions are baked into the statements the WAL
records.

**`knowledge_stamp_origin`** (admin scope) stamps origin onto *existing* episodes, for a graph that
predates the field and so reads entirely `primary`. It re-extracts nothing and calls no LLM or
embedder.

```json
{"group_id": "liminis", "dry_run": true, "rules": [
  {"origin": "derived", "source_path_prefix": "wiki/"},
  {"origin": "derived", "attributes_kind": "knowledge"}
]}
```

Rules are evaluated in order and the first match wins; each needs at least one selector, and selectors
within a rule are ANDed. `source_path_prefix` matches the start of the episode's `source_description`
(`<source_file>:<chunk_id>`); `attributes_kind` matches `attributes.kind` exactly. An episode with a
`doc_id` pulls in the rest of its document, and rules that would give one document two origins are
refused. The reply is `{success, dry_run, group_id, matched, changed, unchanged, changes:
[{uuid, doc_id, from, to}], truncated}` (`changes` is capped at 200; the counts are exact).
`dry_run: true` writes nothing. A repeat run changes nothing and writes no WAL entry. The stamp is
fixed-valued per episode, so it survives rebuild and overlapping tail replay. It is not retroactive:
supersessions and invalidations already made are not revisited. Episodes with
`attributes.kind = "knowledge"` are **not** treated as derived until you stamp them.

**Operating an LLM wiki.** Send `origin: "derived"` (and a `doc_id`) for every page the agent writes
and `"external"` for clipped material; leave notes unmarked. Search with
`origins: ["primary", "external"]` while the agent refreshes a page. To adopt this on an existing
notebook, run `knowledge_stamp_origin` as a dry run, check `changes`, then run it for real.

### Query variants: `knowledge_search_passages` `queries` (issue #802)

`knowledge_search_passages` takes one required `query`. A question that *describes* a thing instead of
naming it shares few words with the note that answers it, so a direct search can come back thin. The
caller, usually an LLM agent, can write rewrites of its own question and pass them as the optional
**`queries`** array. LCG adds no model call of its own: the variants are the caller's.

```json
{"query": "Whose sign-off lets someone get around the rule covering new-hire induction?",
 "queries": ["onboarding policy exception approver",
             "onboarding policy",
             "The policy exception approver for onboarding is a named person."]}
```

Good variants are keywords the note itself would use, a likely name for the thing described, and a
one-sentence hypothetical answer. Use them when a direct search came back thin or the question
describes something rather than naming it.

**Guidance for callers.**

- **Keep the named entities.** Every variant must keep the question's named entities (people, projects,
  policies, suppliers) and rephrase only the rest. A variant that drops the entity ("supplier uptime
  guarantee percentage" for a question about Lumen Core's supplier) matches every company's note for that
  attribute equally and pulls the right note down.
- **No placeholders.** Do not send template answers such as `[supplier name]`, `[Name]` or `X%`; they add
  a noise token that matches no note.
- **Use variants where they help.** They help most for described-not-named lookups and for exact passage
  lookups. Do not send them for multi-part questions whose entities are already explicit: on the v2
  eval these lose to the plain search (see below).

**Known regression.** On the v2 retrieval eval, with the committed fixture, variants lower multi-hop
nDCG@10 by 0.062 [-0.107, -0.026] (vector) and 0.059 [-0.097, -0.027] (hybrid) against the same config
without variants, while raising passage-lookup nDCG@10 by about 0.11 and, in hybrid mode, paraphrase by
0.082 [+0.017, +0.153]. Overall the difference is within noise. `queries` is opt-in, so callers that do not
send it are unaffected. The cause and the planned mitigation are in
[ADR-0802](adr/0802-caller-supplied-query-variants-weighted-rrf.md).

**Validation** (an error naming the broken rule; nothing is silently truncated):

| Rule | Value |
|---|---|
| Shape | an array of strings; absent, `null` and `[]` all mean "no variants" |
| Count | at most **5** |
| Each entry | non-empty after trimming whitespace |
| Total length | at most **2000** Unicode scalar values (`chars()`) across the trimmed entries |

Variants are trimmed; `query` is not. A trimmed variant equal (case-sensitive) to `query` as sent
or to an earlier variant is dropped before searching, so repeating a phrase never boosts a result. `query` is
compared verbatim: if it carries surrounding whitespace, a variant equal to its trimmed text is not dropped and
is searched as a near-identical second query, so send `query` without stray whitespace.

**Execution.** `[query, ...queries]` are embedded in one batched query-role call. Each query then runs the
request's **whole retrieval pipeline**, concurrently and each on its own connection: the same `mode`
(`vector`, or `hybrid` with BM25 + vector fused per query, so a variant's keywords reach BM25), `group_ids`,
`origins`, probe size and `min_score`. `group_ids` and `origins` therefore bind every query: a variant can never
surface a passage from a group or origin that would be excluded for `query`. The `min_score` cosine floor binds
each query's **vector** list only; in `hybrid` mode a variant's BM25 hits are not subject to it (as in
single-query hybrid), so a variant keyword can add a lexical-only passage whose cosine is below `min_score`. The per-query rankings are then fused with weighted Reciprocal Rank Fusion
(`weight / (rank + 60)`, UUID tie-break): `query` weighs **2** (`PRIMARY_QUERY_WEIGHT`, the qmd choice) and the
variants **share a total weight of 1** (`VARIANT_BUDGET_WEIGHT`, split equally: 1 each for one variant, 0.2
each for five). However many variants there are, `query` keeps at least **2/3** of the weight, so variants
widen the candidate set but together can never outvote the primary query. Belief-aware ranking (`belief_aware`;
on by default in both modes since #806; `belief_aware: false` opts out) runs **once, on the fused candidates**, never per variant. The first
`num_results` of the result are returned.

**Response.** With `queries`, every passage also carries **`matched_queries`**: ascending indices into the
*caller's* `[query, ...queries]` of the queries that retrieved it (0 is `query`; a dropped duplicate's index is
simply absent). `score` is the **fused rank score** (rank-ordered, so it stays monotone through belief
demotion), and `similarity` is the best cosine across the vector legs that matched the passage (absent for a
lexical-only hit), exactly as in hybrid mode. `min_score` floors each query's vector list before fusion.
Without `queries` (or with `[]` / `null`) the response is byte-for-byte what it was before: no
`matched_queries` key.

Why a fixed variant budget: with one weight per variant, five variants outweighed `query` 5 : 2, and on the v2
retrieval eval that cost 0.046 overall nDCG@10 (ADR-0802). Rerank-on-fused-candidates and
`knowledge_graph_search` `queries` are follow-ups
([ADR-0802](adr/0802-caller-supplied-query-variants-weighted-rrf.md)).

### Notebook health: `knowledge_lint` (issue #755)

Wiki-style notebooks need a health pass — contradictions, stale agent-written pages, thinly
supported facts, orphans, missing links. Doing it by having an LLM re-read the notes is slow,
nondeterministic and grows with the notebook. LCG already holds the structure (belief time,
document supersession, episode origin), so `knowledge_lint` answers the same questions as graph
queries: **no LLM call, no embedding-model call, read-only, deterministic**. It reports; it never
merges, edits, deletes or suggests a fix — that stays with the caller. `read` scope, `ReadOnly` for
`--writable-groups`.

**Request** (all optional):

| Parameter | Meaning |
|---|---|
| `group_ids` / `group_id` | Groups to lint, as `knowledge_get_episodes`. Omitted ⇒ every group; every finding states its `group_id`. A group that does not exist is an empty report, not an error. |
| `categories` | Subset of the six below (default all). An unselected category costs no work and is absent from `summary`. |
| `limit` | Findings per page, `1..=500` (default 50). |
| `cursor` | Opaque keyset cursor; pass back `next_cursor` until it is `null`. Bound to the groups, categories, thresholds and single-valued set it was issued for; replaying it against another query fails with `invalid cursor: …`. |
| `single_valued_relations` | Relation names (matched against a fact's `name` or `relation_type`, trimmed and case-insensitive) that hold at most one target per source, e.g. `["lives_in"]`. Needed for `contradiction`. |
| `near_duplicate_threshold` | Similarity in `(0, 1]` at or above which two entities are a candidate merge. Default `0.9`. |
| `min_mentions` | Distinct documents that must mention an entity before it is reported as lacking a page. Default `3`. |

Invalid input (unknown category, `limit` outside `1..=500`, threshold outside `(0, 1]`,
`min_mentions < 1`, malformed cursor) is rejected with the usual `-32000` before any work is done.

**Response**: `{findings, count, next_cursor, summary, effective}`. `next_cursor` is always present
(`null` on the last page). `effective` echoes the categories, `limit`, `near_duplicate_threshold`,
`min_mentions` and normalised `single_valued_relations` that were used. `summary` is computed over
the **whole scope on every call**, so it is identical on every page of one report and its counts
equal the findings you page through:

```json
{"summary": {"categories": {
   "weak_support": {"status": "ok", "count": 12},
   "contradiction": {"status": "unavailable", "count": 0, "reason": "no_single_valued_relations"},
   "near_duplicate_entity": {"status": "ok", "count": 3, "skipped_blocks": 0}},
  "total": 15}}
```

`unavailable` (with a `reason`) means the category could not be evaluated; it is not the same as
`ok` with count `0`, and it is not counted as healthy. Findings are ordered by category (the fixed
order below), then by `id`. The response carries no timestamp: an unchanged graph returns
byte-identical output.

**A finding**: `{id, category, group_id, reason, details, citations}`. `details` holds the numbers
behind the decision. `citations` carries sorted lists — `episodes` (uuids), `documents` (`doc_id`s;
an episode without one is cited by episode only), `entities` (`{uuid, name}`) and `facts`
(`{uuid, source, target, relation}` with endpoint names) — each capped at 25 with a `*_total`
beside it, enough to open the cited notes and check the claim.

**Stable ids.** `id` is the SHA-256 of the category, the group and the identities of the subject
items (stored uuids, `doc_id`s and the designation) — never of the time of the run, the page
position or the row order. It is unchanged by unrelated ingests, a WAL rebuild or a checkpoint
restore, and a finding that disappears and later reappears carries the same id, so an app can track
and dismiss findings over time. LCG stores nothing about findings. A finding whose subject changes
(a contradiction side replaced; a document re-ingested under new fact uuids) is a new finding.

| Category | `reason` | What it reports |
|---|---|---|
| `contradiction` | `single_valued_conflict` | Two currently-believed facts with the same source and a relation you designated single-valued, but different targets. One finding per pair of targets; sibling edges to one target form one side. `details.sides` carries each side's `support`. `unavailable` (`no_single_valued_relations`) without a designation. |
| `stale_derived_page` | `lost_corroboration` | A document made only of `derived` episodes that restates a fact whose primary/external support has since been corrected (`corrected`), removed (`removed`), had its document superseded (`document_superseded`) or otherwise superseded, so nothing live backs it any more. One finding per document; `details` has `affected_facts` / `total_facts` and the formerly supporting episodes with their cause. |
| `weak_support` | `derived_only`, `single_source` | A believed fact with zero, or exactly one, corroborating (primary + external) source documents. `details.support` is the per-origin breakdown. Facts with no producing episode (asserted, cross-group, legacy) are `primary` and are not reported. |
| `orphan_entity` | `support_lost`, `never_supported` | An entity with no live mention and no believed relationship. `support_lost` cites the superseded episodes that once mentioned it. |
| `near_duplicate_entity` | `similar_name` | Two entities of the same group and kind whose normalised names (lowercase, punctuation stripped, tokens sorted) have a Levenshtein ratio at or above the threshold. Each pair appears once, ordered by uuid. `details.score` is the name score; `details.embedding_cosine` is the cosine of the stored `name_embedding`s when both exist (informative only — never a gate, and no embedding is computed). |
| `unlinked_mention` | `unlinked_mention`, `missing_page` | A document that mentions an entity with a page but does not link to it; or an entity mentioned by at least `min_mentions` documents that has no page. See the convention below. |

**Stale pages and corrections.** Detection reads supersession state at the time of the call, so a
correction (ADR-0705) or document supersession (ADR-0707) is reflected as soon as it is recorded,
with no re-ingest of the derived page. LCG does not record which note a derived page was written
from; the relation is inferred through shared facts using the same-fact key of
[ADR-0752](adr/0752-episode-origin-and-corroboration.md). A fact that never had primary support is
`weak_support`, not stale. A corroborating episode that was hard-deleted has no readable origin and
is ignored.

**Contradictions are deliberately narrow.** LCG stores no contradiction signal (ingest never
invalidates one fact because of another), and without an LLM the only non-noisy deterministic rule
is the single-valued-relation one. "X is in Berlin" against "X moved to Paris" under different
relations is out of reach.

**The page/link convention (`unlinked_mention`).** LCG does not model notebook pages or links, so
the category reads a convention in episode `attributes` (ADR-0528), parsed only when the category is
selected and never by text matching:

- `{"page_for": "<entity uuid or name>"}` on any live episode makes that episode's document the
  entity's page (a uuid is exact; a name must match one entity in the group, case-insensitively).
  The episode must carry a `doc_id`: `links` entries are `doc_id`s, so a doc-less page could never be
  linked to and its declaration is ignored.
- `{"links": ["<doc_id>", …]}` on any live episode opts its document into link tracking (an empty
  array counts); the union over its live episodes is its outgoing link set.

The category is `unavailable` — reason `no_page_data` or `no_link_data` — until the scope carries
both keys somewhere. Whether the notes app writes this convention is for the app to agree.

**Cost model.** Because counts are exact on every call, each call is a full pass over the selected
categories: chunked, uuid-keyed scans (no content, no embeddings) into memory, then pure analysis;
the cursor only selects which slice of the ordered result is returned, and only that page is
hydrated with entity names. Time grows near-linearly with group size (`benches/lint.rs` measures
1k/10k/50k episodes; run it with `cargo bench --bench lint`). The read lock is held only while the
snapshot loads, so writers do not queue behind the analysis; the page may reflect a slightly newer
graph than the summary. Near-duplicate candidates come from a deterministic blocking scheme (shared
long token or shared 3-character prefix of the sorted-token name) so work stays bounded; a block of
more than 200 entities is skipped and counted in `summary…skipped_blocks`, and a near-duplicate
sharing no block with its twin is not found. Likewise a `(source, relation)` with more than 50
distinct believed targets is skipped for `contradiction` (it would yield one finding per pair) and
counted in `summary…skipped_subjects`. Findings depend on the evaluated instant (a
future-dated document supersession takes effect when its time arrives), so determinism holds for an
unchanged graph at the same instant.

**Origin is a claim.** Lint reads origin exactly as ADR-0752 does: the caller's label, never
authenticated. Derived and answer content never counts as corroboration in any category. A group in
degraded mode fails like every other read. Start with the
[Episode origin](#episode-origin-derived-pages-never-corroborate-themselves-issue-752) and
[Belief time](#belief-time-corrections-supersede-they-do-not-delete-issue-705) sections; design in
[ADR-0755](adr/0755-knowledge-lint-deterministic-health.md).

### Bulk reads: group scope, paging, projection, prefix (issue #667)

`knowledge_get_episodes` and `knowledge_list_entities` are the two bulk reads, and a large graph
can overflow an MCP client's tool-result limit if read in one call. Both accept the same optional,
additive parameters; with none of them the response is exactly `{<collection>, count}` as before.

| Parameter | Tools | Meaning |
|---|---|---|
| `group_ids` (array) | both | Groups to read. `knowledge_get_episodes` also accepts the single-group alias `group_id`; both given ⇒ their deduplicated union. **Neither given ⇒ every group** (episodes used to default to `liminis`). An explicit `[]` also means all groups. |
| `last_n` / `num_results` | episodes / entities | Page size (defaults 50 / 500, unchanged). |
| `cursor` | both | Opaque keyset cursor. Send `""` for the first page, then send back each response's `next_cursor` until it is `null`. |
| `fields` (array) | both | Return only these keys per item. Episodes: `uuid, name, group_id, created_at, ingested_at, source, source_description, content, valid_at, entity_edges, attributes, superseded_at, superseded_by, superseded_by_kind, supersession_reason, doc_id`. Entities: `uuid, name, group_id, labels, kind, created_at, ingested_at, summary, attributes, episode_uuids, source_descriptions`. Unknown or empty `fields` is an error; embeddings are never returned. |
| `name_prefix` | both | Names starting with this prefix, matched literally. Case-insensitive for entities (via Cypher `lower()`, so non-ASCII folds only as far as lbug does); case-sensitive for episodes. |

`next_cursor` appears only when the request contained `cursor`, `fields` or `name_prefix`; it is
`null` on the last page. Order is deterministic (episodes newest first with a `uuid` tiebreaker;
entities by `uuid` descending) and the cursor is a keyset position, so inserts and deletes between
pages never repeat or skip an item that existed throughout. A cursor is bound to the tool and to
its `group_ids`, `kind` and `name_prefix`; a malformed cursor or one replayed against a different
query fails with `invalid cursor: …` (JSON-RPC `-32000`, like every other validation error here).
`count` is always the number of items in the page. Filtering episodes by `attributes` is not
supported yet. See [ADR-0667](adr/0667-read-path-paging-cursor-and-status-label.md).

`knowledge_status`: the flat `wal` block always describes the **default group's** WAL stream. When
a WAL root is configured it now also carries `"scope": "default_group"`, `"default_group":
"liminis"` and `"see": "wal_groups"`, so `exists: false` there means the default group has no
stream — read `wal_groups` for every group's position. With no WAL root the block is unchanged.

Each `wal_groups.<gid>` entry also carries `zero_vector_rows`
(`{"total": N, "by_kind": {"name": …, "fact": …, "summary": …, "content": …}}`, issue #808): how
many rows of that group are stored with a fallback zero vector because the embedder failed during
replay, and so are missing from vector search. Always present (all zeros when clean), exact, and
durable across restarts; additive, so a client that ignores unknown fields is unaffected. See
[Operations](operations.md#embedding-failures-during-replay-retry-and-the-zero-vector-record).

### Deletion (`delete_chunk_episode`, `delete_by_source`)

> For a chunk that merely disappeared from its document, prefer
> `knowledge_mark_chunk_removed` (see "Belief time" above), which ends belief without destroying the evidence. `knowledge_delete_chunk_episode` keeps its
> explicit-deletion contract unchanged.

**Breaking change in 0.13.2 (issue #406): `group_ids` is required and non-empty on both
`knowledge_delete_chunk_episode` and `knowledge_delete_by_source`.** A call that previously
omitted `group_ids` deleted matching episodes across every group in the workspace; that was
cross-group-unsafe, so as of 0.13.2 an omitted, `null`, or empty `group_ids` is rejected outright
with an error, before any delete runs. **There is no default to fall back to** — a caller
upgrading from a pre-0.13.2 client that relied on omission must start passing its own group(s)
explicitly:

```json
{"chunk_id": "notes-0001", "group_ids": ["liminis"]}
```

```json
{"source_file": "notes.md", "group_ids": ["liminis"]}
```

This is a different contract from the read-tool one above — omitting `group_ids` here never means
"all groups"; it means "reject the call." A `group_ids` naming a group with no matching rows still
succeeds, returning `deleted_count: 0`.

### Direct assertion (`assert_entity`, `assert_relationship`)

`knowledge_assert_entity` and `knowledge_assert_relationship` (issue #379) are a direct write
path: the caller already knows a fact and records it as a single entity or edge, without a prose
round-trip through `knowledge_process_chunk`'s LLM-driven extraction. Use `process_chunk` when
you have unstructured text and want the graph populated by extraction; use the assert tools when
you already know exactly which entity or edge you want to write.

**`knowledge_assert_entity`** accepts `name` (required), `entity_uuid` (optional), `kind`
(optional, see [Entity kinds](#entity-kinds-kind-identity)), `labels`, `summary`, `attributes`, and
`group_id` (default `liminis`).

- **Upsert identity is `(group_id, kind, name)`** (kind defaults to `Entity`), unless `entity_uuid` is supplied. `entity_uuid`, when
  given, is a **strict, group-scoped lookup** — the call fails if no entity with that UUID exists
  in `group_id`; there is no create-under-this-UUID fallback (ADR-0379 Decision 3). A caller
  cannot mint an entity at a UUID of its own choosing.
- **On an update, omitting `summary` or `attributes` clears the previously stored value** rather
  than leaving it untouched — both fields are always overwritten with whatever the call supplies:
  `summary` defaults to `""` if omitted, `attributes` defaults to `"{}"` (an empty JSON object,
  not an empty string) if omitted or non-object, matching how `labels`/`name` are handled.
- **Only `name` is embedded for semantic search** (`name_embedding`). `summary` is stored and is
  full-text searchable (it's part of `Entity`'s `[name, summary]` FTS index), but is **not**
  semantically searchable — a `knowledge_find_entities` vector query will not match on `summary`
  content until issue #470 lands. `attributes` is not indexed at all — full-text or semantic — and
  is retrievable only via direct UUID lookup or `knowledge_query_cypher`.
- **An update never re-embeds `name_embedding` — only a create does** (ADR-0379 Decision 6, forced
  by a lbug constraint: the embedded column sits under an HNSW index once indexes are built, and
  lbug rejects a plain `SET` on an indexed column). Re-asserting an existing entity with a changed
  `name` updates the stored `name`, but `name_embedding` keeps reflecting the *old* name until the
  entity is deleted and recreated. There is currently no supported way to refresh it in place.

**`knowledge_assert_relationship`** accepts `source_name`, `target_name`, `predicate` (all
required), `source_kind` / `target_kind` (optional, see
[Entity kinds](#entity-kinds-kind-identity)), `fact` (optional — auto-derived as `"{source_name} {predicate} {target_name}"` if
omitted), `attributes`, `relation_type`, `valid_at`, and `group_id` (default `liminis`).

- **Upsert identity is `(source_node_uuid, predicate, target_node_uuid, group_id)`**, resolved via
  `find_active_relates_to_uuid` (an invalidated edge is skipped, so re-asserting after an
  invalidation creates a fresh edge rather than resurrecting the old one).
- **Endpoint resolution is strictly scoped to the call's own `group_id` — it never falls back to a
  cross-group search.** If `source_name` or `target_name` doesn't resolve to an entity already in
  that group, the call fails with an error naming
  [`knowledge_add_cross_group_edge`](#cross-group-pointers-add_cross_group_edge-rebind_pointers)
  as the tool to use for connecting entities across groups.
- **`fact` is the field embedded for semantic search** (`fact_embedding`) — not `name`/`predicate`,
  the opposite of `knowledge_assert_entity`. `fact` is also part of `RelatesToNode_`'s `[name,
  fact]` FTS index, so it's both full-text and semantically searchable; `attributes` is indexed
  neither way, same as on the entity side.
- **Same never-re-embed-on-update caveat as `knowledge_assert_entity`** (ADR-0379 Decision 6): a
  re-assert that changes `fact` updates the stored `fact` text but leaves `fact_embedding`
  reflecting the prior text until the edge is deleted and recreated.

- **Both tools stamp `ingested_at` with the service clock** at the moment a record is created,
  regardless of any `valid_at` supplied; see
  [Time: event time vs ingest time](#time-event-time-vs-ingest-time-reference_time-ingested_at).

See [ADR-0379](adr/0379-direct-assertion-conventions.md) for the full rationale behind these
upsert and embedding decisions.

### Entity kinds (`kind` identity)

Entity identity is `(group_id, kind, name)` (issue #615,
[ADR-0615](adr/0615-kind-scoped-entity-identity.md)). `kind` is a single string classifying an
entity (`Topic`, `KnowledgeChannel`, `Team`, …). It is stored in `Entity.kind`, is always one of the
entity's `labels` (appended after `Entity`), and is immutable. The default kind is exactly
`Entity`; every entity written before kinds existed, and every extraction-created entity, is that
kind. A `kind` is trimmed, case-sensitive, non-empty, must not contain U+001F, and must not be the
reserved structural label `Merged`, else the call fails with a validation error. Every entity node in a response carries a `kind` field. So
`Topic "adr"` and `KnowledgeChannel "adr"` are two distinct entities that coexist in one group.

**Omitted `kind`: reads are broad, writes are scoped** (the rule `group_ids` already follows):

| Operation | `kind` given | `kind` omitted |
|-----------|--------------|----------------|
| `knowledge_assert_entity` (write) | resolves/creates exactly that kind | acts on the default kind `Entity` **only** — never touches another kind's same-named node |
| `knowledge_assert_relationship` endpoints (existing entities) | that endpoint resolves exactly within the kind | spans all kinds; more than one matching kind is an ambiguity error |
| `knowledge_resolve_entity` (`name`, `group_id`, `kind?`) | exact lookup, never ambiguous | spans all kinds; one match resolves, several is an ambiguity error, none returns `{found: false}` |
| `knowledge_find_entities`, `knowledge_list_entities` | only that kind | all kinds, each node carrying its `kind` (multi-row reads never pick one, so never ambiguous) |
| `knowledge_add_cross_group_edge` `source_kind`/`target_kind` (foreign endpoints only) | pointer pinned to that kind (`endpoint_kind`) | pointer resolves across all kinds; `ambiguous` once a second kind shares the name |
| `knowledge_merge_entities` `kind` | restricts `canonical_name`/`alias_names` | those names span all kinds; a multi-kind `canonical_name` is an ambiguity error |

**Ambiguity error.** A name-only resolution that matches more than one kind fails with JSON-RPC
error code **`-32002`**; `error.data` is `{"reason": "ambiguous_entity", "name", "group_id",
"candidates": [{"uuid", "kind"}, …]}`. It never picks a candidate. Retry with an explicit `kind`.

**Merges never cross kinds.** `knowledge_merge_entities` refuses the *whole call* (`dry_run`
included, nothing modified) if the canonical and any alias differ in kind, with an error naming the
kinds; `merge_all_by_name` is limited to the canonical's own kind.

**Extraction and kinds.** `knowledge_add_episode`/`knowledge_process_chunk` create kind-`Entity`
entities, except that an entity whose primary type the ontology marks `identity: true` is created
with that type as its `kind` and resolves only within it (see
[Identity-bearing types](ontology.md#identity-bearing-types)). Extraction never merges into an
asserted kind it did not create. For a group whose identity-bearing set changed in a way that would
reinterpret existing entities, `knowledge_add_episode` fails with JSON-RPC error code **`-32003`**;
`error.data` is `{"reason": "identity_set_change_refused", "group_id", "label", "entity_count"}`, and
`knowledge_status` lists the group under `group_identity_refusals`. Nothing is modified; restore
the recorded set (and restart, or call `knowledge_reload_ontology` for the group) or re-ingest the
group from source.
`knowledge_reprocess_entity_types` never changes an entity's kind and adds an additive
`kind_disagreements` array (`entity_id`, `entity_name`, `kind`, `classified_type`) — report-only —
to both its dry-run plan and its result.
Ontology types flagged `extract: false` are never offered as reclassification targets by `knowledge_reprocess_entity_types` / `knowledge_reprocess_relation_types` (nor by `knowledge_canonicalize_relations`); see [Assert-only types](ontology.md#assert-only-types-extract-false).

### Ontology reload (`knowledge_reload_ontology`)

`knowledge_reload_ontology {group_id}` (`admin` scope) discards one group's cached ontology
resolution and re-resolves it, so an edited per-group ontology file takes effect without a service
restart. It returns `{group_id, previous_hash, new_hash, changed, drift, identity_refusal}`; an
identity-bearing-set change on a group that already holds entities of that type is reported in
`identity_refusal` (and enforced as `-32003` on `knowledge_add_episode`) rather than installed
silently. Other groups are untouched and stored data is never modified. See
[Ontology → Reloading a group's ontology](ontology.md#reloading-a-groups-ontology).

### Relation typing (`canonicalize_relations`, `backfill_relation_types`, `reprocess_relation_types`)

Three tools populate an edge's `relation_type`, with different tradeoffs:

**`knowledge_canonicalize_relations`** maps each edge's **existing raw predicate** onto your
ontology's declared `relation_types`. Its behavior has four caveats worth knowing before you
rely on it:

- **`group_id` is required** (issue #447) — candidate selection and the resulting WAL mutations
  are both restricted to that one group; no other group's edges or WAL stream are ever touched. An
  omitted, `null`, or empty `group_id` is rejected before any candidate selection or write, rather
  than falling back to a database-wide rewrite or the default group — there is no supported way to
  canonicalize every group in one call.
- **The primary pass is lexical, over the predicate — not the `fact`.** It matches the edge's
  predicate / current `relation_type` against ontology type names, aliases, and keywords. It does
  **not** read the edge's `fact` sentence, so an edge whose `relation_type` was cleared cannot be
  re-mapped from its fact by this pass.
- **`embedding_threshold` tunes only the fallback promoter** (default `0.7`). The fallback embeds
  each residual edge's `fact` against the ontology types' *descriptions* and force-assigns the single
  nearest type at or above the threshold. Lowering it types more edges, but by nearest-neighbor
  force-fit with **no abstention** — an idiosyncratic fact (e.g. "*X is affiliated with Y*") can land
  on a wrong type (e.g. `HOLDS`).
- **Re-runs are only partly idempotent, and clearing `relation_type` can't be undone by
  canonicalize.** A re-run skips an edge only when it's already at its target — a `Mapped` edge
  already equal to the canonical type, or a residual edge already `UNCLASSIFIED`; an edge whose
  classification *changes* is overwritten (including a previously-assigned type), while
  arrow-named "noise" edges keep any existing predicate. Critically, canonicalize's only input is
  the edge's existing predicate / `relation_type` — if you **null that field to "start clean,"
  canonicalize has nothing to map from and cannot rebuild it.** Snapshot with `knowledge_dump_wal`
  before such an operation.

**Edge direction repair (#816).** When the ontology declares a relation signature
(`source_type`/`target_type`) or `inverse_aliases`, `knowledge_canonicalize_relations` also applies
the ingest-time direction rules to the stored graph, with no re-extraction
([Ontology → Edge direction](ontology.md#edge-direction), [ADR-0816](adr/0816-ontology-edge-direction-check-and-repair.md)):

- An edge whose endpoint entity types fit its relation's signature only when reversed is **swapped**
  (source and target exchanged; the stored `name` is rewritten only if it has the ingest form
  `"A → B"`). An edge retyped through an *inverse alias* (or an inverse keyword hit) is retyped and
  swapped in the same pass. An edge that fits neither way is **never dropped**: it is marked
  `"signature_mismatch": true` in its `attributes` (existing keys such as `original_relation_type`
  are preserved). A stale mark is cleared once the edge fits. Symmetric relations, relations with no
  signature, and edges with an untyped endpoint are skipped.
- `dry_run: true` computes and reports everything and writes nothing.
- The response gains `swapped_count`, `signature_mismatch_count` and
  `by_type: {<RELATION>: {checked, swapped, mismatched}}` (`checked` = edges that could be
  evaluated). `by_type` is `{}` and the counts are `0` for an ontology with no signature and no
  inverse alias, which is processed exactly as before.
- It is idempotent: a second run on a repaired graph reports `swapped_count: 0`. Edges missing
  either endpoint hop (e.g. unbound cross-group pointers) are not checked.
- A swap rewrites the edge's three stored structures (shadow node, direct relationship, two hops) as
  several statements; a crash mid-swap can leave one edge half-wired. Take a snapshot
  (`knowledge_dump_wal`) and preview with `dry_run` before running it on a large graph.

**`knowledge_backfill_relation_types`** (DEPRECATED) does not classify at all — it mints
uppercased fact-prefix pseudo-types (e.g. `THE_SPECIFICATION_DOCUMENT_DEFINES`) for edges with no
`relation_type`, rather than matching against the ontology. Avoid it for building a typed
taxonomy; prefer `knowledge_reprocess_relation_types` below, which supersedes it for that purpose.
Like `knowledge_canonicalize_relations`, **`group_id` is required** (issue #447): candidate
selection and WAL attribution are both restricted to that one group, and an omitted, `null`, or
empty `group_id` is rejected rather than running database-wide or against the default group.

**`knowledge_reprocess_relation_types`** is the relation-side twin of
`knowledge_reprocess_entity_types`: for each in-scope edge, it sends the edge's `fact` and the
ontology's declared relation types (name + description) to the configured extraction LLM and asks
it to pick exactly one type, or honestly abstain. This is the tool to reach for when you want
genuine fact-based classification instead of lexical matching or pseudo-typing:

- **Always reads the `fact`, never the predicate.** Unlike `canonicalize_relations`, classification
  is grounded in the edge's natural-language fact sentence against the ontology's declared menu —
  not lexical/alias/keyword matching on the existing predicate string.
- **A declared ontology relation-type menu is always required.** Unlike
  `knowledge_reprocess_entity_types` (whose `untyped` scope works with no ontology via open-ended
  classification), every scope value (`untyped`, `off_ontology`, `all`) fails with a structured
  `{success: false, error: ...}` if the ontology declares no relation types — there is no
  open-ended fallback (see [ADR-0037](adr/0037-relation-classification-abstention-writes-unclassified.md)).
- **Abstention is an honest, real write of `UNCLASSIFIED`.** If the LLM cannot map a fact to any
  declared type, the edge's `relation_type` is set to the literal string `UNCLASSIFIED` — never a
  force-assigned nearest match. This differs from `knowledge_reprocess_entity_types`, where an
  unclassifiable entity is simply left unchanged (see ADR-0037).
- **`scope`** controls candidates: `"untyped"` (default) — `relation_type` NULL/empty, the same
  predicate `backfill_relation_types` uses; `"off_ontology"` — untyped edges plus edges whose
  `relation_type` isn't a declared type (this naturally covers prior `UNCLASSIFIED` sentinels and
  `backfill_relation_types`'s fact-prefix pseudo-types with no special-casing); `"all"` — every
  edge in the group.
- **Idempotent.** An edge whose computed verdict already matches its current `relation_type`
  (including an edge already correctly `UNCLASSIFIED`) is left unchanged — no write, no WAL entry.
- **`dry_run: true`** returns `would_reclassify_count`, a `plan` array of per-edge
  `{edge_id, fact, old_type, new_type}` entries, and a `breakdown` object counting edges per
  assigned `new_type` (including an `UNCLASSIFIED` count) — without mutating the graph.
- **`dry_run: false` (apply)** returns `reclassified_count`, `unchanged_count`, and — since
  issue #332 — the same `breakdown` object as the dry-run path, so callers can see the per-type
  classification distribution (including how many candidates abstained to `UNCLASSIFIED`)
  without a separate dry-run call. `plan` and `would_reclassify_count` remain dry-run-only, since
  they describe a proposed mutation rather than one that already happened; `breakdown` is always
  present on a successful apply response, as `{}` when there were zero in-scope candidates.

### Semantic search maintenance (`backfill_summary_embeddings`)

**`knowledge_backfill_summary_embeddings`** (issue #470) computes `summary_embedding` for every
entity in `group_id` that has a non-empty `summary`, so entities created before this capability
existed become semantically retrievable by summary paraphrase — not just by name-vector or
full-text match. `knowledge_find_entities` already fuses a summary-vector match into its hybrid
retrieval for entities embedded going forward (both `knowledge_assert_entity` and the extraction
path embed `summary` on creation); this tool is what makes that retrieval available for entities
that predate the capability.

- **Every candidate is unconditionally re-embedded on each call.** There is no cheap way to tell
  "already has a real embedding" apart from "still carries the schema migration's zero-vector
  placeholder" from a stored `FLOAT[]` value alone, so re-running this backfills the same rows
  again. This is safe (idempotent in effect) but not free (an embedder round-trip per candidate,
  same as `knowledge_backfill_relation_types` and subject to the same no-batching caveat, #445) —
  use `dry_run: true` first to see the candidate count.
- **Holds an exclusive lock for the whole run, blocking all other reads and writes.** Unlike most
  admin operations, the summary-vector HNSW index is dropped for the run's duration and rebuilt
  at the end — this is the only way an indexed embedding column can be refreshed for existing
  rows at all (a plain `SET` on an HNSW-indexed column is rejected once the index exists). Prefer
  running this at a low-traffic time, especially for a large group.
- **`group_id` is required** (matching `knowledge_backfill_relation_types`'s convention):
  candidate selection and WAL attribution are both restricted to that one group; an omitted,
  `null`, or empty `group_id` is rejected rather than running database-wide or against the
  default group.
- **A partially-completed backfill is not an error.** Entities not yet processed simply retrieve
  via existing name/lexical behavior, exactly as before this issue — running the tool again
  covers more of the group each time.
- **A `knowledge_assert_entity` re-assert does not refresh `summary_embedding`.** A re-assert
  that changes an entity's `summary` leaves its `summary_embedding` as it was — the vector
  reflects whichever summary was embedded last (at creation, at an extraction merge, or at the
  most recent backfill run), not necessarily the current `summary` text. Re-run this tool to bring
  a changed summary's vector back in sync. (Extraction merges *do* refresh it: when extraction
  folds a new description into an existing entity the merged summary is consolidated into one
  bounded description of at most 600 characters and re-embedded in the same write — ADR-0651.)

### Cross-group pointers (`add_cross_group_edge`, `rebind_pointers`)

`knowledge_add_cross_group_edge` creates an edge whose endpoint(s) may live in a `group_id`
other than the edge's own — the hub/layer-graph topology introduced by issue #369 (see
[ADR-0369](adr/0369-resolvable-cross-group-pointers.md)). Every intra-group edge write
(`knowledge_add_episode`, `knowledge_process_chunk`, and every other existing write path) is
completely unaffected: pointers only ever exist on edges created through this tool.

- **Each endpoint is either a bare UUID or a name to resolve.** `{"uuid": "..."}` names an
  entity already known to live in the edge's own `group_id` — no resolution, no pointer. `
  {"source_group_id": "...", "endpoint_name": "..."}` names a *foreign* endpoint: it is resolved
  by case-insensitive name lookup against that group (the same authority
  `get_entity_by_name_ci_with_scan_fallback` uses for extraction-time endpoint resolution, per
  [ADR-0283](adr/0283-name-index-scan-fallback-for-endpoint-authority.md)), and the edge carries
  a `cross_group_pointers.{src,dst}` object recording the assertion (`source_group_id`,
  `endpoint_name`, plus an optional `endpoint_kind`) and the resolution cache (`resolved_uuid`,
  `bound_at_seq`, `binding_state`). Top-level `source_kind` / `target_kind` (issue #615) pin a
  foreign endpoint to an entity kind: a kind-pinned pointer resolves within that kind only and stays
  `bound` when a same-named entity of another kind appears; a kind-less pointer resolves across all
  kinds and becomes `ambiguous` (no hop) once a second kind shares the name. A kind on a `{uuid}`
  endpoint is rejected. If a pinned kind no longer exists the pointer is `unbound`.
- **A foreign endpoint that doesn't currently resolve is `unbound`, not dropped.** Unlike
  ordinary extraction (which hard-drops an unresolvable endpoint at commit — see
  [ADR-0051](adr/0051-edge-endpoint-salvage-and-deferred-drop.md)), the edge is still created;
  only the hop to that side is missing until the target group is (re)built — which re-binds it
  automatically, see `knowledge_rebuild_from_wal` — or a `knowledge_rebind_pointers` call resolves
  it. A `binding_state` of `ambiguous` means more than one entity currently matches the name —
  also retained, also missing that hop, never a silently-guessed winner.
- **A bare-UUID endpoint that turns out to belong to a different group than the edge is
  rejected** before any write happens — this is what keeps a cross-group edge from silently
  losing its pointer fields the first time a caller passes a UUID instead of a name.
- **`knowledge_rebind_pointers`** (`{"source_group_id": "..."}`, required) re-resolves every
  pointer whose `source_group_id` matches, after that source group's own hydration, incremental
  replay, or refresh cycle — including an ordinary `knowledge_rebuild_from_wal` targeting that one
  group, not only a full purge-and-rehydrate (issue #378). Since issue #741 every successful
  `knowledge_rebuild_from_wal` already runs this pass for the rebuilt group and reports it as
  `cross_group_rebind`, so a manual call is only needed after hydrating a group by some other
  route (e.g. assertion-API writes). A pointer currently `bound` is skipped
  once its `bound_at_seq` is already at or past `source_group_id`'s **own** applied WAL position —
  never any other group's, even when the edge carrying the pointer lives in a third, different
  group — this staleness gate is what makes a second call with no intervening change to
  `source_group_id`'s stream a true no-op for pointers that are already correct. A pointer
  currently `unbound` or `ambiguous` is always re-resolved regardless of `bound_at_seq` (issue
  #392): a known-broken pointer is repaired unconditionally, since the position comparison alone
  cannot tell "nothing changed" apart from "the source group was purged and then restored to the
  same position the pointer was originally bound at." A resolution that would create a self-loop
  or duplicate an existing directed edge invalidates the edge instead of writing a broken or
  redundant one, reusing `knowledge_merge_entities`'s own self-loop/dedup handling rather than a
  new policy. Returns `{checked, bound, unbound, ambiguous, invalidated_self_loop,
  invalidated_duplicate, staleness_skipped}` — `staleness_skipped` counts pointers skipped by the
  gate above, distinct from `checked` (pointers actually re-resolved), so a `checked: 0` result is
  never ambiguous about whether anything was examined.
- **Unbound and ambiguous edges are excluded from normal reads.** Every existing two-hop
  traversal, search, and MCP read path requires both hops to exist — a pointer that hasn't
  resolved is invisible the same way any other incomplete edge would be, no special-casing
  needed. Aggregate counts are visible via `knowledge_status`'s `cross_group_pointers: {bound,
  unbound, ambiguous}` field, so a refresh in progress is observable without a dedicated
  inspection endpoint.

### Ingestion results (`process_chunk`)

`knowledge_process_chunk` reports what it could not write, not only how much. Two additive
result fields exist for that, both introduced in 0.13.2.

- **`dropped_edges` reports the edges behind `edges_dropped_unresolvable`.** An extracted edge
  whose source or target endpoint resolves to no entity — neither in the current extraction batch
  nor in the persisted graph — is dropped rather than written
  ([ADR-0051](adr/0051-edge-endpoint-salvage-and-deferred-drop.md)). `edges_dropped_unresolvable`
  counts those drops; `dropped_edges` describes them, with one entry per counted edge, in
  extraction order:

  ```json
  {
    "edges_dropped_unresolvable": 1,
    "dropped_edges": [
      {
        "source_name": "Ada Lovelace",
        "target_name": "Analytical Engine",
        "relation_type": "WROTE_NOTES_ON",
        "fact": "Ada Lovelace wrote the first published algorithm for the Analytical Engine.",
        "unresolved_endpoint": "target"
      }
    ]
  }
  ```

  `unresolved_endpoint` is `"source"`, `"target"`, or `"both"`. `relation_type` may be `null`,
  mirroring the fact that it is already optional on an extracted edge before resolution is
  attempted. The dropped edge's content is not persisted anywhere, so this result is the only
  place it appears — a consumer that wants to tell a user which fact was lost must read it here
  rather than recover it later. **`dropped_edges` is always present**, an empty list when nothing
  was dropped, so it can be iterated unconditionally. `edges_dropped_unresolvable`'s existing
  meaning is unchanged, and a caller reading only the count is unaffected (issue #411).

- **`edges_dropped_self_loop` counts edges dropped because both endpoints resolved to one entity.**
  Two differently named endpoints can resolve to the same entity UUID (through dedup merges or
  salvage); inserting that edge would create a self-loop, so Phase C drops it. The count is a
  separate top-level field, always present, and these edges are *not* in `dropped_edges` or
  `edges_dropped_unresolvable`. (Edges whose endpoint *names* are identical are filtered earlier
  and never counted.) Separately, an off-list edge endpoint that already exists exactly (same
  group, case-insensitive) in the stored graph resolves to that entity and is never salvaged onto
  a similar entity of the current chunk; a name stored under more than one eligible kind is dropped
  as ambiguous rather than salvaged
  ([ADR-0666](adr/0666-exact-graph-match-and-uuid-self-loop-guard.md)).

- **`edges_swapped` and `edges_signature_mismatch` report the endpoint direction check (#816).**
  Two additive top-level counters, always present. `edges_swapped` counts edges stored with their
  endpoints exchanged because they fit the relation's declared signature only when reversed;
  `edges_signature_mismatch` counts edges stored unchanged but marked `"signature_mismatch": true`
  in their `attributes` because they fit in neither direction. No edge is dropped by this check.
  Both are `0` for an ontology that declares no signature. Swaps made by an inverse alias
  (strict mode) are not counted here. See [Ontology → Edge direction](ontology.md#edge-direction).

- **`warning` reports oversized input.** A `chunk_text` longer than the advisory threshold
  (`LCG_CHUNK_TEXT_ADVISORY_MAX_CHARS`, default 8,000 characters — see
  [Configuration](configuration.md)) adds a `warning` field naming the actual and recommended
  character counts, and emits a `chunk_text_oversized` telemetry event. This is visibility only:
  nothing is truncated, split, or rejected, and the call succeeds exactly as it did before.
  Splitting oversized input is the caller's responsibility (issue #407).

- **`dedup_paths` reports how this chunk's entities were resolved.** An additive object of
  per-path tallies for the chunk's Phase B entity resolution
  ([ADR-0650](adr/0650-identifier-mismatch-veto-for-extraction-dedup.md)), always present:

  ```json
  {
    "dedup_paths": {
      "exact_name": 3,
      "embedding_merge": 1,
      "vetoed": 2,
      "adapter_rejected": 0,
      "llm_confirmed": 0,
      "llm_rejected": 0,
      "llm_unavailable": 0,
      "salvage_vetoed": 0
    }
  }
  ```

  `exact_name` counts entities merged by the exact case-insensitive name match;
  `embedding_merge` counts embedding-path merges where a candidate survived the identifier veto
  and the dedup adapter confirmed it; `vetoed` counts entities that had above-threshold
  embedding candidates, all of which were rejected by the identifier-mismatch veto (names
  differing by a number or identifier, such as `ADR 2018` / `ADR 2019`, or by a distinct short
  all-caps code such as `ACDS` / `ACDM`), so a new entity was
  inserted; `adapter_rejected` counts candidates that survived the veto but were rejected by
  a non-LLM (custom) dedup adapter; `llm_confirmed`, `llm_rejected` and `llm_unavailable` count
  candidates judged by the [LLM dedup check](configuration.md) (`LCG_DEDUP_LLM`,
  [ADR-0652](adr/0652-llm-verified-extraction-dedup-via-extractor.md)) as duplicate (merged),
  distinct (inserted) and unanswerable — error, timeout or malformed answer — (inserted); when the
  check is off, a surviving candidate merges and counts as `embedding_merge`; `salvage_vetoed` counts off-list edge endpoints whose salvage candidates
  were all vetoed. The tallies are per chunk; sum them across chunks for a run total.

### Progress notifications

The six long-running operations — `knowledge_rebuild_from_wal`, `knowledge_canonicalize_relations`,
`knowledge_backfill_relation_types`, `knowledge_backfill_summary_embeddings`,
`knowledge_reprocess_relation_types`, and `knowledge_reprocess_entity_types` — bridge to MCP
progress notifications when the client
attaches a progress token to the `tools/call` request (`_meta.progressToken`), in both
standalone and attached mode. Without a progress token, these calls simply block until they
complete, same as over the socket protocol. In attached mode, each progress notification also
re-arms `LCG_ATTACHED_CALL_TIMEOUT_MS`'s per-read-line idle timer (see
[DB-access modes](#db-access-modes) above), so a progress-tracked call isn't falsely reported as
timed out just because it runs longer than that timeout in total.

### Example MCP client config

```json
{
  "mcpServers": {
    "liminis-context-graph": {
      "command": "liminis-context-graph",
      "args": ["--mcp-stdio", "--scope=read,write"],
      "cwd": "/path/to/your/workspace"
    }
  }
}
```

To attach to an already-running socket service instead of opening the DB directly:

```json
{
  "mcpServers": {
    "liminis-context-graph": {
      "command": "liminis-context-graph",
      "args": ["--mcp-stdio", "--connect", "/path/to/your/workspace/.lcg/service.sock", "--scope=read"]
    }
  }
}
```

See [ADR 0035](adr/0035-mcp-stdio-transport.md) for the transport's internal architecture.

To point the embedder used by an MCP-launched instance at a different backend (a local
OpenAI-compatible server, a hosted provider with an API key, or a non-default sidecar socket),
see [MCP client config recipes](configuration.md#mcp-client-config-recipes) in Configuration.
