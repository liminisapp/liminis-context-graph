# ADR-0862: The Minimal Client Profile and Its Compatibility Guarantee

**Status**: Accepted
**Date**: 2026-11-23
**Issue**: #862 (implements #840)

## Context

LCG dispatches about 60 methods. A client that ingests, searches, shows neighbours and deletes
needs only a few of them, but nothing said which. One integrator rebuilt features that already
existed (`delete_by_source`, `process_document`, `attributes` and `title`,
`get_entities_by_source`), because the reference is a method index, not a map of what to start
with. A short list is only worth recommending if LCG commits to it, and a commitment nobody
checks decays, so the commitment is recorded here and enforced by a test.

## Decision

### 1. The profile

Nine methods form the **minimal client profile**, listed in `docs/integrator-guide.md`:

| Method | Why it is in the profile |
|---|---|
| `knowledge_capabilities` | Discover the server, its protocol version and its features ([ADR-0854](0854-knowledge-capabilities-protocol-version-and-feature-registry.md)). |
| `health_check` | Liveness, `ready_for_reads` and write readiness ([ADR-0855](0855-health-check-ready-for-reads.md)). |
| `knowledge_process_chunk` | Ingest and update one chunk. |
| `knowledge_process_document` | Ingest and update a whole document ([ADR-0785](0785-document-mode-revisions-and-commit-boundary.md)). |
| `knowledge_retire_document` | Stop believing a removed document without deleting it ([ADR-0859](0859-retire-document-and-orphan-reporting.md)). |
| `knowledge_delete_by_source` | Destroy a source's episodes ([ADR-0853](0853-episode-source-identity-and-exact-source-matching.md)). |
| `knowledge_search_passages` | Passage search. |
| `knowledge_find_entities` | Entity search; the only profile method that returns the `entity_uuid` the next one needs. |
| `knowledge_get_entity_neighbors` | An entity's neighbourhood. |

The issue asked for "about 8". The seven operations FR-008 names (discovery, health, chunk and
document ingest, search, neighbours, delete by source) are not enough to run ingest, search,
neighbours and delete end to end, because `knowledge_search_passages` returns passages, not entity
UUIDs, and neighbours is addressed by UUID. `knowledge_find_entities` closes that gap.
`knowledge_retire_document` is there because the spec asks for ingest, update, retire and delete to
be presented together; update needs no method of its own (it is the same
`knowledge_process_document` call with the same `doc_id`).

### 2. What is guaranteed

For each profile method, across minor and patch releases:

- the **method name**;
- the **required request parameters** and their meaning (the parameters `knowledge_capabilities`
  and `health_check` take: none);
- the **names, types and meaning of the result fields** that the reference documents for it,
  including the `status` values of `process_chunk` and `process_document`;
- the **documented error codes** carried in the message prefix (`document_too_large`,
  `empty_document`, `origin_conflict`) and the JSON-RPC code of a refused write (`-32004`);
- the **scope** a tool is in (read or write), so an MCP client's `--scope` keeps working;
- presence in the dispatcher and, except for `health_check`, in the MCP tool registry.

### 3. What is not guaranteed

These may change in any release and a client must tolerate them:

- **new optional parameters, new result fields, new `features` strings, new methods and new
  `status` or `phase` values** (clients ignore what they do not know; this is the same additive
  rule as ADR-0854);
- the **text** of error and warning messages after their stable code;
- **ranking, scores and the contents of search results** for a given query: the ordering of
  `knowledge_search_passages`, the `score` and `similarity` values, and the extracted entities and
  facts are the products of an embedder and an LLM, not of the protocol;
- **defaults an operator can set**: `LCG_SEARCH_MIN_SCORE`, `LCG_DOCUMENT_MAX_BYTES` and the other
  environment variables. A default may change in a release that says so (this ADR's sibling does
  exactly that for the document cap);
- performance, logging, telemetry, on-disk format and anything not in the reference;
- the other ~50 methods.

### 4. What counts as a breaking change

Any of: removing or renaming a profile method; adding a required parameter or making an optional
one required; removing or renaming a result field, or changing its type or meaning; changing a
documented error code or the JSON-RPC code of a refusal; moving a tool to a different scope;
removing the tool from the MCP registry; or making a call that used to succeed fail for a request
that was valid under the documented parameters (for example, lowering a hard limit below a
documented value).

Tightening validation to refuse a request the documentation always called invalid is not a breaking
change, but it must be a `Changed` entry in the CHANGELOG.

### 5. How a profile method may change

A breaking change needs all of:

1. A new ADR (named by its issue number) that states the break and the migration.
2. A **deprecation period of at least one minor release** in which the old behaviour still works
   and the CHANGELOG, the reference and the integrator guide say it is deprecated. Where the old
   and new forms can coexist, both are served, and a `features` string says the new one exists.
3. A bump of `protocol_version` in `knowledge_capabilities` when the old behaviour is finally
   removed, and a CHANGELOG `Changed` entry marked **Breaking**.
4. An update of `MINIMAL_PROFILE` and the guide in the same change.

Adding a method to the profile is additive and needs only an ADR note and the same two edits.
Removing one is a breaking change under the same rule.

### 6. Enforcement

`crates/service/src/mcp/tools.rs` carries three tests:

- every profile method is in the dispatcher (`lcg_core::capabilities::methods()`, which
  `crates/core/tests/capabilities.rs` ties to the real dispatch arms) and in the MCP registry,
  failing with the method's name;
- the required parameters of each profile tool equal a pinned list;
- the list in the guide, between `<!-- minimal-profile:begin -->` and
  `<!-- minimal-profile:end -->`, equals the test's `MINIMAL_PROFILE`, so neither changes alone.

`health_check` is exempt from the registry check, and only from it (`PROFILE_NON_MCP`). It is a
socket method with no MCP tool, and adding one would change the registry counts and the write
classification, which is outside this issue. MCP clients use `knowledge_status` for readiness.

## Consequences

- The integrator guide has a stable list to recommend, and a client built on it does not need to
  follow the other methods' churn.
- Changing a profile method is deliberately slower than changing any other method.
- The pinned required-parameter list means an innocent-looking schema edit to a profile tool fails
  a test and has to be justified against this ADR.
- The guarantee covers the request and the documented response shape, not the quality of what the
  extractor and embedder produce, so it never obliges the project to freeze a model.
