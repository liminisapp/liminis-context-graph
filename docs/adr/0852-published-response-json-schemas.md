# ADR-0852: Published JSON Schemas for Response Shapes, Validated in lcg CI

**Status**: Accepted
**Date**: 2026-11-22
**Issue**: #852

## Context

Consumers could discover lcg's method names (Liminis Notes pins them and diffs the dispatch table) but not
the shapes of the results. Responses are assembled with `serde_json::json!` in three layers: serde rows,
imperative patches onto the resulting `Value` (`support`, `belief`, `ontology`, `warning`), and hand-written
objects (`knowledge_lint`, `knowledge_status`, `health_check`, `knowledge_process_chunk`). The only source of
truth was the code, so consumers guessed key names, and one guess shipped a bug (`derived_document`, the
`causes` map and `finding.reason` were each read wrongly). No lcg test could catch it.

## Decision

- **Schema-only mirror types, handlers untouched.** `crates/core/src/contract/` defines structs and enums that
  mirror the seven covered results and derive `schemars::JsonSchema`. Nothing serializes them and nothing on the
  dispatch path uses them, so the wire format cannot change (FR-014). Rejected: refactoring the `json!`
  builders to typed responses (a null-versus-absent, key-set or `preserve_order` change would be a silent wire
  change, for no benefit the tests below do not already give); hand-written schema files (no source of truth to
  diff against).
- **Drift is caught in both directions.** `crates/core/tests/contract_schemas.rs` validates *real*
  `handlers::dispatch` responses (real `Db`, mock providers) against the committed files with the `jsonschema`
  crate, and breaks copies of those responses (rename, drop, retype) to assert the schema rejects them.
  `contract::tests::committed_schemas_match_generated` fails when a committed file differs from the generated
  one; `LCG_UPDATE_CONTRACT=1` rewrites them. `jsonschema` is a dev-dependency of `lcg-core` only; `schemars`
  (already in the lockfile via `rmcp`) is a normal dependency of `lcg-core`.
- **Draft-07**, selected explicitly, for consumer tooling: Ajv and `json-schema-to-typescript` work with it
  without extra setup, whereas 2020-12 needs `Ajv2020`. Schemas are self-contained (internal `#/definitions`
  refs only), with sorted keys, 2-space indent and a trailing newline so the drift check is stable.
- **Open by default.** No `additionalProperties: false`: fields that are always emitted are `required`,
  sometimes-omitted fields are optional, always-emitted-but-`null` fields are required and nullable (the local
  `Nullable<T>`, because `Option<T>` alone makes a field optional and schemars' `required` drops the null).
  Free-form members (`attributes`, `rerank`, `expansion`, the `causes` keys) stay loose so they can grow.
- **Alternatives.** `health_check` is `oneOf` by `state`; `knowledge_status` is `anyOf` degraded / queryable /
  not-queryable (told apart by required keys and types); `knowledge_lint` findings are `oneOf` by `category`,
  and `unlinked_mention`'s two detail shapes are two `oneOf` branches keyed by `reason` (`unlinked_mention` pairs with `{document, page}`, `missing_page` with `{mention_documents, min_mentions}`), so a mismatched pair fails validation.
- **Version-free committed files; version in a release manifest.** Stamping the version into committed files
  would churn them on every bump and collide with the drift check. `scripts/stage-contract-assets.sh <tag>
  <dest>` copies the schemas and writes `contract-manifest.json` (`lcg_version`, `tag`, `draft`, sha256 per
  file) into `artifacts/` in `release.yml`'s `host` job, before `gh release create` attaches `artifacts/*`. It
  fails when `contract/` is empty or a schema is malformed. Per-method files (not a zip) so each is fetchable at
  `releases/download/<tag>/<method>.schema.json`.
- **Process.** `contract/` is added to the code-touching patterns of the docs-only fast path (ADR-0322), so a
  schema-only edit still runs the drift test. A PR that changes `contract/` must change `CHANGELOG.md`
  (`scripts/assert-contract-changelog.sh`). Both scripts have self-tests in the CI lint job.

## Consequences

- A shape change in a covered response fails lcg's CI first, naming the method and JSON path.
- The mirror types can drift from the handlers where no test drives a variant. Variants covered by a real call:
  health (healthy, degraded, busy-writing, busy-rebuilding), status (empty, with WAL groups, degraded),
  `find_entities` with and without `include_episodes`, `find_relationships` with and without `belief`,
  `search_passages`, `process_chunk` (normal, re-ingest, oversized warning, dropped edges) and every lint
  category and reason. **Gaps**, validated against a response derived from a real one rather than produced by
  the code path: status *not queryable* (needs a missing core table), `episodes_truncated` (needs more than the
  cap of episodes), `process_chunk.left_alone_episode_uuids`, and the `rerank` / `expansion` objects on
  `search_passages` (need a configured model; they are free-form).
- The release-asset step cannot run on a PR (ADR-0639). The staging script and its self-test (against the real
  `contract/`) run in CI; the step itself is first exercised on a tag, and `docs/release-process.md` says what to
  check.
- Adding a covered method means a mirror type, a `METHODS` entry, a regenerated file and a real-response test.

## Related

ADR-0020 (collection envelope), ADR-0035 (MCP wraps handler results), ADR-0322 (docs-only fast path),
ADR-0325 (status states), ADR-0378 (`wal_groups`), ADR-0639 (release verification), ADR-0755 (lint),
ADR-0857 (provenance baseline).
