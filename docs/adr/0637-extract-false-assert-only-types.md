# ADR-0637: Assert-Only Ontology Types — `extract: false` Excludes a Type from the LLM Extractor

**Status**: Accepted
**Date**: 2026-09-30
**Issue**: #637 (implements community report #636)
**Builds on**: ADR-0616 (`identity: true`), ADR-0033 / ADR-0310 / ADR-0312 (off-ontology handling), ADR-0627 (reload), ADR-0032 (append-only-when-present hash)

## Context

Hosts ship a base ontology with *structural* types they assert themselves (`Source`,
`DERIVED_FROM`). Every declared type was rendered into the extraction prompts, so the extractor kept
minting them from prose — and since #616 an extracted entity of an identity-bearing type is created
with that type's `kind`, polluting the identity-bearing kind with fuzzy, unkeyed entities that
collide with the host's canonically keyed assertions. A "do not extract" sentence in a description
is only a hint.

## Decision

**Format.** `extract: bool` (default `true`) on entity and relation types. Not inherited through
`parent`.

**Declared vs. extractable.** `entity_type_names()` / `relation_type_names()` remain the *declared*
sets (off-ontology detection, assert typing, ancestry, `identity_set`). New
`extractable_entity_types()` / `extractable_relation_types()` (and `*_names()`) are the subset
*offered to the LLM*, used by the prompts, the strict-mode vocabulary and alias map, the
`classify_*` menus, and the canonicalize alias/keyword/gloss tables. Changing the declared accessors
in place would have made asserted `Source` entities off-ontology reprocess candidates.

**Open mode reclassifies too (the Research decision).** A stray `extract: false` label is
reclassified to `Unclassified` / `UNCLASSIFIED` (original label preserved) in **both** strict and
open mode. In open mode the raw label is otherwise stamped onto the entity's `labels` (plus
ancestors), so leaving it would pollute the entity even without `identity`. The existing mechanism
is reused, nothing is dropped, and only `extract: false` labels are affected — every other
undeclared open-mode label is untouched. The match normalizes the raw label first. Open-mode
reclassifications are counted in the same tallies; the edge tally stays in Phase C (ADR-0051) and
is keyed on `original_relation_type` being set rather than on strict mode alone.

**Relations get the same treatment**, including the tooling that could retype an extracted edge
*into* an assert-only relation: `knowledge_reprocess_relation_types` and
`knowledge_canonicalize_relations` (via the shared `build_alias_map`). Already-canonical declared
names stay idempotent so asserted edges are not disturbed.

**Defence in depth.** `identity_kind()` returns `None` for an `extract: false` type, and the
reprocess handlers discard classifier output naming one.

**All types `extract: false`.** Treated as "no extractable types": the entity section falls back to
the default vocabulary, the relation section is omitted, and reprocess returns a clear error.
Ingest agrees with the prompt: the strict-mode vocabulary / alias-map filters only run when at least
one extractable type exists, so ordinary labels are not mass-reclassified to `Unclassified`; stray
labels naming an `extract: false` type are still reclassified via the same direct-label path as open
mode.

**Hash and reload.** `content_hash` appends `extract_false:` / `extract_false_relations:` sections
only when at least one type sets the flag, so unflagged ontologies hash byte-identically.
`identity_set()` is untouched, so flipping `extract` is drift but never an identity refusal, and
`knowledge_reload_ontology` accepts it.

## Consequences

- No schema, WAL, IPC or MCP registry change; no migration.
- `knowledge_reprocess_entity_types scope=all` may report asserted `Source` entities in
  `kind_disagreements` (the classifier cannot choose `Source`); it never retypes into it.
- Flagged ontologies render different prompts and therefore different cassette keys; flagless
  ontologies are unchanged.
