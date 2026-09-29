# Graph Report - issue-615  (2026-09-29)

## Corpus Check
- 625 files · ~1,346,421 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 10325 nodes · 20107 edges · 602 communities (580 shown, 22 thin omitted)
- Extraction: 98% EXTRACTED · 2% INFERRED · 0% AMBIGUOUS · INFERRED: 390 edges (avg confidence: 0.79)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `198d13f3`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- migration.rs
- adr/index.md
- embedder_transport.rs
- args
- cassette.rs
- wal_group.rs
- judge.rs
- checkpoint.rs
- runner.rs
- db.rs
- Conn<'db>
- ExtractOptions
- handlers_wal_admin.rs
- ontology.rs
- handlers.rs
- corrections.rs
- recovery.rs
- embedding_cache.rs
- ipc_parity.rs
- IpcRequest
- make_db
- report.rs
- ontology_integration.rs
- makeApp
- AppState
- ontology_sidecar.rs
- transport.rs
- Extraction-quality eval — local-LLM model selection (2026-04)
- McpClient
- wal.rs
- tests/group_purge.rs
- extractor.rs
- assert_ok_resp
- pairwise.rs
- tests/assert.rs
- Extractor
- cassette_record_replay.rs
- Native Rust Embedder Spike: candle vs ort — Decision Report
- SetupCacheTests
- replay.rs
- canonicalize_integration.rs
- Sendable
- types.rs
- wal_generation_reset.rs
- judge_cache.rs
- test_hybrid_llm_integration.py
- .query_params
- kind_identity.rs
- cross_group_pointers.rs
- real_corpus_e2e.rs
- extraction_failures.rs
- failure_taxonomy.rs
- scoring.rs
- mcp_attached.rs
- common.sh
- candle-bench/src/main.rs
- canonicalize.rs
- EntityRow
- WalWriter
- telemetry.rs
- extraction_quality.rs
- per_group_ontology.rs
- src/backend.rs
- scripts
- concurrent_rw_integration.rs
- LocalInferenceTests.swift
- ExtractedEntity
- lbug_extension_home.rs
- pointer.rs
- prompts/mod.rs
- wal_exec.rs
- tier1c_deletion.rs
- wal_population.rs
- wal_strip_embeddings.rs
- .replay_opts
- TelemetrySink
- plan.rs
- mcp_real_corpus_admin_lifecycle_e2e.rs
- Architecture Decisions
- Setup Guide: native-embedder spike
- .makeApp
- Tasks: [FEATURE NAME]
- setup_bench_db_n
- PairwiseVerdict
- speckit-analyze/SKILL.md
- merge_entities
- String
- core/src/embedder.rs
- error.rs
- Ontology
- reprocess_relations.rs
- cancel_shutdown.rs
- extractor_transport.rs
- binary_path
- Embedder
- wal_appender.rs
- eval/src/corpus.rs
- Implementation Plan: Issue #6 — Telemetry and Operator Visibility
- Feature Specification: Integration Architecture — In-Process MCP + Direct Socket Client + App-Bundled Binary
- handleEmbeddings
- Conn
- Db
- cross_group_incremental_replay.rs
- dedup_auto_heal_integration.rs
- migration_binary.rs
- User Scenarios & Testing *(mandatory)*
- Issue #6 Spec: Telemetry and Operator Visibility
- .call_tool
- backfill.rs
- Path
- sync-docs.mjs
- Implementation Plan: Issue #3 — WAL Parity for Git-Friendly Persistence
- Feature Specification: WAL checkpoints — named recovery positions stored in the WAL directory
- .new
- Decision
- CoreMLEmbeddingActor
- Architecture Decisions
- String
- legacy_wal.rs
- backfill_wal.rs
- lookup_key_index.rs
- summary_semantic_search.rs
- AttachedBackend
- tools.rs
- Decision
- Implementation Plan: Issue #1 — Foundation
- Tasks: Issue #3 — WAL Parity for Git-Friendly Persistence
- User Scenarios & Testing *(mandatory)*
- Feature Specification: Tier 2 WAL Admin — prepare_checkpoint, rebuild_from_wal, rebuild_status
- add_episode
- WalLine
- dispatch_val
- ipc_response_shapes.rs
- test-scripts.sh
- Event Types
- Feature Specification: Tier 1c — Deletion Methods (delete_by_source, delete_chunk_episode, clear_all)
- Feature Specification: `knowledge_status` errors instead of reporting degraded state when a core table is missing
- Feature Specification: Persist and expose an applied WAL sequence in knowledge_status
- .new
- cross_group.rs
- Error
- String
- metrics.rs
- ADR-0038: In-Process NameIndex Accelerator for Entity Name Lookup
- Decision
- Decision
- Liminis Context Graph
- Research Findings: Issue #6 — Telemetry and Operator Visibility
- Feature Specification: Relation Canonicalization — Map Free-Text Relation Names to Controlled `relation_type` (Two-Layer; Drop Co-occurrence Noise)
- Feature Specification: Record/Replay LLM Cassette for Extraction
- Feature Specification: MCP Write/Mutation-Path E2E Suite Over the Real-Corpus Fixture
- Feature Specification: CI: build release artifacts once and reuse them across the five e2e jobs
- Feature Specification: Multi-stream WAL: one WAL directory per group
- Feature Specification: `applied_seq` never advances for `wal_flush_ungrouped` writes
- Feature Specification: WAL Stream Generation Identity
- Feature Specification: Group-Scope `canonicalize_relations` and `backfill_relation_types`
- User Scenarios & Testing *(mandatory)*
- Feature Specification: Kind-scoped entity identity
- backfill_summary_embeddings_wal.rs
- edge_endpoint_resolution.rs
- make_entity_line
- wal_root_migration.rs
- BoxFuture
- Decision
- Decision
- MCP-over-stdio transport
- LocalInferenceMode
- Feature Specification: OSS Launch Scaffolding — LICENSE, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, CHANGELOG, README Polish
- Feature Specification: Fix FTS Query Syntax for lbug 0.16.1
- Feature Specification: Audit — Route All Direct-Write Paths Through Shared Type Coercion
- Feature Specification: Ontology Subtype/Parent Support — Hierarchical Entity Types via Additive Multi-Labeling
- Feature Specification: Relations Must Consistently Carry a Semantic `relation_type` (Extractor Fix + Additive Backfill; Never Delete Arrow Edges)
- Feature Specification: Decide the max_tokens Policy and Edge Budget-Exhaustion Semantics
- Feature Specification: A missing `summary` field discards the entire chunk, and is misreported as malformed JSON
- Feature Specification: docs-only changes pay a full 15–18 minute Rust CI cycle
- Feature Specification: run `real-corpus-e2e` on the PR path as a non-required check
- Feature Specification: Per-Group Ontology Drift Detection
- Feature Specification: MCP client config recipes for pointing the embedder elsewhere
- Feature Specification: Bundle lbug vector/fts extensions with the release so startup never downloads from the CDN
- Feature Specification: Optional Ontology Support — Workspace-Scoped Entity and Edge Vocabularies that Guide Extraction
- clean_shutdown.rs
- capture_real_corpus.py
- attempt_uds_send
- .log_mutation
- .new
- Golden Real-Corpus WAL Fixture
- Issue #1 Spec: Foundation — Cargo Scaffold, CI, LadybugDB Spike, Library+Binary Symmetry
- Feature Specification: Application WAL Not Written After Recreate — Regression of #74
- Feature Specification: Prebuilt-Binary Release Workflow via cargo-dist
- Feature Specification: WAL Replay: Fix Apostrophe-Induced Parse Failures and Cascading Binder Errors
- Feature Specification: Autonomous WAL-Corruption Self-Recovery
- Feature Specification: Cross-Episode Entity Resolution — Fix Identical-Name Duplication at Ingest
- Feature Specification: Extend `reprocess_entity_types` — Off-Ontology and Full-Graph Re-typing with Additive Labels
- Feature Specification: Native MCP-over-stdio transport for the liminis-context-graph binary
- Feature Specification: Add `knowledge_reprocess_relation_types` — Fact-Based LLM Relation Classification
- Feature Specification: Local/OpenAI-Compatible Extraction Adapter — Make the "Fully Local" Promise True
- Feature Specification: Replace In-Process Entity-Name Lookup Map with a Secondary ART Index
- Feature Specification: Rust extraction-quality eval harness (replay + LLM-as-judge) over the public Wikipedia corpus
- Feature Specification: WAL replay transaction boundaries — defined recovery state on failure and cancellation
- Feature Specification: Benchmark run — full-corpus extraction on Anthropic vs local qwen3.6-27b, capturing cassettes for both
- Feature Specification: Ontology-aware extraction-quality evaluation
- Feature Specification: Tier 1b — Inventory + Semantic Search + Neighbors
- Feature Specification: Constrain edge endpoints to the extracted entity set, and stop banning the concepts edges hub on
- Feature Specification: GitHub Pages documentation site
- Feature Specification: `real-corpus-e2e` failed 24 consecutive runs over 4 days with no signal to anyone
- Feature Specification: Strict mode still deletes out-of-vocabulary entities, while edges are now preserved
- Feature Specification: validate the extraction provider on first use, not at startup
- Feature Specification: Include `breakdown` in `knowledge_reprocess_relation_types`' apply response
- Feature Specification: Attribute delete_by_group and rebind_pointers WAL mutations to the groups they actually modify
- Feature Specification: Extend the Multi-Stream E2E Test to Cover Generation Reset, Cross-Group Merge and Ambiguous Resolution
- Feature Specification: `knowledge_delete_chunk_episode` and `knowledge_delete_by_source` must require an explicit group scope
- Feature Specification: `knowledge_process_chunk` — warn, document, and count oversized `chunk_text`
- Feature Specification: Consistent Omitted-`group_ids` Semantics Across MCP Read Tools
- Feature Specification: Fix four tests failing on `main` behind the masked CI gate
- Feature Specification: Legacy WAL migration does not stamp .wal-generation.json
- Feature Specification: Recompute embeddings on WAL replay with a content-addressed cache
- Feature Specification: Close the 0.13.x documentation drift (breaking change, group_ids contract, assertion API, multi-graph entry points)
- Feature Specification: Publish the docs site from release tags, not main, and keep every released version available
- Feature Specification: Batch WAL-replay embedding recompute calls
- Feature Specification: Reconsider fatal embedder-unreachable startup failure for MCP-launched mode
- Feature Specification: `--mcp-stdio`: complete the handshake without dialling, then dial `--connect` lazily per call
- Feature Specification: WAL File Rotation by Size and Entry Count
- Feature Specification: Port Graphiti's Extraction Prompts to liminis-graph for Quality Parity (Ontology-Aware)
- Feature Specification: Add `relation_type` Field to Edges — Separate Normalized SCREAMING_SNAKE_CASE Predicate Alongside `fact` Paraphrase
- handlers_wal_dump.rs
- real_corpus_replay_perf.rs
- spawn_stub_auth_embedder
- mcp_multistream_e2e.rs
- mcp_real_corpus_mutation_e2e.rs
- Decisions
- buildRouter
- Tasks: Issue #2 — IPC Parity (US1)
- Issue #4 Spec: Concurrent Reader/Writer with Per-Role LLM Routing
- Tasks: Issue #4 — Concurrent Reader/Writer with Per-Role LLM Routing
- Architecture Decisions
- Feature Specification: Safe `.graphiti/` → `.lcg/` Workspace Migration That Restructures File Layout
- Feature Specification: WAL Replay Timestamp Typing — Fix STRING→TIMESTAMP Cast Failures
- Feature Specification: Publish Versioned macOS Release Binary for liminis-context-graph
- Feature Specification: Batch WAL Replay Writes via UNWIND for Throughput
- Feature Specification: Reload owns index maintenance — drop FTS before WAL replay, build all indexes once after
- Feature Specification: knowledge_dump_wal — DB→WAL Dump / Compaction
- Feature Specification: knowledge_merge_entities — Collapse Duplicate Entities
- Feature Specification: Fix knowledge_merge_entities TIMESTAMP Coercion Bug
- Feature Specification: Eager HNSW Index Build + Dedup-Path Auto-Heal to Fix Missing-Index Ingest Failures
- Feature Specification: Restore an Indexed Access Path for Entity Name Lookup
- Feature Specification: Document Extraction-Quality Evaluation — Methodology, Model Rankings, and Local-LLM Guidance
- Feature Specification: UDS embedder dials a new connection + handshake + detached task per embedding call — pool it
- Feature Specification: Fix Silent Data Loss in WAL Replay (Discarded Stats, Out-of-Sequence Files, Uncounted No-Ops)
- Feature Specification: WAL replay diagnostics — deduplicated failure samples, safe rebuild semantics, honest fidelity warnings
- Feature Specification: blind pairwise judging for lcg-eval
- Feature Specification: Tier 1a Service Handshake Methods
- Feature Specification: move the benchmark guards out of shell and into lcg-eval
- Feature Specification: #219's NameIndex silently narrowed #218's global endpoint fallback
- Feature Specification: Capture extraction failures whole, and surface truncation in the eval report
- Feature Specification: Tier 3 — Corrections Workflow (apply_corrections, validate_corrections, reprocess_entity_types)
- Feature Specification: the 15–18 minute release test suite is the root cause behind the headless-stall class
- Feature Specification: Reject semantically-empty required fields during item salvage
- Feature Specification: Group-scoped complete purge: remove entities and edges, not just episodes
- Feature Specification: Group-Scope Duplicate-Edge Detection During Entity Merge
- Feature Specification: Resolvable semantic pointers for cross-graph references
- Feature Specification: Merge must never write another group's data
- Feature Specification: Direct assertion API — `knowledge_assert_entity` / `knowledge_assert_relationship`
- Feature Specification: Tier 1b Bug — Edge-as-Node Schema Mismatch in Relationship Queries
- Feature Specification: Upgrade lbug dependency from 0.17.0 to 0.19.1
- Feature Specification: Correct the documented meaning of edge `episode_uuids`
- Feature Specification: WAL Stream Generation — Publish Contract & Unknown-State Guard
- Feature Specification: Required CI test gate cannot fail — tee without pipefail masks test failures
- Feature Specification: Fix startup migration ordering so legacy WAL files are relocated, not left loose and invisible
- Feature Specification: Embedder Sidecar HTTP Server (BGE bge-base-en-v1.5)
- Feature Specification: Batch embedding API for bulk-extraction ingest and summary-embedding backfill
- Feature Specification: Per-Group Ontology Support
- Feature Specification: Bearer-Token Authentication for the Embedder HTTP Transport
- Feature Specification: Integration Tests Leak Spawned liminis-context-graph Processes
- Feature Specification: Swift sidecar: select embeddings / completions / both, so completions-only costs no model setup
- Feature Specification: Document embedding options and conventions: one capability matrix, local and remote
- Feature Specification: Fix missing `weight.bin` in Swift CoreML test fixtures
- Feature Specification: Vectors are a local cache — stop writing them to the WAL, ignore them on replay
- Feature Specification: Upgrade lbug 0.19.1 -> 0.20.1 (storage 43 -> 47) so 0.14.0 carries a single migration
- Feature Specification: `knowledge_strip_wal_embeddings` — Strip Embedding Vectors from Pre-0.14 WALs
- Feature Specification: Fast Clean Shutdown — Cancel In-Flight LLM Work on SIGTERM Instead of Waiting Out the Inner Timeout
- Feature Specification: liminis-graph speaks OpenAI-compatible embeddings over UDS by default, with HTTP as opt-in
- Feature Specification: Ontology Drift Detection — Notify When the Ontology Has Changed Since the Last Ingestion
- test_capture_real_corpus.py
- wal_vector_stripping.rs
- embedder_degraded_mcp.rs
- Decision
- Decisions
- ADR-0371: Merge Skips Foreign-Group Edges Entirely; `merged_into` Forwarding Closes the Rename Gap
- Feature Specification: Fix Ontology Drift Detection for First-Ever Ontology Addition
- Feature Specification: Spike — native cross-platform Rust embedder for liminis-graph (candle vs ort)
- Feature Specification: WAL Replayer Must Distinguish Failed Mutations From Unrecognised Lines
- Feature Specification: Bump lbug Pin From 0.16.1 to 0.17.0
- Feature Specification: WAL Replay Real-Time Progress — Add `files_total` to `ReplayProgress`
- Feature Specification: Schema Parity — `RelatesToNode_` Missing `expired_at` Column → WAL Replay Drops Expired/Invalidated Relationships
- Feature Specification: Periodically Log WAL-Replay Progress to the Service Log
- Feature Specification: Fix Hybrid Dedup Overlap Failure (R-003)
- Feature Specification: `knowledge_rebuild_from_wal` Must Leave Entity/Relationship Search Immediately Queryable
- Feature Specification: Resolve Edge Endpoints Against the Global Entity Table
- Feature Specification: Lint Sweep — 6 Clippy Errors + 20 fmt Diffs from Rust 1.94
- Feature Specification: Golden Real-Corpus WAL Fixture + Rebuild→Assert E2E Harness
- Feature Specification: OaiExtractor UDS path — adopt pooled connection (same defect as #229)
- Feature Specification: MCP Read-Path E2E Suite Over the Real-Corpus Fixture
- Feature Specification: Bound Prepared-Statement Growth During WAL Replay
- Feature Specification: Capture and Log the Sender PID of Received SIGTERM
- Feature Specification: Cassette replay backend for `lcg-eval`
- Feature Specification: make lcg-eval's scoring loop testable
- Feature Specification: `indices_built` is not set after runtime recovery, so `knowledge_status` under-reports readiness
- Feature Specification: two e2e tests call `knowledge_rebuild_from_wal` without `force_clear` and have failed since the guard landed
- Feature Specification: Strict ontology mode discards relations the ontology knows how to keep
- Feature Specification: Salvage malformed extracted items instead of failing the whole chunk
- Feature Specification: CI — migrate remaining Node 20 actions to Node 24 releases
- Feature Specification: Re-derive `WalWriter` `global_seq` after rebuild/clear to prevent duplicate WAL seqs
- Feature Specification: Bounded WAL replay — `to_seq` upper bound for `knowledge_rebuild_from_wal`
- Feature Specification: Cheap WAL seq bounds — eliminate the full-directory scan in `wal_max_seq`/`wal_min_seq`
- Feature Specification: Rebind Pointers Must Repair Already-Unbound Cross-Group Pointers
- Feature Specification: Embedder Sidecar for liminis-graph
- Feature Specification: Assert handlers compute embeddings before the existence check
- Feature Specification: knowledge_status cannot distinguish an empty group from an unhydrated one
- Feature Specification: Semantic search over Entity summaries
- Feature Specification: Streaming WAL-rebuild's build_ok doesn't reflect a failed lookup_key backfill
- Feature Specification: Close (or formally accept) the resolve_ontology stale-drift-insert race
- Feature Specification: Re-enable Swift Sidecar CI
- Feature Specification: Consolidate the Swift Sidecar — lcg is Source of Truth
- Feature Specification: Bounded Timeouts for `OaiEmbedder`'s UDS Transport
- Feature Specification: Fix TOCTOU port race in `embedder_degraded_mcp.rs` test helpers
- Feature Specification: Fix clean_shutdown Integration Test on macOS (lbug hash_index Assertion)
- Feature Specification: Standardize IPC Collection Responses to `{<key>: [...], count: N}` Envelope Shape
- liminis-context-graph — Claude guidance
- Contributing to Liminis Context Graph
- stage_corpus
- send_and_read_uds
- String
- token_budget.rs
- entity_create_wal_line
- SeededWorkspace
- Decision
- Decision
- ADR-0029: Name-First Entity Resolution in add_episode Phase B
- Decisions
- Decision
- ADR-0322: CI Docs-Only Fast Path via Job-Level Skip
- Decision
- ADR-0402: Per-Group Mutation Attribution for Multi-Group Episode Deletes
- Decision
- Full-corpus extraction benchmark runbook (#248)
- 038B1EE0-2AF2-48F0-B401-E6460DB2A7F8
- A9455776-87AF-40BE-9601-89C752C3866C
- 5ADDBC7B-30D6-4DE2-AC1C-707AA4904044
- 68A9BCBE-6E95-400D-8604-19FCA73C7ABA
- 42571EF2-B78E-43F9-9F0B-EB43ABBD84B6
- Feature Specification: Spike — ort with CoreML execution provider on macOS (can it match or beat the Swift sidecar?)
- Feature Specification: Cache lbug C++ Build Artifacts Across CI Runs to Cut PR Wall-Clock From ~1h to ~15min
- Feature Specification: Legacy-WAL Replay Compatibility — `episodes` Schema Parity + FalkorDB-Dialect Translation (VECF32, bulk-SET)
- Feature Specification: Deprecate `knowledge_backfill_relation_types` In Place — Stop Implying It Classifies
- Feature Specification: Attached MCP Mode — Fix False-Timeout on Long Whole-Graph Ops and Add Reconnect After Service Restart
- Feature Specification: Multi-Stream / Layer-Graph E2E Test
- Feature Specification: Determine and resolve cross-group WAL attribution for multi-group episode deletes
- Feature Specification: Report dropped-edge detail from `knowledge_process_chunk`
- Feature Specification: Socket bind precedes #378 WAL-root migration, so readiness can be observed before migration completes
- Feature Specification: `migrate_workspace` must honor `LCG_WAL_DIR`/`GRAPHITI_WAL_DIR` when relocating legacy `.graphiti/wal`
- Feature Specification: Cover embedder cassette recordings with API-key leak tests
- Feature Specification: Unify relationship/edge response keys across read RPCs
- Feature Specification: Structured Attributes on Episodes
- Feature Specification: Narrow `state.write_lock`'s critical section around the embedder round trip
- Contributor Covenant Code of Conduct
- backfill_summary_embeddings
- core/src/lib.rs
- cross_episode_dedup.rs
- ingest_embed_batching.rs
- StandaloneBackend
- scope.rs
- Decisions
- Decisions
- ADR-0316: One `[[bench]]` Target Per `criterion_group!`
- Decision
- Decision
- Extraction-quality evaluation: methodology, model rankings, and local-LLM guidance
- Extraction-quality eval harness
- FoundationModelsAdapter
- generate-bad-stub-models.py
- Feature Specification: [FEATURE NAME]
- Tasks: Issue #6 — Telemetry and Operator Visibility
- Feature Specification: WAL Replayer Must Accept MATCH-Prefixed Mutation Queries
- Feature Specification: Move Performance Benches Off Per-Push CI to On-Demand Invocation
- Feature Specification: Remove Incorrect 30% Perf-Ratio Assertion from bench_dedup_hybrid_10k
- Feature Specification: Remove broken `LBUG_BUILD_FROM_SOURCE` flag from `bench.yml`
- Feature Specification: CLAUDE.md's long-task guidance names the wrong failure and misses the safe technique
- Feature Specification: OaiEmbedder HTTP transport has no request/connect timeout
- BgeEmbedder
- main
- .new
- wal_manipulate.rs
- bench.py
- ADR-0043: WAL Replay — Seq-Based File Ordering and MATCH-Write No-Op Accounting
- Decisions
- ADR-0325: `knowledge_status` Reports "Open But Not Queryable" as a Second Degraded State
- Decision
- Decision
- Decision
- BgeEmbedder
- Implementation Plan: [FEATURE]
- Tasks: Issue #1 — Foundation
- OaiEmbedder
- compute_unbound_impacts
- IpcResponse
- test_knowledge_process_chunk_multibyte_chars_use_char_count_not_byte_count
- ADR-0014: Pass `Option<&Ontology>` as a call-time parameter to `Extractor::extract`
- ADR-0020: IPC Collection Response Envelope Contract
- Decision
- Decisions
- ADR-0042: OaiExtractor UDS Connection Pooling
- ADR-0298: CI Failure Notification for Non-Gating Workflows
- Decisions
- ADR-0368: Duplicate-Edge Detection During Merge Scopes by the Edge's Own `group_id`, Not the Merge's
- ADR-0375: WAL Seq Bounds Manifest
- Decision
- Decision
- Decision
- ADR-0543: Narrow `state.write_lock`'s Critical Section Around the Embedder Round Trip
- Decision
- Integration Test Fixtures
- Core Principles
- Core Principles
- ArcSwapOption
- BarrierEmbedder
- resolve_entity_by_name
- Parity Test Fixtures
- ldb_spike_ipc.rs
- search_result_order.rs
- mcp_real_corpus_admin_data_e2e.rs
- as_uuid_set
- ADR-0005: Streaming IPC Progress Framing via `_progress_token`
- ADR-0008: Named Multi-Connection Pool for ContextGraphSocketClient
- ADR-0039: UDS Embedder Connection Pooling
- Decisions
- ADR-0045: WAL Replay Prepared-Statement Cache — LRU-1 Scope and Deferred Connection Recycling
- ADR-0046: WAL Replay — Deduplicated Failure Samples and Fail-Fast Rebuild Idempotency
- ADR-0052: `lcg-eval --dry-run` Shares the Real Run's Resolution Path
- Decision
- 0.14.3 — 2026-09-15 (full release notes)
- local-inference — macOS Swift sidecar for liminis-context-graph
- extractJSON
- render-diagrams.mjs
- Changelog
- telemetry_ipc.rs
- ADR-0007: Two-Hop RELATES_TO Traversal as Canonical Read Pattern
- ADR-0015: WAL Drain-and-Flush Pattern for Production Write Handlers
- Decision
- Consequences
- ADR-0036: Eager HNSW/FTS Index Build at Startup + Dedup-Path Auto-Heal
- ADR-0314: Missing-Summary Salvage and `schema_invalid` Classification
- ADR-0347: Reject Semantically-Empty Required Fields During Item Salvage
- ADR-0430: Workflow-Level `shell: bash` to Restore `pipefail` for `| tee` Steps
- ADR-0440: Recompute Embeddings on WAL Replay, With a Sync Bridge and a Two-Mechanism Identity Split
- Decision
- ADR-0502: Pin an Explicit `macos-26` Runner for Swift Sidecar CI, Not Yet a Required Check
- Configuration
- verify-embedding-parity.py
- Tasks: Issue #5 — HNSW + BM25 Hybrid Dedup at Scale
- wal_snapshot
- speckit-checklist/SKILL.md
- speckit-plan/SKILL.md
- speckit-specify/SKILL.md
- speckit-tasks/SKILL.md
- fixture_path
- StderrSink
- ADR-0009: Degraded-Mode Startup and In-Process Recovery
- ADR-0013: CancellationToken as the Single Shutdown Signal on AppState
- ADR-0018: Ontology Hash Sidecar for Drift Detection
- ADR-0025: Auto-Heal Index Build and Bulk-Load Reload Pattern
- ADR-0037: Relation Classification Has No Open-Ended Mode and Abstention Writes `UNCLASSIFIED`
- ADR-0445: Embedder Batch API — Wire-Level Batching, Index-Ordered Reassembly, Chunk-Size Knob
- ADR-0446: Per-Group Ontology Resolution
- ADR-0470: Entity Summary Embedding for Semantic Search
- ADR-0486: Batch WAL-Replay Embedding Recompute — An Independent, Unaligned Window Upstream of the Cypher Batch
- ADR-0503: `liminis-context-graph` Is the Sole Source of Truth for the Swift Sidecar
- ADR-0550: Link OpenSSL dynamically, resolved through `@rpath` on macOS
- ADR-0575: Lazy-Dial the Attached `--connect` Socket by Default, With `--connect-eager` Opt-Out
- Getting Started
- Ontology
- Operations
- Docs publishing
- 0.14.0 — 2026-09-02 (full release notes)
- 0.15.0 — 2026-09-15 (full release notes)
- .makeActor
- check-links.mjs
- concurrent_rw.rs
- make_state_without_indices
- build_db_with_entities
- mcp_clean_shutdown.rs
- ADR-0002: Reader/Writer Split via `tokio::sync::RwLock`
- ADR-0003: `ArcSwap<Db>` for Live Database Replacement in `clear_all`
- ADR-0004: Add `classify_entities` to the `Extractor` trait
- ADR-0021: Inject `LBUG_BUILD_FROM_SOURCE` via cargo-dist `github-build-setup`
- ADR-0026: Episode-Cursor WAL Resume for Checkpoint Recovery
- ADR-0030: Batched Write-Lock Acquisition for Long-Running Passes
- ADR-0040: Attached-Mode Reconnect — Retry Only Write-Time Failures
- ADR-0310: Strict-Mode Relation-Type Filtering Reclassifies, Never Drops
- ADR-0312: Strict-Mode Entity-Type Filtering Reclassifies, Never Drops
- ADR-0398: Link OpenSSL Statically So Release Artifacts Stay Self-Contained
- ADR-0528: Structured Attributes on Episodes (`Episodic.attributes`)
- Embedding Options
- bug_report.md
- Self-describing group graphs: the ontology as graph content
- [0.11.0] - 2026-07-30
- [0.12.0] - 2026-08-04
- [0.13.2] - 2026-08-16
- [0.14.0] - 2026-09-02
- telemetry_overhead.rs
- setup_dedup_db
- register
- ADR-0010: Migrate do_extract to tool_use structured output
- ADR-0011: Auto-Heal Write-Lock Acquisition from Search Handlers
- ADR-0012: Edge-to-Episode Associations via Either-Endpoint Entity Traversal
- ADR-0017: Replace `std::process::exit(0)` with Normal Return in async main
- ADR-0022: lbug Cypher Escaping Convention — Backslash, Not SQL Doubling
- ADR-0023: Legacy-WAL Translation Layer — Cypher-text/Param-shape vs. Param-value Module Split
- ADR-0031: Orphaned Direct RELATES_TO Rels After Noise Edge Deletion
- ADR-0032: Ontology `parent_edges:` segment conditionally included in content hash
- ADR-0033: Noise Edges Are Reclassified to UNCLASSIFIED, Not Deleted
- ADR-0049: Bare-Path Ontology Loader and CLI Mode-Override Precedence
- ADR-0331: Validate the Extraction Provider on First Use, Not at Startup
- ADR-0365: WAL Checkpoints — Directory-Per-Name, Generation-Numbered Exclusive-Create Store
- ADR-0392: `rebind_pointers`'s Staleness Gate Keys on Binding State, Not Only Position
- ADR-0451: Per-Group Ontology Drift — Cache Placement and Clear Scope
- ADR-0495: Close the `resolve_ontology` Stale-Drift-Insert Race via Insert-If-Absent
- ADR-0500: Unified Pre-Bootstrap Signal Handling Across All `CliMode`s
- test-patterns.sh
- feature_request.md
- check-fixture-freshness.sh
- main
- [0.13.3] - 2026-08-22
- speckit-clarify/SKILL.md
- speckit-constitution/SKILL.md
- speckit-taskstoissues/SKILL.md
- main
- WalReplayer
- .embed
- setup_db
- ADR-0001: Record Architecture Decisions
- ADR-0024: Bound-Parameter DB Access — Retire Cypher String Interpolation
- ADR-0051: Edge Endpoint Salvage and Deferred Drop Decision
- ADR-0414: Unknown-Generation Streams Refuse to Advance, Not Warn
- ADR-0581: Link OpenSSL Statically on Windows
- ADR-0615: Kind-Scoped Entity Identity — `Entity.kind` and `lookup_key = group_id ␟ kind ␟ name`
- PULL_REQUEST_TEMPLATE.md
- [CHECKLIST TYPE] Checklist: [FEATURE NAME]
- [0.13.1] - 2026-08-15
- [0.13.4] - 2026-08-24
- [0.14.1] - 2026-09-06
- [0.14.2] - 2026-09-10
- [0.14.3] - 2026-09-15
- [Unreleased]
- speckit-implement/SKILL.md
- socket
- .for_read
- storage_v41_migration.rs
- storage_v42_migration.rs
- test_embedder_ctx
- 06-ontology-matrix.sh
- `lcg-eval` operational scripts
- build_program
- stage-lbug-extensions.sh
- stage-openssl-dlls-windows.sh
- stage-openssl-rpath.sh
- stage-openssl-windows.sh
- [0.13.0] - 2026-08-13
- [0.15.0] - 2026-09-15
- [0.9.0] - 2026-07-13
- placeholder.rs
- wal_replay_bench.rs
- run_mode_matrix.sh
- check-embedding-assets.sh
- convert-embedding-model.py
- refresh-test-fixtures.sh
- docs-publish-build.sh
- generate-docs-llms-full.sh
- count_jsonl_files
- 01-start-server.sh
- 02-timing-check.sh
- 03-capture-qwen.sh
- 04-full-run.sh
- 05-score-only.sh
- Package.swift
- prepare-embedding-assets.sh
- assert-openssl-linkage.sh
- docs-publish-latest-stable-version.sh
- content.config.ts
- run-linux.sh

## God Nodes (most connected - your core abstractions)
1. `AppState` - 215 edges
2. `make_db()` - 141 edges
3. `dispatch_val()` - 138 edges
4. `Db` - 127 edges
5. `Conn<'db>` - 121 edges
6. `assert_ok_resp()` - 114 edges
7. `Conn` - 75 edges
8. `Ontology` - 73 edges
9. `IpcRequest` - 68 edges
10. `handle()` - 55 edges

## Surprising Connections (you probably didn't know these)
- `add_episode()` --calls--> `load_db()`  [INFERRED]
  crates/core/src/episode.rs → crates/core/src/app_state.rs
- `clear_group_for_rebuild()` --calls--> `load_db()`  [INFERRED]
  crates/core/src/handlers.rs → crates/core/src/app_state.rs
- `handle_get_entity_neighbors()` --calls--> `load_db()`  [INFERRED]
  crates/core/src/handlers.rs → crates/core/src/app_state.rs
- `handle_list_relationships()` --calls--> `load_db()`  [INFERRED]
  crates/core/src/handlers.rs → crates/core/src/app_state.rs
- `handle_rebuild_from_wal()` --calls--> `load_db()`  [INFERRED]
  crates/core/src/handlers.rs → crates/core/src/app_state.rs

## Import Cycles
- None detected.

## Communities (602 total, 22 thin omitted)

### Community 0 - "migration.rs"
Cohesion: 0.06
Nodes (82): CliMode, CoreError, errors_when_neither_name_set(), falls_back_to_deprecated_name_when_new_name_unset(), lcg_env_var(), lcg_wins_when_both_set(), Result, String (+74 more)

### Community 2 - "embedder_transport.rs"
Cohesion: 0.07
Nodes (73): assert_oai_request_body(), EmbedBatchSizeEnvGuard, embedder_http_cassette_never_leaks_authorization_or_key(), EmbeddingTimeoutEnvGuard, http_transport_connect_timeout_independent_of_request_timeout(), http_transport_default_batch_size_succeeds_under_normal_latency(), http_transport_embed_batch_chunks_oversized_batch(), http_transport_embed_batch_empty_input_issues_no_request() (+65 more)

### Community 3 - "args"
Cohesion: 0.06
Nodes (69): Args, BackendArg, both_mode_rejects_a_single_backend(), CassetteArg, CliMode, defaults_judge_cache_and_model(), dry_run_defaults_to_false(), dry_run_flag_is_parsed() (+61 more)

### Community 4 - "cassette.rs"
Cohesion: 0.09
Nodes (64): CassetteRecord, CassetteWriter, classify_entities_key_changes_with_allowed_types(), classify_entities_key_order_dependent_but_stable(), classify_entities_request_value(), classify_relations_key_stable_and_sensitive_to_edges(), classify_relations_request_value(), count_records() (+56 more)

### Community 5 - "wal_group.rs"
Cohesion: 0.06
Nodes (69): check_no_case_insensitive_collision(), collision_check_allows_reopening_its_own_existing_directory(), collision_check_allows_unrelated_sibling_names(), collision_check_is_a_noop_for_missing_root(), collision_check_rejects_a_differently_cased_existing_sibling(), decode_group_dir_name(), encode_decode_roundtrip_is_bijective_and_collision_free(), encode_group_dir_name() (+61 more)

### Community 6 - "judge.rs"
Cohesion: 0.05
Nodes (43): a_real_match_wins_over_a_declared_unmatched_entry(), a_real_match_wins_over_a_sentinel_claiming_the_same_index(), AnthropicJudgeClient, circuit_breaker_opens_exactly_at_the_limit(), circuit_breaker_resets_on_any_success(), circuit_breaker_starts_closed_and_stays_closed_below_the_limit(), CircuitBreaker, extract_json_block() (+35 more)

### Community 7 - "checkpoint.rs"
Cohesion: 0.07
Nodes (58): CheckpointInfo, checkpoints_dir(), concurrent_create_of_same_name_exactly_one_wins(), create(), create_after_delete_reuses_the_name_as_a_new_generation(), create_file_path(), create_rejects_duplicate_active_name(), create_rejects_invalid_name_without_touching_disk() (+50 more)

### Community 8 - "runner.rs"
Cohesion: 0.07
Nodes (48): captures_latency_and_results_per_chunk(), counting_sink_ignores_other_events(), counting_sink_tallies_by_outcome(), counting_sink_tallies_truncation_distinguishing_retry_outcome(), counting_sink_truncation_and_structured_output_tallies_are_independent(), CountingSink, edge_with_no_relation_type_counts_as_out_of_vocab(), errors_are_counted_not_dropped() (+40 more)

### Community 9 - "db.rs"
Cohesion: 0.08
Nodes (55): accepts_rfc3339(), accepts_space_format(), apostrophe_string_binds_verbatim(), backslash_is_escaped_before_quote_handling(), bool_becomes_value_bool(), build_sync_recompute_fn(), different_groups_have_independent_applied_seq_rows(), different_groups_have_independent_generations() (+47 more)

### Community 10 - "Conn<'db>"
Cohesion: 0.10
Nodes (9): Conn<'db>, Error, GroupedMutations, Item, Iterator, PreparedStatement, Result, value_as_usize() (+1 more)

### Community 11 - "ExtractOptions"
Cohesion: 0.12
Nodes (30): AnthropicExtractor, build_edge_tool_schema(), ChatFailure, classify_parse_failure(), ConfigurableExtractor, dial_uds(), EdgeOutcome, EntityOutcome (+22 more)

### Community 12 - "handlers_wal_admin.rs"
Cohesion: 0.12
Nodes (64): all_seqs_in_wal_dir(), assert_has_stat_fields(), dispatch(), entity_create_conflict_wal_line(), entity_wal_line(), episodic_wal_line_without_attributes(), make_db(), make_db_with_path() (+56 more)

### Community 13 - "ontology.rs"
Cohesion: 0.07
Nodes (53): content_hash_adding_parent_changes_hash(), content_hash_description_update_changes_hash(), content_hash_entity_addition_changes_hash(), content_hash_flat_ontology_unchanged_vs_pre_173_format(), content_hash_mode_flip_changes_hash(), content_hash_order_independent(), content_hash_parent_edges_order_independent(), content_hash_relation_rename_changes_hash() (+45 more)

### Community 14 - "handlers.rs"
Cohesion: 0.07
Nodes (41): build_progress_fn(), clear_group_for_rebuild(), count_jsonl_files_in_dir(), edge_endpoint_uuids(), embedding_model_status(), enrich_edge_from_entity_ep_info(), extract_optional_group_ids(), extract_optional_group_ids_preserve_empty() (+33 more)

### Community 15 - "corrections.rs"
Cohesion: 0.10
Nodes (53): AliasInfo, apply_corrections_file(), apply_entity_type_labels(), apply_retract(), apply_same_as(), ApplyDetail, ApplyResult, CorrectionEntry (+45 more)

### Community 16 - "recovery.rs"
Cohesion: 0.08
Nodes (51): attempt_checkpoint_drop(), backfill_applied_seq_if_absent(), backfill_does_not_borrow_a_different_groups_episode(), backfill_entity_survives_episode_deletion_is_not_treated_as_fresh(), backfill_fresh_db_writes_zero_without_scanning_wal(), backfill_is_a_noop_when_row_already_present(), backfill_populated_db_uuid_match_writes_cursor_seq(), backfill_populated_db_uuid_not_found_leaves_null() (+43 more)

### Community 17 - "embedding_cache.rs"
Cohesion: 0.09
Nodes (46): batch_cache_hit_avoids_calling_compute_batch(), batch_different_dim_does_not_collide_on_same_text_and_model(), batch_different_model_identity_does_not_collide_on_same_text(), batch_distinct_texts_resolve_in_request_order(), batch_duplicate_text_within_one_call_costs_one_compute_slot(), batch_empty_input_returns_empty_without_calling_compute(), batch_length_mismatch_from_compute_batch_is_an_error_and_caches_nothing(), batch_mixed_cache_hit_and_miss_preserves_order() (+38 more)

### Community 18 - "ipc_parity.rs"
Cohesion: 0.10
Nodes (53): copy_path(), entity_wal_line(), knowledge_status_concurrent_first_backfill_does_not_race(), knowledge_status_reports_per_group_wal_positions(), make_degraded_state(), make_degraded_state_with_wal(), make_state_with_extractor(), make_state_with_live_wal() (+45 more)

### Community 19 - "IpcRequest"
Cohesion: 0.21
Nodes (54): load_db(), attributes_param_to_string(), dispatch(), extract_required_group_id(), extract_required_group_ids(), handle(), handle_add_cross_group_edge(), handle_add_episode() (+46 more)

### Community 20 - "make_db"
Cohesion: 0.11
Nodes (53): assert_err_resp(), dispatch_val(), make_db(), make_state(), parity_add_cross_group_edge_rejects_bare_uuid_foreign_endpoint(), parity_add_cross_group_edge_rejects_mixed_endpoint_fields(), parity_assert_relationship_refuses_cross_group_name(), parity_backfill_dry_run_counts() (+45 more)

### Community 21 - "report.rs"
Cohesion: 0.09
Nodes (43): CandidateReport, errors_exceeding_chunks_run_is_rejected_as_impossible_accounting(), exact_match_is_accepted(), freeform_report_has_no_vocabulary_compliance_field_value(), high_error_rate_alone_does_not_fail_the_check(), human_readable_report_mentions_key_metrics(), human_readable_report_omits_pairwise_section_when_none(), human_readable_report_omits_truncated_line_when_clean() (+35 more)

### Community 22 - "ontology_integration.rs"
Cohesion: 0.10
Nodes (48): cosmetic_edit_produces_same_hash(), drift_clears_after_write_sidecar(), drift_detected_after_entity_type_addition(), drift_summary_names_added_and_removed_types(), knowledge_status_group_ontology_drift_distinguishes_drifted_and_clean_groups(), knowledge_status_group_ontology_drift_empty_until_a_group_is_used(), knowledge_status_ontology_field_populated(), knowledge_status_ontology_field_present_false() (+40 more)

### Community 23 - "makeApp"
Cohesion: 0.09
Nodes (20): InferenceAdapter, BlockingCompletionTests, DisabledByModeTests, EmbeddingNoModelTests, ErrorHandlingTests, FoundationModelsUnavailableTests, HealthTests, JSONModeTests (+12 more)

### Community 24 - "AppState"
Cohesion: 0.12
Nodes (32): already_resolved_group_is_unaffected_by_insert_if_not_resolved(), AppState, build_indices_once(), clear_group_drift_is_a_noop_when_no_resolution_is_in_flight(), clear_group_drift_upsert_during_in_flight_resolution_records_the_given_ontology(), concurrent_remediation_clear_survives_a_stale_first_resolution_insert(), GroupDriftStatus, GroupOntologyCacheState (+24 more)

### Community 25 - "ontology_sidecar.rs"
Cohesion: 0.13
Nodes (40): content_hash(), content_hash_none_differs_from_empty_would_be_same_sentinel(), build_drift_summary(), compute_drift(), compute_drift_from_sidecar(), compute_group_drift(), compute_group_drift_detects_change_to_own_sidecar(), compute_group_drift_is_isolated_from_other_groups_sidecar() (+32 more)

### Community 26 - "transport.rs"
Cohesion: 0.08
Nodes (33): a_second_listener_on_the_same_path_is_refused(), binds_and_serves_a_literal_pipe_name(), client_endpoint(), connect(), create_instance(), current_user_sid(), endpoint_file_for(), Listener (+25 more)

### Community 27 - "Extraction-quality eval — local-LLM model selection (2026-04)"
Cohesion: 0.05
Nodes (41): Action items (suggested), Candidates evaluated, Cross-corpus validation, Cross-family didn't surprise, Distillation-from-Claude doesn't help, Extraction-quality eval — local-LLM model selection (2026-04), Failure-mode taxonomy (top two locals), Hardware context (+33 more)

### Community 28 - "McpClient"
Cohesion: 0.11
Nodes (23): ChildStdin, build_stub_response_content(), extract_json_array_after_prefix(), ExtractionFixture, hash_embedding(), McpClient, Arc, Command (+15 more)

### Community 29 - "wal.rs"
Cohesion: 0.09
Nodes (29): checkpoints_subdirectory_is_invisible_to_jsonl_scans(), establishing_one_bound_benefits_the_other(), jsonl_file_set_fingerprint(), looks_like_mutation(), read_last_seq_tolerates_truncated_final_line(), reset_first_seq_in_file_call_count(), reset_read_last_seq_call_count(), strip_quoted_literals() (+21 more)

### Community 30 - "tests/group_purge.rs"
Cohesion: 0.20
Nodes (41): assert_err(), assert_ok(), clear_group_for_rebuild_routes_forced_rebind_to_owning_group_not_default(), delete_by_group_attributes_deletions_and_forced_rebind_to_their_own_groups(), dispatch_val(), dry_run_creates_no_wal_directory_for_any_group(), dry_run_mutates_nothing_and_matches_a_following_real_purge(), dry_run_true_with_confirm_true_still_does_not_mutate() (+33 more)

### Community 31 - "extractor.rs"
Cohesion: 0.08
Nodes (29): build_edge_tool_schema_enum_contains_exactly_sanitized_names(), extraction_truncated_emitted_on_budget_exhaustion(), ExtractTransport, OaiChatOutcome, parse_edge_response(), parse_edge_response_blank_endpoint_names_dropped_and_counted(), parse_edge_response_blank_fact_dropped_and_counted(), parse_edge_response_edge_with_two_blank_fields_counted_once() (+21 more)

### Community 32 - "assert_ok_resp"
Cohesion: 0.10
Nodes (41): assert_ok_resp(), explicit_empty_group_ids_matches_zero_rows_not_all_groups(), explicit_null_group_ids_matches_omitted_all_groups(), extract_uuids(), list_relationships_includes_relation_type(), make_state_with_mock_embed(), omitted_group_ids_find_entities_matches_explicit_all_groups(), omitted_group_ids_find_relationships_matches_explicit_all_groups() (+33 more)

### Community 33 - "pairwise.rs"
Cohesion: 0.11
Nodes (33): to_report_entry(), a_judge_failure_drops_its_chunk_rather_than_aborting_the_run(), agreeing_verdicts_produce_a_win_for_the_agreed_side(), AxisTally, chunk(), chunk_pair_seed(), disagreeing_verdicts_produce_a_tie_and_increment_inconsistency(), either_side_err_chunk_is_skipped_not_scored_as_a_loss() (+25 more)

### Community 34 - "tests/assert.rs"
Cohesion: 0.19
Nodes (38): CountingEmbedder, assert_api_write_to_one_group_leaves_another_groups_applied_seq_untouched(), assert_entity_and_relationship_survive_wal_replay(), assert_entity_by_name_merged_forward_rename_is_not_a_self_collision(), assert_entity_by_uuid_forwards_through_merged_tombstone_to_canonical(), assert_entity_by_uuid_rejects_rename_that_collides_with_another_entity(), assert_entity_by_uuid_rename_to_unused_name_succeeds(), assert_entity_dead_end_merged_chain_errors_without_writing() (+30 more)

### Community 35 - "Extractor"
Cohesion: 0.17
Nodes (20): Extractor, Send, Sync, edge_budget_exhaustion_error_triggers_the_permanent_fallback_switch(), FailingExtractor, LlmRouter, opts(), primary_failure_without_fallback_returns_err_without_latching() (+12 more)

### Community 36 - "cassette_record_replay.rs"
Cohesion: 0.15
Nodes (37): a_2xx_response_with_invalid_json_is_classified_malformed_not_http_error(), budget_exhaustion_after_retry_produces_one_complete_sidecar_record(), chunk_key_does_not_affect_the_cassette_request_hash(), edge_budget_exhaustion_after_retry_returns_err_with_entities_extracted_count(), edge_tool_response(), entity_tool_response(), failure_sink_and_path(), http_error_produces_one_complete_sidecar_record() (+29 more)

### Community 37 - "Native Rust Embedder Spike: candle vs ort — Decision Report"
Cohesion: 0.05
Nodes (37): 1. Cosine parity vs PyTorch BGE-base (FR-004, SC-001), 2. Single-input latency — macOS Apple Silicon (FR-001, FR-003, SC-002), 3. Batch throughput — macOS Apple Silicon (FR-003), 4. Single-input latency — Linux x86_64 via Docker (FR-001, FR-003, SC-003), 5. Memory — resident set size (FR-005, SC-004), 6. Cold start — process launch to first embed (FR-006, SC-005), 7. Build impact (FR-007, SC-006), 8. Deployment story (FR-008) (+29 more)

### Community 38 - "SetupCacheTests"
Cohesion: 0.16
Nodes (11): FileHandle, SetupCache, SetupCacheError, sentinelWriteFailed, sentinelWriteRace, Bool, Set, String (+3 more)

### Community 39 - "replay.rs"
Cohesion: 0.13
Nodes (29): cache_bounds_embedder_invocations_for_repeated_text_within_one_window(), embeddings_recompute_had_no_failures_false_after_an_embed_failure(), embeddings_recompute_had_no_failures_true_despite_a_skip(), embeddings_recompute_had_no_failures_true_when_all_rows_recomputed_cleanly(), FlushOutcome, is_delete_form(), log_embed_failure(), one_row_window() (+21 more)

### Community 40 - "canonicalize_integration.rs"
Cohesion: 0.20
Nodes (36): count_wal_lines(), dispatch(), dispatch_raw(), make_edge(), make_edge_with_rt(), make_edge_with_rt_in_group(), make_entity(), make_entity_in_group() (+28 more)

### Community 41 - "Sendable"
Cohesion: 0.14
Nodes (35): Codable, blockingResponse(), sendSSEChunk(), streamingResponse(), ChatCompletionChoice, ChatCompletionRequest, ChatCompletionResponse, ChatCompletionUsage (+27 more)

### Community 42 - "types.rs"
Cohesion: 0.09
Nodes (24): default_kind(), deserialize_summary_or_default(), DroppedEdgeDetail, edge_blank_fact_fails(), edge_blank_source_name_fails(), edge_blank_target_name_fails(), edge_two_blank_fields_still_single_false(), edge_whitespace_only_fact_fails() (+16 more)

### Community 43 - "wal_generation_reset.rs"
Cohesion: 0.26
Nodes (35): assert_ok_resp(), dispatch_rebuild_sync(), dispatch_val(), dry_run_mismatch_is_report_only_and_does_not_mutate(), entity_wal_line(), fr001_hydration_status_hydrated_when_applied_seq_reaches_max_seq(), fr001_hydration_status_not_applicable_for_group_with_no_wal_content(), fr001_hydration_status_wal_ahead_when_applied_seq_never_backfilled() (+27 more)

### Community 44 - "judge_cache.rs"
Cohesion: 0.13
Nodes (29): cache_key(), cache_key_changes_with_judge_model(), cache_key_changes_with_prompt_name(), cache_key_changes_with_reference_or_candidate(), cache_key_is_24_hex_chars(), cache_key_is_stable_and_order_independent_of_construction(), CachedVerdict, CacheEntry (+21 more)

### Community 45 - "test_hybrid_llm_integration.py"
Cohesion: 0.10
Nodes (30): AsyncClient, asyncio, BaseModel, fixture, Message, ModelSize, _binary_available(), client() (+22 more)

### Community 46 - ".query_params"
Cohesion: 0.18
Nodes (10): json_params_to_values(), Option, Value, Vec, value_as_f64(), value_as_float_array(), value_as_match_count(), value_as_optional_string() (+2 more)

### Community 47 - "kind_identity.rs"
Cohesion: 0.17
Nodes (34): add_cross_group_edge_accepts_kind_for_foreign_endpoints_only(), assert_entity(), call(), dedup_candidate_queries_never_offer_a_non_default_kind(), dst_state(), dump_round_trip_preserves_kinds(), entity(), invalid_kinds_are_rejected() (+26 more)

### Community 48 - "cross_group_pointers.rs"
Cohesion: 0.17
Nodes (33): ambiguous_edge_excluded_from_two_hop_read_paths_without_erroring(), bare_uuid_endpoint_foreign_to_edge_group_is_rejected_with_no_partial_write(), count_cross_group_pointers_excludes_invalidated_edges(), count_cross_group_pointers_reports_correct_state_counts(), cross_group_edge_persists_pointer_fields_for_foreign_endpoint(), cross_group_edge_via_foreign_but_no_match_is_unbound_not_dropped(), intra_group_edge_has_no_pointer_fields(), make_entity() (+25 more)

### Community 49 - "real_corpus_e2e.rs"
Cohesion: 0.15
Nodes (28): as_str_vec(), as_uuid_set(), CallCounts, copy_dir_recursive(), CountingEmbedder, CountingExtractor, dispatch(), expected_results() (+20 more)

### Community 50 - "extraction_failures.rs"
Cohesion: 0.16
Nodes (27): append_across_multiple_opens_accumulates_without_truncating(), append_writes_one_jsonl_line_with_complete_body(), ExtractionFailureRecord, ExtractionFailureSink, ExtractionFailureWriter, highest_existing_numbered_file(), numbered_path(), open_creates_sidecar_file_eagerly() (+19 more)

### Community 51 - "failure_taxonomy.rs"
Cohesion: 0.09
Nodes (15): classify_edge_miss(), classify_entity_extra(), classify_entity_miss(), EdgeKey, eq_ci(), is_article_variant(), is_case_or_format_variant(), is_token_subset() (+7 more)

### Community 52 - "scoring.rs"
Cohesion: 0.17
Nodes (28): average(), axis_that_fails_every_call_reports_none_not_zero(), backend_run_result(), chunk_with_failed_reference_extraction_is_excluded_from_scoring(), empty_candidate_list_reports_zeroed_denominators_not_a_panic(), entity_axis_failure_on_one_chunk_does_not_abort_and_other_axes_still_score(), extraction_result(), fresh_cache() (+20 more)

### Community 53 - "mcp_attached.rs"
Cohesion: 0.14
Nodes (32): attached_mode_call_times_out_on_hung_remote_instead_of_blocking_forever(), attached_mode_lazy_connect_serves_handshake_without_daemon(), attached_mode_lazy_connect_succeeds_once_daemon_starts_after_handshake(), attached_mode_matches_socket_result_with_no_lock_conflict(), attached_mode_omits_close_without_allow_remote_close(), attached_mode_post_write_failure_not_retried_but_next_call_reconnects(), attached_mode_reconnects_after_remote_service_restart(), attached_mode_reprocess_entity_types_progress_rearms_idle_timeout() (+24 more)

### Community 54 - "common.sh"
Cohesion: 0.09
Nodes (15): check-prerequisites.sh script, check_dir(), check_feature_branch(), check_file(), feature_json_matches_feature_dir(), get_current_branch(), get_feature_paths(), has_git() (+7 more)

### Community 55 - "candle-bench/src/main.rs"
Cohesion: 0.10
Nodes (27): Args, main(), Output, platform_string(), ReferenceEmbeddings, Option, PathBuf, Result (+19 more)

### Community 56 - "canonicalize.rs"
Cohesion: 0.16
Nodes (29): build_alias_map(), build_lexical_index(), canonicalize_relations(), CanonicalizeParams, CanonicalizeReport, classify_edge_lexically(), EdgeClass, EdgeRecord (+21 more)

### Community 57 - "EntityRow"
Cohesion: 0.21
Nodes (7): Self, value_as_kind(), value_as_str_list(), value_as_string(), value_as_timestamp_str(), EntityRow, Default

### Community 58 - "WalWriter"
Cohesion: 0.24
Nodes (28): dump_community_nodes(), dump_edges_phase(), dump_entity_nodes(), dump_episodic_nodes(), dump_has_episode_edges(), dump_has_member_community_edges(), dump_has_member_entity_edges(), dump_mentions_edges() (+20 more)

### Community 59 - "telemetry.rs"
Cohesion: 0.12
Nodes (24): failure_event(), capture_sink_stores_events(), CaptureSink, cost_for_usage(), cost_for_usage_cache_tokens(), cost_for_usage_known_model(), cost_for_usage_unknown_model(), load_pricing() (+16 more)

### Community 60 - "extraction_quality.rs"
Cohesion: 0.14
Nodes (18): BadEndpointExtractor, edge_with_unresolvable_target_is_dropped(), edges_have_screaming_snake_case_relation_type(), make_db(), make_state(), ontology_entity_types_injected_into_system_prompt(), ontology_injected_into_all_source_type_prompts(), Arc (+10 more)

### Community 61 - "per_group_ontology.rs"
Cohesion: 0.21
Nodes (31): add_episode_succeeds_without_wal_root_configured(), add_episode_writes_published_ontology_sidecar_for_the_extracting_group(), dispatch(), drift_clears_after_add_episode_reingest_for_that_group_only(), drift_clears_after_wal_rebuild_for_that_group_only(), entity_wal_line(), group_with_neither_ontology_extracts_free_form(), group_with_per_group_file_does_not_use_workspace_fallback() (+23 more)

### Community 63 - "src/backend.rs"
Cohesion: 0.10
Nodes (20): BackendKind, build_anthropic_without_api_key_env_is_rejected(), build_extractor(), cassette_without_path_is_rejected(), oai_http_without_url_is_rejected(), parse_backend_spec(), provider_label(), resolved_model() (+12 more)

### Community 64 - "scripts"
Cohesion: 0.06
Nodes (30): astro, @astrojs/react, @astrojs/starlight, @liminis/diagrams, react, react-dom, dependencies, astro (+22 more)

### Community 65 - "concurrent_rw_integration.rs"
Cohesion: 0.13
Nodes (25): assert_entity_concurrent_creates_of_same_identity_yield_one_row(), assert_entity_during_embed_collision_takes_update_path_not_duplicate_insert(), assert_entity_unrelated_write_not_gated_by_slow_embedder(), assert_relationship_concurrent_creates_of_same_identity_yield_one_row(), assert_relationship_endpoint_merged_during_embed_attaches_to_canonical(), concurrent_add_episode_no_write_conflict(), DelayEmbedder, EnvVarGuard (+17 more)

### Community 66 - "LocalInferenceTests.swift"
Cohesion: 0.10
Nodes (19): Accelerate, Accelerate.vImage, CoreML, CryptoKit, Error, Foundation, FoundationModels, HTTPTypes (+11 more)

### Community 67 - "ExtractedEntity"
Cohesion: 0.10
Nodes (21): DedupAdapter, LocalDedupAdapter, PassthroughDedupAdapter, BoxFuture, Client, Error, Result, Self (+13 more)

### Community 68 - "lbug_extension_home.rs"
Cohesion: 0.18
Nodes (23): check_candidate(), env_root_wins_over_exe_root_when_both_resolve(), exe_dir(), exe_dir_from(), exe_root_resolves_when_env_root_absent(), expose_extension_dependencies(), extension_version(), ExtensionFiles (+15 more)

### Community 69 - "pointer.rs"
Cohesion: 0.14
Nodes (22): absent_key_is_empty(), CrossGroupPointer, CrossGroupPointers, empty_pointers_removes_key_rather_than_writing_empty_object(), EndpointSide, merged_into_preserves_unrelated_existing_keys(), merged_into_round_trips(), PointerStateCounts (+14 more)

### Community 70 - "prompts/mod.rs"
Cohesion: 0.16
Nodes (25): build_entity_types_section(), build_fact_types_section(), edge_system_prompt(), edge_user_prompt(), edge_user_prompt_contains_entities(), entity_system_prompt(), entity_user_prompt(), entity_user_prompt_for() (+17 more)

### Community 71 - "wal_exec.rs"
Cohesion: 0.15
Nodes (26): advance_wal_position(), emit_rotation_if_any(), GlobalSeqResyncGuard, GlobalSeqResyncGuard<'a>, mutation(), resync_global_seq_after_rebuild(), resync_guard_fires_on_drop_when_not_marked_done(), resync_guard_is_a_noop_on_drop_when_marked_done() (+18 more)

### Community 72 - "tier1c_deletion.rs"
Cohesion: 0.28
Nodes (29): add_episode(), assert_err(), assert_ok(), clear_all_followed_by_process_chunk(), clear_all_reinits_wal_writer(), clear_all_rejected_without_confirm(), clear_all_wipes_and_reinitializes(), delete_by_source_basic() (+21 more)

### Community 73 - "wal_population.rs"
Cohesion: 0.22
Nodes (29): add_episode_to_two_groups_never_crosses_wal_streams(), count_wal_lines(), dispatch(), group_wal_dir(), has_wal_files(), make_db(), make_state_no_wal(), make_state_with_wal() (+21 more)

### Community 74 - "wal_strip_embeddings.rs"
Cohesion: 0.21
Nodes (29): copy_dir_jsonl_files(), copy_fixture_into(), dispatch_val(), dry_run_reports_would_be_stats_without_mutating_anything(), fixtures_dir(), group_dir(), group_id_param_scopes_to_a_single_groups_wal_directory(), jsonl_files() (+21 more)

### Community 75 - ".replay_opts"
Cohesion: 0.16
Nodes (21): CancelFn, classify_replay_failure(), compute_fidelity_warning(), flush_batch(), PreparedCache, push_resolved_rows(), ReplayBatch, ReplayOptions (+13 more)

### Community 76 - "TelemetrySink"
Cohesion: 0.20
Nodes (19): do_extract_edges_emits_clean_structured_output_parse_with_edges_call_type(), do_extract_edges_skips_http_call_when_entity_names_sanitize_to_empty(), do_extract_entities_all_malformed_batch_salvages_to_empty_success(), do_extract_entities_emits_clean_structured_output_parse(), do_extract_entities_emits_entities_missing_summary_on_mixed_batch(), do_extract_entities_emits_malformed_structured_output_parse(), do_extract_entities_emits_recovered_structured_output_parse(), do_extract_entities_missing_summary_retains_entities_and_stays_clean() (+11 more)

### Community 77 - "plan.rs"
Cohesion: 0.16
Nodes (28): BackendPlan, corrupt_cassette_is_a_guard_violation_naming_corruption(), coverage_shortfall_reported_when_records_below_scope(), Decision, distinct_cassette_content_is_accepted(), duplicate_key_cassette_is_a_guard_violation_naming_duplication(), exact_match_is_not_a_shortfall(), hash_file() (+20 more)

### Community 78 - "mcp_real_corpus_admin_lifecycle_e2e.rs"
Cohesion: 0.13
Nodes (25): ChildGuard, Child, Drop, corrupt_via_readonly_dir(), mcp_admin_lifecycle_operations_over_real_corpus_fixture(), PermissionGuard, restore_writable(), Child (+17 more)

### Community 79 - "Architecture Decisions"
Cohesion: 0.07
Nodes (28): AD-10: ADR-042 content, AD-1: `AppState` struct replaces four separate `Arc` args in `dispatch`, AD-2: `Extractor` becomes an object-safe async trait, AD-3: `DedupAdapter` trait + `LocalDedupAdapter` + `PassthroughDedupAdapter`, AD-4: `episode::add_episode` split — async dedup before commit `spawn_blocking`, AD-5: `LlmRouter` for extraction primary→fallback, AD-6: Prompt caching on Sonnet path, AD-7: HTTP 529 backoff in `AnthropicExtractor` (+20 more)

### Community 80 - "Setup Guide: native-embedder spike"
Cohesion: 0.07
Nodes (27): 1. Build, 2. Download models, 3. Run candle-bench (macOS / Linux), 4. Run ort-bench (macOS / Linux), 5. Run on Linux x86_64 (via Docker), Corpora, Decision report, native-embedder spike (+19 more)

### Community 81 - ".makeApp"
Cohesion: 0.13
Nodes (18): CaseIterable, CustomStringConvertible, decode(), EmbeddingIntegrationTests, postEmbeddings(), StubAdapter, StubFixture, .description (+10 more)

### Community 82 - "Tasks: [FEATURE NAME]"
Cohesion: 0.07
Nodes (27): Constitution compliance (liminis-context-graph v1.0.0), Dependencies & Execution Order, Format: `[ID] [P?] [Story] [Constitution-tag?] Description`, Implementation for User Story 1, Implementation for User Story 2, Implementation for User Story 3, Implementation Strategy, Incremental Delivery (+19 more)

### Community 83 - "setup_bench_db_n"
Cohesion: 0.14
Nodes (21): measure_brute_force_ns(), Arc, TempDir, setup_bench_db_n(), bench_dedup_brute_force_50k(), bench_dedup_hybrid_50k(), Criterion, bench_dedup_overlap_check() (+13 more)

### Community 84 - "PairwiseVerdict"
Cohesion: 0.20
Nodes (17): JudgeClient, JudgeVerdict, PairwiseVerdict, Self, Send, Sync, StaticJudge, AlwaysSlotAJudge (+9 more)

### Community 85 - "speckit-analyze/SKILL.md"
Cohesion: 0.08
Nodes (25): 1. Initialize Analysis Context, 2. Load Artifacts (Progressive Disclosure), 3. Build Semantic Models, 4. Detection Passes (Token-Efficient Analysis), 5. Severity Assignment, 6. Produce Compact Analysis Report, 7. Provide Next Actions, 8. Offer Remediation (+17 more)

### Community 86 - "merge_entities"
Cohesion: 0.28
Nodes (25): merge_entities(), count_active_entities_named(), make_edge(), make_edge_in_group(), make_entity(), open_db(), TempDir, test_alias_uuid_foreign_group_rejected() (+17 more)

### Community 87 - "String"
Cohesion: 0.12
Nodes (17): compute_lookup_key(), cosine_similarity(), enforce_entity_first(), format_datetime(), format_datetime_iso8601(), format_datetime_rfc3339_subsecond(), labels_with_kind(), LookupKeyStatus (+9 more)

### Community 88 - "core/src/embedder.rs"
Cohesion: 0.10
Nodes (14): HashEmbedder, is_auth_error(), is_transport_error(), OaiEmbedRequest, resolve_embedding_api_key_empty_string_treated_as_absent_at_every_tier(), resolve_embedding_api_key_falls_back_to_graphiti(), resolve_embedding_api_key_falls_back_to_openai(), resolve_embedding_api_key_falls_back_to_openai_against_non_openai_host() (+6 more)

### Community 89 - "error.rs"
Cohesion: 0.10
Nodes (15): Error, format_candidates(), is_already_exists_error(), is_missing_index_error(), is_missing_table_error(), is_named_catalog_entry_already_exists_error(), From, Self (+7 more)

### Community 90 - "Ontology"
Cohesion: 0.16
Nodes (22): normalize_relation_type_applied_during_edge_parse(), build_ontology(), compute_ancestor_map(), EntityTypeDef, EntityTypeRaw, load_group_ontology(), normalize_entity_type(), normalize_relation_type() (+14 more)

### Community 91 - "reprocess_relations.rs"
Cohesion: 0.15
Nodes (21): EdgeCandidate, is_off_ontology(), is_off_ontology_declared_type_is_not_off_ontology(), is_off_ontology_fact_prefix_pseudo_type_is_off_ontology(), is_off_ontology_unclassified_sentinel_is_off_ontology(), is_off_ontology_untyped_edge_is_off_ontology(), is_untyped(), list_edges_for_scope() (+13 more)

### Community 92 - "cancel_shutdown.rs"
Cohesion: 0.18
Nodes (19): cancel_before_episode_returns_cancelled(), cancel_during_phase_a_batch_embed_returns_cancelled(), cancel_during_phase_a_returns_cancelled(), make_db(), make_state_with_fast_extractor(), make_state_with_slow_batch_embedder(), make_state_with_slow_extractor(), no_cancel_completes_normally() (+11 more)

### Community 93 - "extractor_transport.rs"
Cohesion: 0.15
Nodes (24): assert_oai_chat_request_body(), make_extractor(), oai_chat_response_json(), read_http_request_body_keepalive(), Arc, AsyncBufRead, AsyncWriteExt, AtomicUsize (+16 more)

### Community 94 - "binary_path"
Cohesion: 0.13
Nodes (23): binary_path(), attached_mode_connect_eager_fails_fast_on_unreachable_socket(), per_group_ontology_zero_cross_group_leakage_over_mcp(), Value, structured(), attached_rebuild_surfaces_bridged_progress(), Duration, Path (+15 more)

### Community 95 - "Embedder"
Cohesion: 0.20
Nodes (23): Embedder, Send, Sync, hybrid_edge_search(), hybrid_entity_search(), hybrid_entity_search_kind(), order_by_rank(), order_by_rank_follows_the_fused_ranking_not_fetch_order() (+15 more)

### Community 96 - "wal_appender.rs"
Cohesion: 0.17
Nodes (23): count_lines(), jsonl_files(), open_writer(), open_writer_with_bytes(), parse_seq(), Path, PathBuf, TempDir (+15 more)

### Community 97 - "eval/src/corpus.rs"
Cohesion: 0.13
Nodes (22): CorpusChunk, default_corpus_path(), default_corpus_path_points_at_the_217_fixture(), invalid_json_line_is_a_descriptive_error(), load_corpus(), loads_the_real_217_fixture(), missing_file_is_a_descriptive_error(), AsRef (+14 more)

### Community 98 - "Implementation Plan: Issue #6 — Telemetry and Operator Visibility"
Cohesion: 0.08
Nodes (23): AD-T1: TelemetrySink trait in the library, StderrSink in the binary, AD-T3: dispatch() gains one sink parameter for IPC timing, AD-T4: Compiled-in pricing table via include_str!, AD-T5: StderrSink is channel-backed; UNIX socket deferred, AD-T6: WAL and fallback event types are defined but not emitted, Architecture Decisions, Complexity Tracking, Constitution Check (+15 more)

### Community 99 - "Feature Specification: Integration Architecture — In-Process MCP + Direct Socket Client + App-Bundled Binary"
Cohesion: 0.08
Nodes (24): App Bundling, Assumptions, Background, `ContextGraphSocketClient` (TS, in liminis-app/src/main), Edge Cases, Feature Specification: Integration Architecture — In-Process MCP + Direct Socket Client + App-Bundled Binary, Framework Boundary, Functional Requirements (+16 more)

### Community 100 - "handleEmbeddings"
Cohesion: 0.11
Nodes (21): CodingKey, Decoder, Encoder, handleEmbeddings(), BasicRequestContext, Request, Response, CodingKeys (+13 more)

### Community 101 - "Conn"
Cohesion: 0.25
Nodes (22): Conn, backfill_entity_lookup_keys(), backfill_entity_lookup_keys_and_record_status(), create_edge_tables(), create_fts_indexes(), create_node_tables(), drop_fts_indexes(), ensure_lookup_key_backfill() (+14 more)

### Community 102 - "Db"
Cohesion: 0.20
Nodes (24): Db, insert_test_edge(), insert_test_entity(), make_person_ontology(), make_relation_ontology(), make_state_with_capture_sink(), make_state_with_ontology_and_extractor(), parity_rebuild_from_wal_force_clear_succeeds() (+16 more)

### Community 103 - "cross_group_incremental_replay.rs"
Cohesion: 0.25
Nodes (23): assert_ok_resp(), cross_group_pointer_resolves_after_target_groups_incremental_replay(), dispatch_rebuild_sync(), dispatch_val(), make_db(), make_state_with_wal(), named_wal_line(), read_wal_cyphers() (+15 more)

### Community 104 - "dedup_auto_heal_integration.rs"
Cohesion: 0.18
Nodes (17): AxisEmbedder, concurrent_ingest_past_hybrid_threshold_auto_heals(), make_state_without_indices(), post_clear_all_ingest_past_threshold_succeeds(), Arc, AtomicUsize, BoxFuture, Error (+9 more)

### Community 105 - "migration_binary.rs"
Cohesion: 0.16
Nodes (21): corrupt_lbug_wal(), fresh_startup_reports_indices_built_true(), post_recovery_startup_reports_indices_built_true(), Duration, Path, PathBuf, Value, send_request() (+13 more)

### Community 106 - "User Scenarios & Testing *(mandatory)*"
Cohesion: 0.08
Nodes (22): Content Quality, Feature Readiness, Notes, Requirement Completeness, Specification Quality Checklist: Rust Knowledge Graph Service, Assumptions, Edge Cases, Feature Specification: Rust Knowledge Graph Service (+14 more)

### Community 107 - "Issue #6 Spec: Telemetry and Operator Visibility"
Cohesion: 0.08
Nodes (23): Assumptions, Event Schema, Functional Requirements, Goal, In scope, `ipc_call`, Issue #6 Spec: Telemetry and Operator Visibility, Key Entities (+15 more)

### Community 108 - ".call_tool"
Cohesion: 0.15
Nodes (16): CallToolRequestParams, CallToolResult, CancelledNotificationParam, ipc_response_to_call_tool_result(), LcgMcpServer<B>, Option, RequestContext, Result (+8 more)

### Community 109 - "backfill.rs"
Cohesion: 0.15
Nodes (20): backfill_relation_types(), BackfillParams, BackfillReport, derive_relation_type(), derive_relation_type_from_fact(), EdgeCandidate, Arc, Error (+12 more)

### Community 110 - "Path"
Cohesion: 0.27
Nodes (23): current_jsonl_file_set_fingerprint(), first_seq_in_file(), is_safe_wal_file_name(), read_last_seq(), read_last_seq_in_range(), read_wal_bounds_manifest(), Error, Option (+15 more)

### Community 111 - "sync-docs.mjs"
Cohesion: 0.09
Nodes (12): ADR-0295, ADR-0477, REPO, OUT, pages, PUBLIC, REPO, SITE (+4 more)

### Community 112 - "Implementation Plan: Issue #3 — WAL Parity for Git-Friendly Persistence"
Cohesion: 0.09
Nodes (22): AD-W1: Named `WalLine` struct, field-declaration-order JSON serialization, AD-W2: Chunk-atomicity via closure API, AD-W3: Soft `max_events_per_file` — chunk wins, never splits, AD-W4: Backward scan for global sequence initialization, AD-W5: Structural forward-compat test — no Python runtime required, AD-W6: Small committed golden fixtures for backward/forward-compat tests, AD-W7: Simple Cypher first-token mutation classification, AD-W8: New workspace dependencies (+14 more)

### Community 113 - "Feature Specification: WAL checkpoints — named recovery positions stored in the WAL directory"
Cohesion: 0.09
Nodes (22): Assumptions, Background, Edge Cases, Explicitly not snapshotting, Feature Specification: WAL checkpoints — named recovery positions stored in the WAL directory, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes (+14 more)

### Community 114 - ".new"
Cohesion: 0.20
Nodes (21): cancel_mid_embed_window_drops_pending_rows_without_touching_counters(), last_partial_window_at_eof_is_still_resolved(), make_db_with_schema(), prepare_calls_bounded_across_wal_file_boundary_for_same_template(), prepare_calls_bounded_for_homogeneous_wal(), prepare_calls_proportional_to_distinct_templates(), prepare_calls_unbounded_for_fully_interleaved_wal(), replay_aborted_at_file_boundary_flush_drops_leftover_pending_window() (+13 more)

### Community 115 - "Decision"
Cohesion: 0.09
Nodes (22): ADR-0378: Multi-Stream WAL — One WAL Directory Per Group, Alternatives Considered, Consequences, Context, Decision, Directory topology: a WAL root, one subdirectory per group, Fail loudly on any `group_id` outside `checkpoint::validate_name`'s charset, with no encoding at all, FR-001: migration relocates the WAL directory's entire contents as a unit, crash-safely, with no marker file (+14 more)

### Community 116 - "CoreMLEmbeddingActor"
Cohesion: 0.13
Nodes (17): Float, MLMultiArray, MLMultiArrayDataType, CoreMLEmbeddingActor, MLModel, Set, String, Tokenizer (+9 more)

### Community 117 - "Architecture Decisions"
Cohesion: 0.09
Nodes (21): AD-1: tokio async runtime in the binary; sync library wrapped in spawn_blocking, AD-2: JSON-RPC 2.0, newline-delimited, over Unix socket, AD-3: RELATES_TO as a rel table from Entity to Entity; RelatesToNode_ node table for fact-vector search, AD-4: Brute-force cosine dedup in add_episode (HNSW dedup is issue #4), AD-5: Adapters are out-of-process, reached via HTTP; no trait objects required, AD-6: group_id default of "liminis" applied at the IPC dispatch layer, AD-7: RRF fusion formula and ranking, AD-8: Entity-first label invariant enforced at insert time (+13 more)

### Community 118 - "String"
Cohesion: 0.16
Nodes (12): MockEmbedder, NameMapEmbedder, redact_url_userinfo(), redact_url_userinfo_leaves_plain_url_unchanged(), redact_url_userinfo_passes_through_unparseable_input(), redact_url_userinfo_strips_user_and_pass(), resolve_embedding_api_key(), HashMap (+4 more)

### Community 119 - "legacy_wal.rs"
Cohesion: 0.17
Nodes (15): expand_bulk_property_set(), expand_bulk_set_empty_object_skipped(), expand_bulk_set_expands_object_param(), expand_bulk_set_key_prefix_avoids_collision(), expand_bulk_set_leaves_individual_assignments_unchanged(), expand_bulk_set_mixed_bulk_and_individual(), expand_bulk_set_non_object_param_unchanged(), promote_inline_vector_literal_handles_multiple_keys_in_one_row() (+7 more)

### Community 120 - "backfill_wal.rs"
Cohesion: 0.30
Nodes (20): count_wal_lines(), dispatch(), dispatch_raw(), make_edge(), make_edge_in_group(), make_entity(), make_entity_in_group(), make_state_with_wal() (+12 more)

### Community 121 - "lookup_key_index.rs"
Cohesion: 0.22
Nodes (20): apply_same_as_label_mutation_does_not_affect_name_lookups(), backfill_entity_lookup_keys_surfaces_a_genuine_query_failure(), deleting_the_winner_falls_through_to_the_next_same_named_row(), explain_shows_art_indexed_scan_for_lookup_query(), fresh_db_has_no_entities_lookupable(), insert_entity_is_immediately_lookupable_by_name(), lookup_is_scoped_to_group_id(), lookup_resolves_to_a_merged_tombstoned_winner() (+12 more)

### Community 122 - "summary_semantic_search.rs"
Cohesion: 0.29
Nodes (20): backfill_batches_embed_calls_by_write_chunk(), backfill_dry_run_issues_zero_batch_calls(), dispatch(), dispatch_raw(), empty_summary_entity_skips_embedder_call_for_summary(), find_entities_names(), make_state(), open_db() (+12 more)

### Community 123 - "AttachedBackend"
Cohesion: 0.19
Nodes (13): AttachedBackend, parse_ipc_response(), AtomicU64, BufReader, ClientStream, Duration, Mutex, Option (+5 more)

### Community 124 - "tools.rs"
Cohesion: 0.15
Nodes (16): backfill_relation_types_description_warns_and_points_to_replacement(), backfill_summary_embeddings_is_registered_admin_scope_with_required_group_id(), cypher_scope_is_exactly_query_cypher(), edge_tool_descriptions_state_episode_uuids_semantics_accurately(), empty_schema(), every_schema_is_a_valid_object_schema(), group_ids_prop(), kind_filter_prop() (+8 more)

### Community 125 - "Decision"
Cohesion: 0.10
Nodes (21): A separately-named method, not a change to `get_entity_by_name_ci` itself, ADR-0283: Bounded Scan Fallback and Trust State for NameIndex Endpoint Resolution, Alternatives Considered, Consequences, Context, Decision, `handle_query_cypher` rebuilds the index for mutation-shaped raw Cypher (FR-004), Make the index authoritative by construction (every write path updates/invalidates it before considered complete) (+13 more)

### Community 126 - "Implementation Plan: Issue #1 — Foundation"
Cohesion: 0.10
Nodes (20): AD-1: Two-crate workspace, no driver abstraction, AD-2: Extension loading enforced at connection construction, AD-3: Sync API, async boundary via spawn_blocking, AD-4: HNSW index creation enforced after insertion, AD-5: Embedding dimension parameterized at schema creation, AD-6: Extension network dependency on CI, Architecture Decisions, CI Workflow (+12 more)

### Community 127 - "Tasks: Issue #3 — WAL Parity for Git-Friendly Persistence"
Cohesion: 0.10
Nodes (20): Constitution Compliance, Dependencies & Execution Order, Format: `[ID] [P?] [Story] [Constitution-tag?] Description`, Implementation, Implementation, Implementation, Parallel Opportunities, Phase 1: Dependencies & Error Infrastructure (+12 more)

### Community 128 - "User Scenarios & Testing *(mandatory)*"
Cohesion: 0.10
Nodes (20): Assumptions, Background, Edge Cases, Feature Specification: MCP Admin/Lifecycle E2E Suite Over the Real-Corpus Fixture, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+12 more)

### Community 129 - "Feature Specification: Tier 2 WAL Admin — prepare_checkpoint, rebuild_from_wal, rebuild_status"
Cohesion: 0.10
Nodes (20): Assumptions, Background, Common, Edge Cases, Feature Specification: Tier 2 WAL Admin — prepare_checkpoint, rebuild_from_wal, rebuild_status, Functional Requirements, Key Entities, Measurable Outcomes (+12 more)

### Community 130 - "add_episode"
Cohesion: 0.19
Nodes (18): ActiveWriteGuard, add_episode(), AddEpisodeResult, DedupDecision, hybrid_threshold(), PhaseBResult, resolve_phase_b(), resolve_phase_b_genuine_error_is_not_classified_as_missing_index() (+10 more)

### Community 131 - "WalLine"
Cohesion: 0.14
Nodes (14): WalLine, fixture_path(), read_fixture_lines(), PathBuf, String, Vec, test_backward_compat_python_fixture_parseable(), test_backward_compat_seq_monotonic() (+6 more)

### Community 132 - "dispatch_val"
Cohesion: 0.23
Nodes (19): dispatch_val(), make_degraded_state_with_capture(), req(), Arc, String, Value, test_degraded_mode_from_corrupt_wal(), test_failed_drop_lbug_wal_leaves_indices_built_unchanged() (+11 more)

### Community 133 - "ipc_response_shapes.rs"
Cohesion: 0.37
Nodes (19): assert_envelope(), assert_facts_absent(), dispatch_val(), make_db(), make_state(), Arc, TempDir, Value (+11 more)

### Community 134 - "test-scripts.sh"
Cohesion: 0.22
Nodes (16): api_post(), backup_if_present(), cassette_complete(), die(), require_release_binary(), require_server_healthy(), _common.sh script, assert_contains() (+8 more)

### Community 135 - "Event Types"
Cohesion: 0.10
Nodes (20): `chunk_text_oversized`, `entities_missing_summary`, Environment Variables, Event Types, `extraction_failure`, `extraction_truncated`, `ipc_call`, `llm_fallback` (+12 more)

### Community 136 - "Feature Specification: Tier 1c — Deletion Methods (delete_by_source, delete_chunk_episode, clear_all)"
Cohesion: 0.10
Nodes (19): Assumptions, Background, clear_all, Common, delete_by_source, delete_chunk_episode, Edge Cases, Feature Specification: Tier 1c — Deletion Methods (delete_by_source, delete_chunk_episode, clear_all) (+11 more)

### Community 137 - "Feature Specification: `knowledge_status` errors instead of reporting degraded state when a core table is missing"
Cohesion: 0.10
Nodes (19): Assumptions, Background, Blast radius and the 0.12.0 milestone, Edge Cases, Feature Specification: `knowledge_status` errors instead of reporting degraded state when a core table is missing, Functional Requirements, Likely cause (unverified — Research must confirm), Measurable Outcomes (+11 more)

### Community 138 - "Feature Specification: Persist and expose an applied WAL sequence in knowledge_status"
Cohesion: 0.10
Nodes (20): Assumptions, Background, Blocked by the seq-uniqueness bug, Edge Cases, Feature Specification: Persist and expose an applied WAL sequence in knowledge_status, Functional Requirements, Key Entities *(if applicable)*, Measurable Outcomes (+12 more)

### Community 139 - ".new"
Cohesion: 0.20
Nodes (16): B, Scope, admin_scope_exposes_wal_lifecycle_tools(), attached_mode_hides_close_without_allow_remote_close(), cypher_scope_exposes_only_query_cypher(), LcgMcpServer, read_scope_hides_write_and_cypher_and_admin_tools(), CancellationToken (+8 more)

### Community 140 - "cross_group.rs"
Cohesion: 0.33
Nodes (18): create_cross_group_edge(), CreateCrossGroupEdgeParams, EndpointSpec, follow_merged_into_chain(), rebind_pointers(), rebind_pointers_forced(), rebind_pointers_impl(), RebindCounts (+10 more)

### Community 141 - "Error"
Cohesion: 0.33
Nodes (8): default_embed_batch_matches_sequential_embed_calls_in_order(), extract_embedding(), extract_embeddings_ordered(), resolve_embed_batch_size(), BoxFuture, Error, Result, Vec

### Community 142 - "String"
Cohesion: 0.33
Nodes (10): ClassifyingExtractor, PartiallyMalformedExtractor, RelationClassifyingExtractor, BoxFuture, HashMap, LcgError, Option, Result (+2 more)

### Community 143 - "metrics.rs"
Cohesion: 0.20
Nodes (18): edge(), edge_exact_match_is_perfect(), edge_missing_relation_type_normalizes_to_empty_string(), edge_relation_type_synonym_is_penalized_strictly(), edge_strict_prf1(), entity_both_empty_is_perfect(), entity_case_and_whitespace_insensitive(), entity_exact_match_is_perfect() (+10 more)

### Community 144 - "ADR-0038: In-Process NameIndex Accelerator for Entity Name Lookup"
Cohesion: 0.11
Nodes (19): 1. Incremental hooks on the two typed mutation methods that can affect a name→uuid mapping, 2. Full rebuild via `Conn::rebuild_name_index()` at every path that populates Entity rows without going through the typed methods above, ADR-0038: In-Process NameIndex Accelerator for Entity Name Lookup, Alternatives Considered, Amendment (2026-08-16, issue #398), `Arc<NameIndex>` clone per `Conn::connect()`, Consequences, Context (+11 more)

### Community 145 - "Decision"
Cohesion: 0.11
Nodes (19): ADR-0221: Secondary ART Index for Entity Name Lookup (Replaces In-Process NameIndex), Alternatives Considered, `compute_lookup_key`: one Rust function, called everywhere, Consequences, Context, Cypher-side `lower()` for the migration backfill, Decision, Keep `NameIndex`, close this issue without merging (+11 more)

### Community 146 - "Decision"
Cohesion: 0.11
Nodes (19): ADR-0361: Group-Scoped Complete Purge, Alternatives Considered, Atomicity across multiple `group_ids` (FR-002), Bespoke inline unbind logic instead of reusing `rebind_pointers`, Changing `rebind_pointers`'s signature to add a `force` parameter directly, Consequences, Context, Decision (+11 more)

### Community 147 - "Liminis Context Graph"
Cohesion: 0.11
Nodes (19): Architecture decisions, Build from source, Contributing, Dependencies, Documentation, `error while loading shared libraries: libssl.so.3` (Linux), How it works, Install prebuilt binary (+11 more)

### Community 148 - "Research Findings: Issue #6 — Telemetry and Operator Visibility"
Cohesion: 0.11
Nodes (18): 1. Codebase State Summary, 2.1 IPC timing (`ipc_call` event), 2.2 Token usage (`token_usage` event), 2.3 Fallback events (`llm_fallback` event), 2.4 WAL instrumentation (`wal_append`, `wal_replay_complete` events), 2.5 Binary wiring, 2. Integration Points, 3. Architecture Decision: Sink Design (+10 more)

### Community 149 - "Feature Specification: Relation Canonicalization — Map Free-Text Relation Names to Controlled `relation_type` (Two-Layer; Drop Co-occurrence Noise)"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Edge Cases, Feature Specification: Relation Canonicalization — Map Free-Text Relation Names to Controlled `relation_type` (Two-Layer; Drop Co-occurrence Noise), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+10 more)

### Community 150 - "Feature Specification: Record/Replay LLM Cassette for Extraction"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Edge Cases, Feature Specification: Record/Replay LLM Cassette for Extraction, Functional Requirements, Key Entities *(if applicable)*, Measurable Outcomes, Out of Scope *(optional)* (+10 more)

### Community 151 - "Feature Specification: MCP Write/Mutation-Path E2E Suite Over the Real-Corpus Fixture"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Edge Cases, Feature Specification: MCP Write/Mutation-Path E2E Suite Over the Real-Corpus Fixture, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+10 more)

### Community 152 - "Feature Specification: CI: build release artifacts once and reuse them across the five e2e jobs"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Edge Cases, Feature Specification: CI: build release artifacts once and reuse them across the five e2e jobs, Functional Requirements, Key Entities, Measurable Outcomes, Observed cost: the duplicate-symbol race, now with five more contenders (+10 more)

### Community 153 - "Feature Specification: Multi-stream WAL: one WAL directory per group"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Edge Cases, Feature Specification: Multi-stream WAL: one WAL directory per group, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+10 more)

### Community 154 - "Feature Specification: `applied_seq` never advances for `wal_flush_ungrouped` writes"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Current architecture (post-#378), Edge Cases, Feature Specification: `applied_seq` never advances for `wal_flush_ungrouped` writes, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes (+10 more)

### Community 155 - "Feature Specification: WAL Stream Generation Identity"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Edge Cases, Feature Specification: WAL Stream Generation Identity, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+10 more)

### Community 156 - "Feature Specification: Group-Scope `canonicalize_relations` and `backfill_relation_types`"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Edge Cases, Feature Specification: Group-Scope `canonicalize_relations` and `backfill_relation_types`, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+10 more)

### Community 157 - "User Scenarios & Testing *(mandatory)*"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Edge Cases, Feature Specification: Upgrade lbug 0.18.1 -> 0.20.4 (storage 42 -> 47): stale-row cached-plan fix and the fixes forgone by the rollback, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+10 more)

### Community 158 - "Feature Specification: Kind-scoped entity identity"
Cohesion: 0.11
Nodes (18): Assumptions, Background, Edge Cases, Feature Specification: Kind-scoped entity identity, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+10 more)

### Community 159 - "backfill_summary_embeddings_wal.rs"
Cohesion: 0.27
Nodes (16): count_wal_lines(), dispatch(), dispatch_raw(), make_entity(), make_state_with_wal(), open_db(), req(), Arc (+8 more)

### Community 160 - "edge_endpoint_resolution.rs"
Cohesion: 0.37
Nodes (17): make_db(), make_state_with(), Arc, TempDir, run_episode(), test_both_endpoints_unresolvable_reports_both(), test_cross_batch_edge_endpoint_is_resolved_and_persisted(), test_edge_dropped_when_one_endpoint_unresolvable_even_if_other_resolves_globally() (+9 more)

### Community 161 - "make_entity_line"
Cohesion: 0.11
Nodes (18): make_entity_line(), test_community_node_replays_into_stub_table(), test_fidelity_warning_fires_above_threshold(), test_match_delete_no_op_does_not_feed_fidelity_warning(), test_match_prefixed_no_op_feeds_fidelity_warning(), test_replay_match_prefixed_counter(), test_replay_opts_dry_run(), test_replay_opts_from_seq() (+10 more)

### Community 162 - "wal_root_migration.rs"
Cohesion: 0.26
Nodes (17): assert_ok_resp(), dispatch_val(), entity_wal_line(), make_db(), make_state_with_wal(), make_state_with_wal_dim(), migration_is_a_noop_on_second_startup(), pre_378_flat_wal_dir_migrates_and_preserves_position_and_checkpoint_reachability() (+9 more)

### Community 163 - "BoxFuture"
Cohesion: 0.30
Nodes (10): FailingExtractor, judged_f1_with_cache(), BoxFuture, Error, Option, Result, String, Value (+2 more)

### Community 164 - "Decision"
Cohesion: 0.11
Nodes (18): A `merged_into` forwarding pointer on merged aliases, ADR-0369: Resolvable Semantic Pointers for Cross-Group Edges, Alternatives Considered, `binding_state`: a tri-state independent of `invalid_at`, Consequences, Context, Data shape, Decision (+10 more)

### Community 165 - "Decision"
Cohesion: 0.11
Nodes (18): A non-destructive `Conn` peek/length accessor (record `(group_id, start_idx, end_idx)` boundaries, split after one real `drain_mutations()`), ADR-0385: Per-Group Mutation Attribution for `delete_by_group` and `rebind_pointers`, Alternatives Considered, `clear_group_for_rebuild` gets the identical fix, though not named by the issue, Consequences, Context, Decision, Delete loop and forced-rebind loop stay two separate passes, not interleaved (+10 more)

### Community 166 - "MCP-over-stdio transport"
Cohesion: 0.11
Nodes (18): Cross-group pointers (`add_cross_group_edge`, `rebind_pointers`), DB-access modes, Deletion (`delete_chunk_episode`, `delete_by_source`), Direct assertion (`assert_entity`, `assert_relationship`), Entity kinds (`kind` identity), Example MCP client config, Flags, group_ids semantics: omitted vs. empty (+10 more)

### Community 167 - "LocalInferenceMode"
Cohesion: 0.15
Nodes (11): Equatable, LocalInferenceMode, both, completions, embeddings, .includesCompletions, .includesEmbeddings, ParseError (+3 more)

### Community 168 - "Feature Specification: OSS Launch Scaffolding — LICENSE, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, CHANGELOG, README Polish"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: OSS Launch Scaffolding — LICENSE, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, CHANGELOG, README Polish, Out of Scope, Requirements *(mandatory)*, Scope, Source References (+9 more)

### Community 169 - "Feature Specification: Fix FTS Query Syntax for lbug 0.16.1"
Cohesion: 0.11
Nodes (17): Affected Code, Affected Tests, Assumptions, Background, Edge Cases, Feature Specification: Fix FTS Query Syntax for lbug 0.16.1, Functional Requirements, Key Entities (+9 more)

### Community 170 - "Feature Specification: Audit — Route All Direct-Write Paths Through Shared Type Coercion"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: Audit — Route All Direct-Write Paths Through Shared Type Coercion, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+9 more)

### Community 171 - "Feature Specification: Ontology Subtype/Parent Support — Hierarchical Entity Types via Additive Multi-Labeling"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: Ontology Subtype/Parent Support — Hierarchical Entity Types via Additive Multi-Labeling, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+9 more)

### Community 172 - "Feature Specification: Relations Must Consistently Carry a Semantic `relation_type` (Extractor Fix + Additive Backfill; Never Delete Arrow Edges)"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: Relations Must Consistently Carry a Semantic `relation_type` (Extractor Fix + Additive Backfill; Never Delete Arrow Edges), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+9 more)

### Community 173 - "Feature Specification: Decide the max_tokens Policy and Edge Budget-Exhaustion Semantics"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Constraints that bound the design, Edge Cases, Feature Specification: Decide the max_tokens Policy and Edge Budget-Exhaustion Semantics, Functional Requirements, Key Entities, Measurable Outcomes (+9 more)

### Community 174 - "Feature Specification: A missing `summary` field discards the entire chunk, and is misreported as malformed JSON"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: A missing `summary` field discards the entire chunk, and is misreported as malformed JSON, Functional Requirements, It biases every local-vs-hosted comparison, Key Entities *(data contracts affected)*, Measurable Outcomes (+9 more)

### Community 175 - "Feature Specification: docs-only changes pay a full 15–18 minute Rust CI cycle"
Cohesion: 0.11
Nodes (17): A second constraint specific to this repo, Assumptions, Background, Edge Cases, Feature Specification: docs-only changes pay a full 15–18 minute Rust CI cycle, Functional Requirements, Key Entities, Measurable Outcomes (+9 more)

### Community 176 - "Feature Specification: run `real-corpus-e2e` on the PR path as a non-required check"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: run `real-corpus-e2e` on the PR path as a non-required check, Functional Requirements, Key Entities, Measurable Outcomes, Measured timing (+9 more)

### Community 177 - "Feature Specification: Per-Group Ontology Drift Detection"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: Per-Group Ontology Drift Detection, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+9 more)

### Community 178 - "Feature Specification: MCP client config recipes for pointing the embedder elsewhere"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: MCP client config recipes for pointing the embedder elsewhere, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+9 more)

### Community 179 - "Feature Specification: Bundle lbug vector/fts extensions with the release so startup never downloads from the CDN"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Evidence (from the original report), Feature Specification: Bundle lbug vector/fts extensions with the release so startup never downloads from the CDN, Functional Requirements, Key Entities, Measurable Outcomes (+9 more)

### Community 180 - "Feature Specification: Optional Ontology Support — Workspace-Scoped Entity and Edge Vocabularies that Guide Extraction"
Cohesion: 0.11
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: Optional Ontology Support — Workspace-Scoped Entity and Edge Vocabularies that Guide Extraction, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+9 more)

### Community 181 - "clean_shutdown.rs"
Cohesion: 0.21
Nodes (16): ChildStderr, Child, Duration, ExitStatus, JoinHandle, Option, PathBuf, String (+8 more)

### Community 182 - "capture_real_corpus.py"
Cohesion: 0.21
Nodes (13): build_expected_results(), copy_wal_dir(), fetch_already_ingested_titles(), fetch_json(), fetch_wikitext(), ingest_from_staged(), Shared Phase-2 driver for both the combined default path and --ingest-only:…, GETs `url` with a descriptive User-Agent, retrying on 429 with backoff.… (+5 more)

### Community 183 - "attempt_uds_send"
Cohesion: 0.18
Nodes (13): acquire_uds_slot(), attempt_uds_send(), dial_uds(), PoisonGuard, PoisonGuard<'a>, resolve_embedding_timeout_ms(), Bytes, Drop (+5 more)

### Community 184 - ".log_mutation"
Cohesion: 0.18
Nodes (14): is_index_ddl(), log_mutation_leaves_non_vector_params_and_mutations_unaffected(), log_mutation_strip_is_a_no_op_for_null_params(), log_mutation_strips_vector_params_from_a_create(), log_mutation_strips_vector_params_from_a_dump_shaped_direct_call(), read_wal_lines(), resync_current_file_bytes_is_a_no_op_before_any_file_is_open(), resync_current_file_bytes_picks_up_an_out_of_band_shrink() (+6 more)

### Community 185 - ".new"
Cohesion: 0.15
Nodes (12): new_writer_on_empty_dir_mints_a_generation(), read_last_seq_tail_read_finds_last_line_in_large_file(), reopening_writer_reads_the_same_generation(), resync_global_seq_is_monotonic_against_empty_wal_dir(), resync_global_seq_picks_up_files_written_after_construction(), resync_global_seq_prefers_on_disk_scan_over_lower_last_committed_seq(), resync_global_seq_uses_last_committed_seq_as_a_floor(), Into (+4 more)

### Community 186 - "Golden Real-Corpus WAL Fixture"
Cohesion: 0.12
Nodes (16): CI wiring, Corpus, Golden Real-Corpus WAL Fixture, How this specific fixture was captured, Identifying a stale fixture, Known limitations of this fixture, MCP admin/lifecycle test suite (#236), Measured runtime (SC-005) (+8 more)

### Community 187 - "Issue #1 Spec: Foundation — Cargo Scaffold, CI, LadybugDB Spike, Library+Binary Symmetry"
Cohesion: 0.12
Nodes (16): Assumptions, Functional Requirements, Goal, In scope, Issue #1 Spec: Foundation — Cargo Scaffold, CI, LadybugDB Spike, Library+Binary Symmetry, Key Entities, Out of scope, Out of Scope (+8 more)

### Community 188 - "Feature Specification: Application WAL Not Written After Recreate — Regression of #74"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Application WAL Not Written After Recreate — Regression of #74, Functional Requirements, Measurable Outcomes, Out of Scope, Probable root cause hypotheses (+8 more)

### Community 189 - "Feature Specification: Prebuilt-Binary Release Workflow via cargo-dist"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Prebuilt-Binary Release Workflow via cargo-dist, Functional Requirements, Out of Scope, Requirements *(mandatory)*, Source References (+8 more)

### Community 190 - "Feature Specification: WAL Replay: Fix Apostrophe-Induced Parse Failures and Cascading Binder Errors"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: WAL Replay: Fix Apostrophe-Induced Parse Failures and Cascading Binder Errors, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 191 - "Feature Specification: Autonomous WAL-Corruption Self-Recovery"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Autonomous WAL-Corruption Self-Recovery, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 192 - "Feature Specification: Cross-Episode Entity Resolution — Fix Identical-Name Duplication at Ingest"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Cross-Episode Entity Resolution — Fix Identical-Name Duplication at Ingest, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 193 - "Feature Specification: Extend `reprocess_entity_types` — Off-Ontology and Full-Graph Re-typing with Additive Labels"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Extend `reprocess_entity_types` — Off-Ontology and Full-Graph Re-typing with Additive Labels, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 194 - "Feature Specification: Native MCP-over-stdio transport for the liminis-context-graph binary"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Native MCP-over-stdio transport for the liminis-context-graph binary, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 195 - "Feature Specification: Add `knowledge_reprocess_relation_types` — Fact-Based LLM Relation Classification"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Add `knowledge_reprocess_relation_types` — Fact-Based LLM Relation Classification, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 196 - "Feature Specification: Local/OpenAI-Compatible Extraction Adapter — Make the "Fully Local" Promise True"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Local/OpenAI-Compatible Extraction Adapter — Make the "Fully Local" Promise True, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 197 - "Feature Specification: Replace In-Process Entity-Name Lookup Map with a Secondary ART Index"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Replace In-Process Entity-Name Lookup Map with a Secondary ART Index, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 198 - "Feature Specification: Rust extraction-quality eval harness (replay + LLM-as-judge) over the public Wikipedia corpus"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Rust extraction-quality eval harness (replay + LLM-as-judge) over the public Wikipedia corpus, Functional Requirements, Key Entities *(if applicable)*, Measurable Outcomes, Out of Scope *(optional)* (+8 more)

### Community 199 - "Feature Specification: WAL replay transaction boundaries — defined recovery state on failure and cancellation"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: WAL replay transaction boundaries — defined recovery state on failure and cancellation, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+8 more)

### Community 200 - "Feature Specification: Benchmark run — full-corpus extraction on Anthropic vs local qwen3.6-27b, capturing cassettes for both"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Benchmark run — full-corpus extraction on Anthropic vs local qwen3.6-27b, capturing cassettes for both, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 201 - "Feature Specification: Ontology-aware extraction-quality evaluation"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Ontology-aware extraction-quality evaluation, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 202 - "Feature Specification: Tier 1b — Inventory + Semantic Search + Neighbors"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Tier 1b — Inventory + Semantic Search + Neighbors, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 203 - "Feature Specification: Constrain edge endpoints to the extracted entity set, and stop banning the concepts edges hub on"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Constrain edge endpoints to the extracted entity set, and stop banning the concepts edges hub on, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Measured impact (+8 more)

### Community 204 - "Feature Specification: GitHub Pages documentation site"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Decisions already made, Edge Cases, Feature Specification: GitHub Pages documentation site, Functional Requirements, Measurable Outcomes, Out of Scope (+8 more)

### Community 205 - "Feature Specification: `real-corpus-e2e` failed 24 consecutive runs over 4 days with no signal to anyone"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Current workflow inventory (as of this spec), Edge Cases, Feature Specification: `real-corpus-e2e` failed 24 consecutive runs over 4 days with no signal to anyone, Functional Requirements, Key Entities, Measurable Outcomes (+8 more)

### Community 206 - "Feature Specification: Strict mode still deletes out-of-vocabulary entities, while edges are now preserved"
Cohesion: 0.12
Nodes (17): Assumptions, Background, Decision: entities reclassify, they do not drop, Edge Cases, Fallback type and preservation field, Feature Specification: Strict mode still deletes out-of-vocabulary entities, while edges are now preserved, Functional Requirements, Key Entities *(if the feature involves data)* (+9 more)

### Community 207 - "Feature Specification: validate the extraction provider on first use, not at startup"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: validate the extraction provider on first use, not at startup, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+8 more)

### Community 208 - "Feature Specification: Include `breakdown` in `knowledge_reprocess_relation_types`' apply response"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Design Decision (stated deliberately, per issue's Edge Cases), Edge Cases, Feature Specification: Include `breakdown` in `knowledge_reprocess_relation_types`' apply response, Functional Requirements, Measurable Outcomes, Out of Scope (+8 more)

### Community 209 - "Feature Specification: Attribute delete_by_group and rebind_pointers WAL mutations to the groups they actually modify"
Cohesion: 0.12
Nodes (17): Assumptions, Background, Edge Cases, Feature Specification: Attribute delete_by_group and rebind_pointers WAL mutations to the groups they actually modify, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+9 more)

### Community 210 - "Feature Specification: Extend the Multi-Stream E2E Test to Cover Generation Reset, Cross-Group Merge and Ambiguous Resolution"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Extend the Multi-Stream E2E Test to Cover Generation Reset, Cross-Group Merge and Ambiguous Resolution, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 211 - "Feature Specification: `knowledge_delete_chunk_episode` and `knowledge_delete_by_source` must require an explicit group scope"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: `knowledge_delete_chunk_episode` and `knowledge_delete_by_source` must require an explicit group scope, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 212 - "Feature Specification: `knowledge_process_chunk` — warn, document, and count oversized `chunk_text`"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: `knowledge_process_chunk` — warn, document, and count oversized `chunk_text`, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 213 - "Feature Specification: Consistent Omitted-`group_ids` Semantics Across MCP Read Tools"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Consistent Omitted-`group_ids` Semantics Across MCP Read Tools, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 214 - "Feature Specification: Fix four tests failing on `main` behind the masked CI gate"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Fix four tests failing on `main` behind the masked CI gate, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 215 - "Feature Specification: Legacy WAL migration does not stamp .wal-generation.json"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Legacy WAL migration does not stamp .wal-generation.json, Functional Requirements, Measurable Outcomes, Out of Scope, Reproduction (carried from #428) (+8 more)

### Community 216 - "Feature Specification: Recompute embeddings on WAL replay with a content-addressed cache"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Recompute embeddings on WAL replay with a content-addressed cache, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope *(optional)* (+8 more)

### Community 217 - "Feature Specification: Close the 0.13.x documentation drift (breaking change, group_ids contract, assertion API, multi-graph entry points)"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Close the 0.13.x documentation drift (breaking change, group_ids contract, assertion API, multi-graph entry points), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 218 - "Feature Specification: Publish the docs site from release tags, not main, and keep every released version available"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Design Decisions Deferred to Plan, Edge Cases, Feature Specification: Publish the docs site from release tags, not main, and keep every released version available, Functional Requirements, Key Entities, Measurable Outcomes (+8 more)

### Community 219 - "Feature Specification: Batch WAL-replay embedding recompute calls"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Current architecture (as of this spec, post-#526), Edge Cases, Feature Specification: Batch WAL-replay embedding recompute calls, Functional Requirements, Key Entities, Measurable Outcomes (+8 more)

### Community 220 - "Feature Specification: Reconsider fatal embedder-unreachable startup failure for MCP-launched mode"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Costing findings, Decision (confirmed by the maintainer, 2026-08-25), Edge Cases, Feature Specification: Reconsider fatal embedder-unreachable startup failure for MCP-launched mode, Functional Requirements, Key Entities (+8 more)

### Community 221 - "Feature Specification: `--mcp-stdio`: complete the handshake without dialling, then dial `--connect` lazily per call"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: `--mcp-stdio`: complete the handshake without dialling, then dial `--connect` lazily per call, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 222 - "Feature Specification: WAL File Rotation by Size and Entry Count"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: WAL File Rotation by Size and Entry Count, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 223 - "Feature Specification: Port Graphiti's Extraction Prompts to liminis-graph for Quality Parity (Ontology-Aware)"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Port Graphiti's Extraction Prompts to liminis-graph for Quality Parity (Ontology-Aware), How this differs from #82, Out of Scope, Requirements *(mandatory)*, Source References (+8 more)

### Community 224 - "Feature Specification: Add `relation_type` Field to Edges — Separate Normalized SCREAMING_SNAKE_CASE Predicate Alongside `fact` Paraphrase"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Add `relation_type` Field to Edges — Separate Normalized SCREAMING_SNAKE_CASE Predicate Alongside `fact` Paraphrase, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+8 more)

### Community 225 - "handlers_wal_dump.rs"
Cohesion: 0.35
Nodes (15): dispatch(), make_state(), open_db(), req(), Arc, Path, Value, test_dump_wal_empty_graph() (+7 more)

### Community 226 - "real_corpus_replay_perf.rs"
Cohesion: 0.27
Nodes (15): cfg_if_uds(), connect_live_embedder(), cosine_similarity(), embedding_dim(), fixture_dir(), list_wal_files(), measure_cold_vs_warm_cache_replay_over_real_corpus_wal(), measure_replay_throughput_over_real_corpus_wal() (+7 more)

### Community 227 - "spawn_stub_auth_embedder"
Cohesion: 0.23
Nodes (15): base_cmd(), openai_api_key_fallback_logs_informational_notice_without_leaking_key(), Arc, Command, Mutex, Option, String, TempDir (+7 more)

### Community 228 - "mcp_multistream_e2e.rs"
Cohesion: 0.33
Nodes (15): assert_entity(), assert_relationship(), binding_states_into(), c_bindings(), cross_group_edge(), dst_pointer(), multistream_layer_graph_composition(), PointerInfo (+7 more)

### Community 229 - "mcp_real_corpus_mutation_e2e.rs"
Cohesion: 0.22
Nodes (14): active_edges(), find_by_name(), flip_case(), mcp_write_path_over_real_corpus_fixture(), Arc, Mutex, Path, String (+6 more)

### Community 230 - "Decisions"
Cohesion: 0.12
Nodes (16): 1. Resolution logic lives in a new `assert.rs`, sharing only the forward-walk with `cross_group.rs`, 2. `valid_at` is scoped to `knowledge_assert_relationship` only, 3. `entity_uuid` is a strict group-scoped lookup — never a caller-chosen mint path, 4. `embedding_warning` is a response field, not just a log line, 5. The edge upsert match excludes invalidated edges, 6. An update never rewrites the stored embedding — only create does, 7. "Empty embedding" on embedder failure is a same-dimension zero vector, not a literal empty list, ADR-0379: Direct Assertion API Conventions (+8 more)

### Community 231 - "buildRouter"
Cohesion: 0.23
Nodes (14): HTTPResponse, buildRouter(), registerEmbeddingsRoute(), BasicRequestContext, estimateTokens(), handleChatCompletions(), handleHealth(), makeErrorResponse() (+6 more)

### Community 232 - "Tasks: Issue #2 — IPC Parity (US1)"
Cohesion: 0.12
Nodes (15): Constitution Compliance, Dependencies & Execution Order, Format: `[ID] [P?] [US1] [Constitution-tag?] Description`, Parallel Opportunities, Phase 10: Polish & Principle II Gate, Phase 1: Dependencies & LadybugDB Spike, Phase 2: Schema & Type Extensions, Phase 3: Adapter Implementations (+7 more)

### Community 233 - "Issue #4 Spec: Concurrent Reader/Writer with Per-Role LLM Routing"
Cohesion: 0.12
Nodes (15): Assumptions, Functional Requirements, Goal, In scope, Issue #4 Spec: Concurrent Reader/Writer with Per-Role LLM Routing, Key Entities, Out of scope, Out of Scope (+7 more)

### Community 234 - "Tasks: Issue #4 — Concurrent Reader/Writer with Per-Role LLM Routing"
Cohesion: 0.12
Nodes (15): Constitution Compliance Summary, Dependencies & Execution Order, Format: `[ID] [P?] [Story] [Constitution-tag?] Description`, Implementation, Implementation, Implementation, Phase 1: Foundation (Blocking Prerequisites), Phase 2: User Story 1 — Reader/Writer Split (Priority: P1) (+7 more)

### Community 235 - "Architecture Decisions"
Cohesion: 0.12
Nodes (15): AD-1: `hybrid_dedup_similar_entity` is a synchronous `Conn` method in `db.rs`, AD-2: candidate_k = 10 per retrieval path before RRF, AD-3: Post-RRF cosine recheck via batch embedding fetch, AD-4: Configurable threshold via `LIMINIS_DEDUP_HYBRID_THRESHOLD` env var, AD-5: Python baseline committed as `benches/python_baseline_ns.json`, AD-6: Bench job runs actual iterations in CI, gated by scale, Architecture Decisions, Complexity Tracking (+7 more)

### Community 236 - "Feature Specification: Safe `.graphiti/` → `.lcg/` Workspace Migration That Restructures File Layout"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Safe `.graphiti/` → `.lcg/` Workspace Migration That Restructures File Layout, Out of Scope, Requirements *(mandatory)*, Source References, Success Criteria *(mandatory)* (+7 more)

### Community 237 - "Feature Specification: WAL Replay Timestamp Typing — Fix STRING→TIMESTAMP Cast Failures"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: WAL Replay Timestamp Typing — Fix STRING→TIMESTAMP Cast Failures, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 238 - "Feature Specification: Publish Versioned macOS Release Binary for liminis-context-graph"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Publish Versioned macOS Release Binary for liminis-context-graph, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+7 more)

### Community 239 - "Feature Specification: Batch WAL Replay Writes via UNWIND for Throughput"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Batch WAL Replay Writes via UNWIND for Throughput, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 240 - "Feature Specification: Reload owns index maintenance — drop FTS before WAL replay, build all indexes once after"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Reload owns index maintenance — drop FTS before WAL replay, build all indexes once after, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 241 - "Feature Specification: knowledge_dump_wal — DB→WAL Dump / Compaction"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: knowledge_dump_wal — DB→WAL Dump / Compaction, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 242 - "Feature Specification: knowledge_merge_entities — Collapse Duplicate Entities"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: knowledge_merge_entities — Collapse Duplicate Entities, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 243 - "Feature Specification: Fix knowledge_merge_entities TIMESTAMP Coercion Bug"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Fix knowledge_merge_entities TIMESTAMP Coercion Bug, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 244 - "Feature Specification: Eager HNSW Index Build + Dedup-Path Auto-Heal to Fix Missing-Index Ingest Failures"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Eager HNSW Index Build + Dedup-Path Auto-Heal to Fix Missing-Index Ingest Failures, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 245 - "Feature Specification: Restore an Indexed Access Path for Entity Name Lookup"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Restore an Indexed Access Path for Entity Name Lookup, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 246 - "Feature Specification: Document Extraction-Quality Evaluation — Methodology, Model Rankings, and Local-LLM Guidance"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Document Extraction-Quality Evaluation — Methodology, Model Rankings, and Local-LLM Guidance, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 247 - "Feature Specification: UDS embedder dials a new connection + handshake + detached task per embedding call — pool it"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: UDS embedder dials a new connection + handshake + detached task per embedding call — pool it, Functional Requirements, Key Entities *(if applicable)*, Measurable Outcomes, Out of Scope (+7 more)

### Community 248 - "Feature Specification: Fix Silent Data Loss in WAL Replay (Discarded Stats, Out-of-Sequence Files, Uncounted No-Ops)"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Fix Silent Data Loss in WAL Replay (Discarded Stats, Out-of-Sequence Files, Uncounted No-Ops), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 249 - "Feature Specification: WAL replay diagnostics — deduplicated failure samples, safe rebuild semantics, honest fidelity warnings"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: WAL replay diagnostics — deduplicated failure samples, safe rebuild semantics, honest fidelity warnings, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+7 more)

### Community 250 - "Feature Specification: blind pairwise judging for lcg-eval"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: blind pairwise judging for lcg-eval, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 251 - "Feature Specification: Tier 1a Service Handshake Methods"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Tier 1a Service Handshake Methods, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 252 - "Feature Specification: move the benchmark guards out of shell and into lcg-eval"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: move the benchmark guards out of shell and into lcg-eval, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+7 more)

### Community 253 - "Feature Specification: #219's NameIndex silently narrowed #218's global endpoint fallback"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: #219's NameIndex silently narrowed #218's global endpoint fallback, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 254 - "Feature Specification: Capture extraction failures whole, and surface truncation in the eval report"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Capture extraction failures whole, and surface truncation in the eval report, Functional Requirements, Key Entities *(include if feature involves data)*, Measurable Outcomes, Out of Scope (+7 more)

### Community 255 - "Feature Specification: Tier 3 — Corrections Workflow (apply_corrections, validate_corrections, reprocess_entity_types)"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Tier 3 — Corrections Workflow (apply_corrections, validate_corrections, reprocess_entity_types), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 256 - "Feature Specification: the 15–18 minute release test suite is the root cause behind the headless-stall class"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: the 15–18 minute release test suite is the root cause behind the headless-stall class, Functional Requirements, Measurable Outcomes, Out of Scope, Related in-flight work (+7 more)

### Community 257 - "Feature Specification: Reject semantically-empty required fields during item salvage"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Reject semantically-empty required fields during item salvage, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 258 - "Feature Specification: Group-scoped complete purge: remove entities and edges, not just episodes"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Group-scoped complete purge: remove entities and edges, not just episodes, Functional Requirements, Key Entities, Measurable Outcomes, Notes (+7 more)

### Community 259 - "Feature Specification: Group-Scope Duplicate-Edge Detection During Entity Merge"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Group-Scope Duplicate-Edge Detection During Entity Merge, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 260 - "Feature Specification: Resolvable semantic pointers for cross-graph references"
Cohesion: 0.12
Nodes (16): Assumptions, Background, Edge Cases, Feature Specification: Resolvable semantic pointers for cross-graph references, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+8 more)

