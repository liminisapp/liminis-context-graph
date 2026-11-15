# Feature Specification: Episode origin — distinguish primary sources from derived content so a wiki's own pages never corroborate themselves

**Feature Branch**: `fabrik/issue-752`
**Created**: 2026-11-15
**Status**: Specified
**Input**: User description:

> ## Problem
>
> LCG's main consumer, Liminis Notes, follows the "LLM wiki" pattern: the human writes notes and clips
> sources, and an AI agent writes summary, topic and synthesis pages **back into the same notebook**.
> LCG then indexes those agent pages exactly like human notes.
>
> So a fact the agent *derived* from note A comes back from the agent's page as a second, apparently
> independent episode supporting the same fact. Today this happens three ways:
>
> - **Inflated support.** A relationship's `episodes[]` grows with derived episodes, so "how many
>   sources say this" overstates the evidence.
> - **Self-reinforcing retrieval.** When the agent writes or refreshes a page, search returns its
>   own earlier pages alongside the primary notes. Errors in a page get re-read as evidence and
>   propagate.
> - **No way to ask "what do the primary sources say?"** No read can be restricted to
>   human-authored or external material.
>
> Liminis Notes already half-tags derived content: chat consolidations are sent with
> `attributes.kind = "knowledge"`. LCG stores `attributes` on the episode but ignores them for
> extraction, merging duplicates, support counting and retrieval. A free-form attributes map is
> also the wrong carrier for something LCG must reason about.
>
> The other projects in this space (Karpathy's LLM Wiki gist, claude-second-brain, gbrain, COG) all
> file agent output back into the store, and none of them guards against this. It is also the
> precondition for #732, where observations must not let "duplicate evidence masquerade as
> independent corroboration".
>
> ## Desired behaviour
>
> 1. **First-class `origin` on episodes.** `knowledge_process_chunk` and `knowledge_add_episode`
>    accept an optional `origin` from a closed set, for example:
>    - `primary`: a human-written note
>    - `external`: clipped or imported third-party material
>    - `derived`: an agent-written page or a consolidation
>    - `answer`: a filed Q&A response
>
>    The exact vocabulary is for Specify. The value is validated, recorded in the WAL, and survives
>    `knowledge_rebuild_from_wal`. An episode with no `origin` is `primary`, so current behaviour is
>    unchanged.
> 2. **Derived episodes cite but don't corroborate.** Facts extracted from a `derived` or `answer`
>    episode stay linked to it for provenance. They are reported separately from primary support and
>    never count toward it.
> 3. **Derived content never supersedes primary content.** A derived episode can't invalidate or
>    overwrite a fact whose support is primary-only. This covers dedup and merge decisions too: a
>    derived episode must not rename or re-summarise an entity on its own authority. It also covers
>    the belief-time machinery: a `derived` document must not supersede a `primary` one through
>    `doc_id` supersession (ADR-0707), and must not end belief in a primary episode through
>    revision or removal (ADR-0705).
> 4. **Origin-aware reads.** `knowledge_find_relationships`, `knowledge_find_entities` and
>    `knowledge_search_passages` accept an `origins` filter. Results show support broken down by
>    origin, e.g. `support: {primary: 2, external: 1, derived: 3}`.
> 5. **Migration path.** Existing graphs have no origin. Provide a way to stamp origin onto existing
>    episodes (by source path pattern, or from the `attributes.kind` already present), without a
>    full re-extraction.
>
> ## Acceptance scenarios (request level)
>
> - Note A says "X uses Postgres". An agent page summarising A repeats it. The fact shows one primary
>   supporting episode plus one derived citation, not two supports.
> - The agent searches with `origins: [primary, external]` while refreshing its page, and its own
>   earlier pages are not returned.
> - An agent page wrongly says "X uses MySQL". The primary fact "X uses Postgres" stays valid; the
>   conflict can be reported but is not resolved in the page's favour.
> - A WAL rebuild preserves every episode's origin and the resulting support breakdowns.
> - A client that never sends `origin` sees no behaviour change.
>
> ## Questions for Research
>
> - How does ingest currently merge a duplicate relationship? Does it append to `episodes[]`, and
>   where? That is where rule 2 attaches.
> - Should support counting use distinct source documents rather than episodes? One long note
>   split into many chunks can otherwise corroborate itself too.
> - Should origin also live on entities and relationships, or always be derived from their
>   episodes?
> - Is origin a property of the **document** (`doc_id`, ADR-0707) rather than of each episode? An
>   agent page is derived as a whole. A per-document record alongside `DocSupersession` might be the
>   natural carrier, with episode-level origin only for documents that have no `doc_id`.
> - The ingestion-surface proposal (`ideas/ingestion-surface-consolidation.md`) introduces document
>   mode. Origin should be one of the metadata fields document mode carries.
>
> ## Related
>
> - #732 (observations / mental models): this is its corroboration guard, as a prerequisite.
> - #733 (document-aware extraction): origin is a property of the document/revision there.
> - Companion app issue shadoworg/liminis-notes#1327: the app stamps origin on what it sends.

## Background

Liminis Notes, LCG's main consumer, follows the "LLM wiki" pattern: a human writes notes and clips sources, and an AI agent writes summary, topic and synthesis pages back into the same notebook. LCG indexes those agent pages exactly like human notes. A fact the agent *derived* from note A therefore returns from the agent's page as a second, apparently independent episode supporting the same fact.

The effects today:

- **Inflated support.** "How many sources say this" overstates the evidence once derived episodes are counted.
- **Self-reinforcing retrieval.** When the agent writes or refreshes a page, search returns its own earlier pages next to the primary notes, so an error in a page is re-read as evidence and propagates.
- **No primary-only question.** No read can be restricted to human-authored or external material.

Liminis Notes already half-tags derived content: chat consolidations are sent with `attributes.kind = "knowledge"`. LCG stores episode `attributes` (ADR-0528) but ignores them for extraction, duplicate merging, support counting and retrieval. A free-form map is also the wrong carrier for something LCG must reason about, so origin becomes a first-class, validated field.

Related existing mechanisms the feature must respect:

- **Belief time (ADR-0705)**: a correction or removal of an episode supersedes it and ends belief in it.
- **Document supersession (ADR-0707)**: a new `doc_id` can replace an old one.
- **Provenance**: a relationship records its producing episode(s); the either-endpoint `episode_uuids` inference (ADR-0012) is separate from that.

Other LLM-wiki-style systems (Karpathy's LLM Wiki gist, claude-second-brain, gbrain, COG) file agent output back into the store and none guards against this. This work is also the corroboration guard that #732 (observations / mental models) needs: "duplicate evidence must not masquerade as independent corroboration". #733 (document-aware extraction) and the document-mode proposal in `ideas/ingestion-surface-consolidation.md` should treat origin as a property of the document/revision. The companion app issue shadoworg/liminis-notes#1327 makes the app stamp origin on what it sends.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Derived episodes cite but never corroborate (Priority: P1)

An integrator submits episodes with an `origin`. A fact the agent restates on a derived page stays linked to that page for provenance, but is counted separately from primary support. Reads show support broken down by origin.

**Why this priority**: This is the core defect (inflated support). Everything else builds on origin being recorded and counted correctly. It is also the precondition for #732.

**Independent Test**: Ingest note A (no origin) saying "X uses Postgres". Ingest an agent page with `origin: derived` restating it. Read the relationship and check that the support shows `primary: 1, derived: 1`, and that the derived episode is still reachable as provenance.

**Acceptance Scenarios**:

1. **Given** a primary episode stating "X uses Postgres", **When** a `derived` episode restates the same fact, **Then** the fact reports one primary support and one derived citation, and total corroboration is 1, not 2.
2. **Given** a client that never sends `origin`, **When** it ingests and reads exactly as before, **Then** every episode is treated as `primary` and no result changes except for additive fields.
3. **Given** an `origin` value outside the closed vocabulary, **When** it is submitted, **Then** the request is rejected with a validation error naming the allowed values, and nothing is written (no episode, no WAL entry).
4. **Given** episodes ingested with `external` and `answer` origins, **When** support is read, **Then** each is counted under its own origin key.

---

### User Story 2 - Derived content never overrides primary content (Priority: P1)

An agent page contains a wrong or conflicting statement. The graph keeps the primary fact valid and believed. The derived claim is stored as a derived-only fact and is not resolved in the page's favour.

**Why this priority**: Without this, the agent page can silently rewrite the user's own knowledge. That is worse than inflated counts.

**Independent Test**: Ingest primary "X uses Postgres". Ingest a derived page saying "X uses MySQL". The Postgres fact is still valid (`invalid_at` unset) and believed. The MySQL fact exists with derived-only support.

**Acceptance Scenarios**:

1. **Given** a fact with primary or external support, **When** a `derived` or `answer` episode contradicts it, **Then** the existing fact is not invalidated, expired or overwritten.
2. **Given** an entity that has at least one primary or external mention, **When** a derived episode mentions it with a different name form or summary, **Then** the entity's name and summary are not changed by the derived episode.
3. **Given** two entities that dedup would merge, **When** the only evidence for the merge is a derived episode and either entity has primary or external support, **Then** no merge is performed on that authority.
4. **Given** a document with `doc_id` P ingested as primary, **When** a document with `origin: derived` is recorded as superseding P (ADR-0707), **Then** the supersession is rejected with a clear error and nothing is written.
5. **Given** a primary episode, **When** a derived submission would end belief in it through revision (resubmitting the same `chunk_id`) or removal (ADR-0705), **Then** the request is refused and belief in the primary episode is unchanged.
6. **Given** a fact whose support is derived-only, **When** a primary episode contradicts it, **Then** the primary episode may invalidate it as it does today.

---

### User Story 3 - Origin-aware reads (Priority: P1)

An agent refreshing its own page searches only primary and external material, so its earlier pages cannot be re-read as evidence.

**Why this priority**: This is the second half of the self-reinforcement loop, and the only way a client can ask "what do the primary sources say?".

**Independent Test**: With both primary notes and derived pages indexed, call `knowledge_search_passages` with `origins: [primary, external]`. No passage from a derived episode is returned. Repeat for `knowledge_find_entities` and `knowledge_find_relationships`.

**Acceptance Scenarios**:

1. **Given** derived and primary passages that both match a query, **When** `knowledge_search_passages` is called with `origins: [primary, external]`, **Then** only passages from episodes of those origins are returned.
2. **Given** a relationship supported only by derived episodes, **When** `knowledge_find_relationships` is called with `origins: [primary, external]`, **Then** it is not returned.
3. **Given** an entity mentioned only by derived episodes, **When** `knowledge_find_entities` is called with `origins: [primary, external]`, **Then** it is not returned.
4. **Given** no `origins` filter, **When** any of the three reads is called, **Then** the result set and ordering are exactly as before.
5. **Given** an `origins` list containing an unknown value or an empty list, **When** a read is called, **Then** it is rejected with a validation error.

---

### User Story 4 - Origin survives WAL rebuild (Priority: P2)

An operator rebuilds the graph from the WAL. Every episode keeps its origin and every support breakdown comes out identical.

**Why this priority**: Without it, origin silently reverts to `primary` after recovery and the guard disappears. It follows P1 because the first stories deliver value on a healthy graph.

**Independent Test**: Ingest a mixed-origin corpus, record the support breakdowns, run `knowledge_rebuild_from_wal`, and compare.

**Acceptance Scenarios**:

1. **Given** episodes with `primary`, `external`, `derived` and `answer` origins, **When** the graph is rebuilt from the WAL, **Then** each episode has the same origin and every fact has the same support breakdown.
2. **Given** a WAL written before this feature, **When** it is replayed, **Then** every episode has origin `primary` and replay succeeds.
3. **Given** the "derived never supersedes primary" guards of User Story 2, **When** the WAL is replayed, **Then** the guards give the same outcome as on the original ingest (rebuild never re-opens a protected fact).

---

### User Story 5 - Stamp origin onto an existing graph (Priority: P2)

An operator has a graph full of unlabelled episodes. They stamp origin onto the agent-written ones by source path pattern or from the existing `attributes.kind`, without re-extraction.

**Why this priority**: Existing users have no origin at all. Without migration the feature protects only new data. It is separable from the ingest-time work.

**Independent Test**: Take a graph with episodes whose `attributes.kind = "knowledge"` and others under `wiki/`. Run the stamping operation as a dry run, then for real. Origins and support breakdowns change as expected and no extraction runs.

**Acceptance Scenarios**:

1. **Given** episodes carrying `attributes.kind = "knowledge"`, **When** stamping runs with a kind-to-origin mapping, **Then** those episodes get the mapped origin.
2. **Given** episodes whose source path matches a pattern, **When** stamping runs with that pattern and an origin, **Then** the matching episodes get that origin and non-matching episodes are untouched.
3. **Given** a stamping request, **When** it is run in dry-run mode, **Then** it reports which episodes would change and how, and writes nothing.
4. **Given** a completed stamp, **When** it is repeated with the same arguments, **Then** it changes nothing (idempotent).
5. **Given** a stamp, **When** the graph is rebuilt from the WAL, **Then** the stamped origins are preserved.
6. **Given** a stamp that sets origin on a large set of episodes, **When** it runs, **Then** it needs no LLM or embedding calls and no episode is re-extracted.

---

### Edge Cases

- **Same fact, many chunks of one document.** One long note split into many chunks must not corroborate itself. Support counts distinct documents (by `doc_id`) where a `doc_id` exists. An episode without a `doc_id` counts as its own document, which is the best available identity.
- **Mixed origins within one `doc_id`.** A `doc_id` has a single origin. A submission whose origin differs from the origin already recorded for that `doc_id` is rejected, and nothing is written.
- **Fact supported by both a primary and a derived episode, then the primary is superseded.** Belief in the fact follows ADR-0705 unchanged. Derived episodes are live sources for belief purposes but never make the fact "primary-supported", so reads report the fact as derived-only support.
- **Derived episode extracts a fact that no other episode states.** The fact is created, linked to the derived episode, and visible with derived-only support. It is excluded by `origins: [primary, external]`.
- **Derived episode that duplicates an existing fact.** It adds a derived citation to the existing fact and never adds to the primary count.
- **`chunk_id` collision across origins.** A derived submission reusing a primary episode's `chunk_id` is a revision attempt and is refused. See User Story 2, scenario 5.
- **Asserted facts (`knowledge_assert_*`, cross-group warrants).** They carry no producing episode, so they have no origin breakdown. Their reads are unchanged.
- **Legacy edges with no producing-episode record (ADR-0705).** They report no origin-specific support and are treated as primary for filtering, matching the "no origin = primary" rule.
- **Origin is a claim by the caller, not an authenticated attribute.** LCG does not detect misdeclared origin.
- **Stamping after supersession decisions were already taken.** Stamping changes how episodes are counted and guarded from then on. It does not retroactively undo supersessions or invalidations that already happened.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `knowledge_process_chunk` and `knowledge_add_episode` MUST accept an optional `origin` taken from the closed vocabulary `primary`, `external`, `derived`, `answer`. Invalid values MUST be rejected before any mutation, with no WAL entry.
- **FR-002**: An episode submitted without `origin` MUST be treated as `primary`. Episodes that existed before this feature MUST read as `primary` until stamped.
- **FR-003**: Origin MUST be recorded in the WAL and MUST be restored by `knowledge_rebuild_from_wal`. A WAL written before this feature MUST replay successfully with every episode as `primary`.
- **FR-004**: Origin MUST be exposed on the MCP-over-stdio tool surface as well as the IPC surface, wherever the underlying method accepts or returns it.
- **FR-005**: The vocabulary MUST classify `primary` and `external` as *corroborating* origins and `derived` and `answer` as *non-corroborating* origins.
- **FR-006**: Facts extracted from a non-corroborating episode MUST stay linked to that episode for provenance. They MUST be reported separately from corroborating support and MUST NOT count toward it.
- **FR-007**: Relationship and entity read results MUST include a `support` breakdown keyed by origin, counting distinct source documents. A `doc_id` identifies a document, and an episode without a `doc_id` counts as its own document. Adding this field MUST NOT change existing fields, result membership or ordering.
- **FR-008**: When a duplicate relationship is merged, a derived or answer episode MUST add a provenance citation but MUST NOT increase primary or external support.
- **FR-009**: A `derived` or `answer` episode MUST NOT invalidate, expire or overwrite a fact that has any primary or external support. A primary or external episode MAY invalidate a fact whose support is derived-only, exactly as ingest does today.
- **FR-010**: A `derived` or `answer` episode MUST NOT rename an entity or change its summary when that entity has any primary or external mention, and MUST NOT cause a dedup or merge between entities on its own authority when either has primary or external support.
- **FR-011**: Recording a document supersession (ADR-0707) in which the superseding document is `derived` or `answer` and the superseded document is `primary` or `external` MUST be rejected with a clear error and nothing written.
- **FR-012**: A `derived` or `answer` submission MUST NOT end belief (ADR-0705) in a `primary` or `external` episode through revision. Where a removal path knows the acting origin, the same rule applies. Such requests MUST be refused with nothing written.
- **FR-013**: All episodes sharing a `doc_id` within a group MUST share one origin. A conflicting submission MUST be rejected.
- **FR-014**: `knowledge_find_relationships`, `knowledge_find_entities` and `knowledge_search_passages` MUST accept an optional `origins` filter, a non-empty list drawn from the vocabulary. An unknown value or an empty list MUST be rejected. A relationship or entity matches if at least one of its supporting or mentioning episodes has an origin in the list. A passage matches if its episode's origin is in the list. Omitting the filter MUST leave behaviour unchanged.
- **FR-015**: Passage results MUST report the origin of the episode they come from.
- **FR-016**: Origin of an entity or relationship MUST always be derived from its episodes. It MUST NOT be stored as an independent field that can drift from them.
- **FR-017**: A client that never sends `origin` or `origins` MUST see unchanged results from every existing method, apart from additive fields.
- **FR-018**: An administrative stamping operation MUST set origin on existing episodes without re-extraction, selecting episodes by source-path pattern and/or by the value of `attributes.kind` with an explicit kind-to-origin mapping. It MUST support a dry run that reports what would change and writes nothing.
- **FR-019**: The stamping operation MUST be idempotent, MUST be recorded in the WAL so it survives rebuild, MUST NOT call extraction or embedding, and MUST NOT retroactively undo supersessions or invalidations that already happened.
- **FR-020**: Documentation MUST be updated for the new field, the guard rules, the filter and the stamping operation. This includes an ADR numbered by issue (`docs/adr/0752-…`), the README tool and scope notes, and the `ipc_parity` and MCP tool-registry tests that the new surface requires.

### Key Entities *(if applicable)*

- **Origin**: A closed-vocabulary label on an episode (`primary`, `external`, `derived`, `answer`) declaring what kind of content the episode is. It is a caller's claim. Corroborating origins are `primary` and `external`. Non-corroborating origins are `derived` and `answer`.
- **Support breakdown**: A per-origin count of distinct source documents supporting a relationship, or mentioning an entity. A `doc_id` identifies a document. An episode without a `doc_id` counts as its own document.
- **Episode**: Gains an origin. Origin is constant across all episodes of one `doc_id`.
- **Document (`doc_id`)**: The natural unit of origin. An agent page is derived as a whole.
- **Stamp operation**: An administrative, WAL-recorded, idempotent bulk update of episode origin.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In the scenario "primary note + derived page restating it", the fact reports exactly one primary support and one derived citation, and corroboration is 1.
- **SC-002**: Across a test graph containing both primary and derived episodes, `origins: [primary, external]` returns zero results whose only evidence is derived, for all three read methods.
- **SC-003**: After a derived episode contradicts a primary-supported fact, that fact's `invalid_at` is unset, its belief is unchanged, and the entity name and summary involved are unchanged. The derived contradiction is stored as a separate fact.
- **SC-004**: After a WAL rebuild of a mixed-origin graph, origin and the support breakdown are identical for 100% of episodes, relationships and entities.
- **SC-005**: An existing client test suite that never sends `origin` or `origins` passes unchanged, including existing IPC parity tests.
- **SC-006**: A derived document recorded as superseding a primary document, and a derived revision of a primary chunk, are both rejected in 100% of attempts and leave the graph byte-for-byte unchanged (no WAL entry).
- **SC-007**: Stamping an existing graph of N episodes completes without any extraction or embedding calls. A repeat run reports zero changes.
- **SC-008**: A single long document split into many chunks contributes exactly 1 to the support count of a fact that every chunk restates, when a `doc_id` is supplied.

## Assumptions

- **Vocabulary**: `primary`, `external`, `derived`, `answer`, as proposed in the issue. A closed set, extended only by a later change.
- **Protection threshold is wider than the issue's "primary-only"**: derived content cannot override a fact with any primary *or* external support. Clipped third-party material is still a source, and protecting only primary would let a derived page overwrite it. Primary versus external interplay is unchanged from today, and neither can lose to the other on origin grounds.
- **Support counts distinct documents**, falling back to one per `doc_id`-less episode. A chunk-split note without a `doc_id` can still over-count. Callers who want the guarantee supply a `doc_id`. This is the answer to the "distinct documents rather than episodes" research question.
- **Origin lives on episodes and is derived upward** onto entities and relationships. It is not stored independently on them.
- **Document-level origin**: origin is semantically a property of the document, so one `doc_id` has one origin (FR-013). Whether the carrier is a per-episode column, a per-document record alongside `DocSupersession`, or both is a Plan-stage decision. The behaviour in this spec holds either way. Episodes without a `doc_id` carry origin themselves.
- **Where a duplicate relationship is merged** (the first research question) is deliberately left to Research. The spec requires only the outcome in FR-006 and FR-008.
- **`support` is an additive, always-present field** on relationship and entity reads. Additive fields are not a behaviour change, and the `origins` filter itself is opt-in.
- **Conflict reporting** is satisfied by the derived contradiction being stored as its own fact that is distinguishable by its derived-only support and filterable by origin. No dedicated conflict-report method is needed.
- **Origin is trusted as declared** by the calling application. The companion app issue (shadoworg/liminis-notes#1327) is responsible for stamping it correctly.
- **Existing derived-ish data** (episodes with `attributes.kind = "knowledge"`) is not auto-reinterpreted. It becomes `derived` only through an explicit stamp, so behaviour never changes silently.
- **Document mode** (`ideas/ingestion-surface-consolidation.md`) does not exist yet. When it lands it must carry origin as document metadata, but that is out of scope here.

## Out of Scope *(optional)*

- Detecting or inferring origin from content, and authenticating origin claims.
- A dedicated conflict-report method or UI.
- Observations and mental models (#732), which consume this feature.
- Document-aware extraction (#733) and document mode itself.
- App-side stamping of origin (shadoworg/liminis-notes#1327).
- Per-chunk differing origins within one `doc_id`.
- Re-running extraction or re-evaluating past supersession decisions after a stamp.
- Changing the `episode_uuids` either-endpoint inference of ADR-0012.
- Per-tenant or per-client configurable vocabularies.

## Source References *(optional)*

- ADR-0705 (belief time separate from validity), ADR-0707 (document supersession), ADR-0528 (episode attributes), ADR-0012 (episode_uuids inference).
- `ideas/ingestion-surface-consolidation.md` (document mode).
- #732, #733, shadoworg/liminis-notes#1327.
- Karpathy's LLM Wiki gist, claude-second-brain, gbrain, COG: the prior art named in the issue, which files agent output back into the store without a guard.
