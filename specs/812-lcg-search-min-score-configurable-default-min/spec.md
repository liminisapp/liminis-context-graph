# Feature Specification: Configurable default min_score for passage search (`LCG_SEARCH_MIN_SCORE`)

**Feature Branch**: `fabrik/issue-812`
**Created**: 2026-11-19
**Status**: Specified
**Input**: User description: "Implements #811.

## Problem
`knowledge_search_passages` uses `min_score` 0.5 whenever a request omits it (`crates/core/src/handlers.rs`, `handle_search_passages`: `p[\"min_score\"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0)`). The right floor depends on the embedding model and corpus. In #811's report, a CPU bge-base embedder on a small mixed set scored relevant passages at 0.73–0.77 and unrelated ones at 0.58–0.62, so 0.5 lets the wrong passages through. Most callers (MCP clients, skills) never set `min_score` and can't know the embedder's score distribution. The operator who chose the embedder can.

## Change
Add `LCG_SEARCH_MIN_SCORE`: the default `min_score` for `knowledge_search_passages` when a request omits it.

- **Unset:** 0.5, exactly as today.
- **Valid float:** clamped to 0.0–1.0, in the same way as the request parameter.
- **Non-numeric, NaN or infinite:** startup fails with a message naming the variable and the value. Validate it once at startup, before anything opens, following the #737 pattern (`crates/core/src/lbug_config.rs`, `crates/service/src/main.rs` ~L1495, `crates/service/tests/lbug_tuning_env.rs`). Note that this deliberately differs from `LCG_PASSAGE_SEARCH_MODE`, which warns and falls back. #811 asks for a hard failure.
- **Precedence:** an explicit `min_score` in the request always wins, including `0.0`.
- **Scope:** both `vector` and `hybrid` modes. The semantics in hybrid are unchanged: the floor applies only to the vector retriever's candidates.
- **Logging:** log the effective default once at startup.
- **Visibility:** expose the effective default in `knowledge_status` as an additive field. Put it wherever the other search or config settings sit, or at top level if there's no such place.

## Docs
- `docs/configuration.md`: a new row for `LCG_SEARCH_MIN_SCORE`.
- `docs/ipc-mcp-reference.md`: the `min_score` bullet says the default is `LCG_SEARCH_MIN_SCORE`, else 0.5.
- The MCP tool schema in `crates/service/src/mcp/tools.rs` (~L537): the description says the default is configurable. Keep `\"default\": 0.5` in the schema.
- Regenerate `docs/llms-full.txt` (`bash scripts/generate-docs-llms-full.sh`).
- A CHANGELOG `Unreleased` entry.

## Acceptance
- Unset: an omitted `min_score` filters at 0.5, and results are identical to today's.
- `LCG_SEARCH_MIN_SCORE=0.7`: an omitted `min_score` filters at 0.7, while an explicit `min_score: 0.0` still returns everything and an explicit `0.3` uses 0.3. Test both modes.
- `1.5` behaves as 1.0 and `-0.2` as 0.0.
- `abc`, an empty string, `NaN` and `inf` each make startup fail fast with a non-zero exit and the variable named in stderr. This needs a service-level test in the style of `lbug_tuning_env.rs`.
- `knowledge_status` reports the effective value.
- The tool count is unchanged, and no existing test changes behaviour.

