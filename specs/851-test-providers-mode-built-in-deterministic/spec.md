# Feature Specification: Test-providers mode: built-in deterministic embedder and fact-line extractor

**Feature Branch**: `fabrik/issue-851`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "## Problem

Every consumer that runs lcg in its own tests has to fake lcg's embedder and extractor collaborators. Liminis Notes alone has had three copies: `e2e/graph-helpers.ts`, `tests/integration/real-engine.ts`, and a since-deleted Notebook Health harness. Each copy reverse-engineers lcg internals:

- The extractor fakes recognise lcg's requests by matching phrases in lcg's system prompts (`'entity extraction'`, `'{\"entities\"'`, `'{\"edges\"'`). Any prompt rewording in lcg silently breaks consumer tests.
- The embedder fakes get the subtle parts wrong. In one, every single text got the same vector, so lcg's embedding-based entity resolution merged unrelated entities across tests and three tests failed for reasons unrelated to what they checked (shadoworg/liminis-notes#1359).

lcg already has deterministic fakes for its own tests: `HashEmbedder` (`crates/core/src/embedder.rs`) and `MockExtractor` (`crates/core/src/extractor.rs`). They aren't reachable from the shipped binary.

## Proposal

Add a **test-providers mode** to the shipped service binary, so consumers start the real engine with no external embedder or extractor and lcg owns the fakes:

- `--embedder test` (name to taste): deterministic and offline. For consumer search tests to be meaningful, texts that share words should be closer than unrelated texts, for example a hashed bag of words that is L2-normalised. Plain `HashEmbedder` noise makes every query an arbitrary match. Dimension follows the configured embedding dimension.
- `--extractor test`: deterministic and offline. Extracts facts from **fact lines** in the chunk text:
  - `SRC -> DST: fact text` (relation `RELATED_TO`);
  - `SRC -[REL_NAME]-> DST: fact text` (a named relation, so a test can exercise single-valued relations such as `HOSTED_ON`).

  Entities are the names in those lines. Text with no fact lines extracts nothing. Because it implements the `Extractor` trait directly, it never goes near the prompts.
- **Startup announces the mode** in `--version`, or in `health_check` / `knowledge_status`, so a test can confirm it isn't hitting real providers. The flags must be impossible to enable by accident in production: explicit flags only, never inferred from the environment.
- **Document the fact-line syntax** as a stable contract for consumers, with its own tests in lcg.

## Acceptance

- `liminis-context-graph --embedder test --extractor test` starts, ingests fact-line chunks, and returns the expected entities and edges from search and lint, all without network access.
- Identical inputs give identical results across runs and platforms.
- In the same engine, two documents asserting the same fact resolve to the same entities. This is the case that broke in shadoworg/liminis-notes#1359.

## Consumer follow-up

Once released, Liminis Notes deletes its stub embedder and extractor servers and the `LIMINIS_E2E_EXTERNAL_EMBEDDER` switch in shipped code, and launches the engine with these flags."

## Background

Consumers that run lcg in their own tests (Liminis Notes has had three separate harnesses) must stand in for the two external collaborators lcg normally talks to: the embedding sidecar and the LLM extractor. Today each consumer writes its own fakes. They are fragile in two ways:

1. **Prompt coupling.** Extractor fakes identify lcg's requests by phrases in lcg's own system prompts. Rewording a prompt inside lcg silently breaks every consumer's tests, with no signal in lcg's own CI.
2. **Embedder quality.** Fake embedders are easy to get subtly wrong. One returned the same vector for every text, so lcg's embedding-based entity resolution merged unrelated entities and three tests failed for reasons unrelated to what they were checking (shadoworg/liminis-notes#1359).

lcg already owns correct deterministic fakes for its internal tests (a hash-based embedder and a fixed-output mock extractor), but they are not reachable from the shipped binary, and neither is suitable as-is for consumers: the hash embedder produces unrelated vectors even for texts that share words (so search results are arbitrary), and the mock extractor returns the same fixed entities regardless of input.

