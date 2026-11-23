# ADR-0828: Retention of Superseded Episodes — Measured Cost, Consumers, and a Decision Not to Prune Yet

**Status**: Decided — no retention policy now; a contingency design and reopening triggers are recorded below
**Date**: 2026-11-22
**Issue**: #828 (research; no implementation). Origin of the concern: shadoworg/liminis-framework#64
(retired Python engine, closed as obsolete).

## Context

When a document is re-ingested lcg does not delete the episodes the new revision replaces; it marks
them superseded (`superseded_at`, `superseded_by`, `superseded_by_kind`, `supersession_reason`) and
keeps the `Episodic` row, its `MENTIONS`, and every edge it produced (ADR-0705, ADR-0292, ADR-0785).
Several reads depend on that history (belief reads, correction reads, passage rerank, lint). Nothing
bounds it, so a frequently revised note keeps every past revision's episodes, edges and entities.

#828 asks whether that is a real cost, who depends on it, whether pruning could be done safely, and
what shape a policy should take. This ADR answers those with measurements. It changes no code.

## Decision

**No retention policy is added now.** The measurements show that:

1. The cost of superseded history is real but it is **linear and predictable**: each retained
   superseded episode costs about 22 KB of database, 9 KB of WAL and 46 ms of rebuild time in the
   measured configuration (§Results). Query latency rises in ratio terms under heavy rewriting but stays
   in low double-digit milliseconds (§Results, "Query latency"), so it is not the cost that drives the decision.
2. In the regime that lcg's own measurements say is typical (a revision rewrites about 10% of a note's
   chunks) the cost at 5–10 revisions per note is a **1.2–1.4× database, ~1.4–2.4× WAL and
   ~1.4–2.2× rebuild** multiple of a live-only graph. Database size stays under the 2× threshold; rebuild
   time meets it at the upper end (2.2× at N=10, 2.06× in the scale check). We judge that tolerable
   because a rebuild is a rare recovery operation, not because the threshold was missed.
3. The cost becomes large only for workloads that rewrite whole notes repeatedly. At a
   superseded : live episode ratio of 4 : 1 the WAL is ~5× and rebuild ~4× a live-only graph.
4. The history is a **documented contract** (`get_episodes` returns superseded rows by default, `as_of`
   belief reads, `knowledge_get_correction_changes`, `knowledge_get_affected_work`, lint). Pruning
   changes that contract, needs a new primitive, and the obvious hook point — DB→WAL compaction — has
   a defect today that must be fixed first (§Incidental findings, F1).

The recommendation is to fix F1, make the cost measurable in production (follow-ups 1–2), and
revisit pruning only when a reopening trigger fires (§Reopening triggers). The safety rule and the
policy shape that the numbers favour if a trigger does fire are written down in §Contingency design
so that a follow-up issue can be filed without further research.

## Method (reproducible)

The measurement harness was scratch code kept **outside the repository** (it is deliberately not
shipped, FR-013). It is described here so it can be rewritten; the workload generator is the only
non-obvious part.

* **Binary and profile**: `cargo build --release -p lcg-service` at commit `1b61c2db` (the later
  commits on `main` do not touch the paths measured); service started as
  `liminis-context-graph --embedder test --extractor test` in a fresh workspace per workload
  (`docs/test-providers.md`). Embedding dimension 768. All numbers are **release profile**.
* **Hardware**: Apple M3 Ultra, 28 cores, 103 GB, macOS 27.2.1, `rustc 1.95.0`. The host was shared with
  other workers (load average 10–130 while the runs went; ~20 when idle), so **timings carry roughly
  ±30% noise**. Ratios within a run are more trustworthy than absolute seconds. Sizes and counts are
  exact.
* **Driver**: a Python client over the Unix socket (newline-delimited JSON-RPC, the pattern in
  CLAUDE.md "Debugging a live or degraded service"). Ingest is `knowledge_process_document`
  (document mode, the normal path for a re-ingested note); counts come from `knowledge_query_cypher`.
* **Workload generator**: P notes of S sections. Each section holds ~40 filler words and two fact
  lines (`SRC -[REL]-> DST: fact`) so the test extractor emits 2 edges per chunk. Entities are drawn
  from a shared pool of 200, with 30% of destinations being new per-revision entities (so some
  entities exist only in one revision). Revision *r* either
  * **low-change**: rewrites 10% of a note's sections (≥1), the rest are byte-identical and the
    document-mode diff classifies them `unchanged`, so they are neither re-extracted nor superseded; or
  * **full-rewrite**: rewrites every section (the upper bound; every chunk is `changed`).
* **Matrix**: P=20, S=10 (200 live episodes) for N ∈ {1, 2, 5, 10, 25} revisions per note, both modes,
  checkpointed in one ingest run. One scale-up: P=100, S=10 (1,000 live episodes), low-change, N ∈ {1, 10}.
  N=1 is the live-only baseline.
