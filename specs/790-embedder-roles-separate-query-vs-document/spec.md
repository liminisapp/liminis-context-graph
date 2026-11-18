# Feature Specification: Embedder roles — separate query vs document embedding, with per-model instructions (BGE / nomic / e5)

**Feature Branch**: `fabrik/issue-790`
**Created**: 2026-11-18
**Status**: Specified
**Input**: User description: "## Problem

LCG embeds a search query and a document chunk the same way. The `Embedder` trait (`crates/core/src/embedder.rs:300`) has a single `embed(text)` / `embed_batch(texts)`, with no notion of which side of a search the text is on. That shape comes from the OpenAI `/v1/embeddings` contract (ADR-0006, ADR-0016), whose own models don't need a role.

The open models LCG actually runs with are **asymmetric** and were trained with role prefixes:

| Model | Used by | Expects |
|---|---|---|
| BGE-base-en-v1.5 | the bundled Swift sidecar (`native/local-inference`), i.e. **every Liminis Notes user on macOS** | the query prefix `Represent this sentence for searching relevant passages: `; documents get none |
| nomic-embed-text v1.5 | Ollama users | `search_query: ` / `search_document: ` on both sides |
| e5 family | — | `query: ` / `passage: ` |

Nothing in LCG or the sidecar adds these, so every vector search runs these models outside how they were trained. The #754 eval saw the effect: on the nomic run, LCG's vector-only passage search trailed qmd on paraphrase and answer questions (see #789).

## Desired behaviour

1. **Embedding role.** `embed` / `embed_batch` take a role, either `Query` or `Document`. Every call site states its role:
   - **Query** (a user or agent search string): `search_passages`, the vector half of entity and relationship search, and anything else that embeds a search string.
   - **Document** (stored text): episode content, entity names and summaries, and edge facts.
   - **Symmetric comparisons** such as entity resolution and dedup, which compare a name against a name, use `Document` on both sides unless research shows otherwise. Specify must list every call site and its role.
2. **Per-model instructions**, configured as a named profile (`none`, `bge-v1.5`, `nomic-v1.5`, `e5`) plus explicit overrides for the query and document prefixes. The default is `none`, so behaviour is unchanged unless a profile is chosen. Research should decide whether LCG can pick the profile automatically: the sidecar is ours, so it could advertise its model and recommended instructions, for example from `/v1/models`.
3. **The document prefix is part of embedding identity.** The workspace already records the embedding model and dimension. Add the document prefix to that record. When it changes, treat it like a model change: refuse or warn, and re-embed through the existing recompute path (ADR-0440 / ADR-0486). **A query prefix change needs no re-embed,** because stored vectors are unaffected. So `bge-v1.5`, which has a query instruction only, can be switched on for existing workspaces at no cost.
4. **The embedding cache** (`embedding_cache.rs`) keys on role and prefix as well as text, so a query and a chunk with the same text never share a vector.
5. **Sidecar:** keep it a plain OpenAI-compatible embedder. LCG adds the prefixes, so any embedder behind it benefits. If research picks the advertise option in item 2, the sidecar exposes its recommended profile.
6. **Docs:** `docs/configuration.md` explains the profiles and the re-embed rule.

## Acceptance

- In the #754 eval with the sidecar embedder, `lcg-passages` and the entity/fact configurations are measured with profile `none` and with `bge-v1.5`, and the report shows the difference. With Ollama, do the same for `none` vs `nomic-v1.5`, after a re-embed.
- An existing BGE workspace switched to `bge-v1.5` needs no re-embed, and searches immediately use the instruction.
- Changing to a profile with a document prefix on an existing workspace triggers the documented re-embed path, and never silently mixes vectors.
- With profile `none` everything is unchanged, byte for byte. The existing embedder tests and the WAL replay tests pass.

## Related

This takes step 2 out of #789, because it is independent of the new corpus (#788) and benefits every Notes user now. ADR-0006, ADR-0016, ADR-0440, ADR-0486. The app side, which switches the profile on for the bundled sidecar, is filed in shadoworg/liminis-notes."

## Background

LCG talks to embedders through an OpenAI-compatible `/v1/embeddings` contract (ADR-0006, ADR-0016), which has no notion of whether the text being embedded is a *search query* or a *stored document*. The models LCG actually runs on — BGE-base-en-v1.5 (bundled sidecar, every Liminis Notes user on macOS), nomic-embed-text v1.5 (Ollama), and the e5 family — are **asymmetric**: they were trained with role-specific instructions/prefixes, and their retrieval quality depends on them being applied. Today LCG adds none, so every vector search runs these models outside their training distribution. The #754 eval showed the effect: on the nomic run, vector-only passage search trailed qmd on paraphrase and answer questions (#789).

