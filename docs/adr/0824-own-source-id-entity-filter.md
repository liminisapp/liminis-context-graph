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

Two layers. The **extraction prompt** (`prompts/extract_text.txt`) tells the model that wikilink
citations — `[[note-id]]`, `sources: [[…]]`, `(from [[…]])` — name *other* notes and are not entities.
That is the primary fix. A deterministic post-extraction filter in `add_episode_with_opts`
(`crates/core/src/episode.rs`), backed by the pure module `crates/core/src/source_id.rs`, is the
guarantee (FR-003): it drops an extracted entity that is either named after the episode's **own**
source ID or is a **citation target** (below), and discards any edge with such an endpoint. Both
rules feed one counter.

- **Citation targets** (the rule that covers v2) are dropped only when **all** hold:
  (a) the entity's name, **before any normalisation**, is verbatim the `[[wikilink]]` target in the
  episode body, and that target is all-lowercase, contains no whitespace and is ID-shaped (a letter
  plus a `-`, `_` or digit); and (b) the target is the source ID — `doc_id`, chunk-id prefix or
  source-file stem, derived exactly as an own ID is — of an episode **already ingested in the same
  group**, i.e. the link cites a note LCG knows. `citation_candidates(body)` implements (a);
  `Conn::episode_id_rows_for_target` plus `known_source_ids_of_rows` implement (b), resolved before
  extraction so the post-extraction filter stays pure. `[[target|alias]]`, `[[target#heading]]` and
  `[[folder/target.md]]` are handled (the normalised form strips alias, fragment, path and
  extension). The citation rule applies on every path, since the body is always available.
  - **Why this shape.** No name shape alone separates the v2 junk from real concepts: the 13 v2
    nodes (10 `Document`, 3 `Decision`) all end in `-<digit>`, and so do `GPT-4` and `covid-19`.
    Rule (a) spares `[[GPT-4]]`, `[[Node-RED]]` and `[[S3]]` (not lowercase) and `[[Quill Core]]`
    (not ID-shaped). Rule (b) spares `[[machine-learning]]` unless the vault actually has a note
    with that ID that the model copied verbatim — and then the entity is a citation of a note
    anyway. An ID-shaped name that is neither the own ID nor linked in this body is kept (FR-007).
  - **Accepted misses.** A note ingested *after* the note that cites it is not yet known, so (b)
    misses it (the prompt is meant to cover that case). A model that capitalises or reformats the
    name (`Project-Quill-Core-1`) escapes (a).
  - **Fail-open.** If the lookup cannot run (degraded DB, query error) nothing is dropped; the
    filter is a noise reduction, never a reason to fail an ingest. The lookup costs one scan of the
    group's `Episodic` rows per distinct candidate link (capped at 64 per episode) and only runs
    for bodies that contain such a link.
- The own-ID rule is retained because it is cheap and covers other corpora.

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
- An entity that legitimately shares a name with a cited, already-ingested note's ID — a project
  literally named `project-quill-core-1` that the body also links as a source, when a note with
  that ID is in the group — is dropped from that episode. Accepted: in that situation it is a
  citation of a note.
- **Concept links are spared.** `[[GPT-4]]`, `[[covid-19]]`, `[[machine-learning]]` and
  `[[Quill Core]]` survive unless the group really has a note with that exact ID and the model
  copied it verbatim. Pinned by `concept_links_are_kept`,
  `citation_target_is_kept_when_the_note_is_not_in_the_group` and
  `citation_target_is_kept_when_the_note_is_only_in_another_group`. This replaces the earlier,
  shape-only gate, whose over-matching of hyphenated and versioned concept links the maintainer
  rejected (maintainer decision on PR #827).
- **Observability.** The stderr line names the dropped entity, its type and which rule fired, and a
  summary line gives the number of discarded edges, so collateral loss can be audited. The
  `entities_dropped_source_id` count does not distinguish the two rules.
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
- **A shape-only citation gate** (a letter plus a `-`, `_` or digit on the link target). Shipped
  first and rejected: it also matches `[[GPT-4]]`, `[[covid-19]]` and `[[machine-learning]]`.
- **Prompt guidance only.** Not robust (FR-003): the model may ignore it, and a note cited before it
  is ingested is only reachable by prompt. So the prompt is the primary fix and the filter the
  guarantee. **The prompt edit changes every key of the committed v2 cassette**
  (`crates/eval/fixtures/retrieval-v2/cassette.jsonl`); no test replays it, and the maintainer
  re-extracts v2 after this change (together with #823), so it is not re-recorded here.