* **Per checkpoint**: live / superseded episodes; entities whose only `MENTIONS` come from superseded
  episodes; edges whose `episodes` are all superseded (edges are single-producer, ADR-0705); total
  counts; p50/p95 over 50 calls each of `knowledge_search_passages`, `knowledge_find_entities`,
  `knowledge_find_relationships` with and without `believed_only`; WAL bytes; `knowledge_dump_wal`
  size and time; rebuild time from the WAL and from the dump (fresh workspace holding only the WAL
  directory, `knowledge_rebuild_from_wal`, polled until entity, edge and episode counts match); size
  of the database directory *after* that rebuild (the working database file is pre-allocated and
  noisy, the rebuilt one is not, so it is the DB-size figure used below).
* **Realistic baseline**: `crates/eval/fixtures/retrieval-v2/` (408 notes, 167,435 words, 45 explicit
  supersessions) ingested document-by-document, then the 45 `knowledge_supersede_document` calls from
  its manifest. Passage-only (the notes carry no fact lines, so no entities or edges); the cassette
  cannot be replayed with the test embedder, and the BGE sidecar was not available.
* **Rebuild invariant** and **edge cases**: separate small scripts (§Rebuild invariant, §Edge cases).

## Results

### Q1 — Is it a real cost?

Counts, 20 notes × 10 sections. "Only-superseded" means no live episode supports the entity (via
`MENTIONS`) or edge (via `episodes`).

**Low-change revisions (10% of sections edited per revision)**

| N | live ep | superseded ep | entities | entities only-superseded | edges | edges only-superseded | superseded : live |
|---|---|---|---|---|---|---|---|
| 1 | 200 | 0 | 311 | 0 | 400 | 0 | 0 |
| 2 | 200 | 20 | 320 | 14 (4%) | 440 | 40 (9%) | 0.1 |
| 5 | 200 | 80 | 358 | 43 (12%) | 560 | 160 (29%) | 0.4 |
| 10 | 200 | 180 | 409 | 101 (25%) | 760 | 360 (47%) | 0.9 |
| 25 | 200 | 480 | 600 | 282 (47%) | 1,360 | 960 (71%) | 2.4 |

**Full-rewrite revisions (every section edited every revision)**

| N | live ep | superseded ep | entities | entities only-superseded | edges | edges only-superseded | superseded : live |
|---|---|---|---|---|---|---|---|
| 1 | 200 | 0 | 311 | 0 | 400 | 0 | 0 |
| 2 | 200 | 200 | 435 | 121 (28%) | 800 | 400 (50%) | 1.0 |
| 5 | 200 | 800 | 809 | 482 (60%) | 2,000 | 1,600 (80%) | 4.0 |
| 10 | 200 | 1,800 | 1,399 | 1,085 (78%) | 4,000 | 3,600 (90%) | 9.0 |
| 25 | 200 | 4,800 | 3,198 | 2,879 (90%) | 10,000 | 9,600 (96%) | 24.0 |

**Retrieval-v2 notebook** (realistic structure, passage-only): 948 live and 45 superseded episodes
(4.5% of episodes, superseded : live ≈ 0.05); 0 entities and 0 edges by construction. This corpus
revises decisions by writing *new notes* and superseding the old ones, so it exercises document
supersession rather than re-ingest history; it shows a realistic notebook is mostly live.

Observations that hold in both modes:

* **Edges are not deduplicated** (ADR-0705): every re-extracted fact is a new edge with a single
  producer, so edges grow exactly in proportion to changed chunks (full rewrite: 400 → 10,000 at N=25;
  `edges = 400 × N`). Even a fact that did not change is duplicated when its chunk is re-extracted
  (§Edge cases, oscillation). This is the dominant growth term and is a finding in its own right.
* **Entities grow much more slowly** than edges, because Phase B resolution reuses an entity across
  revisions; entities that become "only-superseded" are mostly the revision-unique ones the generator
  injects (30% of destinations), i.e. what an edit actually removed.
* `MENTIONS` rows grow with episodes (792 → 2,703 low-change, → 19,919 full-rewrite at N=25).

Size, WAL, rebuild and latency. Ratios are against the N=1 row of the *same* matrix. Rebuild
times are with the **test embedder** (cheap), see Caveats. Latency columns for N=1 are from a separate,
quieter re-run (the matrix's own N=1 row ran during a load spike and read about 2× slower:
search_passages 4.0/5.2 ms, find_relationships 25.7/29.1 ms).

**Low-change**

