# ADR-0854: `knowledge_capabilities` — Protocol Version, Feature Registry and a Self-Describing Socket

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #854 (implements #835)

## Context

A raw-socket client could not ask the service what it was. The server version appeared only in
`--version` and MCP `initialize`; `health_check` and `knowledge_status` reported neither version
nor supported methods. One integrator pinned ten minor releases back built workarounds for methods
that already existed; the reverse problem (a new client against an old server) had no answer either.

## Decision

Add a read-only method, `knowledge_capabilities` (IPC and MCP `read` scope), returning:

- `server_version` — `CARGO_PKG_VERSION`, as `--version` and `initialize` report it.
- `protocol_version` — an integer, `1` at introduction.
- `features` — a sorted array of stable feature strings.
- `methods` — the sorted names of every IPC method the dispatcher accepts.

`health_check` additionally reports `server_version` and `protocol_version` in all states
(healthy, busy, degraded), additively (ADR-0628's contract is otherwise unchanged).

The method reads no database and takes no lock, so it is exempt from the degraded-mode guard
(ADR-0009) and classified `ReadOnly` for the writable-groups policy (ADR-0742).

### Protocol-version policy

`PROTOCOL_VERSION` is bumped **only** on a breaking change to the socket protocol: a removed or
renamed method, a changed default scope, changed response semantics, a removed feature string.
Additive changes (new method, new field, new feature string) never bump it. `1` is a baseline,
not a claim that nothing predates it. The server only reports; it does not negotiate or enforce.
CONTRIBUTING.md's release runbook requires such changes to be flagged **BREAKING for raw-socket
clients** in the CHANGELOG.

### Feature registry

`FEATURES` in `crates/core/src/capabilities.rs` is the single source of truth. Strings are
`lower_snake_case`, unversioned, sorted and **append-only**. **Later issues add their own feature
string when they ship**; nothing mechanical forces it, so the module docs, this ADR and
CONTRIBUTING.md state the rule. A unit test enforces sort order, uniqueness and format. Clients
must ignore strings they do not recognise.

### `methods` source

The dispatcher is a plain `match` and cannot be iterated. `methods` is built from
`write_policy::CLASSIFICATION`, which a test already requires to equal the dispatched methods.
`tests/capabilities.rs` additionally compares the advertised list with the dispatch arms in both
directions and names any offender. The arm-extraction helper now lives in `tests/common/mod.rs`,
shared with the classification test; it depends on `health_check` staying the first arm.

### Old servers

A server without the method returns its ordinary unknown-method error (`-32000`, `Method not
found: …`). Clients treat it as "predates capabilities".

## Consequences

- Adding a dispatcher method now touches: the `match` arm, `CLASSIFICATION` (which feeds `methods`),
  the degraded exemption if applicable, and the MCP `ToolSpec` — the first two are test-enforced.
- IPC surface 57 → 58 methods; MCP registry 56 → 57 tools.
- No Python client or Electron change is part of this issue.

## References

ADR-0009 (degraded mode), ADR-0035 (MCP registry), ADR-0628 (health-check contract), ADR-0742
(writable groups).
