# ADR-0816: Ontology Edge Direction — Define, Validate and Repair

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #816 (measured in #815, E13 in `docs/retrieval-experiments.md`)

## Context

With an ontology in strict mode, LCG never established which end of an extracted edge is the source,
and never checked it. On the v2 eval corpus, 34% of typed edges on known-true links pointed the wrong
way (48 of 142): `APPROVES` 22/28, `CONTACT_FOR` 18/24, `OWNS` 6/29. The extractor makes the
sentence's grammatical subject the source ("Sable Core is owned by Lena Moreau" is stored as
`Sable Core OWNS Lena Moreau`). Four causes:

1. `extract_edges.txt` never defined direction.
2. `build_fact_types_section` rendered `(Source → Target)` only when *both* ends were declared.
3. `source_type`/`target_type` were parsed and hashed but never compared with an edge's endpoint types.
4. Aliases map name → canonical unchanged, so they cannot express an inverse reading
   (`APPROVED_BY` for `APPROVES`); and one type per endpoint cannot declare `OWNS` correctly.

ADR-0310 had already noted that "`LAUNCHED_BY` is semantically `LAUNCHED` with inverted endpoints".

## Decision

1. **Schema (additive).** `source_type`/`target_type` accept a name or a list; a type matches if it or
   a declared ancestor is listed, an unset side accepts anything. New `inverse_aliases` and
   `symmetric: true`. The hash includes the new fields only when present (ADR-0032's approach), and a
   one-element list serializes as the legacy single string, so every existing ontology keeps its hash.
   An undeclared type in a signature warns at load and that side is ignored.
2. **Prompt.** `extract_edges.txt` defines direction for every mode, with a worked example.
   `build_fact_types_section` renders half and list signatures, symmetric relations, and (strict only)
   inverse aliases. **This relaxes ADR-0310's byte-identical open-mode constraint** for the edge
   prompt: that constraint concerned the relation-type section's mode-specific text, and direction
   applies to every mode, so the open-mode and no-ontology edge prompts change. The vocabulary-
   restriction text and alias/keyword lines remain strict-only.
3. **Ingest check** (`crate::direction`, shared with repair). After the relation type is resolved and
   both endpoints are known (Phase C, before the write): an inverse alias has already swapped the
   endpoints (strict pre-lock filter, `build_alias_map_directed`); endpoints that fit are kept (both
   orientations fitting keeps the extracted order); endpoints that fit only reversed are swapped and
   counted (`edges_swapped`); endpoints that fit neither way are stored unchanged with
   `signature_mismatch` in `attributes` (`edges_signature_mismatch`). Symmetric relations, relations
   with no usable signature and untyped endpoints are skipped. **An edge is never dropped by this
   check.** Endpoint types come from the stored entity's `labels` (authoritative after dedup merges
   and for endpoints from earlier ingests), normalized and expanded through the ancestor map so
   open-mode raw labels work; one read per unique endpoint per call.
4. **Open mode** does not start resolving aliases at ingest (that would change behaviour beyond the
   check); the check applies to edges whose relation type is exactly a declared name. Inverse-alias
   spellings in open mode are corrected by the repair pass.
5. **Repair.** `knowledge_canonicalize_relations` gains a direction phase: it reads each edge's
   endpoints and labels (skipped entirely when the ontology has no signature and no inverse alias),
   retypes through inverse aliases/keywords and swaps, applies the same swap/mark rules, supports
   `dry_run`, and reports `swapped_count`, `signature_mismatch_count` and per-type
   `{checked, swapped, mismatched}`. An inverse alias only forces a swap when the edge is *actually
   retyped* (stored type differs from canonical): the retype consumes the inverse reading, which is
   what makes a second run report zero swaps even though the stored name still spells the alias.
   An inverse keyword hit counts only when the text also contains one of the type's `inverse_aliases`.
   A stale `signature_mismatch` is cleared when the edge now fits; a skipped edge keeps its mark.
   Attributes are read-merged, never overwritten.
6. **Swap mechanics.** An edge is a shadow node plus a direct relationship plus two hops, so a swap
   deletes and recreates them (the direct relationship is rebuilt from the shadow node's own columns),
   and rewrites `name` only when it has the ingest form `"A → B"`. All statements go through
   `exec_params`, so they are WAL-logged and replay. lbug auto-commits per statement, so a crash
   mid-swap can leave one edge half-wired; this is accepted (consistent with the existing WAL-resume
   model) and documented, with a snapshot and `dry_run` recommended first.
7. **Not covered.** `knowledge_assert_relationship`, reprocess and cross-group writers do not run the
   check ("no other ingest behaviour changes"). Re-extracting corpora is out of scope.

## Consequences

- Fresh extractions are prompted for the right direction, and the stored graph is corrected or
  flagged regardless of what the model did. In the E13 simulation of this rule on the existing
  graph, reversed known-true links went from 48/142 to 1/142 (the real eval is run by the maintainer).
- **Cassettes are invalidated.** `cassette.rs` hashes the rendered edge system prompt, so every
  recorded `extract` call — including `crates/eval/fixtures/retrieval-v2/cassette.jsonl` — misses on
  replay and must be re-recorded to re-extract. The repair eval runs on the stored graph and is
  unaffected.
- `RelationTypeDef.source_type`/`target_type: Option<String>` became `source_types`/`target_types:
  Vec<String>` (empty = unset), so the compiler found every consumer.
- A loosely typed entity can make exactly one orientation fit and cause a wrong swap; this is inherent
  to the rule.

## References

- [ADR-0032](0032-ontology-parent-edges-conditional-hash-segment.md) — conditional hash segments
- [ADR-0310](0310-strict-mode-reclassifies-not-drops.md) — reclassify-not-drop; open-mode prompt constraint
- [ADR-0033](0033-noise-edges-reclassified-not-deleted.md) — never-delete precedent
- `crates/core/src/direction.rs`, `canonicalize.rs` (`plan_direction`), `episode.rs` (Phase C)
