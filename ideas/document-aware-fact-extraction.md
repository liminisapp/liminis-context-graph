# Document-aware fact extraction: chunks, whole documents, and reconciliation

**Status:** Architectural exploration  
**Related:** [Evidence-backed observations and mental models](derived-observations-and-mental-models.md)  
**PR:** #731

## Thesis

**Keep chunk-level extraction as an LCG primitive, but stop treating an independently processed chunk as a sufficient unit of document understanding.** Introduce a higher-level, document-aware ingestion workflow that preserves document structure, supplies global context to local extraction, and reconciles candidate assertions before publication.

The distinction is between the **unit of evidence** (often a passage or chunk) and the **unit of interpretation** (often the whole document). Neither should be forced to serve both roles.

This is a proposal to evaluate, not a claim that a hybrid pipeline always beats whole-document extraction.

## Why the current approach is reasonable—and where it struggles

LCG currently exposes `knowledge_process_chunk`, with caller-controlled chunking and a recommended 8,000-character advisory maximum. Each ingested chunk becomes a timestamped episode linked to extracted entities and relationships. The resulting assertions have useful episode provenance, and the engine already distinguishes chunk correction from document supersession.

That is a good low-level contract:
- Bounded extraction inputs are tractable for models with varying context windows and capabilities.
- Chunks are independently ingestible, searchable, correctable, and parallelizable.
- Evidence can be tied to a relatively small source passage.
- Re-ingesting changed chunks avoids reprocessing an entire large document.

But a chunk is not necessarily semantically self-contained. A model reading one section may miss:
- The document's overall purpose, status, authoritativeness, and time frame.
- Definitions, aliases, and referents introduced elsewhere.
- Whether a statement is a proposal, an option considered, a rejected alternative, or the final decision.
- Qualifications, exceptions, and negations in other sections.
- Tables or lists whose headings and surrounding text establish their meaning.
- Cross-section relationships and document-level conclusions.

Example: an ADR proposes PostgreSQL early, evaluates alternatives in the middle, rejects PostgreSQL later, and finally decides on DynamoDB. Independent extraction can correctly detect all four statements but incorrectly promote a rejected option to a current architectural fact.

## Three strategies to compare

| Strategy | Advantages | Risks |
| --- | --- | --- |
| Independent chunk extraction | Parallel, incremental, relatively precise provenance | Misses document-level context and cross-section reconciliation |
| Whole-document extraction | Sees global structure, decisions, and references | Can omit local details in long inputs; weaker span grounding; costly full reprocessing |
| Document-aware hybrid | Global interpretation with local evidence and reconciliation | More orchestration, potentially more LLM calls and latency |

A large context window does **not** guarantee high-recall extraction across a long document. Conversely, short documents may be best processed in one pass. Prefer an adaptive strategy based on document length, structure, complexity, and evaluation results.

## Proposed pipeline

```text
Source document / revision
  |
  v
Document registration and structural parsing
  |  identity, metadata, sections, headings, offsets, tables
  v
Document interpretation (global pass)
  |  purpose, status, terminology, entity aliases, chronology,
  |  section roles, candidate decisions and their scopes
  v
Semantic segmentation
  |  coherent passages with section path, offsets and
  |  compact document context
  v
Context-aware chunk extraction (parallel where useful)
  |  candidate assertions with exact evidence spans
  v
Document-scoped reconciliation
  |  deduplicate, resolve references, reconcile cross-section
  |  status/negation/qualification, check source support
  v
Validated assertion set
  |
  v
Atomic or staged publication to LCG WAL / derived graph
  |  episodes retain precise passage provenance
  v
Cross-document observations and mental models (separate layer)
```

### 1. First-class document identity

Represent a document as a source object, not merely an informal collection of chunk episodes.

Illustrative metadata:
- `document_id`, `revision_id`, `group_id`, source URI/path, content digest.
- Document type (ADR, specification, meeting notes, policy, etc.), if known.
- Sections with stable IDs, heading paths, and character/byte offsets.
- Recorded/ingested timestamps; explicit effective dates only when sourced.
- Supersession lineage and source metadata.
- Segmentation strategy and extractor/reconciler versions.

Distinguish **document revision** (a new version of the same source) from **document supersession** (a separate decision or policy replacing another). Do not infer one from the other.

### 2. Document interpretation

Before extraction, optionally run a document-level pass that identifies:
- Scope, authority, and whether the document is draft/final/historical.
- Glossary, named entities, aliases, and unresolved references.
- Section roles such as background, alternatives, decision, rationale, or open questions.
- Cross-section chronology and qualifications.
- A compact, explicitly *advisory* context summary for local extraction.

The global pass should not silently create authoritative graph facts. Its output guides candidate extraction and must be verified against source spans.

For very large documents, hierarchical section summaries or a document map can substitute for a single whole-document model call.

### 3. Structure-aware segmentation

Prefer semantic boundaries (headings, paragraphs, list groups, tables) over arbitrary character cuts. Preserve:
- Parent document and revision IDs.
- Section hierarchy and source offsets.
- A small amount of overlap only when needed.
- Table headers, footnotes, and nearby definitions.
- Stable chunk identity where possible to support incremental re-ingestion.