### Community 261 - "Feature Specification: Merge must never write another group's data"
Cohesion: 0.12
Nodes (16): ADR Requirements, Assumptions, Background, Edge Cases, Feature Specification: Merge must never write another group's data, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes (+8 more)

### Community 262 - "Feature Specification: Direct assertion API — `knowledge_assert_entity` / `knowledge_assert_relationship`"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Direct assertion API — `knowledge_assert_entity` / `knowledge_assert_relationship`, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 263 - "Feature Specification: Tier 1b Bug — Edge-as-Node Schema Mismatch in Relationship Queries"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Tier 1b Bug — Edge-as-Node Schema Mismatch in Relationship Queries, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 264 - "Feature Specification: Upgrade lbug dependency from 0.17.0 to 0.19.1"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Upgrade lbug dependency from 0.17.0 to 0.19.1, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+7 more)

### Community 265 - "Feature Specification: Correct the documented meaning of edge `episode_uuids`"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Correct the documented meaning of edge `episode_uuids`, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+7 more)

### Community 266 - "Feature Specification: WAL Stream Generation — Publish Contract & Unknown-State Guard"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: WAL Stream Generation — Publish Contract & Unknown-State Guard, Functional Requirements, Key Entities *(include if feature involves data)*, Measurable Outcomes, Out of Scope (+7 more)

