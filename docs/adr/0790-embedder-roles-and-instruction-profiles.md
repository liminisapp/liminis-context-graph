# ADR-0790: Embedder Roles and Instruction Profiles

**Status**: Accepted
**Date**: 2026-11-18
**Issue**: #790 (extends ADR-0006/ADR-0016's embedder contract; builds on ADR-0440/ADR-0486's embedding identity and recompute)

## Context

LCG embedded a search query and a stored chunk the same way: `Embedder::embed(text)`, the shape of the
OpenAI `/v1/embeddings` contract whose own models need no role. The open models LCG actually runs are
**asymmetric** and were trained with role prefixes — BGE-base-en-v1.5 (the bundled sidecar, i.e. every
macOS Notes user) wants a query instruction and no document prefix; nomic-embed-text v1.5 wants
`search_query: ` / `search_document: `; e5 wants `query: ` / `passage: `. Nothing added them, so every
vector search ran these models outside how they were trained.

The decision has two halves that pull in different directions: stating *which side* a text is on is
cheap and universal, but *applying* an instruction changes vectors, and a measurement taken during this
issue (BGE's query instruction on the v1 Fernhill fixture: no net passage gain, a small entity/fact
gain, all within noise at 43 questions) argued against turning anything on by default.

## Decision

### 1. Every embed call states a role (no defaulted path)

`Embedder::embed(text, role)` / `embed_batch(texts, role)` take an `EmbedRole` — `Query` (a user/agent
search string) or `Document` (stored text). Rejected: keeping `embed(text)` and adding
`embed_with_role`, which leaves an implicit-role path that the next call site can silently use. The
fan-out (~15 implementors, 60+ call sites) is mechanical and compiler-guided. Call sites:

- **Query**: `search_passages`; entity search (one embedding serves the name-vector and
  summary-vector halves); relationship search.
- **Document**: episode content, entity names and summaries, edge facts, off-list endpoint names,
  merged-summary re-embeds, direct entity/edge adds, summary backfill, and all WAL replay/recompute.
- **Symmetric comparisons** — entity resolution/dedup (which reuses the Document vectors computed at
  ingest) and ontology canonicalization (type gloss vs relationship fact) — are `Document` on both
  sides, as the issue directs.

### 2. Profiles are a decorator; one applier per process, chosen once

`InstructionProfile` is `none` (default), `bge-v1.5`, `nomic-v1.5`, `e5`, or `embedder`; the query
and document prefixes can be overridden independently (`LCG_EMBEDDING_PROFILE`,
`LCG_EMBEDDING_QUERY_PREFIX`, `LCG_EMBEDDING_DOCUMENT_PREFIX`, env-only with no legacy `GRAPHITI_*` aliases, since they are new; an explicit
empty string disables a side). Prefixing lives in an `InstructedEmbedder` decorator over any embedder,
not inside `OaiEmbedder`, so transport stays transport and every embedder (including test doubles)
benefits. Whitespace-only text passes through unprefixed so a prefix can never turn a skipped empty
text into a real request (FR-014).

`build_embedder_stack` resolves **who applies the instruction** exactly once at startup
(`InstructionSource::{None, Lcg, Embedder}`): `Lcg` installs the decorator and never sends
`input_type`; `Embedder` sends `input_type` and installs no decorator. Because only one applier is
ever active, **a text can never receive two instructions** — a property of construction, with a test
that runs a role-advertising stub plus a configured profile (SC-009), not a runtime check.

### 3. Sidecar: advertisement is capability-only; using it is opt-in (amends SC-008 / Story 7)

The bundled sidecar stays an OpenAI-compatible embedder with two additive, optional extras: a request
`input_type` (`query`|`document`), and, in the `/v1/embeddings` response, its **real model id** and an
`instruction_roles` object naming the instruction it would apply per role. The advertisement rides on
the embeddings response (read by the startup probe) because that works identically over HTTP and UDS
with no extra round trip; a `/v1/models` route was skipped as non-load-bearing.

The original spec had LCG send `input_type` whenever the embedder advertised support, switching BGE's
query instruction on by default for every macOS user. The measurement above does not justify that, so
**this ADR amends SC-008 and User Story 7 scenarios 1–2**: LCG sends `input_type` only under
`LCG_EMBEDDING_PROFILE=embedder`, and only if the probe saw an advertisement (otherwise it warns and
falls back to `none`). With the default `none`, an advertising sidecar still receives exactly
`{input, model}`, so the advertisement changes no vectors. `embedder` combined with an explicit prefix
override is a startup error (ambiguous). This also satisfies FR-023: `input_type` is never sent to a
server that did not advertise support (real OpenAI rejects unknown parameters).

