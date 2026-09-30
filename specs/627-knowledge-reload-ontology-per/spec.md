# Feature Specification: `knowledge_reload_ontology` — per-group ontology reload without a service restart

**Feature Branch**: `fabrik/issue-627`
**Created**: 2026-09-29
**Status**: Specified
**Input**: User description: "knowledge_reload_ontology: per-group ontology reload without a service restart (implements #611)"

## Background

A group's ontology is resolved on that group's first use in the process and then cached for the life of the process (documented in `docs/ontology.md`). The only way to make one group adopt an edited ontology is to restart the service, which interrupts every co-resident group. The motivating multi-tenant case and downstream workflow are in the community report #611.

The cache also breaks "edit ontology → reprocess" on a live daemon:

- `knowledge_canonicalize_relations` fails with `-32000 "requires a resolved ontology"` when the group was first resolved under the old vocabulary, or under none.
- `knowledge_reprocess_entity_types` re-types against the stale cached resolution.

Release 0.16.0 (#616) added guards that #611 predates and that a reload must honour: the **identity-set guard** (a group whose ontology adds or removes `identity: true` on a type it already holds is *refused*: writes fail with JSON-RPC `-32003`, and the group is listed in `knowledge_status.group_identity_refusals`), the **identity stamp** (recorded by a single stamp writer during resolution), and per-group **drift** reporting (`knowledge_status.group_ontology_drift`). #616 also added the building block `invalidate_group_ontology`.

This issue adds an admin RPC that reloads **one group only**, so an operator can edit that group's ontology file and have it take effect on a live service without disturbing other groups.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Adopt an edited per-group ontology on a live service (Priority: P1)

An operator has a running service in which group `G` has already been used under ontology A and co-resident group `H` is also resolved. They replace G's per-group ontology file with ontology B and call `knowledge_reload_ontology { group_id: "G" }`.

**Why this priority**: This is the core value: picking up an ontology edit without a restart that interrupts every other group.

**Independent Test**: Start a service, use G under A, use H, swap G's file for B, call the reload, then verify extraction, canonicalization, reprocessing and status for G, and that H is unchanged.

**Acceptance Scenarios**:

1. **Given** G was resolved under ontology A and its file now holds ontology B, **When** `knowledge_reload_ontology { group_id: "G" }` is called, **Then** the response has `changed: true`, `previous_hash` ≠ `new_hash`, and `identity_refusal` is null/absent.
2. **Given** the reload above succeeded, **When** new extraction runs for G, **Then** entities and relations are typed by ontology B.
3. **Given** G had no resolved ontology (or was resolved under a vocabulary lacking the relation types) before the reload, **When** `knowledge_canonicalize_relations { group_id: "G" }` is called after the reload, **Then** it succeeds against B and does not return `-32000`.
4. **Given** the reload succeeded, **When** `knowledge_reprocess_entity_types { group_id: "G", scope: "off_ontology" }` is called, **Then** entities are re-typed against B.
5. **Given** the reload succeeded, **When** `knowledge_status` is read, **Then** `group_ontology_drift` for G reflects B.
6. **Given** H was resolved before the reload, **When** G is reloaded, **Then** H's resolution, hash and drift status are unchanged and H continues to serve reads and writes.

---

### User Story 2 - Reload that would reinterpret existing entities is refused explicitly (Priority: P1)

An operator edits G's ontology so that it adds or removes `identity: true` on a type G already holds, then reloads.

**Why this priority**: The reload must never silently install an ontology that reinterprets existing entities; this is the safety property of #616 and must survive the new entry point.

**Independent Test**: Give G identity-kind entities, change the identity-bearing set in its file, reload, and check the response, write behaviour and status; then restore A and reload again.

**Acceptance Scenarios**:

1. **Given** G holds entities of a type whose `identity` flag B changes, **When** G is reloaded, **Then** the response states the refusal explicitly in `identity_refusal`, exactly as a restart would refuse the group.
2. **Given** G is refused after the reload, **When** a write is made to G, **Then** it fails with JSON-RPC `-32003`.
3. **Given** G is refused, **When** `knowledge_status` is read, **Then** `group_identity_refusals` lists G, and other groups (including H) are unaffected.
4. **Given** G is refused, **When** the file is restored to A and G is reloaded again, **Then** the refusal is cleared, G's writes succeed, and `group_identity_refusals` no longer lists G.
5. **Given** a reload does not touch the identity-bearing set, or G has no identity-kind entities, **When** G is reloaded, **Then** it succeeds normally with no refusal.

---

### User Story 3 - Reloading an unchanged ontology is a safe no-op (Priority: P2)

An operator (or automation) calls the reload without having changed the file.

**Why this priority**: Makes the call idempotent and safe to invoke defensively (e.g. from a file watcher or deploy script).

**Independent Test**: Call reload twice with no file change and compare status/state before and after.

**Acceptance Scenarios**:

1. **Given** G's file has not changed since its last resolution, **When** G is reloaded, **Then** the response has `changed: false` and `previous_hash` equals `new_hash`.
2. **Given** the same, **When** the reload completes, **Then** G's drift status, identity refusal state and served ontology are exactly what they were before.

---

### User Story 4 - Reload is available to MCP admin clients (Priority: P2)

An MCP-over-stdio client with admin scope invokes the reload as a tool.

**Why this priority**: The MCP tool surface is a hand-maintained registry; the method must be reachable there, but only with admin scope.

**Independent Test**: List tools under each scope and confirm the tool appears only under admin.

**Acceptance Scenarios**:

1. **Given** an MCP client with admin scope, **When** it lists tools, **Then** `knowledge_reload_ontology` is present.
2. **Given** a client with only read or write scope, **When** it lists or calls tools, **Then** the reload is not available to it.

---

### Edge Cases

- **Per-group file removed** since last resolution: the group re-resolves through normal precedence (workspace `.lcg/ontology.yaml`, else free-form); the response reflects the resulting hash (or the absence of an ontology) and `changed` accordingly.
- **Per-group file malformed or unreadable**: treated as missing per existing resolution rules (falls through to workspace ontology or none), logged, and not a hard error; the response reports what actually got resolved.
- **Group never resolved in this process**: the reload performs a first resolution; `previous_hash` is null/absent, and `changed` is reported consistently (assumption below).
- **Group with no per-group file, relying on the workspace ontology**: reload re-resolves it through the same precedence; it does not reload the workspace file for any other group.
- **Extraction in flight for any group** when the reload arrives: the reload waits for it to finish, and no extraction observes a half-swapped cache entry.
- **Missing or empty `group_id`**: rejected with a standard invalid-params error.
- **Degraded mode** (database not open): behaviour follows the other admin operations that need a resolved group; the reload must not panic or leave the cache entry invalidated-but-unresolved.
- **Reload of a group that is already refused** where the new file still triggers the refusal: the group remains refused and the response reports it.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The service MUST expose an RPC `knowledge_reload_ontology` taking `{ group_id }` that reloads the ontology of that one group only.
- **FR-002**: The reload MUST be exclusive with all extraction: it MUST hold the service write lock for the duration of invalidate-and-re-resolve, using the same discipline as `knowledge_delete_by_group`, so no extraction for any group is mid-flight while the cache entry is swapped.
- **FR-003**: The reload MUST discard the group's cached ontology resolution and immediately re-resolve it through the normal first-resolution path, with unchanged precedence: per-group `.lcg/ontology/<group>.yaml`, then workspace `.lcg/ontology.yaml`, then free-form.
- **FR-004**: Re-resolution MUST go through the existing single identity-stamp writer; the feature MUST NOT introduce a second writer of the identity stamp.
- **FR-005**: The response MUST include `group_id`, `previous_hash`, `new_hash`, `changed`, `drift`, and `identity_refusal`.
- **FR-006**: `changed` MUST be `true` iff the group's ontology content hash differs before and after the reload; a reload with an unchanged file MUST return `changed: false`.
- **FR-007**: A reload with `changed: false` MUST have no side effects: no change to the group's drift status, refusal state, stored identity stamp, or data.
- **FR-008**: If the re-resolved ontology adds or removes `identity: true` on a type the group already holds, the group MUST be refused exactly as a restart would refuse it: writes for that group fail with `-32003`, the group appears in `knowledge_status.group_identity_refusals`, and the reload response MUST report this explicitly in `identity_refusal`.
- **FR-009**: A reload MUST NEVER silently install an ontology that reinterprets existing entities.
- **FR-010**: A refusal caused or held by a reload MUST NOT affect any other group's reads, writes, resolution or status.
- **FR-011**: Restoring the previous identity-bearing set in the group's file and reloading again MUST clear the group's refusal.
- **FR-012**: A reload that does not change the identity-bearing set, or on a group with no identity-kind entities, MUST succeed normally with no refusal.
- **FR-013**: After a reload, `knowledge_status.group_ontology_drift` for the group MUST reflect the newly resolved ontology, and the `drift` field of the response MUST report that same state.
- **FR-014**: After a successful reload, new extraction for the group MUST be typed by the new ontology; `knowledge_canonicalize_relations` MUST resolve against it (no `-32000 "requires a resolved ontology"` where the new resolution provides one); and `knowledge_reprocess_entity_types` MUST re-type against it.
- **FR-015**: A reload of one group MUST NOT change any other group's cached resolution, hash or drift status.
- **FR-016**: The reload MUST NOT modify stored graph data (entities, relationships, episodes) or the database schema; it is an additive API with no storage change.
- **FR-017**: The method MUST be registered as an MCP tool in the **admin** scope bucket, with the MCP registry count and per-scope bucket size assertions updated accordingly.
- **FR-018**: `docs/ontology.md` MUST be updated so that every statement that a restart is required to pick up an ontology edit also mentions `knowledge_reload_ontology`, documenting its behaviour, response shape and identity-refusal semantics.
- **FR-019**: The IPC parity tests for the dispatch surface MUST cover the new method.

### Key Entities

- **Group ontology resolution**: The per-group cached result of resolving which ontology governs a group (per-group file, workspace file, or none), identified by a content hash.
- **Identity-bearing set**: The set of entity types flagged `identity: true` in a group's ontology, recorded as an identity stamp and compared on resolution.
- **Identity refusal**: Per-group state under which the group's writes fail with `-32003` and which is surfaced in `knowledge_status.group_identity_refusals`.
- **Reload result**: `{ group_id, previous_hash, new_hash, changed, drift, identity_refusal }`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An operator can make a live group adopt an edited ontology with zero service restarts, and every co-resident group remains available throughout apart from the brief exclusive-lock window.
- **SC-002**: After a reload, 100% of new extraction for the group is typed by the new ontology.
- **SC-003**: After a reload, `knowledge_canonicalize_relations` succeeds for the group where the new ontology permits it, with no `-32000`.
- **SC-004**: Every reload that would change the identity-bearing set of a group holding identity-kind entities is reported as refused in the response, in `group_identity_refusals`, and by `-32003` on the group's writes; no such reload ever installs the new ontology silently.
- **SC-005**: Reloading an unchanged ontology returns `changed: false` and leaves all observable state identical.
- **SC-006**: A reload of group G leaves every other group's hash, drift status and serving behaviour unchanged.
- **SC-007**: All seven acceptance scenarios from the issue are covered by automated tests.

## Assumptions

- The reload is per-group only; a workspace-wide variant is out of scope.
- A first-time reload of a never-resolved group behaves as a first resolution; `previous_hash` is absent and `changed` is `true` when the group resolves to an ontology, `false` when it resolves to none.
- "Ontology hash" is the same content hash already used for drift detection and returned by existing status output.
- A refusal raised by a reload has the same lifetime as one raised at startup: it holds until the identity-bearing set is restored and the group is reloaded (or the service restarted).
- The reload is an admin-scope operation and is not exposed under read/write scopes.
- Versioning: additive, patch-sized change; no storage or schema migration.

## Out of Scope

- Workspace-wide reload for groups that rely on the workspace file (can be filed separately).
- Automatic file-watching or scheduled reloads.
- Automating re-ingestion to resolve an identity refusal.
- Any change to ontology precedence, file format, or the identity-guard rules themselves.

## Source References

- Community report #611; identity guard, stamp and drift work in #616; write-lock race fix #620
- `crates/core/src/app_state.rs` (`resolve_ontology`, `invalidate_group_ontology`, `all_group_identity_refusals`)
- `crates/core/src/handlers.rs` (`knowledge_delete_by_group` locking discipline; dispatch)
- `crates/service/src/mcp/tools.rs` (MCP tool registry and scope buckets)
- `crates/core/tests/ipc_parity.rs`
- `docs/ontology.md`