### Community 267 - "Feature Specification: Required CI test gate cannot fail — tee without pipefail masks test failures"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Required CI test gate cannot fail — tee without pipefail masks test failures, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 268 - "Feature Specification: Fix startup migration ordering so legacy WAL files are relocated, not left loose and invisible"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Fix startup migration ordering so legacy WAL files are relocated, not left loose and invisible, Functional Requirements, How it surfaced, Key Entities, Measurable Outcomes (+7 more)

### Community 269 - "Feature Specification: Embedder Sidecar HTTP Server (BGE bge-base-en-v1.5)"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Embedder Sidecar HTTP Server (BGE bge-base-en-v1.5), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 270 - "Feature Specification: Batch embedding API for bulk-extraction ingest and summary-embedding backfill"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Batch embedding API for bulk-extraction ingest and summary-embedding backfill, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 271 - "Feature Specification: Per-Group Ontology Support"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Per-Group Ontology Support, Functional Requirements, Key Entities *(if applicable)*, Measurable Outcomes, Out of Scope (+7 more)

### Community 272 - "Feature Specification: Bearer-Token Authentication for the Embedder HTTP Transport"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Bearer-Token Authentication for the Embedder HTTP Transport, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+7 more)

### Community 273 - "Feature Specification: Integration Tests Leak Spawned liminis-context-graph Processes"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Integration Tests Leak Spawned liminis-context-graph Processes, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 274 - "Feature Specification: Swift sidecar: select embeddings / completions / both, so completions-only costs no model setup"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Swift sidecar: select embeddings / completions / both, so completions-only costs no model setup, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 275 - "Feature Specification: Document embedding options and conventions: one capability matrix, local and remote"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Document embedding options and conventions: one capability matrix, local and remote, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 276 - "Feature Specification: Fix missing `weight.bin` in Swift CoreML test fixtures"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Fix missing `weight.bin` in Swift CoreML test fixtures, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 277 - "Feature Specification: Vectors are a local cache — stop writing them to the WAL, ignore them on replay"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Vectors are a local cache — stop writing them to the WAL, ignore them on replay, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 278 - "Feature Specification: Upgrade lbug 0.19.1 -> 0.20.1 (storage 43 -> 47) so 0.14.0 carries a single migration"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Upgrade lbug 0.19.1 -> 0.20.1 (storage 43 -> 47) so 0.14.0 carries a single migration, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+7 more)