| N | DB after rebuild (MB) | × | WAL (MB) | × | WAL after dump (MB) | × | rebuild from WAL (s) | × | rebuild from dump (s) | search_passages p50/p95 (ms) | find_entities p50/p95 | find_relationships p50/p95 | … `believed_only` p50/p95 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 26.6 | 1.00 | 2.0 | 1.00 | 1.5 | 1.00 | 16.2 | 1.00 | 7.7 | 2.1/2.3 | 6.7/7.1 | 11.1/12.0 | 12.2/12.9 |
| 2 | 27.6 | 1.04 | 2.3 | 1.16 | 1.7 | 1.11 | 16.4 | 1.02 | 6.5 | 2.5/2.8 | 8.1/9.3 | 11.3/13.0 | 13.0/15.2 |
| 5 | 30.5 | 1.15 | 3.3 | 1.65 | 2.2 | 1.43 | 22.0 | 1.36 | 8.1 | 2.8/3.1 | 10.1/10.9 | 13.9/16.3 | 20.0/27.7 |
| 10 | 34.0 | 1.28 | 4.9 | 2.45 | 3.0 | 1.97 | 35.7 | 2.21 | 10.4 | 2.8/3.2 | 10.3/11.1 | 12.8/14.6 | 15.1/16.6 |
| 25 | 43.7 | 1.64 | 9.5 | 4.78 | 5.4 | 3.57 | 56.9 | 3.52 | 18.6 | 3.1/3.6 | 9.1/9.9 | 14.0/17.6 | 15.8/17.3 |

**Full-rewrite**

| N | DB after rebuild (MB) | × | WAL (MB) | × | WAL after dump (MB) | × | rebuild from WAL (s) | × | rebuild from dump (s) | search_passages p50/p95 (ms) | find_entities p50/p95 | find_relationships p50/p95 | … `believed_only` p50/p95 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 26.4 | 1.00 | 2.0 | 1.00 | 1.5 | 1.00 | 16.1 | 1.00 | 7.6 | 2.1/2.3 | 6.7/7.1 | 11.1/12.0 | 12.2/12.9 |
| 2 | 33.3 | 1.26 | 4.1 | 2.07 | 2.9 | 1.92 | 28.7 | 1.78 | 10.9 | 2.7/3.1 | 11.3/12.0 | 12.7/13.7 | 14.0/15.2 |
| 5 | 49.7 | 1.88 | 9.7 | 4.90 | 7.0 | 4.63 | 70.9 | 4.39 | 25.5 | 3.3/3.6 | 9.1/10.1 | 14.6/16.8 | 16.2/17.9 |
| 10 | 70.3 | 2.66 | 18.4 | 9.29 | 13.7 | 9.10 | 98.9 | 6.13 | 40.3 | 3.4/4.1 | 8.1/8.5 | 17.1/18.6 | 19.7/21.0 |
| 25 | 132.6 | 5.02 | 44.6 | 22.49 | 33.9 | 22.53 | 236.0 | 14.62 | 96.4 | 5.9/6.2 | 11.2/11.8 | 24.6/26.0 | 29.0/30.2 |

**Scale check** (P=100 × S=10 = 1,000 live episodes, low-change): the multiples do not change with
size, i.e. the cost is linear in retained rows and the small matrix is representative of a larger
workspace.

| N | live / superseded ep | entities (only-sup.) | edges (only-sup.) | DB after rebuild (MB) | WAL (MB) | WAL after dump (MB) | rebuild from WAL (s) | rebuild from dump (s) | search_passages p50 / find_relationships p50 (ms) |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 1,000 / 0 | 803 (0) | 2,000 (0) | 50.3 | 9.3 | 6.9 | 55.2 | 22.7 | 3.1 / 15.4 |
| 10 | 1,000 / 900 | 1,346 (530) | 3,800 (1,800) | 70.5 (1.40×) | 22.0 (2.37×) | 14.0 (2.03×) | 113.7 (2.06×) | 38.8 (1.71×) | 3.6 / 17.2 |

(At 200 live episodes the same N=10 row reads 1.28× / 2.45× / 1.97× / 2.21×.)