The goal is a **test-providers mode**, opted into by explicit command-line flags, where the shipped binary runs the real engine against lcg-owned deterministic embedding and extraction. lcg then owns the contract, and consumers stop reverse-engineering internals.

Existing context: the service currently selects its embedder via `--embedder-uds` / `--embedder-http` (or `LCG_EMBEDDING_URL`) and its extractor via `--extractor-uds` / `--extractor-http` or provider env configuration; the embedding dimension is configurable (`LCG_EMBEDDING_DIM`). The new flags are additive and must fit alongside these.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Start the real engine with lcg-owned fake providers (Priority: P1)

A consumer's test harness launches `liminis-context-graph --embedder test --extractor test` (socket mode, or `--mcp-stdio`) with no sidecar, no API key and no network. The engine starts healthy, accepts ingestion, and serves search and lint.

**Why this priority**: This is the core deliverable. Without it nothing else is usable.

**Independent Test**: Start the binary with both flags in a sandbox with networking disabled and no embedder/extractor env vars; call `knowledge_status` and observe a healthy engine.

**Acceptance Scenarios**:

1. **Given** no embedding sidecar and no extractor credentials, **When** the service is started with `--embedder test --extractor test`, **Then** it starts, reports healthy, and makes no network connections.
2. **Given** the same flags with `--mcp-stdio`, **When** a client connects, **Then** the same behaviour holds.
3. **Given** only `--embedder test` (extractor unspecified), **When** the service starts, **Then** it uses the built-in embedder and resolves the extractor exactly as it does today (the two flags are independent).

---

### User Story 2 - Ingest fact lines and get predictable entities and edges (Priority: P1)

A test ingests chunks containing fact lines such as `Alice -> Acme: Alice works at Acme` or `Wiki -[HOSTED_ON]-> ServerA: Wiki runs on ServerA` and then asserts on the entities and edges returned by search and lint.

**Why this priority**: Predictable extraction is the second half of the core value; it replaces prompt-matching fakes.

**Independent Test**: Ingest a chunk with one plain and one named-relation fact line; verify the two entities per line, the edge facts, and the relation types (`RELATED_TO` and `HOSTED_ON`) via search/get-edges calls.

**Acceptance Scenarios**:

1. **Given** a chunk containing `SRC -> DST: fact text`, **When** it is ingested, **Then** entities `SRC` and `DST` exist and one edge `SRC`→`DST` with fact `fact text` and relation `RELATED_TO` exists.
2. **Given** a chunk containing `SRC -[REL_NAME]-> DST: fact text`, **When** it is ingested, **Then** the edge carries relation `REL_NAME`.
3. **Given** a chunk with no fact lines (prose, headings, blank lines), **When** it is ingested, **Then** no entities and no edges are extracted and ingestion still succeeds.
4. **Given** a chunk mixing prose and several fact lines, **When** it is ingested, **Then** only the fact lines contribute, each producing its own edge.
5. **Given** a named relation declared single-valued in the ontology (e.g. `HOSTED_ON`), **When** two documents assert different destinations for the same source, **Then** the engine's single-valued-relation handling is exercised exactly as it would be with a real extractor.

---

### User Story 3 - Same fact in two documents resolves to the same entities (Priority: P1)

Two separate documents assert the same fact (same names). In the same engine they must resolve to the same entities rather than producing duplicates, and unrelated entities must never be merged. This is the failure mode of shadoworg/liminis-notes#1359.

**Why this priority**: It is an explicit acceptance criterion and the motivating bug.

**Independent Test**: Ingest document A and document B each containing `Alice -> Acme: Alice works at Acme`, then ingest a third with unrelated entities; verify entity counts (Alice and Acme appear once each, unrelated entities remain distinct).

**Acceptance Scenarios**:

1. **Given** two documents asserting an identical fact line, **When** both are ingested into one engine, **Then** exactly one entity exists per name and the edge is not duplicated into conflicting copies.
2. **Given** entities with unrelated names (no shared words), **When** they are ingested, **Then** the embedding-based resolution never merges them.