### Community 279 - "Feature Specification: `knowledge_strip_wal_embeddings` — Strip Embedding Vectors from Pre-0.14 WALs"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: `knowledge_strip_wal_embeddings` — Strip Embedding Vectors from Pre-0.14 WALs, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 280 - "Feature Specification: Fast Clean Shutdown — Cancel In-Flight LLM Work on SIGTERM Instead of Waiting Out the Inner Timeout"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Fast Clean Shutdown — Cancel In-Flight LLM Work on SIGTERM Instead of Waiting Out the Inner Timeout, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+7 more)

### Community 281 - "Feature Specification: liminis-graph speaks OpenAI-compatible embeddings over UDS by default, with HTTP as opt-in"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: liminis-graph speaks OpenAI-compatible embeddings over UDS by default, with HTTP as opt-in, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope *(optional)* (+7 more)

### Community 282 - "Feature Specification: Ontology Drift Detection — Notify When the Ontology Has Changed Since the Last Ingestion"
Cohesion: 0.12
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Ontology Drift Detection — Notify When the Ontology Has Changed Since the Last Ingestion, Out of Scope, Requirements *(mandatory)*, Source References, Success Criteria *(mandatory)* (+7 more)

### Community 283 - "test_capture_real_corpus.py"
Cohesion: 0.23
Nodes (6): Removes balanced {{ ... }} templates (infoboxes, citations, etc.). Real…, Removes balanced [[File:...]] / [[Image:...]] / [[Category:...]] links,…, strip_file_links(), strip_templates(), wikitext_to_prose(), WikitextToProseTests

