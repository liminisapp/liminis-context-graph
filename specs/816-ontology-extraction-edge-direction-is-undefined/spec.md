# Feature Specification: Ontology edge direction — define, validate and repair

**Feature Branch**: `fabrik/issue-816`
**Created**: 2026-11-20
**Status**: Specified
**Input**: User description:

> ## Problem
>
> With an ontology in strict mode, LCG never establishes which end of an extracted edge is the source, and never checks it. On the v2 eval corpus (meridian-notebook-2), extracted under a 27-type notebook ontology, **34% of typed edges on known-true links point the wrong way** (48 of 142):
>
> | relation | reversed |
> |---|---|
> | `APPROVES` | 22 / 28 (79%) |
> | `CONTACT_FOR` | 18 / 24 (75%) |
> | `OWNS` | 6 / 29 (21%) |
> | `SUPPLIED_BY` | 2 / 17 (12%) |
> | `LOCATED_IN` | 0 / 27 |
> | `REPORTS_TO` | 0 / 17 |
>
> The extractor makes the **sentence's grammatical subject** the source:
> - "Sable Core is owned by Lena Moreau" is stored as `Sable Core OWNS Lena Moreau`.
> - "Exceptions to the Lumen Policy require sign-off from Bram" is stored as `Lumen Policy APPROVES Bram`.
>
> Relations whose natural subject is the person (`LOCATED_IN`, `REPORTS_TO`) come out right. Relations written from the other party's point of view come out mostly reversed.
>
> Where direction carries the meaning, this gives confidently wrong answers. "Who does X report to" and "who reports to X" read the same edge type in opposite directions, and names and types can't tell them apart. (Measured in #815, E13 in `docs/retrieval-experiments.md`.)
>
> **The four causes, all on `main` @ `19759906`:**
> 1. **The edge prompt never defines direction.** `crates/core/src/prompts/extract_edges.txt` requires only that `source_name` and `target_name` are distinct listed entities.
> 2. **Half signatures are dropped silently.** `build_fact_types_section` (`crates/core/src/prompts/mod.rs:78`) renders `(Source → Target)` only when *both* `source_type` and `target_type` are set. A relation declaring only `source_type: Person` shows the model no direction at all.
> 3. **Endpoints are never validated.** `source_type`/`target_type` are parsed (`ontology.rs`) and hashed, but no code compares an edge's endpoint entity types against them, in strict mode or anywhere else.
> 4. **Aliases and keywords can't express an inverse.** `build_alias_map` (`canonicalize.rs`) maps every alias to its canonical type unchanged. An inverse-reading name such as `HAS_EXCEPTION_APPROVER` for `APPROVES`, or a keyword hit like `approv` on `APPROVED_BY`, keeps the reversed endpoints. The ontology schema also allows one type per endpoint, so a relation like `OWNS` (a person or team owns a project, system, policy or agreement) can't be declared correctly.
>
> ## Change
>
> **Schema (`ontology.rs`), all additive. Every existing ontology loads unchanged and its hash is unchanged.**
> - `source_type` and `target_type` accept a string or a list of entity type names. A type matches if it, or one of its declared ancestors (`parent`), is in the list. An unset side accepts any type.
> - `inverse_aliases: [..]`: names that read the other way. They map to the canonical type with the endpoints swapped.
> - `symmetric: true`: endpoint order carries no meaning, so it is never checked or swapped.
> - Include the new fields in the ontology hash only when they are present, following the ADR-0032 approach for `parent`, so existing ontologies keep their hash.
>
> **Extraction prompt (`prompts/`):**
> - `extract_edges.txt` defines direction for every relation: "`source_name` is the subject of the relation *as its type name reads*, not the subject of the sentence. 'Sable Core is owned by Lena' is `Lena OWNS Sable Core`."
> - `build_fact_types_section` renders half and list signatures: `(Person → any)`, `(Person | Team → Project | System)`. It also renders inverse aliases ("`APPROVED_BY` = `APPROVES` with source and target swapped") and marks symmetric relations.
> - Open-mode output may change too. ADR-0310's byte-identical open-mode constraint was about the type section; the direction definition applies to every mode. Say so in the ADR update.
>
> **Ingest-time endpoint check (strict and open, whenever the ontology declares a signature):** after the relation type is resolved and both endpoints' entity types are known (`episode.rs`, where the strict-mode filter runs), and before the edge is written:
> - An inverse alias or inverse keyword swaps the endpoints first.
> - Endpoints fit the signature: store as is.
> - Endpoints fit only when reversed: swap them. Count the swap in the ingest stats.
> - They fit neither way: **store the edge unchanged** and mark it with a `signature_mismatch` attribute. Don't drop it. Many such edges are incidental facts with loosely typed entities (in E13, 38% of typed edges, mostly `WORKS_ON` between a Project and a Topic or Policy). Dropping them would lose real facts.
> - Symmetric types and types with neither side declared are skipped.
>
> **Repair for existing graphs.** The check runs over a stored graph without re-extraction, through `knowledge_canonicalize_relations`, which already re-resolves relation types offline:
> - It applies the same swap and mark rules.
> - It supports `dry_run` and reports counts per type: checked, swapped, mismatched.
> - It is idempotent.
>
> This lets existing workspaces be fixed without re-ingesting. In the E13 simulation of this exact rule on the existing graph, reversed known-true links went from **48/142 to 1/142**.
>
> **Docs:** the ontology reference (signature lists, `inverse_aliases`, `symmetric`, direction semantics), an ADR update (or a new ADR) for the endpoint check and the repair pass, `docs/llms-full.txt` regenerated, and a CHANGELOG `Unreleased` entry.
>
> ## Acceptance
>
> - **Schema:** an ontology using lists, `inverse_aliases` and `symmetric` loads. An existing ontology without them loads with an identical hash. An entity type in a signature that the ontology doesn't declare logs a load warning naming the relation and the type. It is not an error, so existing ontologies keep loading, and that side of the signature is then ignored by the check.
> - **Prompt:** the edge prompt contains the direction definition, and renders `(Person → any)` for a source-only signature and list signatures joined with `|`. Keep the prompt snapshot tests and update them deliberately.
> - **Endpoint check, unit-tested on `MockExtractor` output:**
>   - (a) correct orientation is kept;
>   - (b) a reversed `Policy APPROVES Person` with `source_type: Person` is swapped;
>   - (c) an `APPROVED_BY` edge (an inverse alias) is stored as `Person APPROVES Policy`;
>   - (d) a neither-way edge is stored unchanged and marked `signature_mismatch`;
>   - (e) a symmetric type is untouched;
>   - (f) parent types match through ancestry.
> - **Repair:**
>   - `knowledge_canonicalize_relations` with `dry_run` reports the swap and mismatch counts without writing.
>   - A real run applies them, and a second run reports zero swaps.
> - **Eval:** on the v2 cassette workspace, the repair pass takes the measured reversal on known-true links from 34% to ≤ 2%. The E13 harness and the fixed test ontology are in #815's scratch (`ontology-v3.2.yaml`); the maintainer runs this and posts the numbers.
> - No other ingest behaviour changes. Edges are never dropped by this check.
>
> ## Out of scope
>
> - Re-extracting corpora. The repair pass covers existing graphs, and fresh extractions get the prompt fix.
> - The typed-walk / planner tool (#815, separate spec).
> - Note-ID junk entities (`project-quill-core-1`) created under ontology extraction. Separate issue if wanted.
> - Inferring signatures automatically from data.

## Background

With an ontology in strict mode, LCG never establishes which end of an extracted edge is the source, and never checks it. On the v2 eval corpus (meridian-notebook-2, 27-type notebook ontology), 34% of typed edges on known-true links (48 of 142) point the wrong way. Reversal is concentrated in relations written from the other party's point of view: `APPROVES` 22/28, `CONTACT_FOR` 18/24, `OWNS` 6/29, `SUPPLIED_BY` 2/17; relations whose natural subject is the person (`LOCATED_IN`, `REPORTS_TO`) are not reversed. The extractor treats the sentence's grammatical subject as the source ("Sable Core is owned by Lena Moreau" becomes `Sable Core OWNS Lena Moreau`).

Where direction carries meaning this gives confidently wrong answers: "who does X report to" and "who reports to X" read the same edge type in opposite directions, and names and types cannot tell them apart (measured in #815, E13 in `docs/retrieval-experiments.md`).

Four causes, all present on `main`:
1. The edge extraction prompt never defines what "source" means.
2. A relation that declares only one end of its signature shows the model no direction at all.
3. Declared endpoint types are parsed and hashed but never compared against an edge's actual endpoint entity types.
4. Aliases and keywords cannot express an inverse reading (e.g. `HAS_EXCEPTION_APPROVER` for `APPROVES`), and an endpoint can declare only one type, so relations like `OWNS` (person or team owns project, system, policy or agreement) cannot be declared correctly.

In a simulation of the proposed rule on the existing E13 graph, reversed known-true links dropped from 48/142 to 1/142.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Ontology author declares direction precisely (Priority: P1)

An ontology author declares, per relation, which entity types may be the source and the target (one type or several), which names read the relation backwards, and which relations are symmetric.

**Why this priority**: Everything else (prompting, checking, repair) depends on the ontology being able to express direction. Without it, relations like `OWNS` cannot be declared correctly.

**Independent Test**: Load an ontology using type lists, `inverse_aliases` and `symmetric`; load an existing ontology without them and compare its hash before and after.

**Acceptance Scenarios**:

1. **Given** an ontology whose relation declares `source_type: [Person, Team]` and `target_type: [Project, System]`, **When** it loads, **Then** it loads without error and the signature is available to the extractor and the endpoint check.
2. **Given** an existing ontology that uses none of the new fields, **When** it loads, **Then** it loads unchanged and its hash is identical to the hash before this change.
3. **Given** a signature naming an entity type the ontology does not declare, **When** it loads, **Then** a warning names the relation and the unknown type, loading succeeds, and that side of the signature is ignored by the check.
4. **Given** a relation declaring only `source_type`, **When** the check runs, **Then** the unset target side accepts any type.

---

### User Story 2 - New extractions come out in the right direction (Priority: P1)

When the extractor is prompted, it is told that `source_name` is the subject of the relation as its type name reads (not the sentence's grammatical subject), and it sees each relation's full signature, inverse aliases and symmetry.

**Why this priority**: Fixing direction at the source reduces how much the endpoint check has to correct.

**Independent Test**: Render the edge prompt for ontologies with source-only, list and symmetric signatures and inverse aliases, and inspect the text.

**Acceptance Scenarios**:

1. **Given** any extraction mode (strict or open), **When** the edge prompt is built, **Then** it contains the direction definition including the worked example ("Sable Core is owned by Lena" is `Lena OWNS Sable Core`).
2. **Given** a relation with only `source_type: Person`, **When** the fact-types section is rendered, **Then** it shows `(Person → any)`.
3. **Given** a relation with list signatures, **When** rendered, **Then** the types in each list are joined with `|`, e.g. `(Person | Team → Project | System)`.
4. **Given** a relation with inverse aliases, **When** rendered, **Then** each is described as the canonical type with source and target swapped; **Given** a symmetric relation, **Then** it is marked as symmetric.

---

### User Story 3 - Ingest corrects or flags endpoints without losing facts (Priority: P1)

At ingest time, once a relation type is resolved and both endpoint entity types are known, the system checks the edge against the relation's signature, and either keeps it, swaps it, or flags it. It never drops it.

**Why this priority**: The extractor will still sometimes get direction wrong; this makes the stored graph correct regardless, and protects loosely typed incidental facts.

**Independent Test**: Unit tests on `MockExtractor` output covering six cases (a–f below).

**Acceptance Scenarios**:

1. **Given** an edge whose endpoints fit the signature, **When** ingested, **Then** it is stored as is.
2. **Given** `Policy APPROVES Person` with `APPROVES` declaring `source_type: Person`, **When** ingested, **Then** it is stored as `Person APPROVES Policy` and the swap is counted in the ingest stats.
3. **Given** an edge extracted as `APPROVED_BY` (an inverse alias of `APPROVES`) from Policy to Person, **When** ingested, **Then** it is stored as `Person APPROVES Policy`.
4. **Given** an edge that fits the signature in neither direction, **When** ingested, **Then** it is stored unchanged and carries a `signature_mismatch` attribute.
5. **Given** a symmetric relation, **When** ingested, **Then** its endpoints are neither checked nor swapped.
6. **Given** a signature naming a parent type and an endpoint whose entity type is a descendant of it, **When** ingested, **Then** the endpoint matches through ancestry.
7. **Given** a relation declaring neither side, **When** ingested, **Then** the check is skipped.

---

### User Story 4 - Existing graphs are repaired without re-ingesting (Priority: P2)

An operator runs the existing relation-canonicalization admin operation over a stored graph to apply the same swap and flag rules, previewing first with a dry run.

**Why this priority**: Existing workspaces already contain reversed edges; re-extraction is costly and out of scope.

**Independent Test**: Run `knowledge_canonicalize_relations` with `dry_run` on a graph containing reversed edges, then a real run, then a second real run.

**Acceptance Scenarios**:

1. **Given** a graph with reversed and mismatched edges, **When** the operation runs with `dry_run`, **Then** it reports per relation type the counts checked, swapped and mismatched, and writes nothing.
2. **Given** the same graph, **When** it runs for real, **Then** the swaps and `signature_mismatch` marks are applied.
3. **Given** a graph already repaired, **When** it runs again, **Then** it reports zero swaps and changes nothing.
4. **Given** the v2 cassette workspace, **When** the repair pass runs, **Then** reversal on known-true links falls from 34% to ≤ 2% (verified by the maintainer using the E13 harness).

---

### User Story 5 - Documentation reflects the new behaviour (Priority: P3)

Users and maintainers can read how direction works.

**Why this priority**: The behaviour is only usable if documented, but it does not block functionality.

**Independent Test**: Review the documentation set.

**Acceptance Scenarios**:

1. **Given** the shipped docs, **When** a reader looks up the ontology reference, **Then** it covers signature lists, `inverse_aliases`, `symmetric` and direction semantics.
2. **Given** the ADR set, **When** read, **Then** the endpoint check and repair pass are recorded (including that the direction definition now applies to open mode, relaxing ADR-0310's byte-identical open-mode constraint for the edge prompt), `docs/llms-full.txt` is regenerated, and the CHANGELOG `Unreleased` section has an entry.

---

### Edge Cases

- An edge endpoint has no known entity type: the check cannot evaluate it and leaves the edge untouched.
- Both orientations fit the signature (e.g. source and target types overlap): the edge is kept as extracted.
- A signature names an undeclared entity type: warn at load; that side is ignored.
- An ontology has an inverse alias that collides with another relation's name or alias: it must be handled consistently with existing alias collision behaviour.
- A relation resolved via an inverse alias or keyword whose canonical type has no signature: the endpoints are still swapped, since the inverse reading is itself the direction information.
- Repair run on an edge already carrying `signature_mismatch` that now fits (e.g. ontology updated): behaviour must be deterministic and idempotent.
- Open mode with an ontology declaring signatures: the check still runs; relations without a signature are skipped.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: An ontology relation's `source_type` and `target_type` MUST each accept either a single entity type name or a list of names.
- **FR-002**: An endpoint entity matches a signature side if its type, or any declared ancestor via `parent`, is in the listed names; an unset side MUST accept any type.
- **FR-003**: An ontology relation MUST accept `inverse_aliases`, a list of names that resolve to the canonical relation type with source and target swapped.
- **FR-004**: An ontology relation MUST accept `symmetric: true`, in which case endpoint order is never checked or swapped.
- **FR-005**: Every existing ontology MUST load unchanged, and the ontology hash for an ontology using none of the new fields MUST be identical to its hash before this change; new fields contribute to the hash only when present.
- **FR-006**: When a signature names an entity type the ontology does not declare, loading MUST log a warning naming the relation and the type, MUST NOT fail, and that side of the signature MUST be ignored by the check.
- **FR-007**: The edge extraction prompt MUST define direction for every extraction mode, stating that `source_name` is the subject of the relation as its type name reads, not the grammatical subject of the sentence, with a worked example.
- **FR-008**: The fact-types section MUST render source-only and target-only signatures (e.g. `(Person → any)`), list signatures joined with `|`, each inverse alias as the canonical type with source and target swapped, and symmetric relations as symmetric.
- **FR-009**: At ingest, in both strict and open modes and whenever the resolved relation declares a signature, after the relation type is resolved and both endpoint entity types are known and before the edge is written, the system MUST apply the endpoint check.
- **FR-010**: An edge resolved through an inverse alias (or an inverse keyword match) MUST have its endpoints swapped before the signature check.
- **FR-011**: If endpoints fit the signature as extracted, the edge MUST be stored as is.
- **FR-012**: If endpoints fit only when reversed, the system MUST swap them and count the swap in the ingest stats.
- **FR-013**: If endpoints fit in neither direction, the edge MUST be stored unchanged and marked with a `signature_mismatch` attribute; it MUST NOT be dropped.
- **FR-014**: Symmetric relation types, and relation types declaring neither side, MUST be skipped by the check.
- **FR-015**: `knowledge_canonicalize_relations` MUST apply the same swap and `signature_mismatch` rules to a stored graph without re-extraction.
- **FR-016**: `knowledge_canonicalize_relations` MUST support `dry_run` (no writes) and report, per relation type, the counts checked, swapped and mismatched.
- **FR-017**: The repair pass MUST be idempotent: a second run on a repaired graph reports zero swaps.
- **FR-018**: The change MUST NOT alter any other ingest behaviour, and the endpoint check MUST NOT drop edges.
- **FR-019**: Documentation MUST be updated: ontology reference (signature lists, `inverse_aliases`, `symmetric`, direction semantics), an ADR (new or updated, noting the open-mode prompt change relative to ADR-0310, numbered per the repo's ADR convention), regenerated `docs/llms-full.txt`, and a CHANGELOG `Unreleased` entry.
- **FR-020**: Prompt snapshot tests MUST be updated deliberately to reflect the new prompt text.

### Key Entities

- **Relation signature**: the declared set of acceptable source entity types and target entity types for a relation type; either side may be unset (any) or a list.
- **Inverse alias**: a name that reads a relation backwards; maps to the canonical relation type with endpoints swapped.
- **Symmetric relation**: a relation type whose endpoint order carries no meaning.
- **`signature_mismatch` mark**: an edge attribute flagging an edge that fits its relation's signature in neither direction.
- **Endpoint check / repair pass**: the ingest-time and offline procedures applying the swap/mark rules.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On the v2 cassette workspace, the repair pass reduces reversed known-true links from 34% (48/142) to ≤ 2% (verified by the maintainer using the E13 harness and posted on the issue).
- **SC-002**: Every existing ontology in the repository and tests loads with an identical hash before and after this change.
- **SC-003**: All six endpoint-check scenarios (kept, swapped, inverse alias, neither-way marked, symmetric untouched, ancestry match) pass as automated tests.
- **SC-004**: A second repair run on a repaired graph reports zero swaps; a dry run changes nothing in the graph.
- **SC-005**: No edge count decreases as a result of the endpoint check, at ingest or repair.
- **SC-006**: The edge prompt in every mode contains the direction definition, and source-only and list signatures render as specified.

## Assumptions

- Entity types are those already assigned to nodes during extraction; an endpoint with no type is treated as unevaluable and the edge is left untouched.
- When both orientations fit a signature, the extracted orientation is kept (no swap).
- "Inverse keyword" means a keyword match that resolves to an inverse alias (e.g. `approv` matching `APPROVED_BY`); no separate inverse-keyword field is introduced beyond `inverse_aliases`.
- The `signature_mismatch` attribute is stored in the same edge attribute mechanism used for other edge attributes, and is visible to queries.
- The repair pass reuses the existing `knowledge_canonicalize_relations` method and its existing request/response conventions, extending the reported counts; the method's scope bucket is unchanged.
- The ADR is numbered by the GitHub issue number (0816) per repo convention unless the existing ADR for the open-mode constraint is simply updated.
- The final eval (SC-001) is run by the maintainer, not by the pipeline.

## Out of Scope *(optional)*

- Re-extracting corpora; the repair pass covers existing graphs and fresh extractions get the prompt fix.
- The typed-walk / planner tool (#815, separate spec).
- Note-ID junk entities (e.g. `project-quill-core-1`) created under ontology extraction; separate issue if wanted.
- Inferring signatures automatically from data.

## Source References *(optional)*

- #815 — E13 retrieval experiments (`docs/retrieval-experiments.md`) and the scratch harness / fixed test ontology (`ontology-v3.2.yaml`).
- ADR-0032 (hash approach for `parent`), ADR-0310 (open-mode byte-identical constraint on the type section).
