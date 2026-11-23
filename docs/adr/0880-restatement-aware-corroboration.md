# ADR-0880: Restatement-Aware Corroboration — Independent Sources, Not Documents

**Status**: Proposed (research draft — nothing here is implemented)
**Date**: 2026-11-22
**Issue**: #880 (extends [ADR-0752](0752-episode-origin-and-corroboration.md); distinct from #732)

## Context

[ADR-0752](0752-episode-origin-and-corroboration.md) stopped a derived page from corroborating
itself: each episode carries a caller-declared `origin`, and `support` counts **distinct documents**
per origin over the same-fact siblings `(group, source, target, name)`. `corroboration` is
`primary + external`. It is aggregated at read time, edges are never deduplicated at ingest (ADR-0705),
there is no backfill, and the numbers are identical after WAL replay.

That treats every *primary* document as independent evidence. In a real notebook it is not. Daily
notes carry an open action forward day after day; standups and weekly digests restate earlier notes by
the same author. A fact copied through twelve daily notes reports `corroboration: 12`. This is the
inflation ADR-0752 removed for derived content, now caused by one author repeating themselves, and it
skews anything that ranks or thresholds on corroboration (`weak_support`, contradiction sides).
The retrieval evaluation already knows this: a primary standup recap is a gain-1 near-duplicate of its
source note (`docs/retrieval-quality-evaluation.md`). The support count does not.