### Community 284 - "wal_vector_stripping.rs"
Cohesion: 0.29
Nodes (14): dispatch(), embedding_dim(), fixture_dir(), fresh_wal_dump_has_no_vector_params_and_is_dramatically_smaller(), jsonl_files(), make_state(), original_wal_dir(), req() (+6 more)

### Community 285 - "embedder_degraded_mcp.rs"
Cohesion: 0.27
Nodes (14): base_cmd(), bind_ephemeral_listener(), knowledge_recover_rejected_while_embedder_unreachable_degraded(), mcp_stdio_degrades_when_embedder_accepts_but_never_responds(), mcp_stdio_degrades_when_embedder_never_reachable(), mcp_stdio_recovers_when_embedder_becomes_reachable_mid_retry(), reserve_ephemeral_port_hint(), Command (+6 more)

### Community 286 - "Decision"
Cohesion: 0.13
Nodes (15): Accounting is deferred to commit time, not applied inline, ADR-0047: WAL Replay Transaction Boundaries — Batch-Aligned, Not Chunk-Aligned, Alternatives Considered, `BEGIN`/`COMMIT` failures are hard errors, by design, Cancellation is checked per-row, inside the transaction, Consequences, Context, Decision (+7 more)

### Community 287 - "Decisions"
Cohesion: 0.13
Nodes (15): 10. `BackendPair` realized as plain nested-index iteration, not a struct, 1. `JudgeCache` generalized via an internal untagged enum, not a second cache file, 2. Deterministic slot assignment via SHA-256, not `DefaultHasher`, 3. Chunk text embedded symmetrically in the cache key, not in `cache_key`'s signature, 4. One `judge_pairwise` call per axis, not one call covering all three, 5. Win rate excludes ties from the denominator, 6. Calibration band and inconsistency threshold are new, chosen constants, 7. FR-011 degenerate-pair rejection scoped narrowly to `cassette:path=` equality (+7 more)

### Community 288 - "ADR-0371: Merge Skips Foreign-Group Edges Entirely; `merged_into` Forwarding Closes the Rename Gap"
Cohesion: 0.13
Nodes (15): A merge in group G touches only edges whose `group_id == G`, ADR-0371: Merge Skips Foreign-Group Edges Entirely; `merged_into` Forwarding Closes the Rename Gap, Alternatives Considered, Bound hop-count cap instead of a visited-`HashSet` for the cycle guard, Consequences, Context, Decision, Keep rewriting foreign edges, but attribute the mutation to the foreign group's own stream (+7 more)

### Community 289 - "Feature Specification: Fix Ontology Drift Detection for First-Ever Ontology Addition"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Fix Ontology Drift Detection for First-Ever Ontology Addition, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+6 more)

### Community 290 - "Feature Specification: Spike — native cross-platform Rust embedder for liminis-graph (candle vs ort)"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Spike — native cross-platform Rust embedder for liminis-graph (candle vs ort), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 291 - "Feature Specification: WAL Replayer Must Distinguish Failed Mutations From Unrecognised Lines"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: WAL Replayer Must Distinguish Failed Mutations From Unrecognised Lines, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+6 more)

### Community 292 - "Feature Specification: Bump lbug Pin From 0.16.1 to 0.17.0"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Bump lbug Pin From 0.16.1 to 0.17.0, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+6 more)

### Community 293 - "Feature Specification: WAL Replay Real-Time Progress — Add `files_total` to `ReplayProgress`"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: WAL Replay Real-Time Progress — Add `files_total` to `ReplayProgress`, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 294 - "Feature Specification: Schema Parity — `RelatesToNode_` Missing `expired_at` Column → WAL Replay Drops Expired/Invalidated Relationships"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Schema Parity — `RelatesToNode_` Missing `expired_at` Column → WAL Replay Drops Expired/Invalidated Relationships, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 295 - "Feature Specification: Periodically Log WAL-Replay Progress to the Service Log"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Periodically Log WAL-Replay Progress to the Service Log, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 296 - "Feature Specification: Fix Hybrid Dedup Overlap Failure (R-003)"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Fix Hybrid Dedup Overlap Failure (R-003), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 297 - "Feature Specification: `knowledge_rebuild_from_wal` Must Leave Entity/Relationship Search Immediately Queryable"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: `knowledge_rebuild_from_wal` Must Leave Entity/Relationship Search Immediately Queryable, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 298 - "Feature Specification: Resolve Edge Endpoints Against the Global Entity Table"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Resolve Edge Endpoints Against the Global Entity Table, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 299 - "Feature Specification: Lint Sweep — 6 Clippy Errors + 20 fmt Diffs from Rust 1.94"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Lint Sweep — 6 Clippy Errors + 20 fmt Diffs from Rust 1.94, Functional Requirements, Key Files, Measurable Outcomes, Out of Scope (+6 more)

### Community 300 - "Feature Specification: Golden Real-Corpus WAL Fixture + Rebuild→Assert E2E Harness"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Golden Real-Corpus WAL Fixture + Rebuild→Assert E2E Harness, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 301 - "Feature Specification: OaiExtractor UDS path — adopt pooled connection (same defect as #229)"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: OaiExtractor UDS path — adopt pooled connection (same defect as #229), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 302 - "Feature Specification: MCP Read-Path E2E Suite Over the Real-Corpus Fixture"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: MCP Read-Path E2E Suite Over the Real-Corpus Fixture, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 303 - "Feature Specification: Bound Prepared-Statement Growth During WAL Replay"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Bound Prepared-Statement Growth During WAL Replay, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 304 - "Feature Specification: Capture and Log the Sender PID of Received SIGTERM"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Capture and Log the Sender PID of Received SIGTERM, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 305 - "Feature Specification: Cassette replay backend for `lcg-eval`"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Cassette replay backend for `lcg-eval`, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 306 - "Feature Specification: make lcg-eval's scoring loop testable"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: make lcg-eval's scoring loop testable, Functional Requirements, Labels, Measurable Outcomes, Out of Scope (+6 more)

### Community 307 - "Feature Specification: `indices_built` is not set after runtime recovery, so `knowledge_status` under-reports readiness"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: `indices_built` is not set after runtime recovery, so `knowledge_status` under-reports readiness, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+6 more)

### Community 308 - "Feature Specification: two e2e tests call `knowledge_rebuild_from_wal` without `force_clear` and have failed since the guard landed"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: two e2e tests call `knowledge_rebuild_from_wal` without `force_clear` and have failed since the guard landed, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 309 - "Feature Specification: Strict ontology mode discards relations the ontology knows how to keep"
Cohesion: 0.13
Nodes (15): Assumptions, Background, Edge Cases, Feature Specification: Strict ontology mode discards relations the ontology knows how to keep, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+7 more)

### Community 310 - "Feature Specification: Salvage malformed extracted items instead of failing the whole chunk"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Salvage malformed extracted items instead of failing the whole chunk, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 311 - "Feature Specification: CI — migrate remaining Node 20 actions to Node 24 releases"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: CI — migrate remaining Node 20 actions to Node 24 releases, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+6 more)

### Community 312 - "Feature Specification: Re-derive `WalWriter` `global_seq` after rebuild/clear to prevent duplicate WAL seqs"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Re-derive `WalWriter` `global_seq` after rebuild/clear to prevent duplicate WAL seqs, Functional Requirements, Impact, Measurable Outcomes, Out of Scope (+6 more)

### Community 313 - "Feature Specification: Bounded WAL replay — `to_seq` upper bound for `knowledge_rebuild_from_wal`"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Bounded WAL replay — `to_seq` upper bound for `knowledge_rebuild_from_wal`, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+6 more)

### Community 314 - "Feature Specification: Cheap WAL seq bounds — eliminate the full-directory scan in `wal_max_seq`/`wal_min_seq`"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Cheap WAL seq bounds — eliminate the full-directory scan in `wal_max_seq`/`wal_min_seq`, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 315 - "Feature Specification: Rebind Pointers Must Repair Already-Unbound Cross-Group Pointers"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Rebind Pointers Must Repair Already-Unbound Cross-Group Pointers, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 316 - "Feature Specification: Embedder Sidecar for liminis-graph"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Embedder Sidecar for liminis-graph, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 317 - "Feature Specification: Assert handlers compute embeddings before the existence check"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Assert handlers compute embeddings before the existence check, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+6 more)

### Community 318 - "Feature Specification: knowledge_status cannot distinguish an empty group from an unhydrated one"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: knowledge_status cannot distinguish an empty group from an unhydrated one, Functional Requirements, Key Entities *(include if feature involves data)*, Measurable Outcomes, Out of Scope (+6 more)

### Community 319 - "Feature Specification: Semantic search over Entity summaries"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Semantic search over Entity summaries, Functional Requirements, Key Entities *(the feature involves data)*, Measurable Outcomes, Out of Scope (+6 more)

### Community 320 - "Feature Specification: Streaming WAL-rebuild's build_ok doesn't reflect a failed lookup_key backfill"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Streaming WAL-rebuild's build_ok doesn't reflect a failed lookup_key backfill, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 321 - "Feature Specification: Close (or formally accept) the resolve_ontology stale-drift-insert race"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Close (or formally accept) the resolve_ontology stale-drift-insert race, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 322 - "Feature Specification: Re-enable Swift Sidecar CI"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Re-enable Swift Sidecar CI, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 323 - "Feature Specification: Consolidate the Swift Sidecar — lcg is Source of Truth"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Consolidate the Swift Sidecar — lcg is Source of Truth, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+6 more)

### Community 324 - "Feature Specification: Bounded Timeouts for `OaiEmbedder`'s UDS Transport"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Bounded Timeouts for `OaiEmbedder`'s UDS Transport, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+6 more)

### Community 325 - "Feature Specification: Fix TOCTOU port race in `embedder_degraded_mcp.rs` test helpers"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Fix TOCTOU port race in `embedder_degraded_mcp.rs` test helpers, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+6 more)

### Community 326 - "Feature Specification: Fix clean_shutdown Integration Test on macOS (lbug hash_index Assertion)"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Fix clean_shutdown Integration Test on macOS (lbug hash_index Assertion), Functional Requirements, Investigation Approach, Measurable Outcomes, Out of Scope (+6 more)

### Community 327 - "Feature Specification: Standardize IPC Collection Responses to `{<key>: [...], count: N}` Envelope Shape"
Cohesion: 0.13
Nodes (14): Assumptions, Background, Edge Cases, Feature Specification: Standardize IPC Collection Responses to `{<key>: [...], count: N}` Envelope Shape, Out of Scope, Requirements *(mandatory)*, Source References, Success Criteria *(mandatory)* (+6 more)

### Community 328 - "liminis-context-graph — Claude guidance"
Cohesion: 0.14
Nodes (14): ADR numbers are issue numbers (NON-NEGOTIABLE), Build artifact, Debugging a live or degraded service, liminis-context-graph — Claude guidance, Long-running commands (NON-NEGOTIABLE), Pre-spec ideas (`ideas/`), Running performance benchmarks, Rust pre-commit checks (MUST run before every commit) (+6 more)

### Community 329 - "Contributing to Liminis Context Graph"
Cohesion: 0.14
Nodes (14): Architecture decisions, Checking for leaked test processes, CI failure issues, Contributing to Liminis Context Graph, Feature specifications, Filing issues, Milestones are named, not numbered, until the release is cut, No CLA, no DCO (+6 more)

