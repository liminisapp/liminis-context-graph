---
layout: default
title: Test Providers
---

# Test Providers

**For tests only.** `--embedder test` and `--extractor test` start the real engine with no
embedding sidecar, no LLM and no network. lcg owns the fakes, so a consumer's test suite stops
reverse-engineering lcg's prompts and sidecar protocol, and a prompt rewording inside lcg can no
longer silently break it.

```bash
liminis-context-graph --embedder test --extractor test            # Unix-socket service
liminis-context-graph --mcp-stdio --embedder test --extractor test # MCP over stdio
```

Everything in this page is a **stable contract**: every example below is executed by lcg's own test
suite (`crates/core/tests/test_providers_contract.rs`), so a change that alters documented
behaviour fails lcg's CI.

## Switching it on, and knowing it is on

- **Explicit flags only.** Test providers are never selected from an environment variable, a config
  file, a default or a fallback (for example when a real provider is unreachable). Variables such
  as `LCG_EMBEDDING_URL` or `LCG_EXTRACTION_URL` never turn them on.
- The two flags are independent. `--embedder test` alone uses the built-in embedder and resolves
  the extractor exactly as it does today; `--extractor test` alone does the reverse.
- `--embedder test` is rejected together with `--embedder-uds`/`--embedder-http`; `--extractor test`
  with `--extractor-uds`/`--extractor-http`. Any value other than `test` is rejected. Neither flag
  is valid with `--connect` (the daemon owns the providers: start the daemon with them).
- **Announcement.** `knowledge_status` and `health_check` carry an additive
  `providers` object: `{"embedder": "test" | "real", "extractor": "test" | "real" | "none"}`
  (`"none"` means no extractor is configured). The startup log prints
  `*** TEST PROVIDERS ACTIVE: … — NOT FOR PRODUCTION ***`. A test can assert it is not hitting real
  providers with one `knowledge_status` call. `--version` is unchanged: it prints before any
  provider is chosen.
- `--embedder test` ignores `LCG_EMBEDDING_URL`, `LCG_EMBEDDING_MODEL` and
  `LCG_EMBEDDING_PROFILE` (it logs one line naming whichever are set) and never contacts a sidecar.
  The reranker and query-expansion stages stay unconfigured and report `not_configured`.
- `--extractor test` together with `LCG_RECORD_LLM` or `LCG_REPLAY_LLM` is a startup error: the
  cassette seam records or replaces a *real* provider.

## The test embedder

A hashed bag of words over whole words and character trigrams, L2-normalised.

| Guarantee | Value |
|-----------|-------|
| Dimension | `LCG_EMBEDDING_DIM` if set to a positive integer of at least **8**, else **768**. A value below 8 is raised to 8; `0` or an unparseable value is ignored and 768 is used (silently, with no warning); a value above **16384** is a startup error. The effective dimension is printed in the startup log line. |
| Model identity | `lcg-test-bow` (so a database shared with a real embedder reports `embedding_model_status: "mismatch"`) |
| Length | Every vector is unit length (L2 norm 1) |
| Determinism | Bit-for-bit identical for the same text, across runs, processes and platforms |
| Similarity | Texts that share words are closer (cosine) than texts that share none; distinct texts never collapse to one vector |
| Roles | `query` and `document` embed identically; no instruction prefixes |

How it works, so you can reason about results:

- **Tokens** are maximal runs of ASCII letters and digits and non-ASCII characters. Only ASCII
  characters separate tokens (space, punctuation, symbols), so no Unicode table is consulted; a
  non-ASCII character, punctuation included, is part of its token. Only ASCII letters are
  case-folded (`Alice` = `alice`); other characters are kept verbatim.
- Each *distinct* token contributes one **word feature** (weight 2) and one weight-1 **trigram
  feature** per character trigram of `^token$`. Features are hashed with 64-bit FNV-1a over UTF-8
  bytes into a dimension-sized vector with a ±1 sign, accumulated in exact integer arithmetic, then
  normalised. No floating-point reduction order, no `DefaultHasher`, no locale.
- **Blank text** (empty, whitespace or ASCII punctuation only) yields a fixed, non-zero unit vector, so it
  is never confused with the all-zero "failed embed" record.
