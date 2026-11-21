# E13 probe files (maintainer hand-off for #821)

Reference material, not for merging as is. #821 ports what it needs into `lcg-eval graph-qa`.

| file | what it is |
|---|---|
| `ontology-v3.1.yaml` | The 27-relation ontology the v2 corpus was extracted under for E13 (pre-#816 schema). |
| `ontology-v3.2.yaml` | v3.1 plus #816's signature lists and `inverse_aliases`, used for the step-5 repair. |
| `gen-graph-questions.py` | Generates the graph question set from the fixture skeleton. |
| `graph-questions-v2.jsonl` | The 129 questions it produced (chain-2, chain-3, list, join, history), with gold answers. |
| `split-dev.json` | Question ids in the dev half; the rest is the test half. |
| `answer-eval.py` | The probe. Planner prompts are `PLAN_SYS` / `PLAN2_SYS` / `PLAN3_SYS` (v5 uses `PLAN3_SYS` + `PLAN3_SCHEMA`); the reader is `READ_SYS` + `READ_SCHEMA`; scoring is `score()`; the v5 walk is `walk3(..., soft_dir=True)`, now shipped as `knowledge_walk`. Route `route-planned5` = passages when the plan routes to passages, else the walk's typed facts. |
| `v5-plans.json` | The recorded v5 plans (`plans3`) keyed by question id, plus the per-question `route-planned5` scores (mean 0.915; test half 0.940). Plan shape: `{route, start, hops: [{types, phrase, direction}], list, history, answer_type}`. Mapping to `knowledge_walk`: `types`→`relation_types`, `mode` = history / list / single, `direction_mode: soft`. |
| `llm.py`, `traverse_lib.py` | Helpers: headless Claude calls with a disk cache; the BGE query prefix and embedder client. |

Planner and reader: `claude-haiku-4-5-20251001`. Passages for the passages route: the shipped default `knowledge_search_passages`, top 10.
