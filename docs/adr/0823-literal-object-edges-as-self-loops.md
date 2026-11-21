# ADR-0823: Literal-Object Edges as Self-Loops Under an Ontology

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #823 (amends [ADR-0051](0051-edge-endpoint-salvage-and-deferred-drop.md) §1 and
[ADR-0666](0666-exact-graph-match-and-uuid-self-loop-guard.md))

## Context

When a fact's object is a **literal value** (a budget, a date, a price, an uptime figure),
ontology-guided extraction often stored it on an edge whose other end was an unrelated entity
mentioned nearby: "Marlin Core has a monthly budget of $4,200" became `Marlin Core HAS_MEASURE
Tomas Kowalski`. The fact text was right and the endpoint wrong, so a walk reached the wrong
entity, and the wrong entity collected facts that were not about it. On the v2 corpus under
ontology v3.1, `lcg-eval graph-integrity` ([ADR-0819](0819-graph-integrity-eval.md)) reported 27
misattached value facts of 440; free-form extraction had 2.

**Cause (FR-001).** The misattachment was built into the edge-extraction contract. Three things
combined:

1. **A literal value can never be an edge endpoint.** The entity pass never creates value nodes
   (`extract_text.txt`: "Do not create nodes for temporal information"), and the Anthropic tool
   schema constrains `source_name`/`target_name` to an `enum` of the batch's entity names.
2. **The edge prompt forbade the alternative.** `extract_edges.txt` required "two DISTINCT
   ENTITIES", said source and target "NEVER refer to the same entity", and rule 3 said "NEVER
   extract facts that describe only a single entity's … attribute. Instead, identify the second
   entity". For a value fact that is an instruction to pin it to *some* other listed entity.
3. **The ontology's relation menu asks for exactly these facts.** `HAS_MEASURE`, `HAS_TERM`,
   `SCHEDULED_FOR` and `HAS_ROLE` (with keywords such as `budget`, `price`, `launch`) have a
   literal object by design, and strict mode removes free-form's escape hatches (invented relation
   names, compound facts gluing in the other entity, incidental concept nodes). Free-form more
   often abstained and left the value in the node summary.

**Entity resolution was ruled out.** In the free-form v2 cassette, 0 of 2,064 edges had an
off-list endpoint: the provider enforces the enum, so salvage never runs and Phase C resolves batch
names exactly. #816's signature check could not catch the pins either: these relations declare no
target type, so the target side is "any".

## Decision

**Under an ontology, a literal-valued fact about one entity is a self-loop on that entity.**
"Marlin Core has a monthly budget of $4,200" is stored as `Marlin Core HAS_MEASURE Marlin Core`;
the value stays in the fact text. Three layers implement it.

1. **Prompt (ontology-gated).** `extract_edges.txt` carries two placeholders,
   `{{TASK_ENDPOINT_CLAUSE}}` and `{{SINGLE_ENTITY_RULES}}`. With no ontology they render today's
   text byte-for-byte (pinned by `prompts/testdata/extract_edges_no_ontology.golden.txt`), so the
   free-form prompt and its cassette keys are unchanged (FR-006). Under an ontology they render a
   rule: when a fact states a literal value (amount, price, budget, cost, date, deadline, duration,
   percentage, count, score, version, role or title) about ONE entity and no other listed entity
   takes part, set **both** `source_name` and `target_name` to that entity; never attach it to
   another entity because it appears nearby; if another listed entity genuinely takes part
   ("Tomas Kowalski set Marlin Core's budget at $4,200"), extract the edge between the two as usual.
2. **Ingest admits the sanctioned self-loop.** In `add_episode_with_opts` (`episode.rs`), when
   literal-object mode is on, an edge whose endpoint *names* are equal after `normalize_name`
   survives the pre-lock self-reference filter, the post-salvage filter and Phase C's UUID-level
   guard. It skips the #816 direction check (swapping a self-loop is meaningless, and marking it
   would trade a misattachment for a spurious `signature_mismatch`), is stored with
   `attributes.literal_object = true`, and is counted in `edges_literal_object`.
3. **Ingest-time diagnostic, every mode (FR-007).** An edge between two different entities whose
   fact states a literal value and does not name both endpoints is stored unchanged but marked
   `attributes.suspected_misattachment = true` and counted in `edges_suspected_misattachment`.
   Never dropped, in the spirit of #816's `signature_mismatch`. It only adds metadata, so free-form
   extraction behaviour is unchanged.

**One predicate gates both the prompt and ingest:** `prompts::literal_object_edges_enabled`, true
iff the resolved ontology offers an extractable relation type (the predicate that already decides
whether `<FACT_TYPES>` renders). The two can never disagree. It covers strict and open modes
alike, and an ontology whose relation types are all `extract: false` counts as no ontology.

**What is still dropped.** The exceptions are narrow:

- With no ontology, a name-level self-reference is still always dropped (ADR-0051 §1).
- Differently named endpoints that collapse onto one UUID (dedup merges, the #666 case) are still
  dropped and counted in `edges_dropped_self_loop`, under an ontology too. Name equality is the
  model's signal; a UUID collapse is not.
- Endpoints made equal *by salvage* are still dropped by the post-salvage filter: the filter keeps
  only edges that were already self-references before salvage rewrote them.

**Downstream invariants.** Self-loops were previously impossible, and several paths treated one as
an invariant violation. Each gets the same narrow exception: keep an edge that was *already* a
self-loop, invalidate one a rewrite *creates*.

- Entity merge (`corrections.rs`: `apply_same_as`, `merge_entities_inner`) moves both ends of an
  alias's pre-existing self-loop to the canonical and skips the `(src, dst, name)` dedup for it, since
  a budget and a headcount legitimately share `(A, A, HAS_MEASURE)`. In `merge_entities_inner`, an
  edge that only becomes a self-loop through the merge is still invalidated, as before;
  `apply_same_as` is otherwise unchanged.
- Cross-group rebind (`cross_group.rs`, phase 2) does not invalidate when the actual hop pair was
  already a self-loop, and skips the duplicate check for it.
- The canonicalize direction repair (`canonicalize::plan_direction`) skips self-loops.
- `Conn::get_full_edges_for_entity` returns a self-loop once (as outgoing), not once per hop, so a
  merge does not copy it twice.
- `knowledge_walk` already tolerates self-loops: they produce no frontier (`walk.rs`).

**The FR-007 heuristic and its limits.** "States a literal value" means the fact contains an ASCII
digit or a currency symbol (`$ € £ ¥`). Month names alone are excluded: "March" can be a product,
and the dates this targets carry a year. "Names an endpoint" is a case-insensitive match of the
normalized name, bounded by non-alphanumeric characters ("Ines" does not match "Inesa"). The
heuristic lives in `crates/core/src/literal.rs`, reimplemented rather than shared with `lcg-eval`'s
matcher: core cannot depend on eval, and moving eval's matcher would risk shifting ADR-0819's metric
definitions. False positives (a fact naming an endpoint by a nickname) and negatives (a fact that
glues in the other entity's name without it taking part) are expected, which is why it marks
rather than drops. A value-free role fact ("Lena is the project lead") is not flagged.

## Alternatives considered

- **A value node** ("$4,200" as an entity). An `Entity` row appears in entity search and typing
  (FR-005 forbids it); a new table breaks schema parity with graphiti's `kuzu_driver.py`. Nodes
  named after an attribute ("monthly budget") deduplicate into hubs that join unrelated entities,
  as free-form's "internal audit" ×21 already does.
- **Prompt guidance alone.** It leaves the model nowhere legitimate to put the fact: either
  "stated" falls (FR-003, SC-002), or pins turn into compound facts that name the other entity and
  pass the metric while staying wrong.
- **Re-pointing a detected pin at ingest.** There is no target to re-point to.
- **A new `ExtractedEdge` field** signalling a literal object. It touches ~55 struct-literal sites
  and the tool schema. Name equality carries the same signal with no schema change: the enum
  already allows equal names.
- **A per-relation ontology flag** (`literal_object: true` on `RelationTypeDef`). ~46 struct-literal
  sites, a conditional hash segment (ADR-0032), and an edit to the v3.1 fixture, which lives on an
  unmerged branch. The prompt rule covers every relation and works on v3.1 as it stands.
- **Admitting a self-loop only when its fact contains a value.** It would drop role/title facts
  (`HAS_ROLE`: "the fact text names the role"), which carry no digits.

## Consequences

- Under an ontology, value facts stay on the entity they describe, and `lcg-eval graph-integrity`
  scores a self-loop as stated and not misattached (`score.rs`: the other end is the subject). No
  node is added, so typing, links and direction are untouched.
- Self-loops now exist in stored graphs. Readers that assumed `source ≠ target` must tolerate them;
  the in-repo ones are covered above. A Python consumer reading `knowledge_get_edges_by_group` sees
  the same edge shape with equal endpoint UUIDs.
- Two additive `knowledge_process_chunk` counters (`edges_literal_object`,
  `edges_suspected_misattachment`) and two edge attribute keys (`literal_object`,
  `suspected_misattachment`).
- Ontology-mode prompts change, so any ontology-mode cassette re-keys. Free-form keys do not.
- Graphs extracted before this change are not repaired (out of scope). A repair could re-point
  edges marked `suspected_misattachment` in a later issue if the fix warrants one.

## Related

- [ADR-0051](0051-edge-endpoint-salvage-and-deferred-drop.md) — self-reference filter (amended).
- [ADR-0666](0666-exact-graph-match-and-uuid-self-loop-guard.md) — UUID self-loop guard (amended).
- [ADR-0816](0816-ontology-edge-direction-check-and-repair.md) — the mark-never-drop precedent.
- [ADR-0819](0819-graph-integrity-eval.md) — the measuring instrument.
- [ADR-0824](0824-own-source-id-entity-filter.md) — prompt fix plus deterministic ingest layer.
