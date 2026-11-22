# ADR-0881: `lcg-eval` Verifies and Reports the Service-Reported Ontology

**Status**: Accepted
**Date**: 2026-11-22
**Issue**: #881 (follow-up to ADR-0821)

## Context

`lcg-eval retrieval --ontology` (ADR-0821) staged the ontology into the eval workspace as
`.lcg/ontology.yaml` but spawned the service without `LIMINIS_WORKSPACE_ROOT`, the only way the
service discovers its ontology. The service started free-form while `lcg-eval`'s own output (it
validated its own copy of the file) said the ontology was loaded. A full re-extraction ran with no
ontology at all, and its cassette was labelled with the strict ontology's hash. `graph-qa` spawns
its service the same way and carried the same defect. Nothing checked what the service had loaded.

## Decision

- **Always set `LIMINIS_WORKSPACE_ROOT`** to the eval workspace in `spawn_service`, overriding any
  inherited value (an unrelated workspace's ontology would otherwise be loaded).
- **`knowledge_get_ontology` returns `ontology_hash`** (additive): `content_hash` of the whole
  ontology in effect for the group, `"none"` when there is none, independent of `include_asserted`.
  It is computed in `ontology_view`, the one place that maps `Ontology` to the wire (ADR-0820).
  Ontology discovery is unchanged. A client-side recomputation was rejected (a second hash
  definition that can drift); `knowledge_reload_ontology` was rejected (mutating admin call that
  reports what is on disk, not what extraction uses).
- **Verify inside `spawn_service`**, after the readiness and embedding-profile checks and before
  any ingest, driven by `ServiceOpts.expect_ontology_hash`. A missing ontology, a different hash, a
  missing `ontology_hash` (older build) or a failed call is an error naming the expected hash.
  Retrieval sets the expectation from `--ontology`; `graph-qa` sets it when the workspace has an
  ontology for the eval group, resolved as the service does (ADR-0446: per-group file, then workspace
  file). Without an expectation the read is best-effort and never fails a run.
- **Report what the service reports.** `ServiceGuard.ontology` feeds `report.md`, `report.json`
  (`meta.ontology_hash` is `"none"` for free-form, not `null`), the cassette `.meta.json` and the
  kept-workspace manifest. The cassette preflight still compares against the staged identity, since
  it runs before any service exists.
- Only the hash is compared: the mode is part of `content_hash`, so a mode mismatch is a hash
  mismatch.

## Consequences

- A run whose service lacks the ontology fails in seconds instead of after hours of extraction.
- `--service-bin` builds older than this change cannot be used with `--ontology` (the error says to
  rebuild).
- The e2e suite now inspects extraction prompts instead of using a stub that ignored them.