### Community 330 - "stage_corpus"
Cohesion: 0.16
Nodes (10): Fetches and cleans each of `articles` into prose, in manifest order. Zero…, Best-effort git SHA of this script's own commit, for the corpus_prose.jsonl…, Writes `staged` as plain (uncompressed) JSONL behind a header record, so…, Reads back a `corpus_prose.jsonl` written by `write_corpus_prose`. Returns…, read_corpus_prose(), script_git_sha(), stage_corpus(), write_corpus_prose() (+2 more)

### Community 331 - "send_and_read_uds"
Cohesion: 0.15
Nodes (10): PoisonGuard, PoisonGuard<'a>, AtomicUsize, Bytes, Drop, Mutex, UdsSender, send_and_read_uds() (+2 more)

### Community 332 - "String"
Cohesion: 0.21
Nodes (14): apply_key_resolution(), FailureSample, find_word(), inject_derived_lookup_key(), is_vector_only_set(), PendingRow, ReplayProgress, ResolvedRow (+6 more)

### Community 333 - "token_budget.rs"
Cohesion: 0.22
Nodes (10): compute_initial_max_tokens(), compute_initial_max_tokens_applies_floor_for_small_chunks(), compute_initial_max_tokens_clamps_to_ceiling(), compute_initial_max_tokens_corpus_max_chunk_stays_under_ceiling(), compute_initial_max_tokens_does_not_panic_when_ceiling_is_below_floor(), compute_initial_max_tokens_edges_ratio_exceeds_entities_ratio(), compute_initial_max_tokens_scales_with_input_size(), ExtractionCallType (+2 more)

### Community 334 - "entity_create_wal_line"
Cohesion: 0.18
Nodes (14): batch_fallback_rolls_back_whole_batch_on_bad_row(), batch_file_boundary_flushes_between_files(), batch_same_template_groups_into_unwind(), batch_size_one_matches_per_row(), batch_template_boundary_flushes_correctly(), entity_create_wal_line(), entity_merge_wal_line(), episodic_merge_wal_line() (+6 more)

### Community 335 - "SeededWorkspace"
Cohesion: 0.22
Nodes (8): copy_dir_recursive(), fixture_dir(), Path, PathBuf, TempDir, Value, seed_real_corpus_workspace(), SeededWorkspace

### Community 336 - "Decision"
Cohesion: 0.14
Nodes (14): ADR-0006: HTTP Embedding Sidecar Contract, Concurrency, Consequences, Context, Decision, Env Var Linkage, Env Vars: `LCG_EMBEDDING_URL`, not `EMBEDDER_HOST`/`EMBEDDER_PORT`, Error Responses (+6 more)

### Community 337 - "Decision"
Cohesion: 0.14
Nodes (13): 1. Separate WalWriter for dump output — never use `state.wal_writer`, 2. Two-phase ordering: nodes before edges, 3. RELATES_TO properties stored on RelatesToNode_ shadow node — Phase 2 is structural only, 4. Embeddings stored as JSON numeric arrays (`FLOAT[]` not `vecf32(...)`), 5. Optional timestamps use `CASE WHEN $x IS NULL THEN NULL ELSE timestamp($x) END`, 6. Pagination via SKIP/LIMIT with page size 500, 7. Write lock held for the entire dump duration, 8. Partial failure cleanup (+5 more)