**Per retained superseded episode** (full-rewrite N=25 vs N=1, 4,800 extra episodes, each carrying
2 edges and ~0.6 entities' worth of growth): **≈ 22 KB database, ≈ 9 KB WAL (≈ 6.7 KB after dump),
≈ 46 ms rebuild from WAL (≈ 19 ms from dump)**. As a rule of thumb, with *r* = superseded : live
episodes: DB ≈ 1 + 0.2 r, WAL ≈ 1 + 0.9 r, rebuild ≈ 1 + 0.6–0.8 r (measured r from 0.1 to 24).

**What dominates.** DB size and rebuild time are embedding-dominated (a 768-d `FLOAT[]` is 3 KB; an
episode carries a content embedding and each edge a fact embedding), and embeddings are recomputed on
replay (ADR-0440/0486/0526), not stored in the WAL (ADR-0577). WAL size is text-dominated. Dump
compaction (ADR-0028) removes only what is absent from the database at dump time; it shrank the WAL by
24–43% (part of that is the `MENTIONS` rows the dump drops, F1), and does not reduce history (superseded rows are dumped).

**Query latency.** Small in absolute terms, but not flat. Every p50 and p95 stays in single or low
double-digit milliseconds (worst case 30 ms). In ratio terms the full-rewrite regime does cross the
pre-declared ~20% p95 threshold, and by more than the ±30% noise band: p95 against the N=1 row is
search_passages 2.3 → 3.6 ms (+57%) at N=5 and 4.1 ms (+78%) at N=10, and find_relationships 12.0 →
16.8 ms (+40%) at N=5 and 18.6 ms (+55%) at N=10. At N=25 (24 superseded per live episode) it is
search_passages 2.1 → 5.9 ms and find_relationships 11 → 25 ms, ≈ 2.2–2.8× for a 24× increase in
retained history (sub-linear). In the low-change regime the movement up to N=10 is mostly inside host-load
noise (the matrix N=1 row, 4.0 ms, is *slower* than N=5–25), though some rows (find_entities p95 7.1 →
11.1 ms at N=10) also exceed the ratio. `believed_only` costs roughly 10–20% more than the plain read and
that overhead does not visibly grow with history (individual rows are noisier).
(Readers should treat ≤ ±30% differences as noise here.)

**Material?** The threshold set before measuring was: superseded-only data costing more than about 2×
the live-only baseline in DB size or rebuild time at a realistic revision depth (N ≈ 5–10), or a
p95 latency regression above ~20% that is not noise. Against it:

| Regime | N = 5 | N = 10 | Verdict |
|---|---|---|---|
| Low-change (≈ the 13.3% re-extracted chunks ADR-0785 measured over this repo's own git history) | DB 1.15×, WAL 1.65×, rebuild 1.4× | DB 1.28–1.40×, WAL 2.4×, rebuild 2.1–2.2× | **Threshold met for rebuild time at N=10** (2.21× at 200 live episodes, 2.06× in the P=100 scale check); not met for DB size (well under 2×) or at N=5 (rebuild 1.4×). **Judged tolerable, not immaterial:** the excess is small, it needs ~10 revisions per note on a workload that is already the lower-cost regime, rebuild is a rare recovery operation, and the test embedder understates real rebuild cost, which is why reopening trigger 2 watches rebuild time with the real embedder. |
| Full-rewrite (upper bound) | DB 1.9×, WAL 4.9×, rebuild 4.4× | DB 2.7×, WAL 9.3×, rebuild 6.1× | **Material** for WAL and rebuild from ~2–3 full revisions. |
| Latency | Low-change: mostly within noise. Full-rewrite: p95 +40–57% | Low-change: up to +56% (find_entities), most rows within noise. Full-rewrite: p95 +55–78% | **Ratio threshold exceeded in the full-rewrite regime; judged not material on absolute grounds** (≤ ~30 ms everywhere, no consumer has a latency budget in this range). The ratio threshold was a poor proxy for a metric this small; if latency becomes a concern, restate the trigger as an absolute p95 budget. |

Whether a real workspace sits in the first or second row is not knowable from here: the demo notebook is
**not in this repository (spec 754) and was not measured**; the realistic retrieval-v2 stand-in is
~5% superseded (r ≈ 0.05, cost multiples ≈ 1.0–1.1×). This is the main reason the answer is "measure it
in production first" rather than "build pruning" (§Reopening triggers).

### Q2 — What depends on superseded history?

Verified by reading the code paths (the Research-stage table was unverified and has been corrected
below), against the 48 non-test files that mention `supersed*`. "Depth" is how far back the consumer must
reach; "if truncated" is what changes if superseded episodes older than that were hard-deleted.

| Consumer | Where | Depth | If truncated |
|---|---|---|---|
| Edge belief state, `believed_only`, `as_of`, the `belief` annotation on relationship reads | `belief.rs` `evaluate` (:121), `belief_interval` (:209), `in_force` (:278), `supersession_info` (:343); `handlers.rs` relationship reads | **All of history** for `as_of`; **none** for current-time reads *provided the producer row still exists* | A deleted producer makes the edge `support_deleted`: unbelieved at *every* `t` (ADR-0705). `as_of` before the prune horizon silently returns less. Hard-deleting the edge too removes it from every read. |
| `knowledge_get_correction_changes` | `handlers.rs` `handle_get_correction_changes` (:2110), `correction_reads.rs` `diff_facts` (:77), `revision_unit_uuids` (:115), `changes_superseded` (:224) | The superseded episode **and its replacing revision**: last revision per chunk `name`. An older generation's `episode_uuid` is a legal input | Explaining a pruned episode returns `episode '<uuid>' not found`; explaining the generation before the retained one loses its "replacing revision" |
| `knowledge_get_affected_work` | `handlers.rs` `handle_get_affected_work` (:2164), `correction_reads.rs` `affected_work_response` (:260), `filter_window` (:148) | **Time window**: the closed belief interval of the edge, from its `T1` (when the superseded fact was first believed) to `T2` | Episodes ingested in the interval, or the producers, disappear; the "what was written while the old belief held" answer shrinks |
| Passage rerank (ADR-0796) and belief-aware default ranking (ADR-0806) | `passage_rerank.rs` `replacements` (:196), `search.rs` `belief_facts` (:570–600), `search_stages.rs` `superseded_factor` (:93) | **Last revision only**, and only while the successor is in the reranked head | Superseded passages are *demoted*, not dropped (ADR-0796 rejected dropping). A pruned predecessor simply leaves the pool; a missing successor makes the demotion apply without the "successor above" guarantee. No error |
| `get_episodes`, page reads, MCP `get_episodes` | `read_page.rs` (:30–32), `tools.rs` (:413) | **All of history, by default** — the `superseded_*` columns are a documented part of the result | A user-visible contract change: `get_episodes(last_n)` returns fewer rows; `supersession_reason`/`superseded_by` chains break |
| Knowledge lint: orphan entities (`support_lost`), stale derived pages, citations | `lint/entities.rs`, `lint/stale.rs` (`cause` :24), `lint/snapshot.rs` (:40) | **Time window**: the supersession state of the episodes that once corroborated a derived page | ADR-0755 treats a hard-deleted corroborator as ignorable, so `stale derived page` detection *degrades silently* rather than failing |
| Origin consistency per `doc_id` (ADR-0752) | `origin.rs` (:170–178), `db.rs` `origins_for_doc` (:2285) | **One** surviving episode per document | None, as long as one episode per `doc_id` survives |
| Supersession writers (must keep working) | `db.rs` (`supersede_*`, ~1694–1830), `document/ingest.rs` commit step (~480–590), `handlers.rs` `knowledge_mark_chunk_removed` / `knowledge_supersede_document` (:3249, :3514) | Live episodes + the *immediately previous* generation of a name | A prune must not touch pending (uncommitted) revision rows; the commit step groups *live* committed episodes by name and commit instant, so pruning superseded rows only is safe |
| DB→WAL dump | `dump.rs` (:272–329) | Dumps superseded columns and rows verbatim | Compaction is where a prune *would* take effect on disk (§Contingency design) |
| `DocSupersession` / `DocRevision` | `schema.rs`; ADR-0707, ADR-0785 | Not episodes: durable per-document records, O(documents) | Must be kept; tiny. They are *not* removed by episode pruning |
| Eval harness | `crates/eval/src/retrieval/lcg.rs` (`superseded_episodes`, :416), `report.rs` | Counts only | Reporting only; no dependency on depth |

Summary by depth: **last revision only** — passage rerank, origin consistency, correction explain for
the newest correction. **Time window** — `get_affected_work`, lint. **All of history** — `as_of`,
default `get_episodes`, correction explain for any past generation. Because three consumers need the
full history (and `get_episodes` exposes it by default), *any* truncation is a behaviour change that
must be documented; the safe floor is "last revision of each name plus an age floor".

### Q3 — What is safe to remove? (the rule, verified)

Defined in §Contingency design and **verified on a live graph** (below). Two measured results the
rule depends on:

#### Rebuild invariant (FR-008)

Three graphs from one history (3 notes × 6 sections, 4 versions per note: the initial ingest plus 3 revisions, each rewriting 2 of the 6 sections, so 3 × 3 × 2 = 18 superseded and 18 live episodes; this is a slightly heavier edit than the matrix's 10% low-change rule): **A** the history as
ingested; **B** the *current* files only, ingested fresh (what a rebuild from the markdown sources
produces); **C** A after a scratch prune using the rule below (superseded episodes, the edges they
produced, entities left without any `MENTIONS` or edge endpoint). Then **R** = a rebuild from C's
WAL and **RU** = a rebuild from an un-pruned twin's WAL.

| Compared | Result |
|---|---|
| B's live-episode content vs A's | identical (chunk *names* differ — a revised chunk keeps its first revision's identity name — so compare by content) |
| B's entity set vs A's live-mentioned entity set | identical (35 = 35) |
| B's edge facts vs C's | identical (36 = 36) |
| C vs A's live subset | live episodes 18 = 18; entities 35 = 35 (16 orphans removed); no live row lost |
| R vs C (a WAL rebuild of the pruned WAL) | identical |
| RU vs A (a WAL rebuild of the unpruned WAL) | identical: 36 episodes, 51 entities, 72 edges — history survives a WAL rebuild |
| RU vs R | **differ** (36/51/72 vs 18/35/36) |

So: **a rebuild from the markdown sources is unaffected by pruning** (B contains no superseded history
whether or not A was pruned), and the "graph is rebuildable from files" invariant holds in the sense
the project means. A rebuild from the **WAL** *does* depend on pruning: the WAL is the durable record
and, since re-extraction is the expensive step, the thing people actually rebuild from (ADR-0577).
Pruned history is gone from a WAL-based rebuild, and that is the whole point of a prune. Nothing else
changes: no live episode, edge or entity differs.

#### Edge cases

Run on a single note (S1 edited Acme → Globex → Acme; S2 then removed):

* **Oscillation A→B→A.** Three S1 episodes: v1 (Acme, superseded), v2 (Globex, superseded), v3 (Acme,
  live). `Acme` is mentioned by v1 *and* v3, so it is live-supported and stays under any rule; `Globex`
  is only-superseded. The edge "Alice works at Acme" exists **twice** (producers v1 and v3) because
  edges are not deduplicated: v1's copy is only-superseded and prunable although an identical live one
  exists. Retention by *entity support* alone would not need to care; retention by *edge text* would be
  wrong (it would keep the v1 copy forever). The rule therefore works on producers, not on fact text.
* **Chunk removed from the note** (`supersession_reason = chunk_removed`, `superseded_by` NULL). The S2
  episode is superseded with no successor. Its entities (`Bob`, `Shared`) become only-superseded.
  These fall *inside* the retention question (an episode with no revision chain can only age out) but
  *outside* any keep-last-N-revisions rule, which has nothing to count: they must be governed by the age
  floor alone.
* **Entity shared across notes / groups**: `Pool*` entities in the matrix are shared by every note and
  are never only-superseded; entity support is a `MENTIONS` scan, not a per-note property, and
  cross-group pointers can reference an entity by uuid (ADR-0361, ADR-0369), so the entity rule must
  check pointers before deleting.
* **Unattributed episodes** (ADR-0292): never superseded, hence never in scope.
* **Small vs large workspace**: linear, see the scale check. ADR-0026's 43,821-file WAL was not
  reproduced; at ~9 KB of WAL per retained superseded episode it is a cost model, not a measurement.

### Incidental findings (not in scope to fix here)

* **F1 — `knowledge_dump_wal` silently drops every native `MENTIONS` edge.**
  `Conn::insert_mentions_edge` (`crates/core/src/db.rs:1239`, `insert_mentions_edge`) creates
  `(ep)-[:MENTIONS {group_id}]->(en)` with **no `uuid`**, and `dump.rs:656–660` skips any `MENTIONS`
  whose `uuid` is empty ("pre-migration edges"). Every document-mode `MENTIONS` row I measured had a
  null uuid (e.g. 792 of 792 at N=1), the dump logged one `[DUMP WARN] … has null uuid — skipping`
  per row, and **a database rebuilt from the dump has 0 `MENTIONS`** (vs 792 from the WAL) while
  episodes, entities and edges all match. I did not trace every consumer of `MENTIONS` after such a
  rebuild; the ones I know of are the lint orphan/support checks and entity→episode provenance. This is
  independent of retention but is a **prerequisite for any compaction-time policy**, because
  "entity supported by a retained episode" is defined by `MENTIONS`. It likely deserves its own bug issue.
* **F2 — raw Cypher mutations are logged to the default group's WAL stream**, not the group they touch
  (`handle_query_cypher`, `handlers.rs:2219`; documented in the handler). My scratch prune went
  through it and a WAL rebuild only reproduced it after replaying *both* the `g` and `liminis` streams.
  A real prune primitive must write its deletes into the owning group's stream (ADR-0378) or per-group
  replay will resurrect pruned history.
* **F3 — `DETACH DELETE` does not shrink the WAL.** The pruned WAL was 394,392 bytes against 394,166
  for the unpruned twin. Only dump compaction reclaims space: the dump of the pruned graph was 155 KB,
  ~39% of the unpruned WAL.

## Contingency design (what to build if a trigger fires)

Not a recommendation to build it now; recorded so a follow-up can be filed directly.

**Safety rule (FR-007).** Let *P* be the set of **prune candidates**: episodes that are superseded
(`superseded_at` set and not pending), attributed (`write_path` NULL, `chunk` or `document`, ADR-0292),
and outside the retained set. The retained set is: all live episodes; for each chunk `name`, the
most recent *K* superseded generations (default K = 1, the immediate predecessor, which keeps
`get_correction_changes` and rerank working for the latest correction); and anything superseded more
recently than an age floor *T* (default 90 days, so `get_affected_work` windows and `as_of` for the
recent past keep working). `chunk_removed` episodes have no generation and are governed by *T* alone.
Applying the rule removes, in one write transaction under the write lock:

1. each episode in *P* with its `MENTIONS` rows (`DETACH DELETE`);
2. each edge (`RelatesToNode_`) **all** of whose producing episodes are in *P*; an edge with at least
   one producer outside *P* is kept;
3. each entity left with **no** `MENTIONS` from any remaining episode, **no** remaining edge as an
   endpoint and **no** cross-group pointer (ADR-0361's `unbound` handling applies to pointers that
   referenced it); anything touched by a retained episode, a live episode, or a hand-asserted edge
   survives;
4. nothing else: `DocSupersession` / `DocRevision` (O(documents)), unattributed and pending episodes,
   and merged entity `summary` text are untouched. A merged summary that still quotes pruned text is
   **accepted residue** (`summary_merge.rs` merges from every contributor and cannot subtract); the
   alternative, recomputing summaries, is a separate follow-up.

*Worked example* (the oscillation note above, K = 0 and T = 0, to show the maximal prune):

| Item | Support | Outcome |
|---|---|---|
| S1 v1 episode (Acme, superseded) | — | removed |
| S1 v2 episode (Globex, superseded) | — | removed |
| S1 v3 episode (Acme, live) | — | kept |
| S2 episode (`chunk_removed`) | — | removed |
| Edge "Alice works at Acme" (producer v1) | v1 only | removed |
| Edge "Alice works at Globex" (producer v2) | v2 only | removed |
| Edge "Bob uses Shared" (producer S2) | S2 only | removed |
| Edge "Alice works at Acme" (producer v3) | v3 (live) | **kept** |
| Entity Alice | mentioned by v3 | kept |
| Entity Acme | mentioned by v1, v3 | kept (still supported by a live episode) |
| Entity Globex | v2 only | removed (no orphan left) |
| Entities Bob, Shared | S2 only | removed |

With K = 1, v2 is retained, so the Globex edge and entity stay and only v1, its edge and the S2 rows go.
This matches what the scratch prune did on the 4-revision graph (18 episodes, 36 edges and 16 orphan
entities removed; live rows unchanged).

**How it differs from existing removal.** ADR-0038's `remove_episode*` primitives delete only
`Episodic` nodes and *deliberately* leave entities and edges (turning them `support_deleted`): the
wrong behaviour here, because it orphans. `group_purge` (ADR-0361) removes a whole group: the wrong
granularity. The rule above is a third primitive: per-episode like ADR-0038, but support-aware like
ADR-0361.

**Policy shape (FR-009)** — chosen for the numbers above:

| Axis | Choice | Why / why not the others |
|---|---|---|
| Rule | **keep last K generations per `name` + an age floor T** | Matches the consumers' depth needs (last revision + time window). Age-only would break `get_correction_changes` on a fresh correction of an old chunk; keep-last-N-only ignores `chunk_removed` and the time-window readers. |
| When | **Explicit admin operation** (`knowledge_prune_superseded`, `admin` scope, with `dry_run`) and, only later, optionally as a step of DB→WAL compaction | Compaction holds the write lock for the whole dump (ADR-0028) and, per F1, cannot be trusted with entity support until fixed; and a delete alone does not shrink the WAL (F3), so the *compaction* is what realises the saving — they are naturally run together by an operator. Automatic pruning at write time would change `get_episodes` behind a caller's back. |
| Default | **Off / opt-in** | Three consumers need full history and `get_episodes` exposes it by default; ADR-0796 rejected dropping superseded data because "a user may ask about history". |
| Scope | **Per group** (parameter and stored policy), no global default | Aligned with the per-group WAL streams (ADR-0378); one group (e.g. an auto-regenerated wiki) can be heavy while another is not. |
| Rejected alternative | "drop `content` and embeddings but keep rows" | Would preserve `as_of`/belief and `get_episodes` rows, but embeddings and content are exactly the bulk (embeddings are ~90% of an old WAL, ADR-0577) so it recovers much of the saving *without* breaking belief. **Not costed here**; worth a measurement before the policy above is built, and likely the better *first* step if the trigger is DB size rather than WAL size. |

## Reopening triggers

Reopen the question — and file the follow-ups below — if **any** of the following is observed in a real
workspace (these use the cost model above; at r ≈ 2 a group is ≈ 1.4× DB, ≈ 3× WAL, ≈ 2–3× rebuild):

1. **Superseded : live episodes exceeds 2 : 1 in any group** (full-rewrite N ≈ 3; the point where the
   WAL is about 3× and rebuild about 2.5× a live-only graph; these are interpolated from the rule of
   thumb above, since nothing was measured at r = 2 — the nearest measured rows are r = 1 at 2.1× / 1.8×
   and r = 4 at 4.9× / 4.4×).
2. **Rebuild from WAL takes more than ~2× what the same group would take live-only**, or exceeds an
   operator's tolerated recovery time (the number to watch with the *real* embedder, which makes each
   retained row far more expensive than the 46 ms measured with the test embedder).
3. **WAL bytes per live episode more than ~3× the group's N=1 figure** (≈ 10 KB per live episode here).
4. A measurement against the **demo notebook** or another production-scale WAL (ADR-0026's 43,821 files)
   that contradicts the linear model or the typical-regime assumption.

Trigger 1 is measured by `knowledge_status {"include_episode_breakdown": true}` (#884, ADR-0884):
per group, `superseded / live` from `episode_breakdown.groups[]`, alongside
`only_superseded_entities` and `only_superseded_edges`. Triggers 2 to 4 still need a rebuild timing,
WAL bytes per live episode and a production-scale measurement.

## Follow-ups to file (none is part of this issue)

1. **Bug: `knowledge_dump_wal` skips every `MENTIONS` edge created by native ingest** (F1).
   Prerequisite for any compaction-hook policy; independent value (a compacted-and-rebuilt graph
   currently loses entity→episode provenance).
2. **Feature: split live / superseded / pending episode counts (and only-superseded entity and edge
   counts) in `knowledge_status`** (or a lint category), so the triggers above are measurable.
   *Done by #884 (ADR-0884): opt-in `include_episode_breakdown` on `knowledge_status`.*
3. *(Conditional, on a trigger)* **Measure "drop content and embeddings, keep rows"** against full
   pruning on the demo notebook or a production WAL.
4. *(Conditional)* **`knowledge_prune_superseded`**: the rule and worked example above, per group,
   opt-in, `dry_run`, writing through the owning group's WAL stream (F2), with a documented change to
   `get_episodes`, `as_of`, `get_correction_changes` and the MCP tool descriptions, and an ADR of its own
   (`docs/adr/<issue>-…`).
5. *(Conditional, optional)* Edge de-duplication on re-extraction (edges grow 1:1 with re-extracted
   chunks; an identical fact is stored again), which attacks the largest single growth term *without
   touching history semantics*; and abandoned pending document revisions (the ADR-0785 follow-up), which
   are adjacent but out of scope here.

## Caveats

* **Test providers.** The extractor emits only fact lines and the embedder is a hashed bag of words,
  so rebuild times understate real embedding cost (a real model makes each retained row far more
  expensive) and there is no real dedup ambiguity. Treat the counts and sizes as exact for this
  workload and the times as relative. Per-episode costs depend on the generator (2 facts, ~330
  characters per chunk).
* **The demo notebook was not measured** (not in this repository, spec 754). `retrieval-v2` is the
  realistic stand-in, but it is passage-only here (no cassette replay) and encodes decision revision as
  new notes plus document supersession, not re-ingest history.
  Consequently FR-001 and SC-001 of `specs/828-research-retention-policy-for-superseded-episodes/spec.md`
  are only partly met: the spec's Assumptions say the demo notebook is available, which it was not, so
  the counts for it are replaced by the `retrieval-v2` row and the synthetic matrix. The spec is the
  engine-generated Specify artifact and is left as written.
* **Timing noise** (shared host, ±30%); sizes and counts are exact. The 2× threshold was chosen by the
  researcher in the Plan stage, before any measurement, as the spec's Assumptions require a stated
  reasoning rather than a pre-fixed numeric threshold; it is a judgment, not a standard, and the
  maintainer is expected to review it.
* **No implementation.** No production code, schema, API or belief/correction read change is part of
  this ADR. The scratch scripts and raw results live outside the repository and are reproducible from
  §Method.

## Consequences

* No behaviour change. `get_episodes`, `as_of`, correction reads, rerank and lint keep their full
  history.
* Superseded history keeps costing about 22 KB DB, 9 KB WAL and (with the test embedder) 46 ms of
  rebuild per episode; operators with heavily rewritten notes should expect WAL and rebuild to grow
  roughly with `1 + 0.9 r`.
* The decision is cheap to reverse: the contingency design needs no schema change, only a primitive,
  and the reopening triggers say when.

## Related

ADR-0705 (belief time, `support_deleted`), ADR-0706 (correction reads), ADR-0707 (document
supersession), ADR-0785 (document mode, abandoned-attempt GC), ADR-0292 (unattributed episodes),
ADR-0796/0806 (passage rerank, belief-aware default), ADR-0755 (lint), ADR-0752 (origin), ADR-0038 and
ADR-0361 (existing removal), ADR-0028 (compaction), ADR-0378 (per-group WAL streams), ADR-0577/0440/0526
(embeddings on replay).