**Not #732.** #732 covers derived observations and mental models. This ADR changes only how
`primary` and `external` evidence is counted. Mis-stamped origin (agent-made pages marked `primary`,
liminis-notes#1369) is an origin-stamping problem and is not solved here; the fixture does not depend on it.

## Options

All three are judged against the same criteria. "ADR-0752 properties" are read-time aggregation (RT),
no ingest edge dedup (ND), no backfill (NB) and identical results after WAL replay (WR).

| | 1. Count independent sources | 2. First assertion vs restatements | 3. Expose the shape only |
|---|---|---|---|
| Caller cost | One optional `source_key` per episode | Same key (to judge "same source"); dates alone cannot tell a second person from the first author repeating | None |
| Legacy (unkeyed) data | Needs an explicit rule (below) | Same | Unchanged |
| Meets SC-002 / SC-003 | Yes (`corroboration` independent of N) | Yes, via `support` plus `restatements` | **No** — `corroboration` stays N |
| RT / ND / NB / WR | All hold if the key is a logged parameter | Same | Hold trivially |
| Weakness | Redefining `corroboration` silently flips `weak_support` for existing clients | Alone, it needs the same key as option 1 | Leaves the inflated number as the headline figure; pushes the work to every consumer |

Date-only restatement without a key (option 2 alone) fails the "second person independently confirms"
case. Option 3 alone cannot meet the acceptance numbers. Options 1 and 2 share the same input, so
choosing both costs the caller nothing extra.

## Decision

**A combination: options 1 and 2, with option 3's `first_asserted` shape. Every new output key is additive.
No existing field changes meaning.**

### 1. A caller-declared `source_key`

An optional string on each episode naming **the independent origin of the claim** — usually an author, but
a caller who knows two notes share a root (two attendees restating one meeting) can declare the meeting
id instead. It follows the `origin` model (ADR-0752 Decision 1): declared by the caller, **never
authenticated**, only as reliable as the declaration.

- **Follow-up implementation carrier:** a nullable `Episodic.source_key` column, validated and logged in
  the WAL, with the dump, page and replay columns changed in lockstep (ADR-0752 Decision 1, ADR-0028,
  ADR-0853 §1). A dedicated column is validated, uniform and cheap to read for every episode a sibling
  scan loads.
- **This research's fixture carrier:** the existing free-form `attributes` JSON (ADR-0528), already
  accepted, WAL-logged and replayed, so the fixture runs against live lcg without a schema change (FR-014).
  The two differ in carrier only, not in counting semantics.

### 2. The counting unit: a *source unit*

Within one same-fact class `(group, source, target, name)`, over episodes live at read time (or at `as_of`):

- Only `primary` and `external` episodes form units. `derived` and `answer` never corroborate (unchanged).
- A **keyed** episode's unit is `(origin, source_key)`. The same key under `primary` and `external` does not merge.
- A **keyless** episode is its own unit, keyed by its existing document key (`doc:<doc_id>`, else `ep:<uuid>`).
  This is exactly today's counting, so unlabelled legacy data **needs no backfill** (SC-006, FR-005).
  A keyed restatement beside keyless notes therefore adds one unit for the key and each keyless document
  stays its own unit (fixture fact F). The alternative — all keyless documents sharing one anonymous source —
  would change existing counts and is rejected.
- A keyed note split into chunks collapses to one unit, even without a `doc_id`. This closes the
  no-`doc_id` chunk over-count (ADR-0752) for keyed data only.

### 3. First assertion

The **first assertion** of a fact is the live corroborating episode minimising `(valid_at, ingested_at, uuid)`.

- `valid_at` is the caller's `reference_time`, the date the note speaks for. `ingested_at` alone would
  mis-order a note backfilled late.
- Ties break on `ingested_at`, then `uuid`. All three replay verbatim from the WAL, so the choice is a pure
  function of replayed data.
- **Chunks of one note tie on `valid_at` and, to the second, on `ingested_at`**, so which chunk the uuid picks
  is arbitrary per ingest, though stable across replay. Consumers should treat `first_asserted` as identifying
  a *note*, not a chunk; the fixture asserts it at note granularity.
- **No time window.** Consecutive days and a gap of months are treated alike. A window is a magic constant
  that cannot tell "still waiting on Acme" from a genuine re-confirmation; the key already carries the
  independence signal.
- Superseding the first assertion promotes the next-earliest live episode (fixture fact G), and `as_of` is
  evaluated at the `as_of` instant, exactly as ADR-0752 Decision 7 does for `support`.

### 4. What is deliberately not solved

- **Two authors restating one meeting count as two units.** Nothing in the data says they share a root. A caller
  who knows can declare the shared `source_key` (see above).
- **Entity-level `support`** (`knowledge_find_entities`, `entity_support` over `MENTIONS`) is unchanged: it has
  no same-fact key, so "restatement" has no anchor. Recorded as a follow-up.
- **Semantic paraphrase detection** is not attempted. A verbatim copy and a rephrase by the same author are one
  unit through the key (fixture fact J); the text is never compared.

## API delta

Applies to `knowledge_find_relationships` (and the lint snapshot). A client that sends no `source_key` sees
only additive keys.

| Field | Change |
|---|---|
| `support` | **Unchanged** — per-origin distinct documents (the raw counts, FR-008) |
| `corroboration` | **Unchanged** — `primary + external` distinct documents |
| `independent_corroboration` | **New** — distinct source units across primary and external |
| `restatements` | **New** — `corroboration − independent_corroboration` |
| `first_asserted` | **New** — `{episode_uuid, valid_at, source_key?}` of the first assertion; absent when nothing corroborates |
| ingest `source_key` (optional string) | **New** input on `knowledge_add_episode`, `knowledge_process_chunk` |

With no `source_key` anywhere: `independent_corroboration == corroboration` and `restatements == 0`.
`corroboration` is not redefined because that would silently flip `weak_support` for every existing client;
the app opts in by reading `independent_corroboration`. SC-002 and SC-003 are met by
`independent_corroboration`.

A new optional field on existing methods does not change the MCP registry's tool or bucket counts
(`crates/service/src/mcp/tools.rs`); no new method is added. `knowledge_find_entities` is unchanged.

## Lint categories that read these values

The readers are exhaustively these (SC-005, zero unnamed readers); the first two read counts, the rest do not.

| Category | Reads | Today, fact restated 12× by one author | After |
|---|---|---|---|
| `weak_support` (`lint/weak.rs`) | `corroboration` 0 → `derived_only`, 1 → `single_source`; emits `details.support`/`corroboration` | Silent (`corroboration` 12) | **Fires `single_source`**: tests `independent_corroboration` (1). `derived_only` likewise. `details` gains the new keys. Finding id subject `(src, dst, name)` is unchanged, so ids stay stable. This is the most visible behaviour change. |
| `contradiction` / `single_valued_conflict` (`lint/contradiction.rs`) | each `sides[]` entry carries `support` and `corroboration` | Side weight 12 | `sides[]` gain `independent_corroboration`, `restatements`, `first_asserted` beside the raw counts. No threshold is applied, so no finding appears or disappears; a consumer ranking sides should rank by `independent_corroboration`. |
| `stale_derived_page` (`lint/stale.rs`) | `Origin::is_corroborating` on episodes, not counts (`lost_corroboration`) | Unaffected | Unaffected. `first_asserted` is informative when the first assertion is superseded and only restatements remain. |
| `orphan_entity` (`lint/entities.rs`) | Does not read corroboration | — | Unaffected |
| `near_duplicate_entity` | No relation to corroboration | — | Unaffected |
| `knowledge_find_relationships` annotation | `support`, `corroboration` | 12 | additive keys above |
| `knowledge_find_entities` annotation | `support`, `corroboration` (via `MENTIONS`) | unchanged | unchanged (see Decision 4) |

**Divergence risk.** Support is computed on two independent paths that both end in `SupportTally::add`:
`origin_reads::edge_support` (live DB, for the two read methods) and `lint::Ctx::tally` (snapshot, for
lint). `EpisodeOriginInfo` and `LintEpisode` currently carry no `valid_at`, `ingested_at` or key. The follow-up
should make `SupportTally` own the restatement rule so the two paths cannot diverge, and widen both
inputs. Editing only one would make `knowledge_find_relationships` and `weak_support` disagree.

## ADR-0752 properties

- **Read-time aggregation** — holds. Nothing is stored beyond the caller's declared input.
- **No edge dedup at ingest** — holds. Each restating note still writes its own `RelatesToNode_` and episodes.
- **No backfill** — holds. A keyless episode counts exactly as today (Decision 2).
- **Identical after WAL replay** — holds when `source_key` is a logged parameter (Decision 1). The fixture
  proves it for the `attributes` carrier: counts, the reference-rule output and every declared key are
  identical after a full replay.

No deviation.

## Consumer note (shadoworg/liminis-notes)

Written against the API only; the companion app issues were not readable from here.

- **New input:** optionally send `source_key` per note (author, or shared root such as a meeting id).
- **No new input:** every existing field and count is unchanged; `independent_corroboration == corroboration`,
  `restatements == 0`, `first_asserted` is the earliest live corroborating episode.
- **Mixed legacy and new data:** keyed notes collapse per `(origin, source_key)`; keyless notes keep counting
  one per document. An old note and a new keyed note by the same author count as two units — declared keys
  are not applied retroactively, and there is no backfill.
- **Expect more `single_source` findings** for well-repeated facts once the follow-up lands and keys are supplied.
- **Risk:** a key set to a constant by a careless client collapses all corroboration into one unit — the same
  class of risk as an unauthenticated `origin`.

## Fixture

`crates/core/tests/restatement_corroboration.rs` with `crates/core/tests/fixtures/restatement/corpus.json`. It
uses scripted extraction (no LLM, no embedder), so it is cheap, and it extends neither retrieval-v2 (a re-render
would need a paid step and change every hash and cassette) nor `lcg-eval`. It does four things:

1. Asserts **today's** `corroboration` per fact against live lcg (the baseline).
2. Applies the rule above as a pure function (`reference_rule`) over episode rows read back from the DB and asserts the
   documented table below. The table is also asserted literally, so a wrong rule cannot rewrite its own expectations.
3. Asserts independence from N (N = 1, 2, 10, 12, 50), the tie-break order, and that stripping every key reproduces today's counts (SC-006).
4. Replays the WAL into a fresh DB and asserts all of the above is identical (SC-004).

| Fact | Setup | Baseline `corroboration` | Independent / restatements | `first_asserted` |
|---|---|---|---|---|
| A | alice, 12 dated daily notes | 12 | 1 / 11 | A-alice-01 |
| B | bob, once | 1 | 1 / 0 | B-bob-01 |
| C | alice ×10 + bob ×1 | 11 | 2 / 9 | C-alice-01 |
| D | 3 keyless notes (legacy) | 3 | 3 / 0 | D-legacy-01 |
| E | one alice note, 3 chunks, no `doc_id` | 3 | 1 / 2 | E-alice-01 (note) |
| F | alice, then 2 keyless notes | 3 | 3 / 0 | F-alice-01 |
| G | alice ×4, day 1 removed | 3 | 1 / 2 | G-alice-02 (promoted) |
| H | external `acme.com` ×3 + primary alice | 4 | 2 / 2 | H-acme-01 |
| I | alice ×2 + derived page citing alice | 2 | 1 / 1 | I-alice-01 |
| J | alice original, verbatim copy, rephrase | 3 | 1 / 2 | J-orig-01 |
| K | alice source note + her standup recap | 2 | 1 / 1 | K-source-01 |

Fact K is consistent with the retrieval-quality fixture's treatment of a primary standup recap as a gain-1
near-duplicate: the recap shares its source note's key, so it is a restatement and adds no independent
support. Both agree that a recap is not new evidence.

## Follow-up implementation scope

Not part of this issue (FR-014). A later issue touches:

- `crates/core`: `Episodic.source_key` column, WAL writer/replayer, dump and page columns; `EpisodeOpts`; `handlers.rs`
  parsing beside `parse_origin_param`; `SupportTally`, `origin_reads::edge_support`, `lint::Ctx::tally`,
  `EpisodeOriginInfo`, `LintEpisode`; `lint/weak.rs`, `lint/contradiction.rs`.
- `crates/service/src/mcp/tools.rs` descriptions; `docs/ipc-mcp-reference.md` ("Episode origin"); `README.md`;
  the Tier 1 parity tests in `crates/core/tests/ipc_parity.rs` only if the field becomes required (it does not).
- Replace the fixture's `attributes.source_key` carrier with the column and assert the live output instead of
  the reference function.

## Alternatives considered

- **Option 1 alone, redefining `corroboration`.** Rejected: flips `weak_support` for existing clients.
- **Option 2 by dates alone, no key.** Rejected: cannot separate an independent second author from the first author repeating.
- **Option 3 alone.** Rejected: misses SC-002 / SC-003 and leaves the inflated number as the headline.
- **All keyless documents as one anonymous source.** Rejected: changes existing counts.
- **A fixed restatement time window.** Rejected: an arbitrary constant that does not track independence.
- **Extending the retrieval-v2 corpus.** Rejected: forces a paid re-render and changes hashes and cassettes.