Avoid treating overlapping copies as independent evidence. A source span should have stable identity independently of how many extraction windows include it.

### 4. Candidate extraction with grounding

Extract assertions from chunks while supplying the document map, relevant glossary, and section role. Require candidate records to include:
- Subject, predicate, object or a normalized claim.
- Source span(s), ideally offsets or exact excerpts with verification.
- Assertion modality/status: asserted, proposed, rejected, hypothetical, historical, uncertain.
- Temporal and scope qualifiers where explicitly present.
- Extractor/model version and any unresolved references.

**Candidate assertions are not yet published beliefs.** Reject or flag claims whose source span cannot be verified. Document-level assertions may cite multiple spans.

### 5. Document-scoped reconciliation

Reconcile candidates across the complete document:
- Resolve pronouns, aliases, and section-local terms.
- Merge equivalent claims without losing independent supporting spans.
- Distinguish an option being discussed from the final accepted decision.
- Retain genuine contradictions or unresolved ambiguity instead of forcing agreement.
- Verify negation, scope, chronology, and effective-date interpretation.
- Flag unsupported inferences and source-interpretation errors.

Reconciliation is not the same as cross-document observation generation. Its task is to determine **what this document actually asserts**, not to infer broader preferences or trends from several documents.

### 6. Publication, corrections, and atomicity

A document-scoped extraction run should have an explicit lifecycle: prepared → reconciled → committed (or failed). Prefer an atomic or staged publication mechanism so readers do not observe a half-updated document.

Respect LCG's WAL-first architecture:
- Accepted mutations must be appended to the authoritative WAL before updating the derived graph.
- Preserve episode-to-assertion provenance, including source span and document revision.
- When a document revision changes, compare old and new accepted assertion sets.
- Preserve correction semantics: corrected-away evidence stops being believed without erasing history.
- Preserve explicit document supersession semantics and its independent effective time.
- A failed extraction/reconciliation must not retire previously believed assertions.

The existing `knowledge_process_chunk` remains available for streaming text, conversational turns, and simple ingestion. The proposed document workflow is a higher-level orchestrator, not a breaking replacement.

**Open implementation question:** should publication be a new document-level WAL transaction/event, or can existing episode and correction operations be safely composed with a commit boundary? Evaluate crash recovery and WAL replay semantics before choosing.

### 7. Incremental processing

On revision, diff the document structure and spans. Re-extract changed semantic units and any units affected by changed global context. Reconcile at document scope, even if only a few chunks changed. Reuse unaffected evidence where safe.

A changed glossary, decision section, or global interpretation may invalidate otherwise unchanged local extractions; content hashes alone are insufficient for dependency tracking.

## API sketches (exploratory)

- `knowledge_register_document` — identify a document and revision, including structural metadata.
- `knowledge_prepare_document` — interpret and segment without publishing assertions.
- `knowledge_extract_document_candidates` — extract grounded candidates, possibly incrementally.
- `knowledge_reconcile_document` — produce a coherent, evidence-backed candidate set.
- `knowledge_commit_document` — publish an accepted set with a defined transaction boundary.
- `knowledge_get_document_extraction` — inspect provenance, candidate decisions, versions, and diagnostics.

These need not all become public RPC endpoints. The first implementation could be a library/client orchestrator over existing APIs plus a minimal publication primitive.

## Evaluation plan

Use 20–30 representative documents, including ADRs, specifications, meeting notes, policies, and short informal notes. Compare:
1. **Chunk-only baseline:** current `knowledge_process_chunk` workflow.
2. **Whole-document:** one extraction pass with required span attribution.
3. **Hybrid:** global interpretation, contextual chunk extraction, document reconciliation.

Build a manually adjudicated reference set and measure:
- **Fact recall:** including details mentioned only once.
- **Precision and status correctness:** especially proposed vs rejected vs accepted decisions.
- **Provenance accuracy:** correct source passages, not merely plausible citations.
- **Cross-section reasoning:** references, exceptions, and chronology.
- **Revision correctness:** corrected chunks, document revision, and explicit supersession.
- **Operational cost:** tokens, latency, throughput, and incremental reprocessing work.

Test adversarial cases: decisions reversed later in a document, repeated text, conflicting sections, changed definitions, long tables, and a document that fits the context window but has many dispersed facts.

Hypothesis: the hybrid will improve accuracy for structured technical documents; whole-document extraction may be adequate and cheaper for short documents. Let measured quality/cost choose the strategy.

## Relationship to derived observations

The companion [observations proposal](derived-observations-and-mental-models.md) starts **after** source-grounded assertion publication.

- **Document reconciliation:** what does this source assert, and what evidence supports that reading?
- **Observation generation:** what can be inferred from multiple assertions or documents?
- **Mental model refresh:** what synthesized answer is currently supported?
- **Context assembly:** what should an agent see for a particular task?

Keeping these separate prevents extraction mistakes from being promoted into seemingly authoritative inferred knowledge.

## Architectural recommendation

**Introduce a first-class document/revision concept and a document-scoped extraction-and-reconciliation workflow. Retain chunk-level episodes as evidence units and `knowledge_process_chunk` as a low-level primitive.** Benchmark chunk-only, whole-document, and hybrid extraction before committing to a more complex production pipeline.