This issue splits the fix out of #789 because it is independent of the new eval corpus (#788) and benefits Notes users immediately. The sidecar stays a plain OpenAI-compatible embedder; **LCG** adds the instructions, so any embedder behind it benefits.

Key design constraint carried from the issue: only the **document-side** prefix changes what is stored. The workspace already records the embedding model and dimension for re-embed-on-change (ADR-0440 / ADR-0486); the document prefix must join that identity, while the query prefix must not — so BGE (query instruction only) can be switched on for existing workspaces at zero cost.

### Embed call sites and their roles

Every place LCG sends text to the embedder, and the role it must state. This is the catalogue the requirements below refer to.

| Call site (behaviour) | Role |
|---|---|
| Passage search (`search_passages`) — the search string | **Query** |
| Entity search — the search string, used for both the name-vector and summary-vector halves | **Query** |
| Relationship/edge search — the search string, used for the fact-vector half | **Query** |
| Episode ingest — episode/chunk content | **Document** |
| Episode ingest — extracted entity names | **Document** |
| Episode ingest — extracted entity summaries (including re-embedding of merged summaries after entity merge) | **Document** |
| Episode ingest — extracted edge facts | **Document** |
| Episode ingest — names of off-list edge endpoints materialised as new entities | **Document** |
| Direct entity add — entity name and summary | **Document** |
| Direct edge add (`add_edge`-style handlers) — fact | **Document** |
| Entity-summary embedding backfill | **Document** |
| WAL replay / embedding recompute (including cache-fronted batch recompute) | **Document** (everything replayed is stored text) |
| Entity resolution / dedup name-vs-name comparison | **Document** on both sides (symmetric; per the issue, unless research shows otherwise) |
| Ontology canonicalization fallback — type gloss descriptions and relationship facts compared by cosine similarity | **Document** on both sides (symmetric text-vs-text classification; research may revisit) |

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Role-aware embedding with a profile that instructs queries (Priority: P1)

An operator (or the Liminis Notes app on behalf of a macOS user) running the bundled BGE sidecar selects profile `bge-v1.5`. Search strings are embedded with BGE's query instruction; stored text is embedded exactly as before. Vector search quality improves with no re-embed.

**Why this priority**: This is the core value — it fixes the model-vs-training mismatch for the largest user population (all macOS Notes users) at zero migration cost.

**Independent Test**: With a stub embedder that records the exact strings it receives, run a passage search under profile `bge-v1.5` and confirm the string sent contains the BGE query instruction followed by the user query, while an episode ingest sends the raw chunk text unchanged.

**Acceptance Scenarios**:

1. **Given** profile `bge-v1.5` and an existing workspace embedded under profile `none`, **When** the service starts, **Then** it reports the embedding identity as matching, requires no re-embed, and a subsequent passage search embeds the query with the BGE instruction.
2. **Given** profile `nomic-v1.5`, **When** a search string is embedded, **Then** it is sent as `search_query: <text>`; **When** a document is embedded, **Then** it is sent as `search_document: <text>`.
3. **Given** profile `e5`, **When** a query and a document are embedded, **Then** they are sent with `query: ` and `passage: ` prefixes respectively.

---

### User Story 2 - Document prefix is part of embedding identity (Priority: P1)

An operator switches an existing workspace to a profile whose document prefix differs from the one the stored vectors were produced with (e.g. `none` → `nomic-v1.5`). LCG treats this like a model change: it never silently mixes old and new vectors, and offers/triggers the documented re-embed path.

**Why this priority**: Without this, switching to a document-prefixed profile would silently corrupt retrieval by mixing vector spaces. Correctness gate for the whole feature.

**Independent Test**: Create a workspace under profile `none`, restart under `nomic-v1.5`, and confirm the status surface reports a mismatch, that the existing recompute path re-embeds stored text with the document prefix, and that afterwards the identity records the new document prefix.

**Acceptance Scenarios**:

1. **Given** a workspace whose recorded identity has an empty document prefix, **When** the service starts with a profile that has a non-empty document prefix, **Then** the embedding-model status reports a mismatch (same treatment as a model-name or dimension change).
2. **Given** a recorded mismatch, **When** the operator runs the documented rebuild/recompute path, **Then** all stored vectors are recomputed with the new document prefix and the recorded identity is updated.
3. **Given** a workspace recorded under document prefix X, **When** only the *query* prefix changes, **Then** the status is `match` and no re-embed is required.
4. **Given** a workspace recorded before this feature (no prefix recorded), **When** the service starts with profile `none`, **Then** the status is `match` (an absent prefix is equivalent to the empty prefix) — existing workspaces are not forced to re-embed by upgrading.

---

### User Story 3 - Configurable profiles and explicit overrides (Priority: P2)

An operator chooses a named profile (`none`, `bge-v1.5`, `nomic-v1.5`, `e5`) and may override the query prefix and/or document prefix explicitly for a model or fine-tune not covered by a profile. Default is `none`.

**Why this priority**: Profiles deliver the common cases; overrides cover everything else. Neither is useful until Stories 1–2 exist.

**Independent Test**: Start with no configuration and confirm every embedder input is byte-identical to today's. Start with a profile plus an override and confirm the override wins for that side only.

**Acceptance Scenarios**:

1. **Given** no profile or override configured, **When** any text is embedded, **Then** the string sent equals the raw text (byte-for-byte unchanged behaviour).
2. **Given** profile `bge-v1.5` and an explicit document-prefix override, **When** documents are embedded, **Then** the override is used and the profile's (empty) document prefix is ignored; the query side still uses the profile's instruction.
3. **Given** an unknown profile name, **When** the service starts, **Then** it fails fast with an error that lists the valid profile names.

---

### User Story 4 - Cache never conflates roles (Priority: P2)

A query and a stored chunk with identical text must never share a cached vector, since the model sees different inputs.

**Why this priority**: A correctness property of the embedding cache; without it, profiles with a query prefix would return wrong vectors for repeated text.

**Independent Test**: Embed the same string once as `Query` and once as `Document` under a prefixed profile and confirm two embedder calls occur and two distinct vectors are returned; repeat each and confirm cache hits.

**Acceptance Scenarios**:

1. **Given** a prefixed profile, **When** identical text is embedded as Query then Document, **Then** the embedder is called for both and results are cached independently.
2. **Given** two embedder contexts differing only in prefix configuration sharing one cache, **When** they embed the same text and role, **Then** they do not collide.

---

### User Story 5 - Measured benefit in the retrieval eval (Priority: P3)

The #754 retrieval eval measures profile `none` vs the model's profile and reports the difference: sidecar (BGE) `none` vs `bge-v1.5`; Ollama (nomic) `none` vs `nomic-v1.5` after a re-embed.

**Why this priority**: Evidence the feature does what it claims; informs the recommended default for Notes. Not needed for the feature to function.

**Independent Test**: Run the eval harness for both configurations and confirm the report includes both rows per configuration (`lcg-passages` and the entity/fact configurations) with the delta.

**Acceptance Scenarios**:

1. **Given** the sidecar embedder, **When** the eval runs under `none` and `bge-v1.5`, **Then** the report shows both results for `lcg-passages` and the entity/fact configurations side by side.
2. **Given** an Ollama nomic embedder, **When** the eval runs under `none` and `nomic-v1.5` (with a re-embed between), **Then** the report shows both.

---

### User Story 6 - Operators can find and understand the settings (Priority: P3)

`docs/configuration.md` documents the profiles, the override settings, and the re-embed rule (document prefix changes need re-embed; query prefix changes do not).

**Why this priority**: Required for adoption but depends on the behaviour being settled.

**Independent Test**: Read the doc and confirm it names every profile with its exact prefixes, every setting, the default, and the re-embed rule.

**Acceptance Scenarios**:

1. **Given** the docs, **When** an operator wants to enable BGE instructions on an existing workspace, **Then** the docs state that no re-embed is needed.
2. **Given** the docs, **When** an operator wants nomic prefixes on an existing workspace, **Then** the docs state a re-embed is required and how to trigger it.

---

### Edge Cases

