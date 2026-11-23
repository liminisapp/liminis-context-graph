# Feature Specification: lcg-eval retrieval — input digests, `--compare`, and ranked side-by-side

**Feature Branch**: `fabrik/issue-872`
**Created**: 2026-11-22
**Status**: Specified
**Input**: User description: "Implements #847. Spec drafted by the #863 research; full evidence in [docs/research/webbrain-retrieval-suggestions.md](https://github.com/liminisapp/liminis-context-graph/blob/main/docs/research/webbrain-retrieval-suggestions.md) (section #847).

## Why
`lcg-eval retrieval` reports already group by question type, but they only partly show dense and fused results side by side, only partly record input digests, and `retrieval` has no `compare` at all. So two runs can be compared even when their inputs differ.

**User Scenarios & Testing**

- **Story 1 (P1): refuse to compare runs that are not comparable.** A maintainer runs `lcg-eval retrieval --compare <earlier report.json>`. *Acceptance:* if `corpus_hash`, `questions_hash`, or `k` differ, the command exits 2 naming the differing input and prints no deltas. If only a soft input differs (embedder model digest, service build, cassette digest, extractor), it prints the deltas with a banner listing what changed; `--strict` refuses on those too.
- **Story 2 (P1): digests of every input.** Each report records a digest for the corpus, questions, variants file, cassette, embedder model, extractor identity, service binary and effective service environment (a sorted list of `LCG_*` overrides). *Acceptance:* two runs with a different cassette file produce different digests in `report.json`; a run with no cassette records `none`.
- **Story 3 (P2): dense and fused side by side per question.** `--record-ranked` writes per-question ranked lists with `score`, `similarity` and (when the service supplies it, see the #848 draft) `legs` to `ranked.jsonl` beside the report. *Acceptance:* the dense and hybrid configs of one run can be joined by question id to show where fusion changed the top-k.
- **Story 4 (P3): per-type headline.** The Markdown report's first table groups by question category (already in JSON); add `lexical` (direct) against `paraphrase` as a named pair in the summary.

**Requirements**

- **FR-001:** `report.json` gains an `inputs` block (schema version 1) with the digests above, each `sha256` hex (first 16 for display, full in JSON).
- **FR-002:** `lcg-eval retrieval --compare <FILE>` parses an earlier `report.json`, checks `inputs`, and prints per-config and per-category nDCG@k deltas.
- **FR-003:** Hard inputs (`corpus_hash`, `questions_hash`, `k`) mismatch → exit 2. A report without an `inputs` block (older runs) → exit 2 with a message to re-run, never a silent pass.
- **FR-004:** The embedder model identity comes from the service's `knowledge_status` (or the embedder response), not from the socket path.
- **FR-005:** `--record-ranked` is off by default; a private run (`--private`) must not write it.
- **FR-006:** Reuse the `graph_integrity::compare` shape and exit codes; no behaviour change for runs without `--compare`.

**Success Criteria**

- **SC-001:** Comparing two reports from different `questions_hash` exits 2 and prints no table.
- **SC-002:** Changing only the cassette file changes exactly the `cassette` digest.
- **SC-003:** Existing report golden tests pass unchanged apart from the added `inputs` block.
- **SC-004:** The `ranked.jsonl` of a hybrid and a raw config join on question id with no missing ids.

**Edge cases:** an older report with no `inputs`; a run where a config is skipped (qmd absent); `--private`; a config present in only one report; a service that does not report its embedder model (record `unknown`, which counts as a mismatch under `--strict`).

**Assumptions:** `graph_integrity::compare` is the model; the digest format matches `fixture.rs` (`sha_hex`); the sidecar model files may be hashed lazily and cached.

**Not in this issue:** changing any metric, changing what a config sends to the service."

## Background

`lcg-eval retrieval` already records `corpus_hash` and `questions_hash`, the shared `k`, a service version string, an embedder endpoint/socket path, an extractor path or endpoint, the ontology hash and mode, and per-category metrics (`by_category`). Paired bootstrap differences between configs are reported within a single run. Missing today (per the #863 research):

- **No cross-run comparison for `retrieval`.** The only precedent is `lcg-eval graph-integrity --compare`, which refuses runs with a different skeleton or mapping hash (exit 2). Two retrieval reports can currently be eyeballed against each other even when their inputs differ, producing misleading deltas.
- **Partial input digests.** The cassette and the variants file are recorded by path or by the questions hash only, the embedder is identified by a socket path rather than by the model it serves, the service binary by a version string, and the `LCG_*` environment in force is not recorded at all. A digest of the cassette would have flagged that the committed v2 cassette stopped replaying.
- **No per-question ranked output.** Ranked lists and per-hit scores are not persisted, so dense and fused results cannot be joined question by question to see where fusion changed the top-k.
- **Retrieval-side attribution now exists.** #871 (merged) added `legs` (`dense | lexical | both`) to hybrid passage results, so ranked output can carry it when the service supplies it.

This issue is the work issue recommended by #863 for #847. Per-category grouping already exists in the report; only a named `lexical`-vs-`paraphrase` pair is new.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Refuse to compare runs that are not comparable (Priority: P1)

A maintainer runs `lcg-eval retrieval --compare <earlier report.json>` to see how a change moved retrieval quality. The tool checks that the two runs measured the same thing before showing any delta.

**Why this priority**: A delta between runs over different corpora, questions or `k` is meaningless; silent acceptance is the failure this issue exists to remove.

**Independent Test**: Produce two reports that differ only in `questions_hash` and compare them; then two that differ only in cassette digest.

**Acceptance Scenarios**:

1. **Given** an earlier report whose `corpus_hash`, `questions_hash` or `k` differs from the current run, **When** the maintainer passes it to `--compare`, **Then** the command exits 2, names each differing input, and prints no deltas.
2. **Given** an earlier report that differs only in soft inputs (embedder model digest, service build, cassette digest, extractor identity, and the other soft digests), **When** compared without `--strict`, **Then** per-config and per-category nDCG@k deltas are printed under a banner listing each soft input that changed.
3. **Given** the same soft-only difference, **When** `--strict` is also passed, **Then** the command exits 2 naming the differing inputs and prints no deltas.
4. **Given** an earlier report with no `inputs` block, **When** compared, **Then** the command exits 2 with a message telling the maintainer to re-run that report, never a silent pass.
5. **Given** an earlier `--compare` file that cannot be read or is not a valid report, **When** the command runs, **Then** it exits 1 (runtime/invalid file), matching `graph-integrity`.

---

### User Story 2 - Digests of every input (Priority: P1)

Every retrieval report records a digest for each input that can change results, so a later reader can tell whether two runs are comparable without re-deriving it.

**Why this priority**: `--compare` (Story 1) is only as trustworthy as the digests it checks.

**Independent Test**: Run twice, changing only the cassette file; diff the `inputs` blocks.

**Acceptance Scenarios**:

1. **Given** a run, **When** it finishes, **Then** `report.json` contains an `inputs` block with schema version 1 and a full-length `sha256` hex digest for each of: corpus, questions, variants file, cassette, embedder model, extractor identity, service binary, and effective service environment.
2. **Given** two runs that differ only in the cassette file's contents, **When** their `inputs` blocks are compared, **Then** exactly the `cassette` digest differs.
3. **Given** a run with no cassette (or no variants file), **When** it finishes, **Then** that entry records `none`.
4. **Given** a service that does not report its embedder model, **When** the run finishes, **Then** the embedder entry records `unknown`.
5. **Given** the Markdown report, **When** it is rendered, **Then** digests are shown as their first 16 hex characters.

---

### User Story 3 - Dense and fused side by side per question (Priority: P2)

A maintainer records per-question ranked lists so they can see exactly where fusion changed the top-k relative to the dense-only config.

**Why this priority**: It turns aggregate deltas into question-level evidence, but depends on nothing the P1 stories need.

**Independent Test**: Run with `--record-ranked` and join the raw-dense and hybrid configs' lines on question id.

**Acceptance Scenarios**:

1. **Given** a run with `--record-ranked` and an output directory, **When** it finishes, **Then** a `ranked.jsonl` is written beside the report with, per config and question, the ranked hits including `score`, `similarity`, and `legs` when the service supplies it.
2. **Given** a `ranked.jsonl` from a run with both a dense (`raw`) and a hybrid config, **When** joined by question id, **Then** every question id appears in both configs with none missing.
3. **Given** a run without `--record-ranked`, **When** it finishes, **Then** no `ranked.jsonl` is written.
4. **Given** a `--private` run with `--record-ranked`, **When** the command is invoked, **Then** no ranked list is written.

---

### User Story 4 - Per-type headline (Priority: P3)

The Markdown report's summary names `lexical` (the `direct` category) against `paraphrase` as a pair, since that gap is the headline signal about dense versus lexical behaviour.

**Why this priority**: Presentation only; the data is already in the report.

**Independent Test**: Render a report containing both categories and check the summary.

**Acceptance Scenarios**:

1. **Given** a report with both `direct` and `paraphrase` questions, **When** the Markdown is rendered, **Then** the summary shows nDCG@k for `lexical` (direct) and `paraphrase` side by side per config.
2. **Given** a report missing either category, **When** rendered, **Then** the pair is omitted rather than shown with placeholder values.

---

### Edge Cases

- An older report with no `inputs` block: exit 2 with a re-run message (never a silent pass).
- A config skipped in a run (for example the qmd config when qmd is absent): it appears in `--compare` as present in only one report.
- A config present in only one of the two reports: its row shows `n/a` for the missing side and is not itself a mismatch.
- `--private` runs: `inputs` digests are recorded (they are hashes, not content); `--record-ranked` writes nothing; `--compare` still works.
- A service that does not report its embedder model: recorded as `unknown`; under `--strict`, `unknown` counts as a mismatch even against `unknown`.
- A hard-input mismatch on several inputs at once: all differing inputs are named in the one error.
- The `--compare` check failing must not discard the new run's own report; the report is written before the comparison is judged (as `graph-integrity` does).
- `--strict` without `--compare`: has no effect and is rejected as a usage error.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `report.json` gains an `inputs` block (schema version 1) holding a digest for each of: corpus, questions, variants file, cassette, embedder model, extractor identity, service binary, and effective service environment (a sorted list of `LCG_*` overrides in force). Each digest is a `sha256` hex string, full length in JSON and first 16 characters for display.
- **FR-002**: `lcg-eval retrieval --compare <FILE>` parses an earlier `report.json`, checks its `inputs`, and prints per-config and per-category nDCG@k deltas.
- **FR-003**: A mismatch in a hard input (`corpus_hash`, `questions_hash`, or `k`) makes the command exit 2, name the differing input(s), and print no deltas. An earlier report with no `inputs` block makes it exit 2 with a message to re-run it.
- **FR-004**: A mismatch in only soft inputs (embedder model digest, service binary/build, cassette digest, extractor identity, variants digest, service environment, and the ontology hash already recorded) prints the deltas under a banner listing each changed input. `--strict` turns any soft mismatch, including `unknown` embedder identity, into an exit 2 with no deltas.
- **FR-005**: The embedder model identity is taken from the service's `knowledge_status` (or the embedder response), not from the socket path or URL. When unavailable it is recorded as `unknown`.
- **FR-006**: A run with no cassette (or no variants file) records `none` for that digest.
- **FR-007**: `--record-ranked` is off by default. When set, it writes `ranked.jsonl` beside the report with per-question ranked hits carrying `score`, `similarity`, and `legs` when the service supplies it. A `--private` run must not write it.
- **FR-008**: The `ranked.jsonl` entries for different configs of one run are joinable by question id, with every scored question present for every config that ranked it.
- **FR-009**: The Markdown report's category summary names `lexical` (`direct`) against `paraphrase` as a pair, omitted when either category is absent.
- **FR-010**: `--compare` reuses the `graph-integrity` compare shape and exit codes (0 ok; 1 runtime error or unreadable/invalid `--compare` file; 2 usage error or incompatible inputs). A run without `--compare` behaves as today apart from the added `inputs` block.
- **FR-011**: A config present in only one report is shown with `n/a` for the missing side; it does not by itself make the reports incompatible.

### Key Entities *(if applicable)*

- **Inputs block**: A versioned record of digests identifying every input to a retrieval run; the basis for comparability checks.
- **Hard input**: An input whose difference makes deltas meaningless (`corpus_hash`, `questions_hash`, `k`).
- **Soft input**: An input whose difference may explain a delta but does not invalidate the comparison (embedder model, service build, cassette, extractor, variants, service environment, ontology).
- **Ranked record**: One line of `ranked.jsonl`: config, question id, and the ordered hits with `score`, `similarity`, `legs`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Comparing two reports with different `questions_hash` exits 2 and prints no delta table.
- **SC-002**: Changing only the cassette file changes exactly the `cassette` digest and no other entry in `inputs`.
- **SC-003**: Existing report golden tests pass unchanged apart from the added `inputs` block.
- **SC-004**: The `ranked.jsonl` of a hybrid config and a raw config from one run join on question id with no missing ids.
- **SC-005**: Comparing a report that lacks an `inputs` block exits 2 with a re-run message in every case.
- **SC-006**: A `--private --record-ranked` run leaves no ranked file on disk.
- **SC-007**: A run without `--compare` and without `--record-ranked` produces the same metrics and exit code as before this change.

## Assumptions

- `graph_integrity::compare` is the model for shape, error type and exit codes.
- Digest format matches `fixture.rs` (`sha_hex`, length-prefixed sha256); the JSON carries the full hex, displays use the first 16.
- Embedder model files in the sidecar may be hashed lazily and cached; the digest identifies the model, and `unknown` is recorded when it cannot be determined.
- `legs` is available on hybrid passage results since #871; for configs or modes where the service does not supply it, the field is simply absent in `ranked.jsonl`.
- `LCG_*` environment values are tuning settings, not private content, so they are recorded in the digest input in `--private` runs too.
- The ontology hash already recorded is treated as a soft input; `--strict` covers it.
- `--record-ranked` requires an output directory (`--out`), since the file is written beside the report.
- Compare output is written alongside the report when `--out` is set, as `graph-integrity` does with `compare.md`.

## Out of Scope *(optional)*

- Changing any metric, or what a config sends to the service.
- Adding or changing service-side fields (`legs` was #871; score semantics are #848).
- Digests for the ontology file contents beyond the existing ontology hash, or the extraction prompt/request shape.
- Making `--compare` compare more than two reports at a time.
- Any change to `graph-integrity`'s own compare.

## Source References *(optional)*

- [docs/research/webbrain-retrieval-suggestions.md](https://github.com/liminisapp/liminis-context-graph/blob/main/docs/research/webbrain-retrieval-suggestions.md) — section #847
- #847 (origin), #863 (research), #871 (`legs` on hybrid passages), #848 (score semantics)
- `lcg-eval graph-integrity --compare` — precedent for hard/incompatible-input refusal and exit codes