---

### User Story 4 - Meaningful, deterministic search (Priority: P2)

Consumer search tests need results that reflect textual similarity: a query sharing words with an entity or fact must rank it above unrelated items, and results must be identical on every run and platform.

**Why this priority**: Needed for consumer search tests to be meaningful, but ingestion (P1) works without it.

**Independent Test**: Embed three texts (two sharing words, one unrelated) and compare distances; embed the same text on two runs/platforms and compare bytes.

**Acceptance Scenarios**:

1. **Given** texts A and B sharing words and text C sharing none, **When** embedded, **Then** A is closer to B than to C.
2. **Given** the same text, **When** embedded in different processes, runs or platforms, **Then** the vector is bit-for-bit identical.
3. **Given** any text, **When** embedded, **Then** the vector has the configured embedding dimension and is unit length (L2-normalised); distinct texts do not all share one vector.
4. **Given** empty or whitespace-only text, **When** embedded, **Then** a valid, deterministic vector of the correct dimension is returned and no error occurs.

---

### User Story 5 - Confirm the mode and prevent accidental production use (Priority: P2)

A test can assert it is talking to fake providers, and operators can be confident a production deployment cannot end up in this mode by accident.

**Why this priority**: Safety and verifiability; does not block functionality.

**Independent Test**: Start with and without the flags and compare the status output; start with test-looking environment variables only and confirm real-provider resolution is unchanged.

**Acceptance Scenarios**:

1. **Given** the service started with the test flags, **When** `knowledge_status` (and `health_check`) is called, **Then** the response states which providers are in test mode.
2. **Given** the service started without the flags, **When** the same calls are made, **Then** the response shows real (non-test) providers.
3. **Given** any environment variable set but no flag, **When** the service starts, **Then** test providers are never selected.
4. **Given** the service started in test mode, **When** it starts, **Then** a prominent startup log line says test providers are active.

---

### User Story 6 - Documented, tested fact-line contract (Priority: P2)

A consumer author can read one stable document describing the fact-line syntax and rely on it across lcg releases.

**Why this priority**: The contract is what lets consumers delete their fakes safely.

**Independent Test**: The documented examples are executed as tests in lcg and must pass; changing parser behaviour breaks those tests.

**Acceptance Scenarios**:

1. **Given** the documentation, **When** each documented example line is fed to the extractor, **Then** the documented entities and edges result.
2. **Given** a change to the parser that alters documented behaviour, **When** lcg's tests run, **Then** they fail.

---

### Edge Cases