- The trigrams are what keep unrelated single-word names far apart even when two word hashes
  collide: unrelated names stay well below the engine's 0.85 entity-merge threshold, while
  `Acme`/`Acmes` still score as related.

The dimension is fixed per database. A database created in test mode with one dimension cannot be
reopened with another.

## The test extractor and fact lines

The extractor derives entities and edges **solely from fact lines** in the chunk text. Text with no
fact lines extracts nothing, and ingestion still succeeds. It implements the extractor interface
directly: no prompt, no HTTP.

A **fact line** is one of:

```text
SRC -> DST: fact text
SRC -[REL_NAME]-> DST: fact text
```

The first form has relation `RELATED_TO`. The second has the named relation.

### Grammar

1. Lines are split on `\n`; a trailing `\r` is stripped (Windows line endings work).
2. Leading whitespace and any run of Markdown prefixes is ignored: `>`, `-`, `*`, `+` (each followed
   by whitespace, except `>`), and numbered markers `1.` / `1)`. Lines inside code fences are
   scanned too; the fence marker lines themselves never match.
3. The line is split at the first `-[` or `->`. In the named form, `REL_NAME` runs to the next
   `]`, which must be followed immediately by `->`.
4. `DST` and the fact are separated by the **first `:` after the arrow**. Colons and arrows inside
   the fact text are part of the fact. Names may not contain `:`, `->` or `-[`. A line whose
   `SRC` contains `:` (such as prose like `Note: Alice -> Acme: ...`) is ignored, and so is a line
   whose `DST` contains `->` or `-[` (such as `A -> B -> C: ...`).
5. `SRC`, `DST`, the relation and the fact are trimmed.
6. A line is **ignored** (never an error) if `SRC`, `DST` or the fact is empty, if the relation is
   empty after normalisation, or if the named form is malformed.
7. `REL_NAME` is normalised like any extractor's relation type: `SCREAMING_SNAKE_CASE`, splitting
   on `_`, `-`, spaces and camelCase (`hosted on`, `hostedOn` and `HOSTED_ON` all give `HOSTED_ON`).
8. **Entities** are exactly the names appearing in recognised lines, one per distinct name (compared
   case-sensitively), in first-appearance order, with entity type `Entity`. An entity's summary is
   the fact text of the first line that mentions it.
9. **Edges** are emitted in line order. A line repeated exactly (same source, relation, target and
   fact) is emitted once.
10. Only the chunk text is scanned, never document context.

Each example below is a `fact-lines` block followed by the `expect` block it must produce
(`entity: NAME` and `edge: SRC -[RELATION]-> DST : fact`). They are run by the test suite.

A plain line:

```fact-lines
Alice -> Acme: Alice works at Acme
```

```expect
entity: Alice
entity: Acme
edge: Alice -[RELATED_TO]-> Acme : Alice works at Acme
```

A named relation, which lets a test exercise single-valued relations such as `HOSTED_ON`:

```fact-lines
Wiki -[HOSTED_ON]-> ServerA: Wiki runs on ServerA
```

```expect
entity: Wiki
entity: ServerA
edge: Wiki -[HOSTED_ON]-> ServerA : Wiki runs on ServerA
```

Prose, headings and blank lines are ignored; only fact lines contribute, each producing its own
edge:

```fact-lines
# Team notes

Some prose, with a colon: it is not a fact line.
Alice -> Acme: Alice works at Acme
Bob -[REPORTS_TO]-> Alice: Bob reports to Alice
```

```expect
entity: Alice
entity: Acme
entity: Bob
edge: Alice -[RELATED_TO]-> Acme : Alice works at Acme
edge: Bob -[REPORTS_TO]-> Alice : Bob reports to Alice
```

A source containing a colon is not a fact line, so prose with a colon in front of an arrow never
creates a surprising entity:

```fact-lines
Note: Alice -> Acme: works at
```

```expect
```

A destination containing an arrow is not a fact line either, so a chain such as a diagram never
creates an entity named after arrow text:

```fact-lines
Alice -> Acme -> Bob: works at
```

```expect
```

