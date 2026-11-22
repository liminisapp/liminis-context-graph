# ADR-0851: Test-Providers Mode — Built-in Deterministic Embedder and Fact-Line Extractor

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #851 (motivated by shadoworg/liminis-notes#1359)

## Context

Consumers that run lcg in their own tests must stand in for its two external collaborators, the
embedding sidecar and the LLM extractor. Liminis Notes had three copies of those fakes, each
reverse-engineering lcg: the extractor fakes recognised requests by phrases in lcg's system
prompts (so a prompt rewording silently broke consumer tests), and one embedder fake returned the
same vector for every text, so embedding-based entity resolution merged unrelated entities.

lcg's own `HashEmbedder` and `MockExtractor` are not suitable: the former is unrelated noise even
for texts that share words (and seeds from `DefaultHasher`, which is not stable across Rust
releases), the latter returns fixed output regardless of input. Neither was reachable from the
shipped binary anyway.

## Decision

1. **Explicit flags only.** `--embedder test` and `--extractor test` (value `test` only) select
   built-in providers. They are never inferred from an environment variable, a config file, a
   default or a fallback. They are independent, rejected with the matching `-uds`/`-http` flags,
   with `--connect`, and (for the extractor) with `LCG_RECORD_LLM`/`LCG_REPLAY_LLM`, because the
   cassette seam wraps or replaces a *real* provider and silently picking one would hide a
   misconfiguration.
2. **Implementations live in `lcg-core`** (`test_providers.rs`), behind the existing `Embedder`
   and `Extractor` traits; the service only selects them. The engine is unchanged, and `lcg-core`
   integration tests and the eval crate can use them too. The extractor implements the trait
   directly and never touches a prompt.
3. **Embedder: signed hashed bag of words plus character trigrams, L2-normalised.** Word features
   weigh 2, trigrams of `^token$` weigh 1; 64-bit FNV-1a, `h % dim` bucket, bit 63 as sign; exact
   integer accumulation, then one `sqrt` and a correctly-rounded divide, so vectors are bit-identical
   on every platform. Tokenisation and case folding are ASCII-only (only ASCII characters separate tokens; non-ASCII
   characters are token characters), so no Unicode-table lookup can change a vector across Rust releases.
   Trigrams are load-bearing: with words alone, two unrelated names collide in a 768-dimension space
   with probability 1/768 and would then score cosine 1.0, crossing the engine's 0.85 merge
   threshold, which is the #1359 failure class. With trigrams a single colliding word scores about
   0.4. Blank text maps to a fixed sentinel feature, giving a non-zero unit vector that cannot be
   mistaken for the zero-vector failed-embed record (ADR-0808).
4. **Default dimension 768, frozen.** The dimension is fixed per database, so changing it later
   would orphan existing test databases. `LCG_EMBEDDING_DIM` overrides it. The model identity is
   `lcg-test-bow`, so a database shared with a real embedder reports an `embedding_model_status`
   mismatch.
5. **Fact-line grammar is the contract**, documented in `docs/test-providers.md` and executed by
   `crates/core/tests/test_providers_contract.rs`. Relation names reuse `normalize_relation_type`
   so they match what a real extractor emits. Entities are typed `Entity`, which strict ontology
   mode leaves unchanged. The other `Extractor` operations are deterministic: classification
   abstains, consolidation concatenates, duplicate judging matches only equal normalised names.
6. **Mode is reported by a defaulted trait method**, `is_test_provider()` on both traits, surfaced
   as an additive `providers` object in `knowledge_status` and `health_check` and a startup log
   line. Alternatives were a process-global (shared across in-process tests) and an `AppState`
   field (about 54 literal construction sites). Wrappers (`InstructedEmbedder`,
   `CountingEmbedder`, `RecordingExtractor`, `LlmRouter`) delegate. `--version` is unchanged.
7. **The reranker and expander stay unconfigured** under `--embedder test` (there is no sidecar to
   default them to), and report `not_configured`.

## Consequences

- Consumers launch the real engine with two flags and delete their stub servers.
- The contract documents the engine's own behaviour rather than the spec's assumptions: entity
  resolution is case-insensitive by name; `single_valued_relations` is a `knowledge_lint` request
  parameter, not an ontology declaration; self-edges are emitted by the extractor but dropped at
  ingest; the engine keeps one edge record per document for a repeated fact.
- Changing the tokeniser, hash, weights or default dimension changes published vectors; the golden
  vector test pins them and the change would be breaking.
