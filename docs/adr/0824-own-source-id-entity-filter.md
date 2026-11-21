# ADR-0824: Drop Extracted Entities Named After the Episode's Own Source ID or a Cited Note ID

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
contains its own ID. The maintainer then traced all 13 v2 note-ID nodes (maintainer review of
PR #827): **none is named after the note it was extracted from.** Each is a `[[wikilink]]` to
*another* note that a wiki page or answer note cites as its source, e.g. `wiki-quill-core`:
"… Sources: [[project-quill-core-1]], [[project-quill-core-2]]", an answer note's
"(from [[project-quill-core-1]])", and `wiki-region-tern-core`'s "Sources:
[[decision-region-tern-core-4]]"; the vendor and decision nodes are the same. **The cause is
citation links, not the note's own ID.** The issue text's original diagnosis (the model naming the
note from its own header) was wrong, and FR-007 was amended to cover citation targets.

## Decision

A deterministic post-extraction filter in `add_episode_with_opts` (`crates/core/src/episode.rs`),
backed by the pure module `crates/core/src/source_id.rs`, drops any extracted entity whose
normalised name equals one of the episode's **own** source IDs *or* the target of an ID-shaped
`[[wikilink]]` in the episode body (a **citation target**), and discards any edge with such an
endpoint. Both rules feed one set and one counter.

- **Citation targets** (the rule that covers v2): `citation_targets(body)` collects the targets of
  `[[target]]`, `[[target|alias]]`, `[[target#heading]]` and `[[folder/target.md]]` links in the
  episode body, normalises them as below, and keeps only ID-shaped ones. An entity whose
  normalised name equals one is dropped. It applies on every path (the body is always available).
  A link to a real entity — `[[Quill Core]]`, `[[Alice]]` — is not ID-shaped, so the entity
  survives; an ID-shaped name that is *not* linked in this episode's body is also kept (FR-007).
  The own-ID rule is retained because it is cheap and covers other corpora.

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
- An entity that legitimately shares a name with an ID-shaped `[[wikilink]]` target in the same
  episode body is dropped (e.g. a project literally named `project-quill-core-1` that the body
  also links as a source).
- **Known limit: the ID-shape gate also admits hyphenated and versioned concept links.** The gate
  is a letter plus a `-`, `_` or digit, so `[[GPT-4]]`, `[[machine-learning]]`, `[[covid-19]]`,
  `[[Node-RED]]`, `[[S3]]` and `[[e-commerce]]` are citation targets too. In a vault that links to
  such pages, a real entity the model extracts under exactly that name (a `GPT-4` Technology) is
  dropped, with its edges, in that episode; the only trace is the stderr line and the
  `entities_dropped_source_id` count. The rule applies to every body on every path. The gate is the
  one the maintainer specified for the v2 fix; a stricter shape (e.g. a trailing numeric segment)
  would not exclude `GPT-4`/`covid-19` and could miss v2 nodes, so it was not applied. Mitigations:
  the drop is per-episode and exact-name only, the entity is still extracted from episodes that do
  not link it, and a corpus whose note IDs are distinguishable from concept names could narrow the
  gate later. Pinned by `hyphenated_and_versioned_concept_links_are_citation_targets`.
- **SC-001 is a prediction until measured.** The citation-target rule is aimed at the traced
  origin of all 13 v2 nodes, but the 13 → 0 figure depends on the extractor naming the node
  exactly as the link target. The `entities_dropped_source_id` counter makes the post-merge run
  measurable; the maintainer measures it with one re-extraction.
- The 13 → 0 and link-recall / typing figures require ontology v3.1 and an extractor run, neither of
  which is in this repository; they are a maintainer-run #819 evaluation after merge.
- Existing graphs are not modified; `knowledge_merge_entities` still covers hand clean-up.

## Alternatives rejected

- **Drop any entity named after any known note ID in the group.** Violates FR-007 and risks real
  entities.
- **Drop only names that appear *solely* inside a citation.** More precise, but needs the model's
  entity span, which the filter does not have; link-target equality plus the ID-shape guard is
  the deterministic approximation.
- **Prompt guidance only.** Not robust (FR-003), invalidates the cassette, drifts the metrics. The
  optional one-line prompt addition ("source citations such as `[[note-id]]` are not entities") was
  not made, so the cassette stays valid; the filter is the guarantee.