Text with no fact lines extracts nothing:

```fact-lines
Just a paragraph of prose.
Nothing here looks like a fact.
```

```expect
```

Whitespace around names, the arrow and the colon is trimmed, and the relation is normalised:

```fact-lines
   A   -[ hosted on ]->   B   :   some fact
```

```expect
entity: A
entity: B
edge: A -[HOSTED_ON]-> B : some fact
```

List items, block quotes and numbered lists are unwrapped:

```fact-lines
- A -> B: from a bullet
> C -> D: from a quote
1. E -> F: from a numbered list
> - G -> H: from a quoted bullet
```

```expect
entity: A
entity: B
entity: C
entity: D
entity: E
entity: F
entity: G
entity: H
edge: A -[RELATED_TO]-> B : from a bullet
edge: C -[RELATED_TO]-> D : from a quote
edge: E -[RELATED_TO]-> F : from a numbered list
edge: G -[RELATED_TO]-> H : from a quoted bullet
```

The first `:` after the arrow splits the fact; later colons and arrows belong to the fact:

```fact-lines
A -> B: ratio 3:1 via x -> y
```

```expect
entity: A
entity: B
edge: A -[RELATED_TO]-> B : ratio 3:1 via x -> y
```

Malformed lines (empty source, destination, fact or relation, or a broken named form) are ignored:

```fact-lines
-> B: no source
A -> : no destination
A -> B:
A -[]-> B: empty relation
A -[REL B: unclosed relation
```

```expect
```

A repeated line is emitted once; a different fact for the same pair is a separate edge:

```fact-lines
A -> B: x
A -> B: x
A -> B: y
```

```expect
entity: A
entity: B
edge: A -[RELATED_TO]-> B : x
edge: A -[RELATED_TO]-> B : y
```

Names are case-sensitive in the extractor and may be non-ASCII:

```fact-lines
alice -> Zoë: x
Alice -> Zoë: x
```

```expect
entity: alice
entity: Zoë
entity: Alice
edge: alice -[RELATED_TO]-> Zoë : x
edge: Alice -[RELATED_TO]-> Zoë : x
```

A self-edge is emitted by the extractor (but see the next section: the engine drops it at ingest):

```fact-lines
A -> A: refers to itself
```

```expect
entity: A
edge: A -[RELATED_TO]-> A : refers to itself
```

### Other extractor operations

All deterministic and offline, so features that call the extractor beyond `extract` work in test
mode:

- **Entity-type and relation-type classification** abstain (an empty string per item), as the
  existing mock does: nothing is reclassified.
- **Summary consolidation** returns the existing summary, the incoming one if there is no existing
  text, the existing one if it already contains the incoming text, otherwise `existing incoming`.
- **Duplicate judging** is conservative: two names are duplicates only if they are equal after
  trimming and ASCII-lowercasing; everything else is distinct. It never errors.

## What the engine still does with the output

The test extractor produces the same shape a real one does, so the engine's own rules apply. These
are engine behaviours, not extractor ones, and they are the ones consumer tests trip over:

- **Entity resolution is name-first and case-insensitive.** `Alice` and `alice` in one group and
  kind resolve to one entity, even though the extractor keeps them as two names. The same fact in
  two documents therefore resolves to the same entities (one entity per name). The engine keeps
  one edge record per document, both linking the same entity pair.
- **Embedding-based resolution** compares name embeddings at cosine 0.85. Short unrelated names
  stay far below it (`Server A`/`Server B` is about 0.5), so unrelated entities are not merged.
- **Self-edges** (`A -> A: …`) are dropped by the engine's ingest filter even though the extractor
  emits them.
- **Ontology.** Entities are typed `Entity`, which a strict ontology leaves unchanged. A relation
  name outside a strict ontology's vocabulary is reclassified by the engine to `UNCLASSIFIED`.
- **Single-valued relations** are a per-request parameter of `knowledge_lint`
  (`single_valued_relations: ["HOSTED_ON"]`), not an ontology declaration. Ingest two documents
  giving `Wiki` different `HOSTED_ON` destinations, then lint with that parameter, to exercise the
  contradiction category exactly as with a real extractor.
