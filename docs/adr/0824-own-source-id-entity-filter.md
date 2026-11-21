# ADR-0824: Drop Extracted Entities Named After the Episode's Own Source ID

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #824

## Context

Under an ontology that declares a `Document` entity type, extraction sometimes returns an entity
named after the note being processed (`project-quill-core-1`). On the v2 corpus under ontology
v3.1 there were 13 such nodes against 2 in the free-form graph (#819, E13). They absorb edges that
belong to the real entities and appear as noise in walks and entity search. The sanctioned way to
have the source note in the graph is the `Document`/`Source` entity the host *asserts*, not one the
model extracts.

Research found that the extractor prompt never contains the episode's `name`, `source_description`
or `doc_id` (`chunk_key` is telemetry only), and that in the v2 fixtures a note's own body never
contains its own ID. The note-ID names there most plausibly come from *other* notes' IDs cited as
`[[wikilinks]]` in derived, wiki and answer notes.

## Decision

A deterministic post-extraction filter in `add_episode_with_opts` (`crates/core/src/episode.rs`),
backed by the pure module `crates/core/src/source_id.rs`, drops any extracted entity whose
normalised name equals one of the episode's **own** source IDs, and discards any edge with such an
endpoint.

- **Own IDs**: `opts.doc_id` on every path; on the chunk path only (`guard_chunk_collision`, i.e.
  `knowledge_process_chunk`) also the chunk id with its `#<n>` suffix removed and the `source_file`
  part of `source_description` (the trailing `:{name}` stripped). A free-form
  `knowledge_add_episode` name (`"Alice"`) is not a source ID and is never used.
- **ID-shaped only**: a candidate own ID is used only if it is slug-shaped — no whitespace and at
  least one `-`, `_` or digit (`project-quill-core-1`). A note filed under its subject's name
  (`people/Alice.md`, `Postgres.md`) has no distinguishable ID, so its stem is never an own ID and
  the real `Alice`/`Postgres` entity is kept. This also keeps the `#`/`/` normalisation from making
  a short name like `C#` or `AC/DC` collide with an ID of `c`/`dc`. An identifier with no letter at
  all (a daily note `2024-01-15.md`, a bare number) is not ID-shaped either, so a real Date entity
  extracted from a dated note is kept. Versioned subject names that contain a letter and a digit
  (`GPT-4.md`, `S3.md`) remain indistinguishable from slug IDs and are accepted as a residual.
- **Normalisation** (both sides): strip control characters, trim, lowercase, wrapping punctuation
  (`[[ ]]`, backticks, quotes, brackets), a `#fragment`, a path prefix and one `.md`/`.markdown`/
  `.mdx`/`.txt` extension. An empty result is never an ID.
- **Placement**: right after the empty-name `retain`, before the strict-mode reclassify tallies and
  edge validation, so no counter counts an entity that is never stored.
- **Edges** touching the dropped name are discarded *before* relation handling and name salvage, so
  salvage cannot re-point them at a look-alike batch entity and Phase C cannot resolve them against
  a pre-existing persisted junk node. They are an intentional discard, like the name-level
  self-reference filter, and are not counted in `edges_dropped_unresolvable` (ADR-0051 keeps Phase C
  the sole authority for that counter).
- **Observability**: additive `entities_dropped_source_id` on `AddEpisodeResult` and the
  `knowledge_process_chunk` response; each drop is logged to stderr.
- **No prompt change.** The deterministic filter is required regardless (the model may ignore
  guidance), and any edit to a prompt file changes every key of the committed v2 cassette
  (3,350 calls to re-record) and risks drift in the link-recall and typing metrics this change must
  not regress.

This is a drop **by name**, a deliberate and narrow exception to the "never delete an entity on type
grounds" ADRs (0033, 0037, 0310, 0312): it is a different axis, and ADR-0650 is the precedent for a
deterministic name predicate in `episode.rs`.

## Consequences

- The asserted `Document`/`Source` entity is untouched; nothing in `crates/core` creates it and the
  filter only removes extracted entities.
- An entity that legitimately shares a name with the episode's own *ID-shaped* ID is dropped (strict
  normalised equality, not fuzzy). Accepted by the spec. Subject-named notes (`Alice.md`) are
  exempt by the ID-shape gate above.
- **Known risk: SC-001 may not reach 0 on v2.** The #819 junk metric counts a node named after *any*
  note ID. If the cited-ID path dominates, the own-ID filter is correct per FR-001/FR-007 but leaves
  most of the 13 in place. The `entities_dropped_source_id` counter makes the post-merge run
  measurable. The follow-up is a separate issue for a citation-target rule (drop an entity whose name
  appears only inside a `[[...]]`/"Sources:" citation), which needs a spec change to FR-007.
- The 13 → 0 and link-recall / typing figures require ontology v3.1 and an extractor run, neither of
  which is in this repository; they are a maintainer-run #819 evaluation after merge.
- Existing graphs are not modified; `knowledge_merge_entities` still covers hand clean-up.

## Alternatives rejected

- **Drop any entity named after any known note ID in the group.** Violates FR-007 and risks real
  entities.
- **Drop citation-only names (`[[...]]`).** Also conflicts with FR-007; deferred as a follow-up.
- **Prompt guidance only.** Not robust (FR-003), invalidates the cassette, drifts the metrics.