## Out of scope
- `min_similarity` on `knowledge_find_entities` and `knowledge_find_relationships` (ADR-0629). It deliberately has no default; leave it alone.
- The `knowledge_graph_search` (#797/#801) and query-variants (#802/#805) work in flight. Don't touch those branches. If one of them lands first and reads `min_score` through `handle_search_passages`, it picks up the new default for free.
- An explicit non-numeric `min_score` in the request still falls back silently to the default. Changing that is a separate behaviour change.
- Documenting score ranges per embedder. We don't have the data yet."

## Background

`knowledge_search_passages` applies a minimum similarity score to filter out weak matches. When a request omits `min_score`, the service uses a hard-coded default of 0.5. The right floor depends on the embedding model and the corpus. In the report behind #811, a CPU bge-base embedder on a small mixed document set scored relevant passages at 0.73–0.77 and unrelated ones at 0.58–0.62, so a 0.5 floor lets the wrong passages through.

Most callers (MCP clients, skills) never set `min_score` and cannot know the embedder's score distribution. The operator who chose the embedder is the one who can set a sensible floor, so the default should be operator-configurable via an environment variable, while unset behaviour stays exactly as today.

The validation style deliberately follows the startup-fail-fast pattern from #737 (tuning env vars), and deliberately differs from `LCG_PASSAGE_SEARCH_MODE`, which warns and falls back on a bad value; #811 asks for a hard failure so a typo cannot silently leave the wrong floor in place.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Operator raises the default relevance floor (Priority: P1)

An operator who runs a particular embedder sets `LCG_SEARCH_MIN_SCORE` (e.g. `0.7`) so that callers who omit `min_score` get results filtered at that floor, without changing any client.

**Why this priority**: This is the whole feature; it fixes the reported problem of unrelated passages passing the 0.5 floor.

**Independent Test**: Start the service with `LCG_SEARCH_MIN_SCORE=0.7`, index passages with known scores, call `knowledge_search_passages` without `min_score`, and confirm passages scoring between 0.5 and 0.7 are excluded.

**Acceptance Scenarios**:

1. **Given** `LCG_SEARCH_MIN_SCORE` is unset, **When** a request omits `min_score`, **Then** results are filtered at 0.5 and are identical to today's.
2. **Given** `LCG_SEARCH_MIN_SCORE=0.7`, **When** a request omits `min_score`, **Then** results are filtered at 0.7, in both `vector` and `hybrid` modes.
3. **Given** `LCG_SEARCH_MIN_SCORE=0.7`, **When** a request sets `min_score: 0.0`, **Then** everything is returned (the explicit value wins).
4. **Given** `LCG_SEARCH_MIN_SCORE=0.7`, **When** a request sets `min_score: 0.3`, **Then** the floor is 0.3.
5. **Given** `LCG_SEARCH_MIN_SCORE=1.5`, **When** a request omits `min_score`, **Then** the floor is 1.0; **Given** `-0.2`, the floor is 0.0.

---

### User Story 2 - Misconfiguration fails fast (Priority: P1)

An operator who mistypes the variable learns at startup, not through silently degraded search quality.

**Why this priority**: A silent fallback would leave the wrong floor in place without anyone noticing, which is the failure this feature is meant to remove.

**Independent Test**: Launch the service binary with each invalid value and check exit status and stderr.

**Acceptance Scenarios**:

1. **Given** `LCG_SEARCH_MIN_SCORE` is `abc`, an empty string, `NaN` or `inf`, **When** the service starts, **Then** it exits non-zero before opening anything, and stderr names `LCG_SEARCH_MIN_SCORE` and the offending value.

---

### User Story 3 - Operator can see the effective default (Priority: P2)

An operator or client can confirm which default is in force without reading the environment.

**Why this priority**: Makes the setting observable and debuggable, but the filtering behaviour works without it.

**Independent Test**: Start the service with and without the variable and call `knowledge_status`; also inspect the startup log.

**Acceptance Scenarios**:

1. **Given** the service has started, **When** `knowledge_status` is called, **Then** it reports the effective default `min_score` as an additive field, and existing fields are unchanged.
2. **Given** the service starts, **Then** the effective default is logged exactly once at startup.

---

### Edge Cases

- An explicit `min_score` of `0.0` in the request is honoured and is not treated as "omitted".
- An explicit non-numeric `min_score` in a request still falls back silently to the configured default (existing behaviour, unchanged).
- Out-of-range values (`1.5`, `-0.2`) in the variable are clamped rather than rejected, matching how the request parameter is treated.
- Hybrid mode: the floor continues to apply only to the vector retriever's candidates; the other retriever's candidates are not filtered by it.
- The variable set to an empty string is invalid (fails startup), not equivalent to unset.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: When `LCG_SEARCH_MIN_SCORE` is unset, a `knowledge_search_passages` request that omits `min_score` MUST use 0.5, with results identical to current behaviour.
- **FR-002**: When `LCG_SEARCH_MIN_SCORE` is a valid finite number, it MUST be clamped to 0.0–1.0 and used as the default `min_score` for requests that omit it.
- **FR-003**: An explicit numeric `min_score` in a request MUST always take precedence over the configured default, including `0.0`.
- **FR-004**: The configured default MUST apply in both `vector` and `hybrid` passage search modes; hybrid semantics are unchanged (floor applies only to vector candidates).
- **FR-005**: If `LCG_SEARCH_MIN_SCORE` is set to a non-numeric value, an empty string, NaN or an infinite value, the service MUST fail at startup, before opening any database or other resource, with a non-zero exit and an error message naming the variable and the value.
- **FR-006**: The variable MUST be read and validated once at startup; the effective default MUST be logged once at startup.
- **FR-007**: `knowledge_status` MUST report the effective default `min_score` as an additive field, placed alongside other search/config settings if such a place exists, otherwise at top level. No existing field changes.
- **FR-008**: The number of registered MCP tools MUST be unchanged, and the `min_score` property in the MCP tool schema MUST keep `"default": 0.5`, with its description stating that the default is configurable.
- **FR-009**: Documentation MUST be updated: a `LCG_SEARCH_MIN_SCORE` row in the configuration reference; the `min_score` bullet in the IPC/MCP reference stating the default is `LCG_SEARCH_MIN_SCORE`, else 0.5; the generated aggregate docs file regenerated; and a CHANGELOG `Unreleased` entry.
- **FR-010**: Automated tests MUST cover: the unset default; a configured value in both modes; explicit `0.0` and explicit `0.3` overriding a configured `0.7`; clamping of `1.5` and `-0.2`; and a service-level test that each of `abc`, empty string, `NaN` and `inf` causes a non-zero exit with the variable named in stderr. No existing test's behaviour changes.

### Key Entities *(if applicable)*

- **Effective default min_score**: the resolved floor (env value clamped, else 0.5) applied to passage-search requests that omit `min_score`; resolved once at startup and surfaced via `knowledge_status`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With the variable unset, passage search results for a request omitting `min_score` are identical to those before the change across the existing test suite.
- **SC-002**: With `LCG_SEARCH_MIN_SCORE=0.7`, no passage scoring below 0.7 is returned for a request omitting `min_score`, in both modes, while explicit `min_score` values still behave as requested.
- **SC-003**: Each of the four invalid inputs (`abc`, empty, `NaN`, `inf`) produces a non-zero exit with the variable name in stderr, 100% of the time.
- **SC-004**: An operator can determine the effective default from `knowledge_status` alone.

## Assumptions

- The variable is read as a decimal float; surrounding whitespace handling follows whatever the existing startup env-var validation (#737) does.
- Clamping applies to the variable exactly as it does to the request parameter (0.0–1.0), so out-of-range values are accepted rather than rejected.
- The `knowledge_status` field name and exact placement are left to planning, subject to FR-007 (additive, no existing field changes).
- The #737 tuning-env validation is the reference pattern for where and how startup validation and its service-level test are done.

## Out of Scope *(optional)*

- `min_similarity` on `knowledge_find_entities` and `knowledge_find_relationships` (ADR-0629); it deliberately has no default.
- The `knowledge_graph_search` (#797/#801) and query-variants (#802/#805) work in flight; those branches are not touched. If one lands first and reads `min_score` through the passage-search handler, it inherits the new default.
- Changing the silent fallback for an explicit non-numeric `min_score` in a request.
- Documenting score ranges per embedder (no data yet).

## Source References *(optional)*

- Issue #811 (original report); #737 (startup env-var validation pattern); ADR-0629 (`min_similarity` has no default).
