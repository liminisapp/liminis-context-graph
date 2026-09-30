# ADR-0627: Per-Group Ontology Reload Without a Restart

**Status**: Accepted
**Date**: 2026-09-30
**Issue**: #627 (implements community report #611)
**Amends**: ADR-0446 (its "restart required" contract for per-group files), ADR-0616 (the identity
refusal's "holds for the life of the process" lifetime)

## Context

A group's ontology is resolved on its first use and cached for the life of the process (ADR-0446).
The only way to make one group adopt an edited file was a restart, which interrupts every
co-resident group and leaves `knowledge_canonicalize_relations` failing with `-32000 "requires a
resolved ontology"` and `knowledge_reprocess_entity_types` re-typing against the stale resolution.
ADR-0616 added an identity-set guard, a single stamp writer (`check_identity_set`), and per-group
drift; a reload must honour all three. `invalidate_group_ontology` already existed as a building
block (`knowledge_delete_by_group`).

## Decision

**`knowledge_reload_ontology {group_id}`, admin scope** (MCP tool of the same name). It reloads one
group only and is implemented as `AppState::reload_group_ontology`, called by the handler under the
service write lock, exactly like `knowledge_delete_by_group`.

**Re-resolution is the normal first-resolution path.** The entry is invalidated and re-resolved
through `resolve_ontology`, so precedence (per-group file, workspace ontology, none), drift and the
identity check are unchanged code. `check_identity_set` stays the only writer of the identity stamp.
The workspace-wide file is still read once at startup; a group relying on it re-resolves to that
value.

**Unchanged is a no-op by construction.** For a cached group, the would-be ontology is loaded (a pure
file read) and its content hash compared with the cached one. Equal → the cached entry is returned
untouched, with `changed: false`. This avoids re-reading the drift sidecar, re-running the DB episode
count and re-running the stamp check, so FR-007 (no side effects) holds even against a concurrent
ingest. It also means a refused group whose file is unchanged stays refused. The hash covers the
identity flags (ADR-0616), so an identity change is never mistaken for "unchanged".

**A refusal is data, not an error.** A refused group still caches the new ontology (reads,
canonicalize, reprocess use it; only `add_episode` is gated, as after a restart). The reload
returns success with a structured `identity_refusal: {message, label, count, adding}`; writes then
fail `-32003` and `knowledge_status.group_identity_refusals` lists the group. Restoring the previous
identity-bearing set and reloading clears it (the stamp is left untouched while refused, so the
restored set equals the recorded one). This changes the refusal's lifetime from "the process" to
"until the group is reloaded with the set restored, or the process restarts".

**All-or-nothing on failure.** If the identity check cannot run (DB unavailable / stamp unreadable),
the prior cache state is restored and the handler returns `DbUnavailable`; the group is never left
invalidated-but-unresolved. The method is deliberately not exempt in degraded mode.

**Response shape.** `{group_id, previous_hash, new_hash, changed, drift, identity_refusal}`.
`previous_hash` is `null` only if the group was not cached; a cached group that resolved to no
ontology reports `"none"`, the sentinel `content_hash` already uses. `new_hash` is `"none"` when
nothing resolves. `changed` is `previous != new`, or, for a never-resolved group, `new != "none"`.
`drift` is the entry `knowledge_status` reports and compares against the last-*ingested* ontology
(the drift sidecar), so it is independent of `changed`.

**Stale-commit guard.** `add_episode` extracts lock-free (ADR-0543); an episode extracted under
ontology A can commit after a reload to B and then record A's hash in the group sidecar and clear
B's drift. Phase C now skips `write_group_sidecar` / `clear_group_drift` when the group's cached
ontology hash differs from the one extraction used (`AppState::cached_ontology_hash`). The episode's
own entities stay typed by A; failing or retrying such episodes was rejected as a semantic change to
`add_episode` for a race bounded by extraction time.

**Invalid params.** A missing or empty `group_id` fails with `Error::Ipc` (`-32000`), like every peer
handler; no `-32602` mapping exists in `handle_ipc` and adding one for a single method would be
inconsistent.

## Consequences

- Locking is limited: the write lock waits behind read-lock holders (reprocess, canonicalize,
  backfill) and queues new readers/writers behind it while it waits. Reprocess/canonicalize resolve
  their ontology before taking their read lock, so only reload-then-call ordering is guaranteed.
- Additive API; no storage, schema, WAL or stamp-format change. MCP registry: 46 tools, 15 admin.
- ADR-0014, ADR-0018 and ADR-0446 describe restart-only reload as the v1 contract; this ADR
  supersedes that for per-group files only.
