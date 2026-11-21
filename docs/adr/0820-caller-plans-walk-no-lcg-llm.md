# ADR-0820: Caller-Planned Typed Walk: LCG Makes No LLM Call

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #820 (E13 probe: #815; edge direction: #816)

## Context

`knowledge_graph_search` uses the graph only to gather candidates and then flattens them into ranked
passages; `knowledge_get_entity_neighbors` goes one hop without ranking. Neither answers a question
that is a chain, list or join. The E13 probe (#815) measured an LLM-planned typed walk at 0.91 on
the v2 graph questions (0.94 on the test half) against 0.37 for the best LCG passage search and
0.33 for `qmd`. The walk existed only as probe Python, so no LCG client could use it.

The probe's planner was an LLM that read the question and the ontology's relation menu and chose,
per hop, relation types, a direction and an answer type. Something has to make that plan.

## Decision

1. **The caller plans; LCG executes.** `knowledge_get_ontology` returns the menu and
   `knowledge_walk` runs the plan it is given. **LCG makes no LLM call** in either method. The only
   model use is embedding the hop phrases and `query` with the existing embedder.
2. **The tool descriptions are the planning prompt.** The MCP description tells the agent to call
   `knowledge_get_ontology` once, pick start / relation types / direction (as the type name reads)
   / answer type, and fall back to `knowledge_search_passages` for how-did-it-change questions or
   questions with no clear start entity (E13's planner routed history to passages, which scored 1.00).
3. **E13's v5 algorithm, kept generic.** No relation names or paths are hard-coded; direction is a
   soft 0.05 cosine bonus or a hard filter; the answer-type filter and its fallbacks are reported.
4. **The completion hop is an edge hop.** When the planned hops end short of an entity-type
   `answer_type`, one more hop takes edges from the frontier to entities of that type, ranked by
   `query` against the edge fact embedding. This reuses the edge fetch and the per-hop response
   shape and needs no entity-embedding scan. It reads the issue's "ranked by `query`" as an edge
   ranking; if the maintainer's parity run against the probe diverges, this is the likeliest cause.
5. **An unknown `answer_type` is a no-match, not an error.** The type filter falls back and says so.
   Free-form groups have no declared entity types and must stay walkable.
6. **Read-only and bounded.** A dedicated `get_walk_edges` read (group filtered, no `LIMIT`, fact
   embeddings included) is used because `get_entity_neighbors` truncates and omits embeddings, which
   would silently drop candidates. Ties break by edge uuid so results are deterministic.
7. **"Live" is `invalid_at IS NULL`.** `history` mode includes invalidated edges. Belief-time
   parameters (ADR-0705) are not exposed; they can be added later.

## Alternatives considered

- **An LCG-side planner (an LLM inside LCG).** Rejected: it adds a model dependency, cost and
  latency to a retrieval service and duplicates the reasoning the calling agent already does with the
  question in context. LCG stays model-free on the read path (as with `queries` in ADR-0802).
- **Folding the walk into `knowledge_search_passages`.** Out of scope; the walk returns typed facts,
  not passages.

## Consequences

- Any MCP or IPC client with an LLM can use the walk. Plan quality depends on the caller.
- Parity with the probe (at least 95% identical edges, reader score within 0.02 of 0.91) is
  verified by the maintainer against E13's recorded plans; no automated gate ships with this change.
- `knowledge_get_ontology` builds its view in one function and emits the declared `source_type`/`target_type`
  lists, `inverse_aliases` and `symmetric` (#816).
- Hub entities cost proportionally per hop (every incident edge's fact embedding is read).