### Community 338 - "ADR-0029: Name-First Entity Resolution in add_episode Phase B"
Cohesion: 0.14
Nodes (13): Add a `case_insensitive: bool` flag to `get_entity_by_name`, ADR-0029: Name-First Entity Resolution in add_episode Phase B, Alternatives Considered, Amendment (2026-07-24, issue #209), Amendment (2026-08-16, issue #398), Consequences, Context, Decision (+5 more)

### Community 339 - "Decisions"
Cohesion: 0.14
Nodes (13): 1. New `crates/eval` workspace member, not an `examples/` binary, 2. New `crates/core` telemetry event, not an `Err(Error)`-only inference, 3. The judge is a standalone Anthropic client, not `AnthropicExtractor` reused, 4. Judge prompt, P/R/F1 derivation, and failure taxonomy are ported verbatim, 5. Judge cache key scheme is ported from the source, extended with `judge_model`, 6. Default corpus subset is the first 50 chunks of the #217 fixture, not all 228, 7. Hand-rolled CLI parsing, no `clap`, 8. `eval.yml` runs only a baseline-vs-itself smoke pass (+5 more)

### Community 340 - "Decision"
Cohesion: 0.14
Nodes (14): ADR-0295: GitHub Pages Documentation Site, ADR front matter via one `defaults:` scope, not 57 file edits, Alternatives Considered, Consequences, Context, Decision, `docs/llms.txt` + generated, drift-checked `docs/llms-full.txt`, `exclude:` replaces Jekyll's own default list — restated it explicitly, plus `vendor`/`.bundle` (+6 more)

### Community 341 - "ADR-0322: CI Docs-Only Fast Path via Job-Level Skip"
Cohesion: 0.14
Nodes (14): 1. Skip the job via `if:`, never filter the workflow via `paths`/`paths-ignore`, 2. Classification lives in one inline bash step, not a third-party action, 3. Any classification error defaults to `code_changed=true`, never to a skip, 4. Classification is a deny-list keyed on what actually affects the Rust build/test job, 5. `push` always runs the full suite unconditionally; no fast path for it, 6. `test`'s gate restates the implicit `build-lbug` success dependency, ADR-0322: CI Docs-Only Fast Path via Job-Level Skip, Alternatives Considered (+6 more)

### Community 342 - "Decision"
Cohesion: 0.14
Nodes (14): 1. `build-release` is the workspace's one and only `cargo build --release` per run, 2. One mechanism (same-run artifacts), two different scopes, 3. Bodily merge into `ci.yml`, not a `workflow_call` orchestrator, 4. Every job gets the same log-grep recompile guard (FR-006/Story 4), 5. `build-lbug` is absorbed into `build-release`, not run alongside it, 6. One shared classifier output, not two, 7. `workflow_dispatch` gains an `e2e_only` input, 8. `ci-failure-notify.yml`'s listener moves from "Real-Corpus E2E" to "CI" (+6 more)

### Community 343 - "ADR-0402: Per-Group Mutation Attribution for Multi-Group Episode Deletes"
Cohesion: 0.14
Nodes (14): A new `_grouped` variant alongside the existing `Db` methods, ADR-0402: Per-Group Mutation Attribution for Multi-Group Episode Deletes, Alternatives Considered, Consequences, Context, Decision, Document the shared-default-group WAL attribution as safe (FR-003's other permitted outcome), `handlers.rs`: the same per-group flush loop as `handle_delete_by_group` (+6 more)

### Community 344 - "Decision"
Cohesion: 0.14
Nodes (14): 1. A pure filesystem transform, not a DB operation, 2. Two-pass per file: a read-only pass decides whether any I/O is needed at all, 3. Byte-for-byte pass-through for every line that doesn't change, 4. Malformed value: fail the whole file, before anything is written. Unparseable line: pass through, never fail., 5. Atomic replace: tmp file in the same directory, rename on success only, 6. Discovery: multi-stream layout first, legacy flat layout as defense-in-depth, 7. Concurrency: the whole-WAL-directory write lock, held for the whole call, 8. Resyncing a live writer's rotation bookkeeping after a rewrite (+6 more)

### Community 345 - "Full-corpus extraction benchmark runbook (#248)"
Cohesion: 0.14
Nodes (14): Committing the cassettes, Full-corpus extraction benchmark runbook (#248), Pairwise judging pass (#269), Prerequisites, Prerequisites, Re-running after a partial failure, Reading the report, Reading the three reports (+6 more)

### Community 346 - "038B1EE0-2AF2-48F0-B401-E6460DB2A7F8"
Cohesion: 0.14
Nodes (13): author, description, name, path, author, description, name, path (+5 more)

### Community 347 - "A9455776-87AF-40BE-9601-89C752C3866C"
Cohesion: 0.14
Nodes (13): author, description, name, path, author, description, name, path (+5 more)

### Community 348 - "5ADDBC7B-30D6-4DE2-AC1C-707AA4904044"
Cohesion: 0.14
Nodes (13): author, description, name, path, author, description, name, path (+5 more)

### Community 349 - "68A9BCBE-6E95-400D-8604-19FCA73C7ABA"
Cohesion: 0.14
Nodes (13): author, description, name, path, author, description, name, path (+5 more)

### Community 350 - "42571EF2-B78E-43F9-9F0B-EB43ABBD84B6"
Cohesion: 0.14
Nodes (13): author, description, name, path, author, description, name, path (+5 more)

### Community 351 - "Feature Specification: Spike — ort with CoreML execution provider on macOS (can it match or beat the Swift sidecar?)"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Spike — ort with CoreML execution provider on macOS (can it match or beat the Swift sidecar?), Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope *(optional)* (+5 more)

### Community 352 - "Feature Specification: Cache lbug C++ Build Artifacts Across CI Runs to Cut PR Wall-Clock From ~1h to ~15min"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Cache lbug C++ Build Artifacts Across CI Runs to Cut PR Wall-Clock From ~1h to ~15min, Functional Requirements, Out of Scope, Requirements *(mandatory)*, Source References (+5 more)

### Community 353 - "Feature Specification: Legacy-WAL Replay Compatibility — `episodes` Schema Parity + FalkorDB-Dialect Translation (VECF32, bulk-SET)"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Legacy-WAL Replay Compatibility — `episodes` Schema Parity + FalkorDB-Dialect Translation (VECF32, bulk-SET), Out of Scope, Requirements *(mandatory)*, Source References, Success Criteria *(mandatory)* (+5 more)

### Community 354 - "Feature Specification: Deprecate `knowledge_backfill_relation_types` In Place — Stop Implying It Classifies"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Deprecate `knowledge_backfill_relation_types` In Place — Stop Implying It Classifies, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+5 more)

### Community 355 - "Feature Specification: Attached MCP Mode — Fix False-Timeout on Long Whole-Graph Ops and Add Reconnect After Service Restart"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Attached MCP Mode — Fix False-Timeout on Long Whole-Graph Ops and Add Reconnect After Service Restart, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+5 more)

### Community 356 - "Feature Specification: Multi-Stream / Layer-Graph E2E Test"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Multi-Stream / Layer-Graph E2E Test, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+5 more)

### Community 357 - "Feature Specification: Determine and resolve cross-group WAL attribution for multi-group episode deletes"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Determine and resolve cross-group WAL attribution for multi-group episode deletes, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+5 more)

### Community 358 - "Feature Specification: Report dropped-edge detail from `knowledge_process_chunk`"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Report dropped-edge detail from `knowledge_process_chunk`, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+5 more)

### Community 359 - "Feature Specification: Socket bind precedes #378 WAL-root migration, so readiness can be observed before migration completes"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Socket bind precedes #378 WAL-root migration, so readiness can be observed before migration completes, Functional Requirements, Key Entities *(if applicable)*, Measurable Outcomes, Out of Scope (+5 more)

### Community 360 - "Feature Specification: `migrate_workspace` must honor `LCG_WAL_DIR`/`GRAPHITI_WAL_DIR` when relocating legacy `.graphiti/wal`"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: `migrate_workspace` must honor `LCG_WAL_DIR`/`GRAPHITI_WAL_DIR` when relocating legacy `.graphiti/wal`, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+5 more)

### Community 361 - "Feature Specification: Cover embedder cassette recordings with API-key leak tests"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Cover embedder cassette recordings with API-key leak tests, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+5 more)

### Community 362 - "Feature Specification: Unify relationship/edge response keys across read RPCs"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Unify relationship/edge response keys across read RPCs, Functional Requirements, Key Entities, Measurable Outcomes, Out of Scope (+5 more)

### Community 363 - "Feature Specification: Structured Attributes on Episodes"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Structured Attributes on Episodes, Functional Requirements, Key Entities *(if the feature involves data)*, Measurable Outcomes, Out of Scope (+5 more)

### Community 364 - "Feature Specification: Narrow `state.write_lock`'s critical section around the embedder round trip"
Cohesion: 0.14
Nodes (13): Assumptions, Background, Edge Cases, Feature Specification: Narrow `state.write_lock`'s critical section around the embedder round trip, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+5 more)

### Community 365 - "Contributor Covenant Code of Conduct"
Cohesion: 0.15
Nodes (12): 1. Correction, 2. Warning, 3. Temporary Ban, 4. Permanent Ban, Attribution, Contributor Covenant Code of Conduct, Enforcement, Enforcement Guidelines (+4 more)

### Community 366 - "backfill_summary_embeddings"
Cohesion: 0.24
Nodes (11): backfill_summary_embeddings(), BackfillParams, BackfillReport, EntityCandidate, Arc, Error, Option, Result (+3 more)

### Community 367 - "core/src/lib.rs"
Cohesion: 0.21
Nodes (8): JobStatus, RebuildJob, JoinHandle, Option, Self, String, Value, Utc

### Community 368 - "cross_episode_dedup.rs"
Cohesion: 0.40
Nodes (12): count_entities_named(), make_db(), make_state_with(), Arc, TempDir, test_add_episode_normalizes_non_object_attributes_for_library_callers(), test_case_insensitive_name_match(), test_embedding_based_dedup_variant_names() (+4 more)

### Community 369 - "ingest_embed_batching.rs"
Cohesion: 0.31
Nodes (12): dispatch(), extraction_batches_all_four_phase_a_sites(), extraction_with_no_edges_batches_only_name_and_summary(), find_entities_names(), make_state(), open_db(), req(), Arc (+4 more)

### Community 370 - "StandaloneBackend"
Cohesion: 0.22
Nodes (9): McpBackend, Arc, Option, Self, Send, Sync, UnboundedSender, Value (+1 more)

### Community 371 - "scope.rs"
Cohesion: 0.17
Nodes (5): cypher_is_not_bundled_into_read_write_or_admin(), rejects_unknown_scope(), Result, String, Vec

### Community 372 - "Decisions"
Cohesion: 0.15
Nodes (13): 1. Hand-rolled `rmcp::ServerHandler`, not the `#[tool_router]` macro, 2. Hand-authored `json!` schemas in a static table, not 33 `schemars` structs, 3. MCP tool name = IPC method name, verbatim; no duplicated graph logic, 4. One `McpBackend` trait, two implementations, 5. `knowledge_close` semantics differ by mode, and the difference is load-bearing, 6. `main.rs`'s bootstrap is now a shared, reusable function, 7. `main()` owns the `Runtime` directly and bounds its blocking-pool drain, instead of relying on `#[tokio::main]`'s indefinite default, 8. No `clap` adoption (+5 more)

### Community 373 - "Decisions"
Cohesion: 0.15
Nodes (12): 1. Trait-level seam, not a sub-trait/transport-level seam, 2. The matching hash includes rendered prompts, not raw `ExtractOptions` alone, 3. Recording wraps each `LlmRouter` leaf individually; replay is a single flat substitute, 4. `ReplayingExtractor` holds no inner extractor and needs no credentials, 5. Credential exclusion is structural, not a scrubbing pass, 6. `Error::CassetteMiss(String)` is a distinct error variant, 7. Cassette writes always append; duplicate keys replay FIFO, ADR-0044: LLM Cassette Record/Replay Seam (+4 more)

### Community 374 - "ADR-0316: One `[[bench]]` Target Per `criterion_group!`"
Cohesion: 0.15
Nodes (12): ADR-0316: One `[[bench]]` Target Per `criterion_group!`, Alternatives Considered, Consequences, Context, Decision, Keep one `criterion_main!`, pass `--bench search --exact <name>` everywhere, Move R-003 off the PR-blocking path (post-merge gate, per #298's precedent), Negative / Residual risks (+4 more)

### Community 375 - "Decision"
Cohesion: 0.15
Nodes (13): ADR-0353: Persist and Expose an Applied WAL Sequence in `knowledge_status`, Advancing `applied_seq`: write-after-commit, not atomic-with-commit (FR-003), Backfill on first open (FR-007/FR-008), Consequences, Context, Decision, `max_seq`: unit-aligned with `applied_seq`, and cheap enough to call on every status request, Relationship to ADR-0026 — a deliberate divergence, not a reinvention (+5 more)

### Community 376 - "Decision"
Cohesion: 0.15
Nodes (13): ADR-0477: Tag-Based, Versioned Docs Publishing via a `gh-pages` Accumulator Branch, Backfill: `v0.12.0`–`v0.13.3` only, not `v0.9.0`–`v0.11.0`, `--baseurl` override, verified empirically, Consequences, Context, Decision, `docs-publish.yml` is a separate workflow from `release.yml`, "Latest stable" is always recomputed fresh, never trusted from the triggering event (+5 more)

### Community 377 - "Extraction-quality evaluation: methodology, model rankings, and local-LLM guidance"
Cohesion: 0.15
Nodes (13): Apple Foundation Models: assessed and not recommended for extraction, Dedup finding, Extraction-quality evaluation: methodology, model rankings, and local-LLM guidance, Guidance, Measured results (this engine) — Status: Pending, not yet measured, Methodology: replay against frozen inputs, not a fresh pipeline run per candidate, Ontology-constrained results (`Open`/`Strict`) — Status: Pending, not yet measured, Rankings (+5 more)

### Community 378 - "Extraction-quality eval harness"
Cohesion: 0.15
Nodes (13): Adding a candidate backend, Blind pairwise judging (`--judge-mode`), Cost implications, Extraction-quality eval harness, Failure-record sidecar, Format, Previewing a run (`--dry-run`), Record/replay cassettes (+5 more)

### Community 379 - "FoundationModelsAdapter"
Cohesion: 0.26
Nodes (6): FoundationModelsAdapter, InferenceActor, AsyncThrowingStream, Bool, Error, String

### Community 380 - "generate-bad-stub-models.py"
Cohesion: 0.27
Nodes (12): absorb_unused(), build_bad_dtype(), build_bad_output_name(), build_bad_shape(), main(), make_inputs(), MLModel, Output shape is [1, 512, 512] — wrong hidden dimension. (+4 more)

### Community 381 - "Feature Specification: [FEATURE NAME]"
Cohesion: 0.15
Nodes (12): Assumptions, Edge Cases, Feature Specification: [FEATURE NAME], Functional Requirements, Key Entities *(include if feature involves data)*, Measurable Outcomes, Requirements *(mandatory)*, Success Criteria *(mandatory)* (+4 more)

### Community 382 - "Tasks: Issue #6 — Telemetry and Operator Visibility"
Cohesion: 0.15
Nodes (12): Constitution Compliance, Dependencies & Execution Order, Phase 1: Core Library Types (Foundational — blocks all phases), Phase 2: IPC Timing — dispatch() instrumentation [HOT], Phase 3: Token Usage — Extractor instrumentation [HOT], Phase 4: Stub Event Types for Future Hooks, Phase 5: Binary Sink Wiring, Phase 6: Benchmarks (HOT-path compliance) (+4 more)

### Community 383 - "Feature Specification: WAL Replayer Must Accept MATCH-Prefixed Mutation Queries"
Cohesion: 0.15
Nodes (12): Assumptions, Background, Edge Cases, Feature Specification: WAL Replayer Must Accept MATCH-Prefixed Mutation Queries, Out of Scope, Requirements *(mandatory)*, Source References, Success Criteria *(mandatory)* (+4 more)

### Community 384 - "Feature Specification: Move Performance Benches Off Per-Push CI to On-Demand Invocation"
Cohesion: 0.15
Nodes (12): Assumptions, Background, Edge Cases, Feature Specification: Move Performance Benches Off Per-Push CI to On-Demand Invocation, Out of Scope, Requirements *(mandatory)*, Source References, Success Criteria *(mandatory)* (+4 more)

### Community 385 - "Feature Specification: Remove Incorrect 30% Perf-Ratio Assertion from bench_dedup_hybrid_10k"
Cohesion: 0.15
Nodes (12): Assumptions, Background, Edge Cases, Feature Specification: Remove Incorrect 30% Perf-Ratio Assertion from bench_dedup_hybrid_10k, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements (+4 more)

### Community 386 - "Feature Specification: Remove broken `LBUG_BUILD_FROM_SOURCE` flag from `bench.yml`"
Cohesion: 0.15
Nodes (12): Assumptions, Background, Edge Cases, Feature Specification: Remove broken `LBUG_BUILD_FROM_SOURCE` flag from `bench.yml`, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+4 more)

### Community 387 - "Feature Specification: CLAUDE.md's long-task guidance names the wrong failure and misses the safe technique"
Cohesion: 0.15
Nodes (12): Assumptions, Background, Edge Cases, Feature Specification: CLAUDE.md's long-task guidance names the wrong failure and misses the safe technique, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+4 more)

### Community 388 - "Feature Specification: OaiEmbedder HTTP transport has no request/connect timeout"
Cohesion: 0.15
Nodes (12): Assumptions, Background, Edge Cases, Feature Specification: OaiEmbedder HTTP transport has no request/connect timeout, Functional Requirements, Measurable Outcomes, Out of Scope, Requirements *(mandatory)* (+4 more)

### Community 389 - "BgeEmbedder"
Cohesion: 0.24
Nodes (8): BertModel, Device, BgeEmbedder, Path, Result, Self, Tokenizer, Vec

### Community 390 - "main"
Cohesion: 0.27
Nodes (7): derive_prose_from_wal(), derive_relation_type_samples_from_wal(), main(), Derives staged-corpus-shaped records `[{title, revision_id, prose}, ...]`…, Derives `relation_type_samples` (as `build_expected_results` would) directly…, DeriveFromWalTests, Covers the #217 one-off backfill path: the WAL fixture actually committed was…

### Community 391 - ".new"
Cohesion: 0.20
Nodes (7): CountingEmbedder, default_embed_batch_empty_input_issues_no_embed_calls(), is_transport_error_recognizes_request_kind_errors_beyond_plain_connect(), Arc, AtomicUsize, Mutex, UdsPool

### Community 392 - "wal_manipulate.rs"
Cohesion: 0.41
Nodes (11): entity_wal_line(), group_dir(), read_group_generation(), remove_group_generation(), reset_group_stream(), Option, Path, PathBuf (+3 more)

### Community 393 - "bench.py"
Cohesion: 0.33
Nodes (9): DateTime, benchmark_coreml(), benchmark_python(), main(), Any, Path, Minimal HTTP/1.1 client over a Unix domain socket., UnixSocketHTTP (+1 more)

### Community 394 - "ADR-0043: WAL Replay — Seq-Based File Ordering and MATCH-Write No-Op Accounting"
Cohesion: 0.17
Nodes (12): ADR-0043: WAL Replay — Seq-Based File Ordering and MATCH-Write No-Op Accounting, Alternatives Considered, Consequences, Context, Decision, Fidelity ratio: `match_prefixed_no_op` joins both sides — `match_delete_no_op` does not, File ordering: sort by each file's first-line `seq`, `match_prefixed_no_op` (SET-form) vs. `match_delete_no_op` (DELETE-form) vs. ADR-0026's resume-overlap (+4 more)

### Community 395 - "Decisions"
Cohesion: 0.17
Nodes (11): 1. Edge budget exhaustion now returns `Err`, matching the entity path (FR-004), 2. A single formula, uniform ceiling, differentiated only by call type (FR-001/FR-002/FR-006), 3. `entities_extracted: Option<usize>` on the failure record answers Decision 1's real cost (FR-007), 4. Uniform ceiling across providers, not a hosted-specific clamp (FR-006), 5. Retry-doubling is preserved, but skipped once already at the ceiling, 6. `LlmRouter`'s permanent-fallback switch now also fires on edge exhaustion — deliberately, not as a side effect, ADR-0307: Token-Budget Policy and Edge Budget-Exhaustion Semantics, Consequences (+3 more)

### Community 396 - "ADR-0325: `knowledge_status` Reports "Open But Not Queryable" as a Second Degraded State"
Cohesion: 0.17
Nodes (12): ADR-0325: `knowledge_status` Reports "Open But Not Queryable" as a Second Degraded State, Alternatives Considered, Blanket-catch every error in the closure, not just missing-table, Consequences, Context, Decision, Fix all 10 handlers found by the FR-005 audit in this PR, Negative / Residual risks (+4 more)

### Community 397 - "Decision"
Cohesion: 0.17
Nodes (12): 1. Add a `pull_request` trigger; run the five jobs in parallel, not sequentially appended to `test`, 2. Non-required, non-gating (FR-002), 3. Reuse `ci.yml`'s classifier via a shared composite action, not a second deny-list, 4. No cache-race mitigation — the five jobs cannot participate in the write-write race, 5. No concurrency-group change, 6. Promotion criteria (FR-007), ADR-0328: Run `real-corpus-e2e` on the PR Path as a Non-Required Check, Alternatives Considered (+4 more)

### Community 398 - "Decision"
Cohesion: 0.17
Nodes (12): 1. Per-item salvage at all four parse sites, via a shared `salvage_items<T>()` helper, 2. A new `ExtractionOutcome` wrapper, not new fields on `ExtractionResult`, 3. The `episode.rs:218` empty-name `retain` stays where it is, and now feeds the same counter, 4. FR-004's "all malformed" case falls out of the existing empty-extraction short-circuit — no new branch, 5. A new `StructuredOutputParse` outcome, `"salvaged"`, taking precedence over `"recovered"`, 6. Eval crate parity: `CountingSink`/`StructuredOutputCounts`/`StructuredOutputReliability` gain `salvaged`, ADR-0342: Per-Item Salvage of Malformed Extraction Items, Alternatives Considered (+4 more)

### Community 399 - "Decision"
Cohesion: 0.17
Nodes (11): 1. Strip vector params in `WalWriter::log_mutation`, the one choke point every WAL write shares, 2. Replay recognizes a vector-bearing row by its Cypher template, not by what's in `params`, 3. Textless rows split into skip-row vs. zero-fill, decided structurally, not by record kind, 4. One shape is genuinely unrecoverable, and stays that way by explicit decision, 5. The embedder becomes a mandatory argument, not an `Option`, throughout the replay API, 6. FR-003's removal is mechanical once (5) lands; FR-004 is untouched by design, ADR-0526: Vectors Are a Local Cache — Stop Writing Them to the WAL, Ignore Them on Replay, Consequences (+3 more)

### Community 400 - "BgeEmbedder"
Cohesion: 0.29
Nodes (7): Session, BgeEmbedder, Path, Result, Self, Tokenizer, Vec

### Community 401 - "Implementation Plan: [FEATURE]"
Cohesion: 0.17
Nodes (11): Complexity Tracking, Constitution Check, Documentation (this feature), Implementation Plan: [FEATURE], Performance budget gates, Principle gates, Project Structure, Source Code (repository root) (+3 more)

### Community 402 - "Tasks: Issue #1 — Foundation"
Cohesion: 0.17
Nodes (11): Constitution Compliance, Dependencies & Execution Order, Format: `[ID] [P?] [Story] [Constitution-tag?] Description`, Parallel Opportunities, Phase 1: Workspace Skeleton, Phase 2: Core Library Implementation [LDB], Phase 3: Integration Test / LadybugDB Spike [LDB], Phase 4: Example Consumer (Principle II Gate) (+3 more)

### Community 403 - "OaiEmbedder"
Cohesion: 0.31
Nodes (7): EmbedTransport, OaiEmbedder, OaiEmbedding, OaiEmbedInput, OaiEmbedResponse, Client, UdsPool

### Community 404 - "compute_unbound_impacts"
Cohesion: 0.35
Nodes (10): compute_unbound_impacts(), GroupPurgeCounts, purge_groups(), PurgeCounts, Error, GroupedMutations, Result, String (+2 more)

### Community 405 - "IpcResponse"
Cohesion: 0.40
Nodes (7): IpcError, IpcResponse, Into, Option, Self, String, Value

### Community 406 - "test_knowledge_process_chunk_multibyte_chars_use_char_count_not_byte_count"
Cohesion: 0.18
Nodes (8): chunk_advisory_env_read_guard(), ChunkAdvisoryEnvOverrideGuard, Drop, RwLockWriteGuard, Self, test_knowledge_process_chunk_advisory_threshold_behavior(), test_knowledge_process_chunk_multibyte_chars_use_char_count_not_byte_count(), RwLockReadGuard

### Community 407 - "ADR-0014: Pass `Option<&Ontology>` as a call-time parameter to `Extractor::extract`"
Cohesion: 0.18
Nodes (10): ADR-0014: Pass `Option<&Ontology>` as a call-time parameter to `Extractor::extract`, Consequences, Context, Decision, `Option<Arc<Ontology>>` vs `ArcSwapOption<Ontology>` in `AppState`, Rationale, Update — Issue #92 (2026-05-25), Why call-time parameter (+2 more)

### Community 408 - "ADR-0020: IPC Collection Response Envelope Contract"
Cohesion: 0.18
Nodes (10): ADR-0020: IPC Collection Response Envelope Contract, Audit table (as of 2026-05-26; `facts`→`edges` rename per issue #524, 2026-09-01), Consequences, Context, Decision, Empty results, Multi-collection responses, Naming conventions by record type (+2 more)

### Community 409 - "Decision"
Cohesion: 0.18
Nodes (11): 1. Autonomous startup recovery before entering degraded mode, 2. Four-step recovery sequence, 3. Episode-cursor derivation over a persistent seq cursor, 4. Shared `recovery` module, 5. New `knowledge_recover_full` IPC command, 6. Pre-existing index gap fix, ADR-0027: Autonomous WAL-Corruption Self-Recovery on Startup, Consequences (+3 more)

### Community 410 - "Decisions"
Cohesion: 0.18
Nodes (10): 1. Index-DDL functions return real outcomes, 2. `drop_vector_indexes` closes the HNSW staleness gap, 3. `indices_built` reflects the real, current outcome — unconditionally, 4. `indices_built` is surfaced on every relevant response, 5. `recovery.rs` is explicitly out of scope, ADR-0034: Observable Index-Build Outcome — Fixing ADR-0025's Dead-Code Failure Path, Consequences, Context (+2 more)

### Community 411 - "ADR-0042: OaiExtractor UDS Connection Pooling"
Cohesion: 0.18
Nodes (10): ADR-0042: OaiExtractor UDS Connection Pooling, Alternatives Considered, Consequences, Context, Decision, Duplicate the pool pattern in `extractor.rs`, not a shared helper, Pool size 4, matching the embedder, References (+2 more)

### Community 412 - "ADR-0298: CI Failure Notification for Non-Gating Workflows"
Cohesion: 0.18
Nodes (11): A single centralized `workflow_run` listener, not a trailer job per source workflow, ADR-0298: CI Failure Notification for Non-Gating Workflows, Alternatives Considered, Consequences, Context, Decision, Dedup key: two labels, not the issue title or body, Lifecycle mapping from `conclusion` (+3 more)

### Community 413 - "Decisions"
Cohesion: 0.18
Nodes (11): 1. A new telemetry event, not a new `Error` variant, 2. `ExtractOptions.chunk_key: Option<&'a str>`, not a content hash, 3. The sidecar is installed at every `RecordingExtractor` construction site, 4. HTTP-error body capture required restructuring, not just an added `sink.emit()` call, 5. `resp.clone()` before Anthropic's destructive parse, not a parse-function rework, 6. Sidecar rotation mirrors `WalWriter`'s byte-size model, ADR-0306: Extraction-Failure Sidecar and Truncation Visibility, Consequences (+3 more)

### Community 414 - "ADR-0368: Duplicate-Edge Detection During Merge Scopes by the Edge's Own `group_id`, Not the Merge's"
Cohesion: 0.18
Nodes (11): ADR-0368: Duplicate-Edge Detection During Merge Scopes by the Edge's Own `group_id`, Not the Merge's, Also scope `get_full_edges_for_entity` by `group_id`, Alternatives Considered, Consequences, Context, Decision, Filter `MergeEntitiesResult`'s existing counts to the merging group instead of adding separate foreign counts, Negative / Residual risks (+3 more)

### Community 415 - "ADR-0375: WAL Seq Bounds Manifest"
Cohesion: 0.18
Nodes (11): A manifest sidecar, not an in-process cache, Accepted limitation, ADR-0375: WAL Seq Bounds Manifest, Consequences, Context, Decision, Extension: `wal_min_seq` (same day, once #367 merged), Maintenance: lazy, read-side, best-effort (+3 more)

### Community 416 - "Decision"
Cohesion: 0.18
Nodes (11): ADR-0387: WAL Stream Generation Identity, Consequences, Context, Decision, Detection: one insertion point in `knowledge_rebuild_from_wal`, row-existence-aware, First-creation race: the loser adopts, it does not error, FR-010: the post-replay re-bind is part of the same operation, not a follow-up call, `knowledge_status` reports the on-disk (source-side) generation, not lcg's own recorded value (+3 more)

### Community 417 - "Decision"
Cohesion: 0.18
Nodes (10): ADR-0510: Bounded Request/Connect Timeouts for `OaiEmbedder`'s HTTP Transport, Consequences, Context, Decision, Fix at the one true choke point: `OaiEmbedder::new_http`'s `Client` construction, `new_http`/`from_env` become fallible, No changes to `is_transport_error`, the UDS transport, or ADR-0499's retry wrapper, Out of Scope (+2 more)

### Community 418 - "Decision"
Cohesion: 0.18
Nodes (11): ADR-0541: Bounded Timeouts for `OaiEmbedder`'s UDS Transport, Combined single budget for acquire+dial, not two stacked timeouts, Consequences, Context, Decision, Mirror #510, plus two UDS-specific hang points, `new_uds` becomes fallible, No new poison-guard mechanism (+3 more)

### Community 419 - "ADR-0543: Narrow `state.write_lock`'s Critical Section Around the Embedder Round Trip"
Cohesion: 0.18
Nodes (10): ADR-0543: Narrow `state.write_lock`'s Critical Section Around the Embedder Round Trip, Alternatives Considered, Consequences, Context, Decision, Edge Pass 2 re-canonicalizes endpoint UUIDs, but does not re-resolve by name, Out of Scope, Pass 2 reuses Pass 1's resolution logic verbatim, via a shared helper (+2 more)

### Community 420 - "Decision"
Cohesion: 0.18
Nodes (11): A deliberate, narrow exception to ADR-0024's bound-parameter convention, A mechanism that was tried and abandoned: `CALL home_directory='<dir>'`, ADR-0559: Bundle lbug vector/fts extensions so startup never downloads from the CDN, Alternatives considered, CI: one touchpoint, not seven, Consequences, Context, Decision (+3 more)

### Community 421 - "Integration Test Fixtures"
Cohesion: 0.18
Nodes (10): Bug 783 — tokenizer offline cache (missing `config.json`), Bug 784 — embedding output dtype (fp32 vs fp16), Bug 785 (#518) — dangling `weights` manifest entry (bad-dtype/shape/output-name stubs), Bug retrospective (FR-009), Checking freshness locally, Divergence guard, Fixture inventory, Integration Test Fixtures (+2 more)

### Community 422 - "Core Principles"
Cohesion: 0.18
Nodes (10): Core Principles, Development Workflow, Governance, I. IPC Surface Is a Stable Contract (NON-NEGOTIABLE), II. Library and Binary Are Peers, III. LadybugDB Only, IV. WAL Is Authoritative, Liminis Context Graph Constitution (+2 more)

### Community 423 - "Core Principles"
Cohesion: 0.18
Nodes (10): Core Principles, Governance, [PRINCIPLE_1_NAME], [PRINCIPLE_2_NAME], [PRINCIPLE_3_NAME], [PRINCIPLE_4_NAME], [PRINCIPLE_5_NAME], [PROJECT_NAME] Constitution (+2 more)

### Community 424 - "ArcSwapOption"
Cohesion: 0.31
Nodes (9): ArcSwapOption, dispatch(), has_wal_files(), req(), Arc, Path, Value, wal_byte_total() (+1 more)

### Community 425 - "BarrierEmbedder"
Cohesion: 0.33
Nodes (6): Barrier, BarrierEmbedder, BoxFuture, Error, Result, Vec

### Community 426 - "resolve_entity_by_name"
Cohesion: 0.33
Nodes (9): ambiguity_error(), KindScope, resolve_entity_by_name(), resolve_entity_by_uuid(), Resolved, Box, Error, Result (+1 more)

### Community 427 - "Parity Test Fixtures"
Cohesion: 0.20
Nodes (9): Capture procedure, Capture script (`scripts/record_corpus.py`), CI behaviour, Directory layout, Fixture file format, Parity Test Fixtures, Prerequisites, Steps (+1 more)

### Community 428 - "ldb_spike_ipc.rs"
Cohesion: 0.33
Nodes (9): format_float_array(), fts_query_matches_summary_only_keyword_not_present_in_name(), String, Vec, test_fts_index_creation_and_query(), test_hnsw_vector_query(), test_rel_table_creation_and_query(), unit_vec() (+1 more)

### Community 429 - "search_result_order.rs"
Cohesion: 0.38
Nodes (9): dispatch(), find_entities_returns_results_in_ranked_order_not_insertion_order(), find_relationships_returns_results_in_ranked_order_not_insertion_order(), one_hot(), Arc, TempDir, Value, Vec (+1 more)

### Community 430 - "mcp_real_corpus_admin_data_e2e.rs"
Cohesion: 0.40
Nodes (9): as_uuid_set(), assert_exact_uuid_enumeration(), mcp_admin_data_operations_over_real_corpus_fixture(), HashSet, Path, String, Value, spawn_fresh() (+1 more)

### Community 431 - "as_uuid_set"
Cohesion: 0.40
Nodes (9): as_name_set(), as_uuid_set(), assert_exact_uuid_enumeration(), mcp_read_path_over_real_corpus_fixture(), BTreeSet, HashSet, String, Value (+1 more)

### Community 432 - "ADR-0005: Streaming IPC Progress Framing via `_progress_token`"
Cohesion: 0.20
Nodes (9): ADR-0005: Streaming IPC Progress Framing via `_progress_token`, Consequences, Context, Decision, `OwnedRwLockWriteGuard` in spawned tasks, `_progress_token` sentinel, References, Streaming vs. non-streaming dispatch (+1 more)

### Community 433 - "ADR-0008: Named Multi-Connection Pool for ContextGraphSocketClient"
Cohesion: 0.20
Nodes (9): ADR-0008: Named Multi-Connection Pool for ContextGraphSocketClient, Connection Pool Timeout, Consequences, Context, Decision, `isRebuilding` Flag, Python-Only Tools, Why Named Connections Over a Generic Pool (+1 more)

### Community 434 - "ADR-0039: UDS Embedder Connection Pooling"
Cohesion: 0.20
Nodes (9): ADR-0039: UDS Embedder Connection Pooling, Alternatives Considered, Cancellation safety: poison-on-drop replaces "always safe", Consequences, Context, Decision, One bounded re-dial on a broken connection, References (+1 more)

### Community 435 - "Decisions"
Cohesion: 0.20
Nodes (10): 1. `OaiExtractor` uses `response_format: json_object` only — no function-calling mode, 2. No live reachability probe gates extraction at startup, 3. Endpoint/provider selection precedence (FR-006), resolved once in `main.rs` — no socket, 4. `LlmRouter` and `AppState` generalize to `Arc<dyn Extractor>`, 5. Cross-provider fallback stays out of scope, ADR-0041: Local/OpenAI-Compatible Extraction Adapter, Consequences, Context (+2 more)

### Community 436 - "ADR-0045: WAL Replay Prepared-Statement Cache — LRU-1 Scope and Deferred Connection Recycling"
Cohesion: 0.20
Nodes (9): A single-entry ("LRU-1") prepared-statement cache, not a multi-template cache, ADR-0045: WAL Replay Prepared-Statement Cache — LRU-1 Scope and Deferred Connection Recycling, Consequences, Context, Decision, FR-004: periodic connection recycling is evaluated and deferred, References, `ReplayStats::prepare_calls` as the observable bound (+1 more)

### Community 437 - "ADR-0046: WAL Replay — Deduplicated Failure Samples and Fail-Fast Rebuild Idempotency"
Cohesion: 0.20
Nodes (9): ADR-0046: WAL Replay — Deduplicated Failure Samples and Fail-Fast Rebuild Idempotency, Alternatives Considered, Consequences, Context, Decision, Failure-sample deduplication: key on `(template, error)`, cap bounds categories, Fidelity ratio: `unrecognised_lines` and `unparseable_lines` join both sides, Non-empty-database guard: fail-fast by default, opt-in `force_clear` (+1 more)

### Community 438 - "ADR-0052: `lcg-eval --dry-run` Shares the Real Run's Resolution Path"
Cohesion: 0.18
Nodes (9): ADR-0052: `lcg-eval --dry-run` Shares the Real Run's Resolution Path, Consequences, Context, Decision, Related, How it fits together, liminis-context-graph, llms.txt (+1 more)

### Community 439 - "Decision"
Cohesion: 0.20
Nodes (9): ADR-0499: Bounded Embedder-Probe Retry and Degrade-Without-Opening-DB for Standalone `--mcp-stdio`, Bounded retry, gated on launch mode, with a fixed 5-second ceiling, Consequences, Context, Decision, `is_transport_error` also recognizes `reqwest`'s request-kind pool errors, `knowledge_recover` rejects every strategy for this specific reason, not per-strategy, `LCG_EMBEDDING_DIM` does not bypass the retry/degrade path (+1 more)

### Community 440 - "0.14.3 — 2026-09-15 (full release notes)"
Cohesion: 0.20
Nodes (9): 0.14.3 — 2026-09-15 (full release notes), Bundled extensions and symlinks, Clients, Prerequisites, Ranked search results, Upgrading, What was verified, and what was not, Why lbug is still 0.18.1 (+1 more)

### Community 441 - "local-inference — macOS Swift sidecar for liminis-context-graph"
Cohesion: 0.20
Nodes (10): Build and run, Dependencies and licenses, Distribution, First-time setup, local-inference — macOS Swift sidecar for liminis-context-graph, Relationship to the main project, Requirements, Selecting a mode (+2 more)

### Community 442 - "extractJSON"
Cohesion: 0.33
Nodes (3): extractJSON(), String, ExtractJSONTests

### Community 443 - "render-diagrams.mjs"
Cohesion: 0.20
Nodes (9): CHECK, DOCS, expected, GENERATED, LEGACY_IMG, OUT_DIR, pages, SITE (+1 more)

### Community 444 - "Changelog"
Cohesion: 0.22
Nodes (9): [0.10.0] - 2026-07-23, [0.12.1] - 2026-08-05, [0.12.2] - 2026-08-06, Added, Added, Added, Changelog, Fixed (+1 more)

### Community 445 - "telemetry_ipc.rs"
Cohesion: 0.44
Nodes (8): ipc_call_event_emitted_on_error_dispatch(), ipc_call_event_emitted_on_successful_dispatch(), make_db(), make_state_with_sink(), req(), Arc, TempDir, Value

### Community 446 - "ADR-0007: Two-Hop RELATES_TO Traversal as Canonical Read Pattern"
Cohesion: 0.22
Nodes (8): ADR-0007: Two-Hop RELATES_TO Traversal as Canonical Read Pattern, Benefits, Consequences, Constraints, Context, Decision, Rationale for not using schema-detection, References

### Community 447 - "ADR-0015: WAL Drain-and-Flush Pattern for Production Write Handlers"
Cohesion: 0.22
Nodes (8): ADR-0015: WAL Drain-and-Flush Pattern for Production Write Handlers, Consequences, Context, Decision, Invariant for future write handlers, WAL failures are non-fatal, Why not add `WalWriter` directly to `Conn`, Why `RefCell` not `Mutex`

### Community 448 - "Decision"
Cohesion: 0.22
Nodes (9): ADR-0016: OpenAI-compatible embedding contract over UDS; hyper for UDS transport, Consequences, Context, Decision, f64 → f32 conversion, Startup probe, Transport selection: CLI flags at startup, UDS transport: hyper 1.x `client::conn::http1` (+1 more)

### Community 449 - "Consequences"
Cohesion: 0.22
Nodes (8): ADR-0019: Workspace Migration Partial-Resume vs. Schism Marker, Consequences, Context, Correct resumption, Decision, False-negative schism: edge case, Future layout changes, Implications for operators

### Community 450 - "ADR-0036: Eager HNSW/FTS Index Build at Startup + Dedup-Path Auto-Heal"
Cohesion: 0.22
Nodes (9): 1. Indices are built eagerly at startup, on both the direct-open and post-recovery paths, 2. Ingest's hybrid dedup path gets the same missing-index auto-heal as the search handlers, 3. Shared auto-heal plumbing relocates out of `handlers.rs`, 4. Fixed a pre-existing, unrelated startup-recovery classification bug discovered while testing this change, ADR-0036: Eager HNSW/FTS Index Build at Startup + Dedup-Path Auto-Heal, Consequences, Context, Decisions (+1 more)

### Community 451 - "ADR-0314: Missing-Summary Salvage and `schema_invalid` Classification"
Cohesion: 0.22
Nodes (9): 1. `ExtractedEntity.summary` defaults to `""` on absence or `null`, 2. A fourth classification value, `schema_invalid`, mechanically derived from `serde_json::Error::classify()`, 3. Missing-summary visibility via a dedicated telemetry event, not a repair pass, ADR-0314: Missing-Summary Salvage and `schema_invalid` Classification, Alternatives Considered, Consequences, Context, Decision (+1 more)

### Community 452 - "ADR-0347: Reject Semantically-Empty Required Fields During Item Salvage"
Cohesion: 0.22
Nodes (9): 1. A `RequiredFieldsPresent` trait, checked inside `salvage_items` via a trait bound, 2. `episode.rs`'s empty-name `retain` stays code-identical — this is not the move ADR-0342 §3 rejected, 3. `crates/eval/src/runner.rs` needs no code change, ADR-0347: Reject Semantically-Empty Required Fields During Item Salvage, Alternatives Considered, Consequences, Context, Decision (+1 more)

### Community 453 - "ADR-0430: Workflow-Level `shell: bash` to Restore `pipefail` for `| tee` Steps"
Cohesion: 0.22
Nodes (9): ADR-0430: Workflow-Level `shell: bash` to Restore `pipefail` for `| tee` Steps, Alternatives Considered, Consequences, Context, Decision, References, `release.yml`: one targeted step-level fix, not a workflow-level `defaults` block, Why not job-level `defaults.run.shell` (+1 more)

### Community 454 - "ADR-0440: Recompute Embeddings on WAL Replay, With a Sync Bridge and a Two-Mechanism Identity Split"
Cohesion: 0.22
Nodes (8): 1. A synchronous callback, bridged per call site — not an async trait threaded through `replay.rs`, 2. Two separate identity-tracking mechanisms, not one, 3. The embedding cache is in-memory-only, keyed by `(model, dim, text)`, ADR-0440: Recompute Embeddings on WAL Replay, With a Sync Bridge and a Two-Mechanism Identity Split, Consequences, Context, Decision, Rejected Alternatives

### Community 455 - "Decision"
Cohesion: 0.22
Nodes (8): A 401/403 at startup is always fatal, never overridable by `LCG_EMBEDDING_DIM`, ADR-0497: Bearer-Token Authentication for the Embedder HTTP Transport, Builder method, not a constructor signature change, Consequences, Context, Decision, Three-tier key resolution, with one warning-free convenience tier, URL-userinfo redaction is scoped to Basic-auth `user:pass@host` only

### Community 456 - "ADR-0502: Pin an Explicit `macos-26` Runner for Swift Sidecar CI, Not Yet a Required Check"
Cohesion: 0.22
Nodes (8): ADR-0502: Pin an Explicit `macos-26` Runner for Swift Sidecar CI, Not Yet a Required Check, Alternatives Considered, Consequences, Context, Decision, Do not make `Swift sidecar CI` a required branch-protection status check yet, Pin `runs-on: macos-26`, not `macos-latest`, References

### Community 457 - "Configuration"
Cohesion: 0.22
Nodes (9): CLI flags, Configuration, Embedder sidecar, Environment variables, Extractor: local or hosted, HTTP transport (CI / Linux / custom embedders / hosted providers), macOS: Swift CoreML sidecar (default), MCP client config recipes (+1 more)

### Community 458 - "verify-embedding-parity.py"
Cohesion: 0.36
Nodes (8): cosine_similarity(), get_coreml_embeddings(), get_pytorch_embeddings(), main(), Run sentences through PyTorch sentence-transformers with normalization., Cosine similarity between two normalized vectors., Run sentences through CoreML model with CLS pooling + L2 normalization., ndarray

### Community 459 - "Tasks: Issue #5 — HNSW + BM25 Hybrid Dedup at Scale"
Cohesion: 0.22
Nodes (8): Acceptance Scenario Coverage, Dependencies & Execution Order, Parallel opportunities, Phase 1: DB Layer — New `Conn` Helpers, Phase 2: Bench Harness (HOT gate — must pass before `episode.rs` change lands), Phase 3: Hot-Path Integration in `episode.rs`, Phase 4: CI Bench Job, Tasks: Issue #5 — HNSW + BM25 Hybrid Dedup at Scale

### Community 460 - "wal_snapshot"
Cohesion: 0.57
Nodes (7): BTreeMap, Path, String, Vec, wal_snapshot(), WalDirSnapshot, walk_dir()

### Community 461 - "speckit-checklist/SKILL.md"
Cohesion: 0.25
Nodes (7): Anti-Examples: What NOT To Do, Checklist Purpose: "Unit Tests for English", Example Checklist Types & Sample Items, Execution Steps, Post-Execution Checks, Pre-Execution Checks, User Input

### Community 462 - "speckit-plan/SKILL.md"
Cohesion: 0.25
Nodes (7): Key rules, Outline, Phase 0: Outline & Research, Phase 1: Design & Contracts, Phases, Pre-Execution Checks, User Input

### Community 463 - "speckit-specify/SKILL.md"
Cohesion: 0.25
Nodes (7): For AI Generation, Outline, Pre-Execution Checks, Quick Guidelines, Section Requirements, Success Criteria Guidelines, User Input

### Community 464 - "speckit-tasks/SKILL.md"
Cohesion: 0.25
Nodes (7): Checklist Format (REQUIRED), Outline, Phase Structure, Pre-Execution Checks, Task Generation Rules, Task Organization, User Input

### Community 465 - "fixture_path"
Cohesion: 0.25
Nodes (8): fixture_path(), PathBuf, test_falkordb_episodes_merge_replays_successfully(), test_falkordb_expired_at_merge_replays_successfully(), test_literal_inlined_fact_embedding_replays_unchanged(), test_open_or_rebuild_recomputes_embeddings_when_embedder_configured(), test_replay_golden_fixture_counts(), test_replay_golden_fixture_field_updates()

### Community 466 - "StderrSink"
Cohesion: 0.32
Nodes (5): Arc, JoinHandle, Self, UnboundedSender, StderrSink

### Community 467 - "ADR-0009: Degraded-Mode Startup and In-Process Recovery"
Cohesion: 0.25
Nodes (8): ADR-0009: Degraded-Mode Startup and In-Process Recovery, Alternatives Considered, Consequences, Context, Decision 1: Bind socket before DB open, Decision 2: Error classification by Display-string matching, Decision 3: `ArcSwapOption<Db>` for degraded state, Decision 4: Recovery serialization via `write_lock.try_write()`

### Community 468 - "ADR-0013: CancellationToken as the Single Shutdown Signal on AppState"
Cohesion: 0.25
Nodes (7): ADR-0013: CancellationToken as the Single Shutdown Signal on AppState, Consequences, Context, Decision, Known Gap: handle_rebuild_from_wal, Rationale, Rejected Alternative: Polling AtomicBool

### Community 469 - "ADR-0018: Ontology Hash Sidecar for Drift Detection"
Cohesion: 0.25
Nodes (7): ADR-0018: Ontology Hash Sidecar for Drift Detection, Consequences, Context, Decision 1: Sidecar file over DB table, Decision 2: Semantic hash over byte hash, Decision 3: Sidecar path in `.lcg/`, Decision 4: Sentinel hash `"none"` for no-ontology state

### Community 470 - "ADR-0025: Auto-Heal Index Build and Bulk-Load Reload Pattern"
Cohesion: 0.25
Nodes (8): ADR-0025: Auto-Heal Index Build and Bulk-Load Reload Pattern, Auto-heal search path (from #58), Bulk-load reload pattern (from #146), Consequences, Context, Decisions, Interrupted-reload self-heal (FR-005), Related

### Community 471 - "ADR-0037: Relation Classification Has No Open-Ended Mode and Abstention Writes `UNCLASSIFIED`"
Cohesion: 0.25
Nodes (8): 1. No open-ended classification mode, 2. Abstention is a real write, not a no-op, ADR-0037: Relation Classification Has No Open-Ended Mode and Abstention Writes `UNCLASSIFIED`, Alternatives Considered, Consequences, Context, Decision, References

### Community 472 - "ADR-0445: Embedder Batch API — Wire-Level Batching, Index-Ordered Reassembly, Chunk-Size Knob"
Cohesion: 0.25
Nodes (7): ADR-0445: Embedder Batch API — Wire-Level Batching, Index-Ordered Reassembly, Chunk-Size Knob, Consequences, Context, Decision 1: A default trait method expressed in terms of `embed`, run concurrently, Decision 2: Wire format extends `input` via an untagged enum, not by unifying on `embed_batch(&[text])`, Decision 3: Batch responses are reassembled by the response's `index` field, not by trusting `data` array order, Decision 4: A dedicated, escape-hatch env var for the chunk size, not a hardcoded constant

### Community 473 - "ADR-0446: Per-Group Ontology Resolution"
Cohesion: 0.25
Nodes (7): ADR-0446: Per-Group Ontology Resolution, Consequences, Context, Decision 1: Additive `AppState` field, lazy-load-and-cache, not eager scan, Decision 2: A malformed per-group file falls back to the workspace ontology, logged, Decision 3: Drift detection stays workspace-scoped — not extended per-group, Decision 4: The published ontology sidecar is write-only — never read back by lcg

### Community 474 - "ADR-0470: Entity Summary Embedding for Semantic Search"
Cohesion: 0.25
Nodes (7): ADR-0470: Entity Summary Embedding for Semantic Search, Consequences, Context, Decision 1: `summary_embedding` is a deliberate divergence from graphiti's schema, not a parity gap, Decision 2: `summary_embedding` is write-once, matching `name_embedding`/`fact_embedding` — not "always current" as the spec's Edge Cases text literally asked for, Decision 3: Migration zero-fills existing rows before any index exists over the column, Decision 4: A new IPC/MCP method (`knowledge_backfill_summary_embeddings`), a deviation from FR-006's default

### Community 475 - "ADR-0486: Batch WAL-Replay Embedding Recompute — An Independent, Unaligned Window Upstream of the Cypher Batch"
Cohesion: 0.25
Nodes (7): ADR-0486: Batch WAL-Replay Embedding Recompute — An Independent, Unaligned Window Upstream of the Cypher Batch, Consequences, Context, Decision 1: A second, independent buffering window — not reuse of the existing Cypher execution batch, Decision 2: The embedding window is not flushed at WAL-file boundaries, Decision 3: Cancellation drops the pending window rather than flushing it, Decision 4: `embed_calls` increments at request-queue time inside window resolution, not at buffer time

### Community 476 - "ADR-0503: `liminis-context-graph` Is the Sole Source of Truth for the Swift Sidecar"
Cohesion: 0.25
Nodes (7): 1. `liminis-context-graph`'s copy is authoritative; production behavior is the default resolution unless a documented reason overrides it, 2. Distribution: a `workflow_dispatch` GitHub Actions build, published to a tagged GitHub Release, 3. `native/local-inference/README.md` states sole ownership and documents distribution, ADR-0503: `liminis-context-graph` Is the Sole Source of Truth for the Swift Sidecar, Consequences, Context, Decision

### Community 477 - "ADR-0550: Link OpenSSL dynamically, resolved through `@rpath` on macOS"
Cohesion: 0.25
Nodes (8): ADR-0550: Link OpenSSL dynamically, resolved through `@rpath` on macOS, Alternatives considered, Amendment (2026-09-05): what 0.14.0 actually shipped, Consequences, Context, Decision, How lbug's `build.rs` finds OpenSSL, per version, Why link time, not post-build

### Community 478 - "ADR-0575: Lazy-Dial the Attached `--connect` Socket by Default, With `--connect-eager` Opt-Out"
Cohesion: 0.25
Nodes (7): A second, infallible constructor — `new_lazy` — alongside the unmodified `connect`, ADR-0575: Lazy-Dial the Attached `--connect` Socket by Default, With `--connect-eager` Opt-Out, `--connect-eager`: an opt-out for operators using fail-fast-at-startup as a health gate, Consequences, Context, Decision, What stays default: `tools/list` remains static

### Community 479 - "Getting Started"
Cohesion: 0.25
Nodes (8): Build from source, Bundling in downstream apps, Getting Started, Install prebuilt binary, Next steps, Run it, Talk to it, Talk to it over MCP

### Community 480 - "Ontology"
Cohesion: 0.25
Nodes (8): Drift detection, Entity type hierarchy, File location, Format, `knowledge_status` summary, Modes, Ontology, Per-group ontologies

### Community 481 - "Operations"
Cohesion: 0.25
Nodes (8): `knowledge_status` health fields, On-disk layout, Operations, Publishing a WAL stream (issue #414), Recovery and export tools, Self-healing and degraded mode, Streaming progress, WAL administration

### Community 482 - "Docs publishing"
Cohesion: 0.25
Nodes (8): Before cutting a release, Docs publishing, One-time manual steps (required once, after this mechanism first ships), Related, Release process, Republishing a correction without a new release (FR-006), What happens automatically when you cut a release, What to check after a release publishes

### Community 483 - "0.14.0 — 2026-09-02 (full release notes)"
Cohesion: 0.25
Nodes (7): 0.14.0 — 2026-09-02 (full release notes), A note on the lbug version, Added, Changed, Fixed, Internal, Upgrading

### Community 484 - "0.15.0 — 2026-09-15 (full release notes)"
Cohesion: 0.25
Nodes (7): 0.15.0 — 2026-09-15 (full release notes), A caveat worth stating plainly, Fixed, Internal, Upgrading, What was verified, Why 0.20.3, not 0.20.4

### Community 485 - ".makeActor"
Cohesion: 0.43
Nodes (3): EmbeddingOutputValidationTests, String, URL

### Community 486 - "check-links.mjs"
Cohesion: 0.25
Nodes (4): BASE, broken, DIST, SITE

### Community 487 - "concurrent_rw.rs"
Cohesion: 0.52
Nodes (6): bench_concurrent_rw(), build_state(), Arc, Criterion, TempDir, setup_db()

### Community 488 - "make_state_without_indices"
Cohesion: 0.48
Nodes (6): find_entities_auto_heals_on_fresh_db(), find_entities_auto_heals_when_only_summary_index_is_missing(), find_entities_second_search_skips_auto_heal(), make_state_without_indices(), Arc, TempDir

### Community 489 - "build_db_with_entities"
Cohesion: 0.48
Nodes (6): build_db_with_entities(), dedup_falls_back_to_brute_force_below_threshold(), dedup_overlap_1k_corpus_100_probes(), dedup_uses_hybrid_above_threshold(), entity_count_in_group_returns_zero_for_empty_group(), TempDir

### Community 490 - "mcp_clean_shutdown.rs"
Cohesion: 0.57
Nodes (6): knowledge_close_produces_clean_exit_and_no_wal_corruption(), Command, TempDir, sigterm_produces_clean_exit_and_no_wal_corruption(), spawn_standalone(), standalone_command()

### Community 491 - "ADR-0002: Reader/Writer Split via `tokio::sync::RwLock`"
Cohesion: 0.29
Nodes (6): ADR-0002: Reader/Writer Split via `tokio::sync::RwLock`, Consequences, Context, Decision, References, SC-003 Deferral Note

### Community 492 - "ADR-0003: `ArcSwap<Db>` for Live Database Replacement in `clear_all`"
Cohesion: 0.29
Nodes (6): ADR-0003: `ArcSwap<Db>` for Live Database Replacement in `clear_all`, Consequences, Context, Decision, Why not `Mutex<Arc<Db>>`?, Why not process restart?

### Community 493 - "ADR-0004: Add `classify_entities` to the `Extractor` trait"
Cohesion: 0.29
Nodes (6): ADR-0004: Add `classify_entities` to the `Extractor` trait, Consequences, Context, Decision, Rationale, Status

### Community 494 - "ADR-0021: Inject `LBUG_BUILD_FROM_SOURCE` via cargo-dist `github-build-setup`"
Cohesion: 0.29
Nodes (7): ADR-0021: Inject `LBUG_BUILD_FROM_SOURCE` via cargo-dist `github-build-setup`, Consequences, Context, Decision, Fallback, How It Works, YAML Serialization Issue and `allow-dirty`

### Community 495 - "ADR-0026: Episode-Cursor WAL Resume for Checkpoint Recovery"
Cohesion: 0.29
Nodes (7): ADR-0026: Episode-Cursor WAL Resume for Checkpoint Recovery, Consequences, Context, Decision, Fallback, Gotchas worth remembering, Why episode-cursor instead of a persisted seq cursor

### Community 496 - "ADR-0030: Batched Write-Lock Acquisition for Long-Running Passes"
Cohesion: 0.29
Nodes (6): ADR-0030: Batched Write-Lock Acquisition for Long-Running Passes, Alternatives considered, Atomicity guarantee, Context, Decision, When to apply this pattern

### Community 497 - "ADR-0040: Attached-Mode Reconnect — Retry Only Write-Time Failures"
Cohesion: 0.29
Nodes (6): ADR-0040: Attached-Mode Reconnect — Retry Only Write-Time Failures, Alternatives Considered, Consequences, Context, Decision, References

### Community 498 - "ADR-0310: Strict-Mode Relation-Type Filtering Reclassifies, Never Drops"
Cohesion: 0.29
Nodes (7): ADR-0310: Strict-Mode Relation-Type Filtering Reclassifies, Never Drops, Alternatives Considered, Consequences, Context, Decision, References, What `strict` actually buys

### Community 499 - "ADR-0312: Strict-Mode Entity-Type Filtering Reclassifies, Never Drops"
Cohesion: 0.29
Nodes (7): ADR-0312: Strict-Mode Entity-Type Filtering Reclassifies, Never Drops, Alternatives Considered, Consequences, Context, Decision, References, Why This Follows Independently, Not Just by Analogy to ADR-0310

### Community 500 - "ADR-0398: Link OpenSSL Statically So Release Artifacts Stay Self-Contained"
Cohesion: 0.29
Nodes (7): ADR-0398: Link OpenSSL Statically So Release Artifacts Stay Self-Contained, Alternatives Considered, Amendment (2026-09-01, issue #529), Consequences, Context, Decision, References

### Community 501 - "ADR-0528: Structured Attributes on Episodes (`Episodic.attributes`)"
Cohesion: 0.29
Nodes (6): ADR-0528: Structured Attributes on Episodes (`Episodic.attributes`), Consequences, Context, Decision 1: Attributes live on `Episodic`, not on a new edge or an entity binding, Decision 2: `Episodic.attributes` is a deliberate divergence from graphiti's schema, not a parity gap, Decision 3: Migration zero-fills existing rows to `"{}"`, even though no index forces it

### Community 502 - "Embedding Options"
Cohesion: 0.29
Nodes (7): Any OpenAI-compatible endpoint is a valid target, Capability matrix, Embedding is not extraction, Embedding Options, No bundled cross-platform option, Switching embedders on an existing workspace is not a drop-in swap, The macOS socket path is not shared with the Electron app

### Community 503 - "bug_report.md"
Cohesion: 0.29
Nodes (6): Actual behaviour, Describe the bug, Environment, Expected behaviour, Relevant logs or output, Steps to reproduce

### Community 504 - "Self-describing group graphs: the ontology as graph content"
Cohesion: 0.29
Nodes (6): Probably not either/or, Self-describing group graphs: the ontology as graph content, The idea, What would make this worth specifying, Why it is attractive, Why it is not obviously right

### Community 505 - "[0.11.0] - 2026-07-30"
Cohesion: 0.33
Nodes (6): [0.11.0] - 2026-07-30, Added, Deprecated, Documentation, Fixed, Upgrade notes

### Community 506 - "[0.12.0] - 2026-08-04"
Cohesion: 0.33
Nodes (6): [0.12.0] - 2026-08-04, Added, Documentation, Fixed, Internal, Upgrade notes

### Community 507 - "[0.13.2] - 2026-08-16"
Cohesion: 0.33
Nodes (6): [0.13.2] - 2026-08-16, Added, Changed, Documentation, Fixed, Internal

### Community 508 - "[0.14.0] - 2026-09-02"
Cohesion: 0.33
Nodes (6): [0.14.0] - 2026-09-02, Added, Changed, Fixed, Internal, Upgrading

### Community 509 - "telemetry_overhead.rs"
Cohesion: 0.73
Nodes (5): bench_capture_sink(), bench_channel_send(), bench_noop_sink(), make_ipc_event(), Criterion

### Community 510 - "setup_dedup_db"
Cohesion: 0.60
Nodes (5): hybrid_dedup_returns_best_match_above_threshold(), hybrid_dedup_returns_none_when_below_threshold(), insert_test_entities(), TempDir, setup_dedup_db()

### Community 512 - "register"
Cohesion: 0.33
Nodes (5): register(), Error, Result, String, sender_pid_display()

### Community 513 - "ADR-0010: Migrate do_extract to tool_use structured output"
Cohesion: 0.33
Nodes (5): ADR-0010: Migrate do_extract to tool_use structured output, Alternatives Considered, Consequences, Context, Decision

### Community 514 - "ADR-0011: Auto-Heal Write-Lock Acquisition from Search Handlers"
Cohesion: 0.33
Nodes (5): ADR-0011: Auto-Heal Write-Lock Acquisition from Search Handlers, Consequences, Context, Decision, Rationale

### Community 515 - "ADR-0012: Edge-to-Episode Associations via Either-Endpoint Entity Traversal"
Cohesion: 0.33
Nodes (5): ADR-0012: Edge-to-Episode Associations via Either-Endpoint Entity Traversal, Consequences, Context, Decision, Rationale

### Community 516 - "ADR-0017: Replace `std::process::exit(0)` with Normal Return in async main"
Cohesion: 0.33
Nodes (5): ADR-0017: Replace `std::process::exit(0)` with Normal Return in async main, Consequences, Context, Decision, Rationale

### Community 517 - "ADR-0022: lbug Cypher Escaping Convention — Backslash, Not SQL Doubling"
Cohesion: 0.33
Nodes (6): ADR-0022: lbug Cypher Escaping Convention — Backslash, Not SQL Doubling, Consequences, Context, Decision, Legacy-Pattern Error Detection, Rationale

### Community 518 - "ADR-0023: Legacy-WAL Translation Layer — Cypher-text/Param-shape vs. Param-value Module Split"
Cohesion: 0.33
Nodes (5): ADR-0023: Legacy-WAL Translation Layer — Cypher-text/Param-shape vs. Param-value Module Split, Consequences, Context, Decision, Rationale

### Community 519 - "ADR-0031: Orphaned Direct RELATES_TO Rels After Noise Edge Deletion"
Cohesion: 0.33
Nodes (5): ADR-0031: Orphaned Direct RELATES_TO Rels After Noise Edge Deletion, Alternatives considered, Cleanup path (future), Context, Decision

### Community 520 - "ADR-0032: Ontology `parent_edges:` segment conditionally included in content hash"
Cohesion: 0.33
Nodes (5): ADR-0032: Ontology `parent_edges:` segment conditionally included in content hash, Consequences, Constraint for future additions, Context, Decision

### Community 521 - "ADR-0033: Noise Edges Are Reclassified to UNCLASSIFIED, Not Deleted"
Cohesion: 0.33
Nodes (6): ADR-0033: Noise Edges Are Reclassified to UNCLASSIFIED, Not Deleted, Alternatives Considered, Consequences, Context, Decision, References

### Community 522 - "ADR-0049: Bare-Path Ontology Loader and CLI Mode-Override Precedence"
Cohesion: 0.33
Nodes (5): ADR-0049: Bare-Path Ontology Loader and CLI Mode-Override Precedence, Consequences, Context, Decision, Related

### Community 523 - "ADR-0331: Validate the Extraction Provider on First Use, Not at Startup"
Cohesion: 0.33
Nodes (6): ADR-0331: Validate the Extraction Provider on First Use, Not at Startup, Consequences, Context, Decision, Related, What did not change

### Community 524 - "ADR-0365: WAL Checkpoints — Directory-Per-Name, Generation-Numbered Exclusive-Create Store"
Cohesion: 0.33
Nodes (6): ADR-0365: WAL Checkpoints — Directory-Per-Name, Generation-Numbered Exclusive-Create Store, Alternatives considered, Consequences, Context, Decision, Why not what the spec's Assumptions section suggested

### Community 525 - "ADR-0392: `rebind_pointers`'s Staleness Gate Keys on Binding State, Not Only Position"
Cohesion: 0.33
Nodes (6): ADR-0392: `rebind_pointers`'s Staleness Gate Keys on Binding State, Not Only Position, Alternatives Considered, Consequences, Context, Decision, Related

### Community 526 - "ADR-0451: Per-Group Ontology Drift — Cache Placement and Clear Scope"
Cohesion: 0.33
Nodes (5): ADR-0451: Per-Group Ontology Drift — Cache Placement and Clear Scope, Consequences, Context, Decision 1: Fold drift into `group_ontologies`'s existing value type, not a new `AppState` field, Decision 2: A remediation clears only the group it remediated, not every cached group

### Community 527 - "ADR-0495: Close the `resolve_ontology` Stale-Drift-Insert Race via Insert-If-Absent"
Cohesion: 0.33
Nodes (5): ADR-0495: Close the `resolve_ontology` Stale-Drift-Insert Race via Insert-If-Absent, Consequences, Context, Decision, Rejected alternatives

### Community 528 - "ADR-0500: Unified Pre-Bootstrap Signal Handling Across All `CliMode`s"
Cohesion: 0.33
Nodes (5): ADR-0500: Unified Pre-Bootstrap Signal Handling Across All `CliMode`s, Consequences, Context, Decision, Rationale

### Community 529 - "test-patterns.sh"
Cohesion: 0.40
Nodes (3): patterns.sh script, expect(), test-patterns.sh script

### Community 530 - "feature_request.md"
Cohesion: 0.33
Nodes (5): Acceptance criteria, Alternatives considered, Constitution alignment, Problem, Proposed solution

### Community 531 - "check-fixture-freshness.sh"
Cohesion: 0.53
Nodes (4): check_exists(), check_manifest_integrity(), check_sentinel(), check-fixture-freshness.sh script

### Community 532 - "main"
Cohesion: 0.60
Nodes (5): default_output(), main(), Path, Write a swift-transformers-compatible offline metadata marker., write_metadata_marker()

### Community 533 - "[0.13.3] - 2026-08-22"
Cohesion: 0.40
Nodes (5): [0.13.3] - 2026-08-22, Added, Changed, Fixed, Internal

### Community 534 - "speckit-clarify/SKILL.md"
Cohesion: 0.40
Nodes (4): Outline, Post-Execution Checks, Pre-Execution Checks, User Input

### Community 535 - "speckit-constitution/SKILL.md"
Cohesion: 0.40
Nodes (4): Outline, Post-Execution Checks, Pre-Execution Checks, User Input

### Community 536 - "speckit-taskstoissues/SKILL.md"
Cohesion: 0.40
Nodes (4): Outline, Post-Execution Checks, Pre-Execution Checks, User Input

### Community 537 - "main"
Cohesion: 0.40
Nodes (4): main(), Box, Error, Result

### Community 538 - "WalReplayer"
Cohesion: 0.50
Nodes (4): Into, PathBuf, Self, WalReplayer

### Community 539 - ".embed"
Cohesion: 0.40
Nodes (4): BoxFuture, Error, Result, Vec

### Community 540 - "setup_db"
Cohesion: 0.50
Nodes (3): insert_entity_with_adversarial_chars(), TempDir, setup_db()

### Community 541 - "ADR-0001: Record Architecture Decisions"
Cohesion: 0.40
Nodes (5): ADR-0001: Record Architecture Decisions, Consequences, Context, Decision, References

### Community 542 - "ADR-0024: Bound-Parameter DB Access — Retire Cypher String Interpolation"
Cohesion: 0.40
Nodes (5): ADR-0024: Bound-Parameter DB Access — Retire Cypher String Interpolation, Alternatives considered, Consequences, Context, Decision

### Community 543 - "ADR-0051: Edge Endpoint Salvage and Deferred Drop Decision"
Cohesion: 0.40
Nodes (5): ADR-0051: Edge Endpoint Salvage and Deferred Drop Decision, Consequences, Context, Decision, Related

### Community 544 - "ADR-0414: Unknown-Generation Streams Refuse to Advance, Not Warn"
Cohesion: 0.40
Nodes (5): ADR-0414: Unknown-Generation Streams Refuse to Advance, Not Warn, Consequences, Context, Decision, Rejected Alternatives

### Community 545 - "ADR-0581: Link OpenSSL Statically on Windows"
Cohesion: 0.40
Nodes (5): ADR-0581: Link OpenSSL Statically on Windows, Amendment (2026-09-15): the lbug extensions still need OpenSSL DLLs, Consequences, Context, Decision

### Community 546 - "ADR-0615: Kind-Scoped Entity Identity — `Entity.kind` and `lookup_key = group_id ␟ kind ␟ name`"
Cohesion: 0.40
Nodes (5): ADR-0615: Kind-Scoped Entity Identity — `Entity.kind` and `lookup_key = group_id ␟ kind ␟ name`, Consequences, Context, Decision, Uniqueness

### Community 547 - "PULL_REQUEST_TEMPLATE.md"
Cohesion: 0.40
Nodes (4): Checklist, Constitution gates, Summary, Testing

### Community 548 - "[CHECKLIST TYPE] Checklist: [FEATURE NAME]"
Cohesion: 0.40
Nodes (4): [Category 1], [Category 2], [CHECKLIST TYPE] Checklist: [FEATURE NAME], Notes

### Community 549 - "[0.13.1] - 2026-08-15"
Cohesion: 0.50
Nodes (4): [0.13.1] - 2026-08-15, Added, Changed, Fixed

### Community 550 - "[0.13.4] - 2026-08-24"
Cohesion: 0.50
Nodes (4): [0.13.4] - 2026-08-24, Added, Fixed, Internal

### Community 551 - "[0.14.1] - 2026-09-06"
Cohesion: 0.50
Nodes (4): [0.14.1] - 2026-09-06, Added, Fixed, Internal

### Community 552 - "[0.14.2] - 2026-09-10"
Cohesion: 0.50
Nodes (4): [0.14.2] - 2026-09-10, Added, Fixed, Internal

### Community 553 - "[0.14.3] - 2026-09-15"
Cohesion: 0.50
Nodes (4): [0.14.3] - 2026-09-15, Added, Fixed, Windows notes

### Community 554 - "[Unreleased]"
Cohesion: 0.50
Nodes (4): Added, Changed, [Unreleased], Upgrading

### Community 555 - "speckit-implement/SKILL.md"
Cohesion: 0.50
Nodes (3): Outline, Pre-Execution Checks, User Input

### Community 556 - "socket"
Cohesion: 1.00
Nodes (3): main(), send_request(), socket

### Community 557 - ".for_read"
Cohesion: 0.50
Nodes (3): KindScope<'a>, Option, Self

### Community 559 - "storage_v41_migration.rs"
Cohesion: 0.83
Nodes (3): extract_fixture(), Path, storage_v41_database_opens_and_migrates_with_correct_reads()

### Community 560 - "storage_v42_migration.rs"
Cohesion: 0.83
Nodes (3): extract_fixture(), Path, storage_v42_database_opens_and_migrates_with_correct_reads()

### Community 561 - "test_embedder_ctx"
Cohesion: 0.50
Nodes (4): test_embedder_ctx(), test_open_or_rebuild_from_wal(), test_open_or_rebuild_returns_none_stats_when_db_already_exists(), test_open_or_rebuild_surfaces_fidelity_warning_on_schema_gap()

### Community 563 - "`lcg-eval` operational scripts"
Cohesion: 0.50
Nodes (3): `lcg-eval` operational scripts, Reference, The traps these encode

### Community 564 - "build_program"
Cohesion: 0.67
Nodes (3): build_program(), main(), Build the MIL program. `output_mil_dtype` is "fp32" or "fp16" and controls the…

### Community 565 - "stage-lbug-extensions.sh"
Cohesion: 0.83
Nodes (3): die(), note(), stage-lbug-extensions.sh script

### Community 566 - "stage-openssl-dlls-windows.sh"
Cohesion: 0.83
Nodes (3): die(), note(), stage-openssl-dlls-windows.sh script

### Community 567 - "stage-openssl-rpath.sh"
Cohesion: 0.83
Nodes (3): die(), note(), stage-openssl-rpath.sh script

### Community 568 - "stage-openssl-windows.sh"
Cohesion: 0.83
Nodes (3): die(), note(), stage-openssl-windows.sh script

### Community 569 - "[0.13.0] - 2026-08-13"
Cohesion: 0.67
Nodes (3): [0.13.0] - 2026-08-13, Added, Fixed

### Community 570 - "[0.15.0] - 2026-09-15"
Cohesion: 0.67
Nodes (3): [0.15.0] - 2026-09-15, Changed, Upgrading

### Community 571 - "[0.9.0] - 2026-07-13"
Cohesion: 0.67
Nodes (3): [0.9.0] - 2026-07-13, Added, Changed

## Knowledge Gaps
- **3639 isolated node(s):** `patterns.sh script`, `common.sh script`, `KindScope<'a>`, `01-start-server.sh script`, `02-timing-check.sh script` (+3634 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **22 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `AppState` connect `AppState` to `migration.rs`, `add_episode`, `dispatch_val`, `ipc_response_shapes.rs`, `.new`, `handlers_wal_admin.rs`, `handlers.rs`, `embedding_cache.rs`, `ipc_parity.rs`, `IpcRequest`, `make_db`, `ontology_integration.rs`, `wal_vector_stripping.rs`, `tests/group_purge.rs`, `backfill_summary_embeddings_wal.rs`, `edge_endpoint_resolution.rs`, `assert_ok_resp`, `tests/assert.rs`, `Extractor`, `wal_root_migration.rs`, `ArcSwapOption`, `canonicalize_integration.rs`, `wal_generation_reset.rs`, `search_result_order.rs`, `kind_identity.rs`, `real_corpus_e2e.rs`, `canonicalize.rs`, `WalWriter`, `extraction_quality.rs`, `per_group_ontology.rs`, `telemetry_ipc.rs`, `concurrent_rw_integration.rs`, `ExtractedEntity`, `wal_exec.rs`, `tier1c_deletion.rs`, `wal_population.rs`, `wal_strip_embeddings.rs`, `TelemetrySink`, `error.rs`, `Ontology`, `reprocess_relations.rs`, `cancel_shutdown.rs`, `Embedder`, `handlers_wal_dump.rs`, `Db`, `concurrent_rw.rs`, `make_state_without_indices`, `cross_group_incremental_replay.rs`, `dedup_auto_heal_integration.rs`, `backfill.rs`, `backfill_summary_embeddings`, `core/src/lib.rs`, `cross_episode_dedup.rs`, `ingest_embed_batching.rs`, `StandaloneBackend`, `backfill_wal.rs`, `summary_semantic_search.rs`?**
  _High betweenness centrality (0.189) - this node is a cross-community bridge._
- **Why does `run_socket_service()` connect `migration.rs` to `AppState`, `transport.rs`, `ExtractOptions`, `TelemetrySink`?**
  _High betweenness centrality (0.149) - this node is a cross-community bridge._
- **Why does `Listener` connect `transport.rs` to `migration.rs`?**
  _High betweenness centrality (0.147) - this node is a cross-community bridge._
- **What connects `patterns.sh script`, `common.sh script`, `KindScope<'a>` to the rest of the system?**
  _3639 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `migration.rs` be split into smaller, more focused modules?**
  _Cohesion score 0.06400208986415883 - nodes in this community are weakly interconnected._
- **Should `adr/index.md` be split into smaller, more focused modules?**
  _Cohesion score 0.07630522088353414 - nodes in this community are weakly interconnected._
- **Should `embedder_transport.rs` be split into smaller, more focused modules?**
  _Cohesion score 0.07346459006758742 - nodes in this community are weakly interconnected._