- **Empty-string text** or text that is empty after trimming: existing behaviour (skipping or zero-vector sentinel) must be preserved; a prefix must never turn an otherwise-skipped empty text into a non-empty embed request.
- **Zero-vector sentinels** for empty summaries and embedder failures are role-independent and unchanged.
- **Text that already begins with the prefix** (e.g. a user query that literally starts with `query: `): the prefix is still applied; no de-duplication heuristics.
- **Pre-feature workspaces** with no recorded prefix: treated as empty prefix (equivalent to profile `none`) so upgrading never triggers a spurious mismatch.
- **Explicit override set to the empty string** under a profile that has a prefix: an explicit empty override disables that side's prefix (distinguishable from "not set").
- **Mixed state during re-embed**: while a document-prefix mismatch is outstanding and un-recomputed, newly ingested documents must not silently be embedded under the new prefix alongside old-prefix vectors without the mismatch being surfaced.
- **Batching**: a batch call applies the role's prefix to every element, and returns vectors in input order as today.
- **Replay on a different process/profile**: WAL replay always embeds as `Document` under the running configuration's document prefix, regardless of the profile in effect when the row was originally written.
- **Token limits**: prefixes lengthen inputs slightly; inputs near an embedder's token limit behave as the embedder already does for over-long input.
- **Embedders that ignore instructions** (e.g. OpenAI models): selecting a profile for such a model is the operator's choice; LCG does not validate model/profile pairing.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Embedding requests MUST carry an explicit role, either `Query` or `Document`, for both single-text and batch embedding. There MUST be no embedding path with an implicit or defaulted role.
- **FR-002**: Every embed call site in the service MUST state its role per the catalogue in `## Background`: search strings (passage, entity, and relationship search) are `Query`; all stored text (episode content, entity names and summaries, edge facts, backfills, WAL replay/recompute) is `Document`; entity-resolution/dedup name-vs-name comparisons and ontology-canonicalization gloss-vs-fact comparisons use `Document` on both sides.
- **FR-003**: The service MUST support a named instruction profile with values `none`, `bge-v1.5`, `nomic-v1.5`, and `e5`, defined as: `none` — no prefixes; `bge-v1.5` — query prefix `Represent this sentence for searching relevant passages: `, no document prefix; `nomic-v1.5` — query prefix `search_query: `, document prefix `search_document: `; `e5` — query prefix `query: `, document prefix `passage: `.
- **FR-004**: The profile MUST default to `none`. With `none` (and no overrides), the text sent to the embedder MUST be byte-identical to today's for every call site, and existing embedder and WAL-replay tests MUST continue to pass unchanged in behaviour.
- **FR-005**: Operators MUST be able to override the query prefix and the document prefix independently of the profile. An explicit override (including an explicit empty string) takes precedence over the profile's value for that side only.
- **FR-006**: An unrecognised profile name MUST cause a clear startup error listing the valid profile names.
- **FR-007**: The prefix for a role MUST be applied by LCG before text reaches the embedder, so any OpenAI-compatible embedder behind the service benefits. The embedding sidecar remains a plain OpenAI-compatible embedder with no role awareness required of it.
- **FR-008**: The effective **document prefix** MUST be recorded as part of the workspace's embedding identity alongside the existing model name and dimension. The query prefix MUST NOT be part of the recorded identity.
- **FR-009**: A difference between the recorded document prefix and the running document prefix MUST be treated like a model or dimension change: the embedding-model status surfaces a mismatch, and re-embedding occurs through the existing recompute path (ADR-0440 / ADR-0486). Stored vectors produced under different document prefixes MUST never be silently mixed.
- **FR-010**: A difference in the query prefix alone MUST NOT produce a mismatch and MUST NOT require or trigger a re-embed; the new query prefix MUST take effect on the next search.
- **FR-011**: An absent recorded document prefix (workspaces written before this feature) MUST be treated as the empty prefix, so upgrading with profile `none` yields a `match` status.
- **FR-012**: The embedding cache key MUST include the role and the effective prefix applied for that role in addition to the text, so identical text embedded as `Query` and as `Document` never shares a cached vector, and contexts with different prefix configurations never collide.
- **FR-013**: Batch embedding MUST apply the correct role prefix to every element and return one vector per input in input order, preserving existing empty-input and error semantics.
- **FR-014**: Empty or skipped texts MUST remain skipped (or zero-vector sentinel) exactly as today; a prefix MUST NOT convert a skipped empty text into an embed request.
- **FR-015**: The retrieval eval (#754) MUST support running each embedder configuration with a chosen profile, and its report MUST show `lcg-passages` and the entity/fact configurations for profile `none` versus the model-appropriate profile: sidecar BGE `none` vs `bge-v1.5`; Ollama nomic `none` vs `nomic-v1.5` after a re-embed.
- **FR-016**: `docs/configuration.md` MUST document the profiles with their exact prefixes, the override settings, the default, and the re-embed rule (document-prefix change requires re-embed; query-prefix change does not).
- **FR-017**: The configured profile and effective prefixes MUST be visible in the service status surface alongside the existing embedding identity, so operators can see what is in effect.
- **FR-018**: Research MUST decide whether LCG can select the profile automatically from information the embedder advertises (e.g. the sidecar's model listing and recommended instructions). Whatever is chosen, explicit operator configuration MUST always take precedence, and an embedder that advertises nothing MUST behave as profile `none`.

### Key Entities *(if applicable)*

- **Embedding role**: Which side of retrieval a text is on — `Query` (a search string) or `Document` (stored text).
- **Instruction profile**: A named bundle of a query prefix and a document prefix for a model family (`none`, `bge-v1.5`, `nomic-v1.5`, `e5`).
- **Embedding identity**: The recorded `(model, dimension)` pair per workspace, extended with the document prefix; the thing compared on startup/replay to decide whether stored vectors are compatible with the running embedder.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With profile `none`, a test capturing every string sent to the embedder across all call sites shows output byte-identical to the pre-change behaviour, and all existing embedder and WAL-replay tests pass.
- **SC-002**: For each profile, a test asserts the exact string sent to the embedder for a Query and for a Document matches the profile definition in FR-003.
- **SC-003**: An existing workspace recorded under `none` and started under `bge-v1.5` reports embedding status `match`, performs zero re-embeds, and its next search embeds the query with the BGE instruction.
- **SC-004**: An existing workspace started under a profile whose document prefix differs from the recorded one reports `mismatch`, and after the documented recompute path completes, every stored vector has been recomputed and the recorded identity reflects the new prefix; no point exists where vectors of two document prefixes are served as one consistent index without a surfaced mismatch.
- **SC-005**: Embedding identical text as `Query` and as `Document` under a prefixed profile produces two embedder calls and two independent cache entries; repeating each produces cache hits with no additional embedder calls.
- **SC-006**: The #754 eval report contains `none` and profile rows for `lcg-passages` and the entity/fact configurations on the sidecar (BGE) embedder, and for nomic on Ollama after re-embed, with the difference visible per configuration.
- **SC-007**: A reader of `docs/configuration.md` can, without reading code, determine which profile to pick for BGE/nomic/e5 and whether switching requires a re-embed.

## Assumptions

- The profile prefixes are exactly those in the issue (BGE query instruction with trailing space; `search_query: `/`search_document: `; `query: `/`passage: `); no profile alters documents for BGE.
- Entity-resolution/dedup and ontology-canonicalization comparisons are symmetric and use `Document` on both sides, as the issue directs; research may revisit if measurement shows a better role for them.
- Re-embed behaviour on a document-prefix mismatch reuses the existing mismatch-detection and recompute machinery (ADR-0440 / ADR-0486) rather than introducing a new mechanism; whether the response is "refuse" or "warn + recompute on rebuild" follows what that machinery already does for a model-name change, so a prefix change behaves identically to a model change.
- The recorded prefix for pre-feature workspaces is treated as empty (not "unknown"), so upgrading never forces a re-embed.
- Configuration follows the project's existing convention for embedder settings (CLI flags and `LCG_EMBEDDING_*` environment variables, plus the `GRAPHITI_EMBEDDING_*` legacy aliases where the existing settings have them); exact setting names are a Plan-stage decision.
- A single embedder (hence a single profile) is in effect per process, matching the current "one embedder per process" model.
- The eval acceptance runs (sidecar and Ollama) are executed on-demand like other eval work, not as a PR-blocking CI gate.

## Out of Scope *(optional)*

- Changing the sidecar's embedding behaviour: it stays a plain OpenAI-compatible embedder (only, optionally, advertising its recommended profile if research selects that option).
- The app-side change that switches the profile on for the bundled sidecar (filed in shadoworg/liminis-notes).
- The new evaluation corpus (#788) and the broader #789 work.
- Model-specific features beyond text prefixes (e.g. Matryoshka truncation, task-specific heads, instruction-following embedders with free-form task descriptions).
- Validating that a chosen profile is appropriate for the configured model.
- Per-group or per-request profile selection.
- Changing the default profile from `none`.

## Source References *(optional)*

- ADR-0006 (embedder HTTP contract), ADR-0016 (OAI embedding contract / UDS transport), ADR-0440 (recompute embeddings on WAL replay), ADR-0486 (batch WAL-replay embedding recompute)
- Issues #754 (retrieval eval), #788 (new corpus), #789 (parent)
- BGE-base-en-v1.5, nomic-embed-text v1.5 and e5 model cards (documented query/document instructions)
