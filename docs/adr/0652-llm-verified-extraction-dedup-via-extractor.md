# ADR-0652: LLM-Verified Extraction Dedup via the Configured Extractor

**Status**: Accepted
**Date**: 2026-10-01
**Issue**: #652 (builds on [ADR-0650](0650-identifier-mismatch-veto-for-extraction-dedup.md))

## Context

Phase B entity resolution ([ADR-0029](0029-name-first-entity-resolution.md)) merges an extracted
entity into the best name-embedding candidate when cosine ≥ `DEDUP_THRESHOLD` and the dedup
adapter confirms. The default adapter always confirmed. The only alternative,
`LocalDedupAdapter` (`LCG_DEDUP_LLM` + `LCG_DEDUP_ADAPTER_URL`), POSTed a bespoke
`{candidate, incoming}` body to an endpoint that nothing in lcg, its sidecars or its docs
implements, so enabling it made every embedding-path check fail — and, because the adapter call
was `?`-propagated, abort the whole chunk. No deployment could get an LLM-verified dedup.

ADR-0650's deterministic veto catches number/identifier differences; semantic false merges (two
different people with similar names) need a real judgement. A wrong merge cannot be undone
without re-ingestion; a missed merge is recoverable.

## Decision

An **`ExtractorDedupAdapter`** asks the extractor lcg already uses — Anthropic, or the
OpenAI-compatible UDS/HTTP endpoint, including the CoreML sidecar's `/v1/chat/completions` — to
judge candidate pairs. No new server, protocol or deployment piece. `LocalDedupAdapter` and the
`LCG_DEDUP_ADAPTER_URL` protocol are deleted (the variable, and its `GRAPHITI_*` alias, now only
trigger a startup deprecation warning).

### Extractor surface: a default trait method

`Extractor::judge_duplicates(&[DuplicatePair]) -> Result<Vec<DedupVerdict>>` has a **default**
body returning an "unsupported" error, plus `Extractor::is_configured()` (default `true`,
`UnconfiguredExtractor` → `false`). Anthropic, OpenAI-compatible, `LlmRouter`, `RecordingExtractor`
and `ReplayingExtractor` override it; the ~20 other impls (tests, mocks) are untouched. A required
method would have forced edits in all of them; a separate `DedupJudge` trait would have duplicated
provider wiring and lost router fallback and cassette support. `classify_entities` is the
precedent. `LlmRouter` falls back primary → fallback for this call but, unlike `extract` and
`classify_*`, **never latches `primary_failed`**: dedup is advisory, and a bad judge call must not
permanently reroute extraction.

### Strict, id-attributed verdicts (`dedup_judge.rs`)

The system prompt carries the instruction; pairs travel as a JSON array of `{id, a, b}` data
(name, type, summary), so a hostile summary is data, not an instruction. The reply must be
`{"verdicts":[{"id":<int>,"duplicate":<bool>}]}`. `parse_verdicts` accepts a verdict only for an
in-range integer `id` that appears **exactly once** with a JSON **boolean** `duplicate`; a missing,
duplicated (both entries discarded), out-of-range or non-boolean entry, and any unparseable reply,
is `Unknown`. It deliberately does **not** copy `classify_entities`' pad-and-resize behaviour: a
short or reordered positional answer would attribute a verdict to the wrong candidate, which for
dedup is a wrong merge.

### Everything but cancellation resolves to Insert

`DedupAdapter::judge_batch` never returns an error. `ExtractorDedupAdapter` splits pairs into
groups of ≤ 16, wraps each extractor call in a 30 s `tokio::time::timeout` (the clients have no
request timeout; the value is a constant, not a new env var), and maps an error, timeout, wrong
length or cassette miss to `Unknown` plus a warning. Phase B treats `Unknown` and `Distinct`
alike: insert. This reverses the old `?` that aborted the chunk. Cancellation is the one
exception: the batched call runs inside `tokio::select!` against `cancel_token` and returns
`Error::Cancelled` as for any other Phase B await. Definite verdicts are cached for the process
lifetime, keyed on a hash of the full content of both sides (normalized names, types **and
summaries**, so a changed summary is a new key), bounded at 4096 entries; `Unknown` is never
cached.

### Two-pass Phase B

Pass 1 (per entity, cancel-checked) resolves everything needing no judgement: exact-name matches,
no-candidate inserts, and candidates with an identical normalized name (merge, no call). The
ADR-0650 veto already filtered candidates at selection time, so vetoed pairs never reach pass 2.
Pass 2 issues **one** `judge_batch` call for the survivors and fills in the decisions. The LLM
call is therefore in the lock-free Phase B, never under the Phase C write lock. WAL replay
(`replay.rs`, `wal_exec.rs`) re-executes recorded Cypher and never reaches `episode.rs`, so
existing WALs replay to identical graphs with zero LLM calls (regression-tested).

### Opt-in default

`LCG_DEDUP_LLM` is **off by default** for 0.16.3: it is a patch release, the check adds a round
trip per chunk with a surviving candidate, it changes merge behaviour, and small local models
(the bundled sidecar's Foundation Models backend) may not follow an id-keyed JSON schema
reliably — failures cost recall, not correctness. The old silent always-merge default failed
because nobody could tell what protected the graph, so the mode is made visible instead: startup
logs `dedup: mode=<veto-only|llm-verified>`, `knowledge_status` reports `dedup_mode`, and
`LCG_DEDUP_LLM` on with no extractor starts anyway, warns, and reports `veto-only`. The default
should be revisited once the extra-call rate and merge outcomes are measured. `LCG_DEDUP_LLM` is
now boolean-parsed (`0`, `false`, `off`, `no`, empty are off); previously any set value enabled
the old adapter. The spec's third "passthrough" mode is subsumed: with the veto unconditional the
adapter is either veto-only or LLM-verified.

### Cassettes

Dedup verdicts are recorded as `call_type: "judge_duplicates"` (key: hash of the pairs) and served
in replay mode (`LCG_REPLAY_LLM` installs `ReplayingExtractor` directly, so no network call is
possible). Existing cassettes carry no such records, so replay with the check on yields a
`CassetteMiss` per group → insert plus a warning; they stay valid and the cassette format is
unchanged. Record fresh cassettes to exercise the check deterministically.

### Observability

`DedupPathCounts` (per chunk, `dedup_paths` on `knowledge_process_chunk`) gains `llm_confirmed`,
`llm_rejected` and `llm_unavailable`; `adapter_rejected` is kept for custom non-LLM adapters, and
`embedding_merge` still counts merges that needed no LLM verdict. Counters are incremented where
the decision is finalized. `knowledge_status` gains an additive `dedup_mode` in every branch
(queryable, not-queryable, degraded). Dedup token usage is emitted with telemetry `role="dedup"`.
No `knowledge_*` method is added, so the MCP tool registry counts are unchanged.

## Consequences

- An operator with an extractor can opt in to LLM-verified dedup with one env var and no extra
  deployment piece; semantic false merges that no deterministic rule catches are rejected.
- A flaky or slow extractor can only cost recall (missed merges) and latency, never a wrong merge
  or an aborted chunk. Ingest latency rises by one round trip per chunk with a surviving candidate.
- A hostile summary cannot force a merge: it is serialized as data and only an exact boolean for a
  uniquely-attributed id counts.
- `LCG_DEDUP_ADAPTER_URL` no longer drives anything; the "deprecated: remove in Phase B (#59)"
  notes on the dedup adapter are resolved by deletion.
- Existing wrong merges are not repaired; recovery is re-ingestion. Alias recall (`Kubernetes` /
  `K8s`) is unchanged.