**Model id (FR-025).** The sidecar's reported id becomes `coreml-<model file stem>`; the default
`bge-base-en-v1.5.mlpackage` reports `coreml-bge-base-en-v1.5` byte for byte, so existing workspaces —
whose recorded identity is that string — are not flagged for a re-embed. `LOCAL_INFERENCE_EMBEDDING_MODEL_ID`
pins it if the file is renamed. Instruction lookup is a pure function of the id (BGE `en-v1.5` family →
query instruction only); an unknown model advertises nothing.

### 3a. Effective instructions travel with the embedder, not `AppState`

`Embedder::instructions()` returns the effective profile/source/prefixes (default: `none`; the
decorator and an opted-in `OaiEmbedder` override it; wrappers delegate). This was chosen over adding
fields to `AppState`, which has ~90 struct-literal constructors across tests: the identity, status
and stamp code read `state.embedder.instructions()`, so the configuration cannot drift from the
embedder that is actually running.

### 4. The document prefix is embedding identity; the query prefix is not

`WalPosition` gains `embedding_doc_prefix STRING` (migrated additively); `EmbeddingIdentity { model,
dim, doc_prefix }` replaces the `(model, dim)` tuple. NULL (every pre-feature row), absent and `""`
all compare as the empty prefix, so upgrading under `none` is a `match` and `none` → `bge-v1.5` (no
document prefix) is free. Under `embedder`, the recorded value is the *advertised* document
instruction, so a document-side change is detected whoever applies it (FR-024). A document-prefix
difference is a `mismatch` handled exactly like a model change: ADR-0440's "warn and self-heal" —
`knowledge_rebuild_from_wal` / `rebuild_from_workspace_wal` recompute every vector as `Document` under
the running prefix and record it.

**Live writes must not paper over it.** ADR-0440 accepts that ordinary writes re-stamp the running
`(model, dim)` as a best-effort marker. For the document prefix that would let the first write after
`none` → `nomic-v1.5` flip the status to `match` while old- and new-prefix vectors are mixed — exactly
what SC-004 forbids. So `set_wal_position_live` (used by `advance_wal_position` and the `add_episode`
stamp) reads the row first and preserves a *recorded, different* prefix; only rebuild/replay, which
recompute everything and call `set_wal_position` directly, claim the new prefix. Model name and
dimension restamp behaviour is unchanged. A row with applied content (`applied_seq > 0`) but no
recorded identity (pre-#440, or cleared by a replay with a failed recompute) holds vectors of unknown
provenance that can only have been produced unprefixed, so a live write records the empty prefix for
it rather than claiming a non-empty running one. Cost: a group that is never rebuilt after a document-prefix
change stays `mismatch` — intended, and documented.

### 5. The embedding cache keys on role and effective prefix

`sha256(model \0 dim \0 role \0 prefix \0 text)`. Production uses the cache only for Document-role WAL
replay, but the key must be correct for any use (FR-012): the same text as `Query` and `Document`
never shares a vector, and two contexts differing only in prefix never collide. Keys are in-memory
only, so there is no migration.

### 6. Status surface

`knowledge_status` gains `embedding_instructions { profile, source, query_prefix, document_prefix }`
and the recorded `embedding_doc_prefix` on `wal` and per group in `wal_groups`. All additive.

### 7. Eval

`lcg-eval retrieval --embedding-profile none,bge-v1.5` runs each LCG configuration once per profile
(`<id>@<profile>` rows), each profile in a fresh workspace with a fresh ingest — a document-prefix
change means a re-embed, so vectors are never reused across profiles.

## Deferred

- **Per-read-path query behaviour.** The maintainer's measurement suggests the BGE instruction may
  help entity/fact search more than passage search. This ADR keeps one `Query` role and one query
  prefix: the v1 data is within noise at 43 questions, and because a query-side change never touches
  stored vectors, per-path control can be added later with no re-embed. Revisit with the #788 (v2
  corpus) measurement.
- **The acceptance measurements.** The harness support ships here; the sidecar `none` vs `bge-v1.5`
  and Ollama `none` vs `nomic-v1.5` runs need a real embedder, an extraction backend and a judge, are
  run on demand, and are pending #788 (v2 corpus, confidence intervals).

## Consequences

- With profile `none` (the default) every string sent to an embedder is byte-identical to before.
- Every embed call site is explicit about its role; a new one cannot omit it.
- nomic and e5 users get correct prefixes on both sides at the price of a one-time rebuild; BGE users
  can opt into the query instruction at no cost.
- The sidecar's reported model id is now truthful, and a swapped model can no longer be mistaken for
  BGE.
- A document-prefix mismatch is sticky until rebuilt (by design).