- A fact line with extra whitespace around names, arrow or colon: trimmed consistently.
- A fact line with an empty source, destination or fact text (e.g. `-> B: x`, `A -> : x`, `A -> B:`): ignored, not an error.
- Fact text that itself contains colons or arrows: the first `:` after the arrow separates the fact text; the rest belongs to the fact.
- Relation names: case, characters allowed, and an empty `-[]->` — defined in the contract (normalised/rejected deterministically).
- Windows (`\r\n`) line endings and fact lines indented or inside Markdown list items/blockquotes: behaviour defined in the contract.
- The same fact line repeated within one chunk: deduplicated deterministically.
- Self-edges (`A -> A: ...`): defined in the contract.
- Names with Unicode or mixed case: embedding and entity resolution are deterministic across platforms; name matching rules (case-sensitive or not) are stated in the contract.
- Very long chunks or very many fact lines: processed without error or non-determinism.
- Conflicting flags: `--embedder test` together with `--embedder-uds` / `--embedder-http` (and `--extractor test` with `--extractor-uds` / `--extractor-http`) is rejected at startup with a clear error, consistent with existing mutual-exclusion behaviour.
- Unknown value for `--embedder` / `--extractor` (anything but `test`): rejected with a clear error.
- A database previously populated by real providers is opened in test mode (or vice versa): the existing embedding-model/dimension mismatch protection applies, so test vectors are never silently mixed with real ones.
- Extractor-backed features beyond `extract` (entity-type classification, relation-type classification, summary consolidation, duplicate judging) must behave deterministically and offline in test mode.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The service binary MUST accept `--embedder test`, selecting a built-in deterministic, offline embedder that needs no sidecar, network or credentials, in both socket mode and `--mcp-stdio` standalone mode.
- **FR-002**: The built-in embedder MUST be deterministic: the same text yields a bit-identical vector across runs, processes and platforms.
- **FR-003**: The built-in embedder MUST produce vectors of the configured embedding dimension and L2-normalise them.
- **FR-004**: The built-in embedder MUST be similarity-preserving: texts sharing words are closer (by the engine's vector distance) than texts sharing none, and distinct texts MUST NOT collapse to the same vector. A hashed bag-of-words is the intended model, not unrelated pseudo-random noise.
- **FR-005**: The built-in embedder MUST return a valid, deterministic vector for empty or whitespace-only text without error.
- **FR-006**: The service binary MUST accept `--extractor test`, selecting a built-in deterministic, offline extractor implemented directly against the extractor interface (no LLM prompts, no HTTP), in both socket and `--mcp-stdio` standalone modes.
- **FR-007**: The test extractor MUST recognise fact lines of the form `SRC -> DST: fact text` and produce entities `SRC` and `DST` and an edge `SRC`→`DST` with the given fact text and relation type `RELATED_TO`.
- **FR-008**: The test extractor MUST recognise fact lines of the form `SRC -[REL_NAME]-> DST: fact text` and produce the same entities and an edge carrying relation type `REL_NAME`.
- **FR-009**: Text containing no fact lines MUST extract no entities and no edges, and MUST NOT cause ingestion to fail.
- **FR-010**: Entities MUST be exactly the names appearing in recognised fact lines; each distinct name yields one entity per extraction, regardless of how many lines mention it.
- **FR-011**: Extraction output MUST be deterministic for identical input (same entities, edges and ordering), independent of platform.
- **FR-012**: Malformed or partial fact lines (empty source, destination, relation or fact text) MUST be ignored without error. The exact syntax rules (whitespace, relation-name characters, line endings, indentation/Markdown prefixes, repeated lines, self-edges) MUST be defined in the documented contract.
- **FR-013**: The other extractor operations the engine invokes (entity-type classification, relation-type classification, summary consolidation, duplicate judging) MUST have deterministic, offline implementations in test mode that never cause two different-named, unrelated entities to be merged, and that do not require any LLM or network access.
- **FR-014**: In the same engine, two documents asserting the same fact line MUST resolve to the same entities (no duplicates), while unrelated entities MUST remain distinct.
- **FR-015**: Test-provider mode MUST be enabled only by the explicit `--embedder test` / `--extractor test` flags. It MUST never be selected from environment variables, config files, defaults or fallbacks (for example when a real provider is unreachable).
- **FR-016**: `--embedder` and `--extractor` MUST be independently selectable (one test provider with one real provider is allowed), MUST be rejected when combined with the corresponding `--embedder-uds`/`--embedder-http` or `--extractor-uds`/`--extractor-http` flags, and MUST reject any value other than `test` with a clear error.
- **FR-017**: When the embedding dimension is configured (e.g. via the existing dimension setting), the test embedder MUST use it; when none is configured it MUST use a documented default so the flag works with no other configuration.
- **FR-018**: The active provider mode (test vs. real, per provider) MUST be observable at runtime through `knowledge_status` and `health_check` (additive fields), so a test can assert it is not hitting real providers. The same information MUST appear in a startup log line, and the flags MUST be listed in `--help`.
- **FR-019**: With test providers active, the service MUST make no outbound network connections for embedding or extraction.
- **FR-020**: The fact-line syntax and test-embedder guarantees MUST be documented as a stable consumer-facing contract, and lcg MUST include tests that execute the documented examples and cover the edge cases above, so contract changes fail lcg's own CI.
- **FR-021**: Existing behaviour without the new flags (provider resolution, error messages, status output other than the additive fields, IPC protocol) MUST be unchanged.

### Key Entities *(if applicable)*

- **Test embedder**: Deterministic, offline embedding provider producing unit-length, word-overlap-preserving vectors of the configured dimension.
- **Test extractor**: Deterministic, offline extraction provider that derives entities and edges solely from fact lines in the text.
- **Fact line**: A line of the form `SRC -> DST: fact text` or `SRC -[REL_NAME]-> DST: fact text`; the stable contract between lcg and consumers.
- **Provider mode indicator**: The additive status field announcing whether each provider is a test provider.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: `liminis-context-graph --embedder test --extractor test` starts and reports healthy on a machine with no network access and no provider configuration, in both socket and `--mcp-stdio` modes.
- **SC-002**: Ingesting chunks with N distinct valid fact lines yields exactly the expected entities and N edges (with the expected relation types) from search and lint, and a chunk with zero fact lines yields zero of each.
- **SC-003**: Repeating the same ingestion in fresh engines (and on each CI platform) produces identical entities, edges and embedding vectors.
- **SC-004**: Two documents asserting the same fact in one engine produce one entity per name (zero duplicates), and a set of unrelated entities produces zero merges.
- **SC-005**: For word-overlapping vs. unrelated text pairs, the overlapping pair is closer in 100% of the tested cases; all vectors have unit L2 norm within floating-point tolerance and the configured dimension.
- **SC-006**: A test can detect test mode from `knowledge_status` / `health_check` in a single call, and a service started without the flags never reports test mode, including when test-related environment variables are present.
- **SC-007**: Every example in the published fact-line documentation is covered by an automated test that fails if the documented behaviour changes.
- **SC-008**: A consumer (Liminis Notes) can replace its stub embedder/extractor servers and the `LIMINIS_E2E_EXTERNAL_EMBEDDER` switch with these two flags without changing its test assertions' intent.

## Assumptions

- The flag form is `--embedder test` / `--extractor test` (value-taking flags, new names alongside the existing `--embedder-uds/-http` and `--extractor-uds/-http`); "test" is the only accepted value for now.
- Mode is announced through `knowledge_status`, `health_check` (additive fields) and a startup log line. `--version` output is left unchanged, since it prints before any provider is chosen.
- Entity type for extracted entities is a single fixed default (no inference), and entity summaries are derived deterministically from the fact text; ontology-aware type classification, when asked, uses a deterministic rule rather than a model.
- Entity names are matched case-sensitively after trimming, so exact-name equality is what drives "same fact, same entities"; the test embedder's word-overlap similarity is what drives embedding-based resolution. If tokenisation for embedding is case-folded, the contract states so.
- Duplicate judging in test mode is conservative (only identical normalised names are considered duplicates), and summary consolidation is a deterministic combination of existing and incoming text.
- Default embedding dimension when none is configured is a fixed documented value chosen by the implementer, and does not need to match any real provider.
- The mode is intended for tests, not production; no production safeguards beyond "explicit flags only, never inferred" and visible status are required.
- Existing embedding-model/dimension mismatch protections for a database already populated by another provider are sufficient.
- Dependent Liminis Notes changes are out of scope here and happen after a release.

## Out of Scope *(optional)*

- Changes in Liminis Notes (deleting its stub servers and the `LIMINIS_E2E_EXTERNAL_EMBEDDER` switch); that is a follow-up once released.
- Test providers for other roles (reranker, query expander) or for any extractor behaviour beyond fact lines (e.g. prose-based or LLM-like extraction).
- Environment-variable or config-file activation of test mode.
- A cassette/record-replay mechanism for real providers (a separate existing concern).
- Semantic (model-quality) embeddings; only word-overlap similarity is required.
- Changing the IPC protocol beyond additive status fields.

## Source References *(optional)*

- shadoworg/liminis-notes#1359 — merged-entities failure caused by a constant-vector fake embedder
- `crates/core/src/embedder.rs` (`HashEmbedder`), `crates/core/src/extractor.rs` (`MockExtractor`, `Extractor` trait), `crates/service/src/cli.rs` (existing provider flags)
