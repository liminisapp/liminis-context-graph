### Changed

- **Graph-question gold answers include facts the v2 prose states (#907, ADR-0907).** The LLM-rendered
  corpus introduced two project owners the skeleton lacks, so a correct extraction was marked wrong
  (g109 scored 0.67 for `[Orbit Core, Telemetry clean-up]`). A new fixture
  `crates/eval/fixtures/retrieval-v2/prose-facts.json` records them; `retrieval-gen` folds a `stated`
  fact into the gold and lists an `ambiguous` one under a new optional, additive `optional_answers`
  field, which `lcg-eval graph-qa` excludes from both precision and recall for `set` questions.
  `graph-questions.jsonl` is regenerated: only g072, g109, i013 and i014 change (ids, text and splits
  do not), and the question-set version is now `meridian-questions-4`, so **graph-qa scores from
  before and after are not comparable on those four questions**. The corpus and cassette are
  unchanged. Optional answers match by normalised equality (a fragment such as "Routing" still costs
  precision), per-question graph-qa report rows list `optional_answers` where there are some, and
  `questions.meta.json` records a `prose_facts_hash` that `retrieval-gen --check` verifies.
