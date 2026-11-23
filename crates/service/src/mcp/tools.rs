//! Static registry of MCP tools derived from the `knowledge_*` dispatch methods in
//! `lcg_core::handlers` (FR-002). Tool name == IPC method name, verbatim, so the registry
//! stays directly auditable against `handlers.rs`'s `match` arms — nothing here is derived by
//! reflection, and adding a new `knowledge_*` method requires a matching new entry here.
//!
//! Schemas are plain `serde_json::Value` literals rather than per-tool `schemars`-derived
//! structs: tool-call arguments pass straight through to `handlers::dispatch` as a raw
//! `Value` (FR-003), so there is no typed deserialization step that would justify ~39 throwaway
//! structs. This is the single source of truth FR-002 requires; there is no second,
//! hand-maintained schema anywhere else.
//!
//! Descriptions and schemas are authored from the issue spec's FRs and from each handler's
//! actual parameter extraction in `handlers.rs`, per the spec's own Assumptions fallback: the
//! app's zod tool defs live in a separate closed-source repo not reachable from this
//! environment, so SC-006's "verified against the zod defs" comparison is a manual step (see
//! the PR description) rather than one this registry can automate.

use serde_json::{json, Value};

use crate::mcp::scope::Scope;

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub scope: Scope,
    pub input_schema: fn() -> Value,
}

fn empty_schema() -> Value {
    json!({"type": "object", "properties": {}})
}

/// `knowledge_status` input schema (issue #884): one optional opt-in flag.
fn status_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "include_episode_breakdown": {
                "type": "boolean", "default": false,
                "description": "When true, add `episode_breakdown`: per group, the live, \
                                 superseded and pending episode counts plus \
                                 only_superseded_entities and only_superseded_edges (ADR-0828 \
                                 reopening triggers). Scans every episode, MENTIONS edge and \
                                 edge under the read lock, so it is off by default."
            }
        }
    })
}

fn group_ids_prop() -> Value {
    json!({
        "type": "array",
        "items": {"type": "string"},
        "description": "Optional group IDs to scope the operation to. Omitting this searches \
                         or lists across every group present in the graph — not a default group."
    })
}

fn kind_filter_prop() -> Value {
    json!({
        "type": "string",
        "description": "Optional entity kind filter (issue #615). Omitted: all kinds are \
                         returned (reads are broad). Given: only entities of that kind."
    })
}

fn origin_prop() -> Value {
    json!({
        "type": "string", "enum": ["primary", "external", "derived", "answer"],
        "description": "What kind of content this episode is (issue #752): `primary` (a \
                         human-written note — the default when omitted), `external` (clipped or \
                         imported third-party material), `derived` (an agent-written page or a \
                         consolidation) or `answer` (a filed Q&A response). A claim by the \
                         caller, not authenticated. `derived` and `answer` episodes are cited \
                         but never counted as corroboration, never override primary/external \
                         content, and every episode of one `doc_id` must share one origin."
    })
}

fn origins_prop() -> Value {
    json!({
        "type": "array", "minItems": 1,
        "items": {"type": "string", "enum": ["primary", "external", "derived", "answer"]},
        "description": "Optional origin filter (issue #752): keep only results supported by at \
                         least one episode with one of these origins (e.g. [\"primary\", \
                         \"external\"] to exclude an agent's own derived pages). Omitted: no \
                         filter. An empty list or unknown value is rejected."
    })
}

fn cursor_prop() -> Value {
    json!({
        "type": "string",
        "description": "Opaque paging cursor (issue #667). Pass an empty string to start \
                         paging, then pass back each response's `next_cursor` until it is \
                         null. Sending `cursor`, `fields` or `name_prefix` makes the response \
                         carry `next_cursor`. A cursor is only valid for the same group, kind \
                         and name_prefix filters it was issued for; anything else is rejected."
    })
}

fn fields_prop(allowed: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "string"},
        "minItems": 1,
        "description": format!(
            "Optional projection (issue #667): return only these keys per item. Allowed: \
             {allowed}. Unknown names are rejected. Embeddings are never returned. Omitted: \
             the full shape."
        )
    })
}

fn min_similarity_prop() -> Value {
    json!({
        "type": "number", "minimum": 0.0, "maximum": 1.0,
        "description": "Optional minimum cosine similarity (clamped 0.0-1.0) for vector \
                         candidates, applied before fusion. Full-text matches are never \
                         dropped. No default: omitted means no floor."
    })
}

/// Belief-time parameters (issue #705) shared by the five relationship reads, spread into each
/// tool's `properties`.
fn belief_props() -> [(&'static str, Value); 2] {
    [
        (
            "believed_only",
            json!({
                "type": "boolean",
                "description": "Opt in to belief filtering (issue #705): drop edges the graph did \
                                 not believe at `as_of` (default now) — e.g. a fact whose only \
                                 source episode was superseded by a correction. Superseded edges \
                                 stay readable without this flag. Every returned edge also gains \
                                 a `belief` object (state, producing_episode_uuids, warrant, \
                                 valid_at_source). With `as_of` set and this false, edges are \
                                 annotated but not filtered. Capped reads over-fetch, so fewer \
                                 than `num_results` edges may come back."
            }),
        ),
        (
            "as_of",
            json!({
                "type": "string", "format": "date-time",
                "description": "Evaluate belief as of this time (RFC 3339) instead of now. Implies \
                                 `believed_only` unless that is explicitly false. Belief is \
                                 transaction time, distinct from `valid_at`/`invalid_at`."
            }),
        ),
    ]
}

/// `props` plus the [`belief_props`] entries, as a JSON object.
fn with_belief(mut props: Value) -> Value {
    for (k, v) in belief_props() {
        props[k] = v;
    }
    props
}

/// The full, ordered registry — one entry per `knowledge_*` dispatch method (58 total),
/// matching FR-004's scope table exactly.
pub fn registry() -> Vec<ToolSpec> {
    vec![
        // ── read (22) ──────────────────────────────────────────────────────────────
        ToolSpec {
            name: "knowledge_status",
            description: "Get knowledge graph status: entity/episode/relationship counts, \
                           embedding config, WAL state (including each group's applied_seq, \
                           max_seq, current on-disk generation, and generation_status — issue \
                           #387/#414, cheap: no extra WAL directory scan), ontology summary, \
                           per-group ontology drift (group_ontology_drift — issue #451, an array \
                           of {group_id, drifted, drift_summary} for every group this process has \
                           resolved an ontology for; a group not yet used in this process is \
                           simply absent, not falsely reported as not drifted), the \
                           extraction-time dedup mode (dedup_mode — issue #652, \"veto-only\" or \
                           \"llm-verified\"), and whether search indices are built. generation_status is \
                           \"not_applicable\" (no WAL stream yet), \"unknown\" (stream exists but \
                           .wal-generation.json is missing or corrupt — knowledge_rebuild_from_wal \
                           will refuse once a position is recorded), or \"known\". Returns a \
                           status object (not a JSON-RPC error) \
                           when the database is open but a core table is missing — check the \
                           `queryable` field (and `reason` when false) to distinguish that state \
                           from a genuinely empty graph, whose counts read as 0 rather than \
                           null. Other query failures still surface as JSON-RPC errors. The flat \
                           `wal` block always describes the DEFAULT group's own stream; when a \
                           WAL root is configured it carries `scope: \"default_group\"`, \
                           `default_group` and `see: \"wal_groups\"` (issue #667), so \
                           `exists: false` there means the default group has no stream, not \
                           that WAL is broken — read `wal_groups` for every group. Pass \
                           `include_episode_breakdown: true` (issue #884, opt-in because it scans \
                           every episode, MENTIONS edge and edge) to add `episode_breakdown`: \
                           `{as_of_us, groups: [{group_id, live, superseded, pending, \
                           only_superseded_entities, only_superseded_edges}]}`; the superseded : \
                           live ratio per group is ADR-0828's reopening trigger 1. When it cannot \
                           be computed it is null with `episode_breakdown_unavailable`.",
            scope: Scope::Read,
            input_schema: status_schema,
        },
        ToolSpec {
            name: "knowledge_capabilities",
            description: "Describe this server (issue #854): `server_version` (the crate \
                           version, as `initialize` reports it), `protocol_version` (integer, \
                           1 today; bumped only on a breaking change to the socket protocol), \
                           `features` (sorted array of stable lower_snake_case feature strings; \
                           append-only, so test for the one you need and ignore unknown ones) \
                           and `methods` (sorted names of every IPC method the dispatcher \
                           accepts). Static build information: no database access, no lock, so \
                           it answers in degraded mode and while a write or rebuild is running. \
                           Takes no parameters.",
            scope: Scope::Read,
            input_schema: empty_schema,
        },
        ToolSpec {
            name: "knowledge_find_entities",
            description: "Hybrid (full-text + vector) search for entities matching a query. \
                           Every returned node carries its `kind` (issue #615). `kind` is an \
                           optional filter: omitted, the search spans ALL kinds and returns \
                           same-named entities of different kinds as separate nodes (a multi-row \
                           read never picks one and never errors on ambiguity — use \
                           `knowledge_resolve_entity` when exactly one entity is needed); given, \
                           only entities of that kind are returned. Every node also carries a \
                           `search` object (issue #629) explaining why it matched: `rrf_score` \
                           (fused, rank-based — NOT a relevance measure on its own), \
                           `text_match` (true iff the full-text path retrieved it), `bm25_score`, \
                           `name_similarity` and `summary_similarity` (cosine similarity, \
                           1 - distance). A path that did not retrieve the node reports null \
                           (`text_match` false). Optional `min_similarity` (0-1, clamped, no \
                           default) drops vector candidates below that cosine similarity \
                           before fusion, per vector path (name, summary); full-text matches \
                           stay eligible regardless, so an exact-name query still returns its \
                           match while a gibberish query returns nothing. Unset, there is no \
                           floor. With `kind`, the floor applies on top of the kind path's \
                           bounded nearest-neighbour probe, which can already miss a rare \
                           kind. Every node also carries `support` (issue #752): the number of \
                           distinct source documents mentioning it per origin (`primary`, \
                           `external`, `derived`, `answer`; a `doc_id` is one document, an \
                           episode without one counts as its own) and `corroboration` \
                           (primary + external). Counts reflect live episodes now. `origins` \
                           keeps only entities mentioned by an episode of one of those origins; \
                           over-fetched and post-filtered, so fewer than `num_results` can come \
                           back. Provenance (issue #857): by default each node's \
                           `episode_uuids` and `source_descriptions` are EMPTY here (unlike \
                           `knowledge_get_entity_neighbors` / `knowledge_get_entities_by_source`, \
                           which fill them) and there is no `source_files`. With \
                           `include_episodes: true` they are filled from the entity's LIVE \
                           mentioning episodes (not superseded, deleted or part of an uncommitted \
                           document revision, the same predicate `knowledge_walk` provenance and \
                           search use), newest first, capped at 50 per entity (`knowledge_walk` \
                           caps at 20 per edge): `episode_uuids` and `source_descriptions` are index-aligned; \
                           `source_files` is the de-duplicated stored source files (episodes \
                           with none recorded are omitted, so it is not aligned); \
                           `episodes_truncated: true` appears only when the cap dropped \
                           episodes. `support` still counts every live document, not just the \
                           capped list. Fetch the episodes themselves with \
                           `knowledge_get_episodes` and `uuids`.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search text."},
                        "group_ids": group_ids_prop(),
                        "kind": kind_filter_prop(),
                        "num_results": {
                            "type": "integer", "minimum": 1, "default": 10,
                            "description": "Maximum number of entities to return."
                        },
                        "min_similarity": min_similarity_prop(),
                        "origins": origins_prop(),
                        "include_episodes": {
                            "type": "boolean", "default": false,
                            "description": "When true, fill each node's `episode_uuids`, \
                                             `source_descriptions` and `source_files` from its \
                                             live mentioning episodes (newest first, at most 50; \
                                             `episodes_truncated: true` when capped). Default \
                                             false: those fields stay empty / absent."
                        }
                    },
                    "required": ["query"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_resolve_entity",
            description: "Resolve ONE entity by `(group_id, name[, kind])` (issue #615) — exact, \
                           case-insensitive, whitespace-normalized name match, following a \
                           Merged tombstone to its canonical. With `kind`, the lookup is exact \
                           and can never be ambiguous. WITHOUT `kind` it spans ALL kinds (reads \
                           are broad): if exactly one kind has an entity of that name it is \
                           returned; if more than one does, the call fails with JSON-RPC error \
                           code -32002 (`error.data.reason` = \"ambiguous_entity\", \
                           `error.data.candidates` = [{uuid, kind}, ...]) — it never silently \
                           picks one. Returns `{found, node}`; `found` is false (node null) when \
                           nothing matches.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Entity name (required)."},
                        "group_id": {"type": "string", "default": "liminis"},
                        "kind": {"type": "string", "description": "Optional kind for an exact lookup. Omitted: spans all kinds and errors (-32002) if more than one kind matches."}
                    },
                    "required": ["name"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_find_relationships",
            description: "Hybrid (full-text + vector) search for relationships (facts) \
                           matching a query. Returned edges' `episode_uuids` is always \
                           empty on this read path (not populated). The response returns \
                           the relationship list under the `edges` key. Does not include \
                           episode `attributes` (issue #528) — this path returns edges, \
                           never a full episode object. Every edge also carries a `search` \
                           object (issue #629): `rrf_score` (fused, rank-based — NOT a \
                           relevance measure on its own), `text_match` (true iff the \
                           full-text path retrieved it), `bm25_score` and `fact_similarity` \
                           (cosine similarity, 1 - distance); a path that did not retrieve \
                           the edge reports null (`text_match` false). Optional \
                           `min_similarity` (0-1, clamped, no default) drops fact-vector \
                           candidates below that cosine similarity before fusion; full-text \
                           matches stay eligible regardless. Unset, there is no floor. Every \
                           edge also carries `support` (issue #752): distinct source documents \
                           per origin (`primary`, `external`, `derived`, `answer`) across the \
                           episodes that produced it and its same-fact siblings (edges sharing \
                           endpoints and relation name), and `corroboration` (primary + \
                           external). A derived restatement adds to `derived`, never to \
                           `corroboration`. Derived from `episodes`, never `episode_uuids`; \
                           counts reflect live episodes now. `origins` keeps only edges whose \
                           own producing episodes include one of those origins; over-fetched \
                           and post-filtered, so fewer than `num_results` can come back.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": with_belief(json!({
                        "query": {"type": "string", "description": "Search text."},
                        "group_ids": group_ids_prop(),
                        "num_results": {
                            "type": "integer", "minimum": 1, "default": 10,
                            "description": "Maximum number of relationships to return."
                        },
                        "min_similarity": min_similarity_prop(),
                        "origins": origins_prop()
                    })),
                    "required": ["query"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_lint",
            description: "Deterministic notebook health report for a group (issue #755): a \
                           paged list of cited findings plus per-category counts, computed from \
                           the graph with no LLM and no embedding-model call. Categories: \
                           `contradiction` (two currently-believed facts with the same source and \
                           a relation you designate in `single_valued_relations`, different \
                           targets; `unavailable` without that list), `stale_derived_page` (a \
                           `derived` document restating facts whose primary/external support was \
                           since corrected, removed or superseded), `weak_support` (believed \
                           facts backed by derived content only or a single source document), \
                           `orphan_entity`, `near_duplicate_entity` (name similarity >= \
                           `near_duplicate_threshold`, same kind) and `unlinked_mention` (needs \
                           `page_for` / `links` in episode attributes, else `unavailable`). Every \
                           finding has a content-derived stable `id` (survives unrelated \
                           ingests, WAL rebuild and checkpoint restore, so you can track and \
                           dismiss it), a `reason`, the numbers behind it and `citations` \
                           (episodes, documents, entities, facts). `summary` counts every \
                           selected category over the whole scope on every page. Page with \
                           `cursor`/`next_cursor`. Read-only: it never merges, edits or deletes \
                           anything and suggests no fix. Origin is a caller claim, not \
                           authenticated (ADR-0752).",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_ids": group_ids_prop(),
                        "group_id": {
                            "type": "string",
                            "description": "Single-group alias for `group_ids`; unioned with it \
                                             when both are given. Omit both for all groups."
                        },
                        "categories": {
                            "type": "array",
                            "items": {"type": "string", "enum": [
                                "contradiction", "stale_derived_page", "weak_support",
                                "orphan_entity", "near_duplicate_entity", "unlinked_mention"
                            ]},
                            "description": "Categories to evaluate (default: all). Unselected \
                                             categories cost no work and are absent from `summary`."
                        },
                        "limit": {
                            "type": "integer", "minimum": 1, "maximum": 500, "default": 50,
                            "description": "Findings per page."
                        },
                        "cursor": cursor_prop(),
                        "single_valued_relations": {
                            "type": "array", "items": {"type": "string"},
                            "description": "Relation names (matched against a fact's name or \
                                             relation_type, trimmed and case-insensitive) that \
                                             hold at most one target per source, e.g. \
                                             `[\"lives_in\"]`. Required for `contradiction`; \
                                             without it that category is `unavailable`."
                        },
                        "near_duplicate_threshold": {
                            "type": "number", "exclusiveMinimum": 0, "maximum": 1, "default": 0.9,
                            "description": "Normalised-name similarity at or above which two \
                                             entities of one group and kind are a candidate \
                                             merge. Echoed back as `effective`."
                        },
                        "min_mentions": {
                            "type": "integer", "minimum": 1, "default": 3,
                            "description": "Distinct documents that must mention an entity \
                                             before it is reported as lacking a page."
                        }
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_get_episodes",
            description: "Retrieve the most recent episodes (ingested source documents), \
                           newest first. Scoped by `group_ids` (array) and/or the single-group \
                           alias `group_id` (both given: their union); with neither, episodes \
                           from EVERY group are returned (issue #667 — this used to default to \
                           the `liminis` group). Each returned episode includes its \
                           `attributes` (issue #528), the caller-supplied structured metadata \
                           set via `knowledge_process_chunk`/`knowledge_add_episode`. Page \
                           with `cursor`/`next_cursor`, drop large fields such as `content` \
                           with `fields`, and fetch e.g. every chunk of one document with \
                           `name_prefix` (issue #667). `uuids` (issue #857) returns exactly \
                           those episodes (still group-scoped; unknown uuids are simply absent, \
                           an empty array returns nothing); pair it with `knowledge_find_entities` \
                           `include_episodes`.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_ids": group_ids_prop(),
                        "group_id": {
                            "type": "string",
                            "description": "Single-group alias for `group_ids`; unioned with it \
                                             when both are given. Omit both for all groups."
                        },
                        "last_n": {
                            "type": "integer", "minimum": 1, "default": 50,
                            "description": "Page size: number of most recent episodes to return. \
                                             With `uuids` the default is the number of uuids."
                        },
                        "uuids": {
                            "type": "array", "items": {"type": "string"},
                            "description": "Only these episodes (by uuid), within the group \
                                             scope. Unknown uuids are absent, not an error; an \
                                             empty array returns no episodes."
                        },
                        "name_prefix": {
                            "type": "string",
                            "description": "Only episodes whose name starts with this prefix \
                                             (case-sensitive, matched literally)."
                        },
                        "cursor": cursor_prop(),
                        "fields": fields_prop(
                            "uuid, name, group_id, created_at, ingested_at, source, source_description, \
                             content, valid_at, entity_edges, attributes, superseded_at, superseded_by, \
                             superseded_by_kind, supersession_reason, doc_id, origin"
                        )
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_get_nodes_by_group",
            description: "List all entity nodes belonging to the given group IDs, or every \
                           group's nodes when group_ids is omitted.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {"group_ids": group_ids_prop()}
                })
            },
        },
        ToolSpec {
            name: "knowledge_get_edges_by_group",
            description: "List all relationship edges belonging to the given group IDs, or \
                           every group's edges when group_ids is omitted. Returned edges' \
                           `episode_uuids` is always empty on this read path (not populated). \
                           Does not include episode `attributes` (issue #528) — this path \
                           returns edges, never a full episode object.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": with_belief(json!({"group_ids": group_ids_prop()}))
                })
            },
        },
        ToolSpec {
            name: "knowledge_get_edges_by_uuids",
            description: "Fetch relationship edges by their UUIDs. Returned edges' \
                           `episode_uuids` is always empty on this read path (not populated). \
                           Does not include episode `attributes` (issue #528) — this path \
                           returns edges, never a full episode object.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": with_belief(json!({
                        "uuids": {
                            "type": "array", "items": {"type": "string"},
                            "description": "Edge UUIDs to fetch."
                        }
                    })),
                    "required": ["uuids"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_get_correction_changes",
            description: "What a correction changed (issue #706). For a SUPERSEDED episode, \
                           returns the facts it supported that its replacement does not \
                           (`removed`) and the facts the replacement supports that it did not \
                           (`added`); a fact on both sides appears on neither. Facts match on \
                           (source entity, target entity, relation name, fact text), so a \
                           reworded fact is one removal plus one addition. For a `chunk_removed` \
                           episode `replacement` is null and `added` is empty. A split chunk's \
                           replacement is the whole revision, not only its final unit. An episode \
                           that was never superseded returns `status: \"not_superseded\"` and no \
                           facts. Compares an episode with its direct superseder only (not later \
                           generations). Read-only: writes nothing, including no \
                           `valid_at`/`invalid_at`. Heading renames and whole-document diffs need \
                           document identity (#707).",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "episode_uuid": {
                            "type": "string",
                            "description": "UUID of the superseded episode to explain (required)."
                        }
                    },
                    "required": ["episode_uuid"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_get_affected_work",
            description: "What was written while a dropped belief held (issue #706). For an edge \
                           that is no longer believed, returns `candidates`: episodes ingested \
                           within the edge's belief interval [start, end) that MENTION either of \
                           its endpoint entities, ordered by ingest time. They are candidates by \
                           ENTITY OVERLAP (`basis: \"entity_overlap\"`), not proof that the work \
                           used the fact; review them. The edge's own producing episodes are not \
                           reported. `interval.state` is `closed` (candidates returned), `open` \
                           (still believed; affected work is not defined), `no_record` (legacy \
                           edge, or its support was hard-deleted) or `not_defined` (warranted \
                           direct assertion); every state other than `closed` returns no \
                           candidates and says why, so an empty list is never ambiguous. \
                           Read-only: writes nothing.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "edge_uuid": {
                            "type": "string",
                            "description": "UUID of the dropped edge (required)."
                        }
                    },
                    "required": ["edge_uuid"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_search_passages",
            description: "Passage search over ingested episode text, returning scored \
                           text snippets. `mode` selects `vector` (cosine similarity, the \
                           default unless the service sets LCG_PASSAGE_SEARCH_MODE) or `hybrid` \
                           (BM25 over episode text fused with the vector ranking by weighted \
                           Reciprocal Rank Fusion, the lexical list weighing less, issue #789); \
                           in hybrid mode `score` is a rank-only fusion value (at most about \
                           0.0233), not a relevance and not comparable across queries; \
                           `similarity` carries the cosine where the vector retriever matched \
                           and is the relevance signal. `min_score` floors the vector leg's \
                           cosine only, so lexical (BM25) hits still appear below it. Each \
                           hybrid result carries `legs` (`dense`, `lexical` or `both`: which \
                           retrievers found it; `lexical` means no `similarity`) and, in \
                           single-query hybrid, `lexical_score` (the raw BM25 value when the \
                           lexical leg matched); both are omitted in `vector` mode. \
                           `belief_aware` (default on in both modes; send `false` to opt out, or \
                           set LCG_PASSAGE_BELIEF_AWARE=off on the service to restore the old \
                           default) ranks a derived page below the primary note it cites and a \
                           superseded passage below its successor; demoted passages can have \
                           a lower `score` as well as a later position. Each result includes the `attributes` (issue \
                           #528) of the episode it belongs to — no separate per-episode fetch \
                           is needed to obtain them. Each result also carries the `origin` \
                           (issue #752) of its episode, and `origins` restricts results to \
                           episodes of those origins (e.g. exclude an agent's own derived \
                           pages); the filter runs after the nearest-neighbour probe, which is \
                           oversampled, so fewer than `num_results` can come back. \
                           Supply `queries` (up to 5 extra phrasings of `query`) when a direct \
                           search came back thin, or when the question describes something \
                           rather than naming it: the note that answers it often shares few \
                           words with the question. Good variants are keywords the note itself \
                           would use, a likely name for the thing described, and a \
                           one-sentence hypothetical answer. Every variant must keep the \
                           question's named entities (people, projects, policies, suppliers) and \
                           rephrase only the rest; do not send template answers with \
                           placeholders such as \"[supplier name]\". Variants help most for \
                           described-not-named lookups and exact passage lookups; do not send \
                           them for multi-part questions whose entities are already explicit, \
                           where they can pull the right notes down. Every variant runs the request's \
                           whole search (its `mode`, filters and `min_score`; in hybrid mode a \
                           variant's keywords reach BM25) and the rankings are fused into one \
                           list in which `query` always keeps at least two thirds of the \
                           weight, so variants widen the candidates but never outvote it; \
                           `belief_aware` ranking then runs once on the fused list. With \
                           `queries`, `score` is the fused rank score, `similarity` the best \
                           cosine, and each result carries `matched_queries`: indices into \
                           `[query, ...queries]` of the queries that retrieved it (0 is \
                           `query`). Without `queries` the response is unchanged. Two \
                           optional stages (issue #796), both off by default: `rerank` \
                           rescores the top candidates with a local cross-encoder and blends \
                           in belief and recency so a superseded note does not outrank its \
                           replacement when both are among the reranked candidates \
                           (`rerank_candidates`, default 50, the maximum; latency target: 150 ms p50 at k = 10 on Apple \
                           silicon, which the default model does not meet — recorded Core ML \
                           p50 is about 1.1 s at 25 candidates and 1.9 s at 50, on one \
                           maintainer machine, p95 not measured; the earlier ~587 ms was a \
                           Python/MPS run; see ADR-0796); `expand` rewrites the query with a \
                           local LLM into keyword, paraphrase and hypothetical-answer \
                           variants fused by RRF (expected to cost on the order of a second; \
                           not yet measured — for deliberate agent searches, not \
                           search-as-you-type). When a stage is requested the \
                           response gains a `rerank` / `expansion` block with `applied` and, \
                           if it could not run, a `reason`; results then fall back to the \
                           un-staged order rather than failing.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search text (required, non-empty). The primary query."},
                        "queries": {
                            "type": "array", "maxItems": 5,
                            "items": {"type": "string", "minLength": 1},
                            "description": "Optional extra query variants, at most 5, each non-empty, at most 2000 characters in total. Use when a direct search came back thin or the question describes something rather than naming it. Good variants: keywords the note would use, a likely name for the thing described, a one-sentence hypothetical answer. Every variant must keep the question's named entities (people, projects, policies, suppliers); no placeholders like \"[supplier name]\". Do not send variants for multi-part questions whose entities are already explicit."
                        },
                        "num_results": {
                            "type": "integer", "minimum": 1, "maximum": 100, "default": 10,
                            "description": "Maximum number of passages to return (clamped 1-100)."
                        },
                        "min_score": {
                            "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.5,
                            "description": "Minimum cosine similarity (clamped 0.0-1.0). When omitted, \
                                the operator-configured default applies (LCG_SEARCH_MIN_SCORE; \
                                0.5 if unset, reported as `search_min_score_default` by \
                                knowledge_status). An explicit value, including 0.0, always \
                                wins. In hybrid mode it floors only the vector retriever's \
                                candidates; a passage found only by the lexical (BM25) retriever \
                                is not excluded by it."
                        },
                        "mode": {
                            "type": "string", "enum": ["vector", "hybrid"],
                            "description": "Retrieval mode: `vector` (cosine, then belief-aware \
                                re-ranking unless `belief_aware` is false) or `hybrid` \
                                (BM25 + vector, weighted RRF). Omitted: the service default \
                                (LCG_PASSAGE_SEARCH_MODE, else `vector`). Unknown values are \
                                rejected."
                        },
                        "belief_aware": {
                            "type": "boolean",
                            "description": "Belief-aware re-ranking: a `derived`/`answer` passage \
                                ranks below the `primary`/`external` note it cites by \
                                [[wikilink]], and a superseded passage below its successor. \
                                Superseded passages are demoted, never dropped. Default: true in \
                                both modes (issue #806). `false` restores the previous, \
                                non-belief ordering; omitted, the service default applies \
                                (LCG_PASSAGE_BELIEF_AWARE=off turns the default off; an \
                                explicit value here always wins)."
                        },
                        "group_ids": group_ids_prop(),
                        "origins": origins_prop(),
                        "rerank": {
                            "type": "boolean",
                            "description": "Rescore the top candidates with a local cross-encoder, belief- and recency-aware. Overrides the workspace default (LCG_RERANK); omitted means the default, which is off."
                        },
                        "rerank_candidates": {
                            "type": "integer", "minimum": 1, "maximum": 50,
                            "description": "How many first-stage candidates the reranker rescores (default 50, which is also the maximum; LCG_RERANK_CANDIDATES). Results beyond it follow in first-stage order."
                        },
                        "expand": {
                            "type": "boolean", "default": false,
                            "description": "Rewrite the query with a local LLM into lexical, semantic and hypothetical-answer variants and fuse their results with RRF (runs before rerank). Expected to cost on the order of a second (not yet measured); never on by default."
                        }
                    },
                    "required": ["query"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_list_entities",
            description: "List entity nodes, optionally scoped to specific group IDs, with \
                           episode provenance attached. Does not include episode `attributes` \
                           (issue #528) — this path surfaces only entity-scoped provenance, \
                           never a full episode object. Every returned node carries its `kind` \
                           (issue #615); `kind` is an optional filter — omitted, ALL kinds are \
                           listed (same-named entities of different kinds appear as separate \
                           nodes); given, only that kind. Page with `cursor`/`next_cursor`, \
                           project with `fields`, and narrow by `name_prefix` (issue #667).",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "num_results": {
                            "type": "integer", "minimum": 1, "default": 500,
                            "description": "Page size: maximum number of entities to return."
                        },
                        "group_ids": group_ids_prop(),
                        "kind": kind_filter_prop(),
                        "name_prefix": {
                            "type": "string",
                            "description": "Only entities whose name starts with this prefix \
                                             (case-insensitive, matched literally)."
                        },
                        "cursor": cursor_prop(),
                        "fields": fields_prop(
                            "uuid, name, group_id, labels, kind, created_at, ingested_at, summary, \
                             attributes, episode_uuids, source_descriptions"
                        )
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_list_relationships",
            description: "List relationship edges (facts), optionally scoped to specific \
                           group IDs. Each edge's `episode_uuids` lists episodes that mention \
                           its source or target entity (either-endpoint co-occurrence), not \
                           evidence for that specific relationship. The response returns the \
                           relationship list under the `edges` key. Does not include episode \
                           `attributes` (issue #528) — this path returns edges, never a full \
                           episode object.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": with_belief(json!({
                        "num_results": {
                            "type": "integer", "minimum": 1, "default": 1000,
                            "description": "Maximum number of relationships to return."
                        },
                        "group_ids": group_ids_prop()
                    }))
                })
            },
        },
        ToolSpec {
            name: "knowledge_get_entity_neighbors",
            description: "Get the immediate graph neighborhood (connected edges and nodes) \
                           of an entity. Each returned edge's `episode_uuids` lists episodes \
                           that mention its source or target entity (either-endpoint \
                           co-occurrence), not evidence for that specific relationship. Does \
                           not include episode `attributes` (issue #528) — this path returns \
                           edges and nodes, never a full episode object.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": with_belief(json!({
                        "entity_uuid": {"type": "string", "description": "Center entity UUID (required)."},
                        "num_results": {
                            "type": "integer", "minimum": 1, "default": 50,
                            "description": "Maximum number of neighbors to return."
                        },
                        "group_ids": group_ids_prop()
                    })),
                    "required": ["entity_uuid"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_get_ontology",
            description: "Return the effective ontology of a group (group file, then workspace, \
                           then none) as a planning menu for `knowledge_walk` (issue #820): \
                           `mode`, `ontology_hash` (the semantic content hash of the \
                           loaded ontology, or \"none\"; issue #881), `entity_types` (name, \
                           description, parent, identity, extract) and `relation_types` (name, description, `source_type` and \
                           `target_type` lists, `inverse_aliases`, `symmetric`, aliases, \
                           keywords, extract). Assert-only types (`extract: false`) are omitted \
                           unless `include_asserted` is true. A group with no ontology returns \
                           `has_ontology: false`, `mode: \"free-form\"` and empty lists — not an \
                           error. Read-only.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {"type": "string", "default": "liminis"},
                        "include_asserted": {
                            "type": "boolean", "default": false,
                            "description": "Include assert-only (`extract: false`) entity and relation types."
                        }
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_walk",
            description: "Answer a question by FOLLOWING TYPED RELATIONS from an entity, along a \
                           walk YOU plan (issue #820). LCG makes no LLM call: you are the \
                           planner, this tool runs the walk and returns the facts. How to plan: \
                           (1) Call `knowledge_get_ontology` once to get the relation and entity \
                           types. (2) Pick the `start` entity (name or UUID; an ambiguous name \
                           walks from every match), then for each hop the `relation_types` to \
                           follow, a short `phrase` describing the hop (e.g. \"supplier of the \
                           project\", used for ranking), and the `direction` AS THE TYPE NAME \
                           READS — \"X REPORTS_TO Y: from X to Y is `out`\", from Y to X is `in`, \
                           `any` for no preference — plus the `answer_type` (an entity type name, \
                           or \"value\") and the question as `query`. (3) Fall back to \
                           `knowledge_search_passages` for questions about how something changed \
                           over time, or questions with no clear start entity. \
                           Each hop keeps the top edges (3 in `single` mode, 10 in `list` and \
                           `history`) ranked by similarity of the hop phrase to the edge fact; a \
                           relation-type, direction or answer-type filter that matches nothing \
                           falls back to all candidates and is reported in `fallbacks`. If the \
                           hops end short of `answer_type`, one completion hop runs (needs \
                           `query`). Returns, per hop, the chosen edges (`uuid`, `relation_type`, \
                           `fact`, `source`/`target` {uuid, name, type}, `valid_at`, \
                           `invalid_at`, `score`, `direction_matched`, `signature_mismatch`), \
                           the resolved `start_entities`, `completion_hop_ran`, and `facts_text` \
                           — typed, deduplicated facts ready to hand to a reader. If `start` \
                           resolves to nothing, `found` is false. Set `include_provenance: true` \
                           to cite sources: each edge then also carries `episodes` (UUIDs of the \
                           LIVE producing episodes only — superseded or deleted ones are never \
                           listed — at most 20), `episodes_truncated` (true when more than 20 \
                           live producers exist) and `source_files` (the distinct sources of the \
                           live producers: each episode's `source_file`, or for a legacy episode \
                           its stored `source_description` unsplit; an edge with no recorded \
                           producing episodes falls back to its legacy sources, capped at 20 \
                           with `episodes_truncated` set when sources were dropped — there it \
                           means the source list was cut, not that live producers exist). \
                           Without the flag the response is unchanged. Read-only.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "start": {"type": "string", "description": "Start entity: a name or an entity UUID (required). An ambiguous name walks from every match."},
                        "group_ids": {
                            "type": "array", "items": {"type": "string"},
                            "description": "Groups to walk in (names are resolved in each). Omitted: the default group `liminis`."
                        },
                        "hops": {
                            "type": "array",
                            "description": "The planned hops, in order. Required unless `answer_type` is an entity type and `query` is set, in which case it may be omitted or `[]` to run only the completion hop from the start entities.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "relation_types": {
                                        "type": "array", "items": {"type": "string"},
                                        "description": "Relation types to follow at this hop. Empty: no relation filter. If none match, all candidates are used (reported)."
                                    },
                                    "phrase": {"type": "string", "description": "Short description of the hop, used for ranking, e.g. \"supplier of the project\" (required)."},
                                    "direction": {
                                        "type": "string", "enum": ["out", "in", "any"], "default": "any",
                                        "description": "As the relation type reads: `out` = the frontier entity is the edge's source, `in` = its target."
                                    }
                                },
                                "required": ["phrase"]
                            }
                        },
                        "answer_type": {"type": "string", "description": "An entity type name, or \"value\". On the last hop (not in history mode) far ends of this type are preferred; if the hops end short of it a completion hop runs. With \"value\" (or when absent), the last hop also offers the frontier entities' own literal-valued facts (self-loops such as a budget or price) as candidates."},
                        "query": {"type": "string", "description": "The question text; ranks the completion hop (which does not run without it)."},
                        "mode": {"type": "string", "enum": ["single", "list", "history"], "default": "single", "description": "single: top 3 per hop; list: top 10; history: top 10 and includes invalidated edges."},
                        "direction_mode": {"type": "string", "enum": ["soft", "hard"], "default": "soft", "description": "soft: matching edges get a +0.05 ranking bonus; hard: only matching edges are kept (falls back to all if no moving edge matches). A frontier entity's own self-loop (a value fact, offered on the last hop of a value walk) matches either direction: it gets the soft bonus, is always kept in hard mode, and never decides the fallback."},
                        "include_provenance": {"type": "boolean", "default": false, "description": "When true, each returned edge also carries `episodes` (UUIDs of its LIVE producing episodes only, at most 20), `episodes_truncated` (true when more than 20 live producers exist) and `source_files` (distinct source files of all live producers; legacy episodes and edges fall back to their stored source description; for an edge with no recorded producers the fallback list is itself capped at 20 and `episodes_truncated` then means sources were dropped). Superseded or deleted episodes are never listed. Default false: the response is unchanged."}
                    },
                    "required": ["start"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_get_entities_by_source",
            description: "List entities that were extracted from a given source document. \
                           Matches the episode's stored source file exactly (byte for byte, \
                           no substring or prefix match, issue #853); an episode written \
                           before source identity was stored matches only when its composed \
                           `source_file:chunk_id` description provably names this source. \
                           Only live (not superseded, committed) episodes count. \
                           Does not include episode `attributes` (issue #528) — this path \
                           surfaces only entity-scoped provenance, never a full episode object.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "source": {"type": "string", "description": "Source identifier (required, non-empty)."},
                        "num_results": {
                            "type": "integer", "minimum": 1, "default": 100,
                            "description": "Maximum number of entities to return."
                        },
                        "group_ids": group_ids_prop()
                    },
                    "required": ["source"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_rebuild_status",
            description: "Poll the status of a background `knowledge_rebuild_from_wal` job \
                           by job ID.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "job_id": {"type": "string", "description": "Job ID returned by knowledge_rebuild_from_wal."}
                    },
                    "required": ["job_id"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_validate_corrections",
            description: "Validate the workspace's `knowledge-corrections.yaml` file against \
                           the current graph without applying anything.",
            scope: Scope::Read,
            input_schema: empty_schema,
        },
        ToolSpec {
            name: "knowledge_preview_chunks",
            description: "Read-only (issue #785): return exactly the chunks \
                           `knowledge_process_document` would produce for `content` — each with \
                           its `heading_path`, `content_hash`, `byte_start`/`byte_end` and the \
                           byte spans of its extraction units — plus the document map. Writes \
                           nothing, so a client never needs its own chunker.",
            scope: Scope::Read,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "The whole document (required)."}
                    },
                    "required": ["content"]
                })
            },
        },
        // ── write (18) ────────────────────────────────────────────────────────────
        ToolSpec {
            name: "knowledge_process_chunk",
            description: "Ingest a text chunk as an episode: extracts entities/relationships \
                           and adds them to the graph. The result reports \
                           `edges_dropped_unresolvable`: extracted edges discarded because an \
                           endpoint matched no entity in this chunk or in the graph. A nonzero \
                           count means facts stated in this chunk were not written. The result \
                           also carries `dropped_edges`, a list with one entry per edge counted \
                           in `edges_dropped_unresolvable`: each entry gives that edge's \
                           extracted `source_name`, `target_name`, `relation_type`, and `fact`, \
                           plus `unresolved_endpoint` (`source`, `target`, or `both`) naming \
                           which endpoint(s) failed to resolve — always present, empty when \
                           nothing was dropped. \
                           `edges_dropped_self_loop` counts edges dropped because both \
                           endpoints resolved to the same entity (they would have been \
                           self-loops); these are not in `dropped_edges`. \
                           Under an ontology, `edges_literal_object` counts literal-valued facts \
                           (a budget, date, price) stored as a self-loop on the one entity they \
                           describe; `edges_suspected_misattachment` counts edges marked \
                           `suspected_misattachment` because their fact states a value but does \
                           not name both endpoints (never dropped). \
                           Recommended maximum `chunk_text` size is 8,000 characters (default, \
                           overridable via `LCG_CHUNK_TEXT_ADVISORY_MAX_CHARS`) — extraction \
                           quality degrades well before any context-window limit is reached. \
                           Splitting oversized input into multiple `knowledge_process_chunk` \
                           calls is the caller's responsibility; this call still succeeds above \
                           the threshold, but the result gains a `warning` field naming the \
                           actual size and the recommended maximum. \
                           `attributes` (issue #528) is written directly to the resulting \
                           episode node — structured metadata co-located with, but independent \
                           of, the facts extracted from the chunk's prose (reachable from the \
                           episode via a MENTIONS hop). Returned by `knowledge_get_episodes` and \
                           `knowledge_search_passages`. \
                           Resubmitting a `chunk_id` is a correction (issue #705): once the new \
                           ingest has fully succeeded, the chunk's earlier live episodes in this \
                           group are superseded, so the graph stops believing the facts they \
                           asserted. Nothing is deleted and `valid_at`/`invalid_at` are never \
                           written — read the old facts with `as_of`, or hide them with \
                           `believed_only`, on the relationship reads. The result lists the \
                           `superseded_episode_uuids`. Only episodes this tool itself wrote (or \
                           that predate the `write_path` marker) are superseded (issue #292): a \
                           same-named episode written by `knowledge_add_episode` is left alone, \
                           the call still succeeds as a fresh ingest, and its UUIDs are listed in \
                           `left_alone_episode_uuids` (present only when non-empty). To submit \
                           one chunk as several calls, send \
                           `supersede: false` on every unit but the last, and pass the earlier \
                           units' `episode_uuid`s as `same_revision_episode_uuids` on the last. \
                           Commit your own record of a chunk as ingested only after this call \
                           succeeds. `origin` (issue #752) declares what kind of content the \
                           chunk is; a `derived` or `answer` chunk is cited for provenance but \
                           never counted as corroboration, may not rewrite or merge into an \
                           entity with primary/external support, and is refused (error message \
                           prefixed `origin_conflict:`, nothing written) when its `chunk_id` \
                           has a live primary/external episode or its `doc_id` already has a \
                           different origin.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "chunk_text": {"type": "string", "description": "Chunk text (required, non-empty)."},
                        "chunk_id": {"type": "string", "description": "Stable ID for this chunk (required, non-empty)."},
                        "source_file": {"type": "string", "description": "Source file path or identifier (required, non-empty)."},
                        "group_id": {"type": "string", "default": "liminis"},
                        "reference_time": {
                            "type": "string", "format": "date-time",
                            "description": "ISO 8601 timestamp; defaults to now."
                        },
                        "attributes": {"type": "object", "description": "Arbitrary JSON object, stored JSON-serialized on the resulting episode. Defaults to {} when omitted or non-object."},
                        "doc_id": {"type": "string", "description": "Stable identity of the logical document this episode came from, unique within the group (issue #707). Optional; an episode without one cannot take part in document supersession. Never defaulted from source_file."},
                        "origin": origin_prop(),
                        "supersede": {"type": "boolean", "default": true, "description": "Set false on every non-final unit of a chunk submitted as several calls under one chunk_id, so a unit does not supersede the previous one."},
                        "same_revision_episode_uuids": {"type": "array", "items": {"type": "string"}, "description": "On the final unit of a multi-unit revision: episode_uuids returned by the earlier units. They stay live; the chunk's older episodes are superseded."}
                    },
                    "required": ["chunk_text", "chunk_id", "source_file"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_process_document",
            description: "Document mode (issue #785): ingest one whole markdown document as a \
                           completeness claim — \"this is document `doc_id`, in full\". The \
                           server chunks it at structural boundaries (headings, paragraphs, \
                           lists, tables), diffs it against the document's previous committed \
                           revision, and extracts only the chunks whose text was added or \
                           changed; unchanged chunks are skipped by content hash, so \
                           re-submitting an identical document makes zero extraction calls. \
                           Once every chunk has succeeded it commits the revision in one step: \
                           changed chunks' earlier episodes are superseded (reason `revision`) \
                           and chunks no longer in the document are retired (reason \
                           `chunk_removed`) — you never send removals. A failed call retires \
                           nothing. Each extraction sees the title, heading path, origin and a \
                           document map as context, but facts are cited only to their own \
                           chunk. Every episode carries the server-minted `revision_id` and its \
                           `byte_start`/`byte_end` span in the submitted content. The result \
                           lists each chunk's `classification` (added / changed / unchanged), \
                           the `removed` chunks, `extraction_calls`, and the episodes created, \
                           superseded and retired. `origin` (issue #752) applies to every \
                           episode; a different origin for a known `doc_id` is refused with \
                           `origin_conflict:`. `attributes` (issue #528) are recorded on the \
                           revision and on every episode the call extracts. Identical content \
                           with the same `title`, `source_path`, `effective_at` and \
                           `attributes` returns `status: unchanged` and writes nothing; with \
                           different metadata it commits a revision with zero extraction \
                           calls. The first document-mode \
                           ingest of a `doc_id` supersedes its live chunk-mode episodes. \
                           Documents over `LCG_DOCUMENT_MAX_BYTES` (default 512 KiB) and empty \
                           or headings-only documents are rejected with nothing written. \
                           `knowledge_process_chunk` is unchanged.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "doc_id": {"type": "string", "description": "Stable identity of the document within the group (required, non-empty). The diff is always against this document's last committed revision."},
                        "content": {"type": "string", "description": "The whole document (markdown or plain text), required."},
                        "group_id": {"type": "string", "default": "liminis"},
                        "title": {"type": "string", "description": "Document title, given to the extractor as context and recorded on the revision."},
                        "source_path": {"type": "string", "description": "Source path or URL, recorded on the revision and used in each episode's source_description."},
                        "origin": origin_prop(),
                        "effective_at": {"type": "string", "format": "date-time", "description": "When this revision took effect; recorded on the revision only (never valid_at, never belief time)."},
                        "reference_time": {"type": "string", "format": "date-time", "description": "Extraction reference time, as for knowledge_process_chunk; defaults to now."},
                        "attributes": {"type": "object", "description": "Arbitrary JSON object, stored on every episode of this revision and on the revision record."}
                    },
                    "required": ["doc_id", "content"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_add_episode",
            description: "Add an episode (a piece of source content) to the graph, extracting \
                           entities and relationships from it. `attributes` (issue #528) is \
                           written directly to the resulting episode node — structured metadata \
                           co-located with, but independent of, the facts extracted from the \
                           episode's body (reachable from the episode via a MENTIONS hop). \
                           Returned by `knowledge_get_episodes` and `knowledge_search_passages`. \
                           `origin` (issue #752) declares what kind of content the episode is \
                           (default `primary`); `derived` and `answer` episodes never count as \
                           corroboration and may not rewrite entities with primary/external \
                           support. A `doc_id` whose origin differs from the one already \
                           recorded is refused with an `origin_conflict:` error.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Episode name."},
                        "origin": origin_prop(),
                        "episode_body": {"type": "string", "description": "Episode content."},
                        "source": {"type": "string", "default": "text", "description": "Source type (e.g. \"text\", \"json\")."},
                        "source_description": {"type": "string"},
                        "reference_time": {"type": "string", "format": "date-time"},
                        "group_id": {"type": "string", "default": "liminis"},
                        "attributes": {"type": "object", "description": "Arbitrary JSON object, stored JSON-serialized on the resulting episode. Defaults to {} when omitted or non-object."},
                        "doc_id": {"type": "string", "description": "Stable identity of the logical document this episode came from, unique within the group (issue #707). Optional; an episode without one cannot take part in document supersession. Never defaulted from source_file."}
                    },
                    "required": ["name", "episode_body"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_delete_episode",
            description: "Delete a single episode by UUID. Entities extracted solely from it \
                           become orphaned (not deleted).",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "episode_uuid": {"type": "string", "description": "Episode UUID (required)."}
                    },
                    "required": ["episode_uuid"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_delete_by_source",
            description: "Delete all episodes ingested from a given source file, superseded \
                           ones included. Matches the episode's stored source file exactly \
                           (byte for byte; deleting `https://a.com` never touches \
                           `https://a.com:8443/...`, issue #853); an episode written before \
                           source identity was stored is deleted only when its composed \
                           `source_file:chunk_id` description provably names this source — \
                           use knowledge_delete_chunk_episode for one it cannot attribute. \
                           Orphaned entities remain in the graph.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "source_file": {"type": "string", "description": "Source file to delete episodes for (required, non-empty)."},
                        "group_ids": {
                            "type": "array", "items": {"type": "string"}, "minItems": 1,
                            "description": "Non-empty array of groups to restrict deletion to (required). An unscoped, all-groups delete is not supported."
                        }
                    },
                    "required": ["source_file", "group_ids"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_delete_chunk_episode",
            description: "Delete all episodes for a given chunk ID. Orphaned entities remain \
                           in the graph.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "chunk_id": {"type": "string", "description": "Chunk ID to delete episodes for (required, non-empty)."},
                        "group_ids": {
                            "type": "array", "items": {"type": "string"}, "minItems": 1,
                            "description": "Non-empty array of groups to restrict deletion to (required). An unscoped, all-groups delete is not supported."
                        }
                    },
                    "required": ["chunk_id", "group_ids"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_mark_chunk_removed",
            description: "Record that a chunk no longer exists in its document (issue #705). \
                           Supersedes the chunk's live episodes with reason `chunk_removed`: the \
                           graph stops believing the facts they asserted, but nothing is deleted \
                           — the episodes and edges stay readable as history, and \
                           `valid_at`/`invalid_at` are never written. Prefer this to \
                           `knowledge_delete_chunk_episode` for a section that was removed from \
                           a document; that method keeps its explicit-deletion contract. \
                           Idempotent. Optional `origin` (issue #752) is the origin acting on \
                           the chunk: a `derived` or `answer` removal of a chunk with a live \
                           primary/external episode is refused (`origin_conflict:`, nothing \
                           written). Omitted, the call behaves exactly as before.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "chunk_id": {"type": "string", "description": "Chunk ID that no longer exists (required, non-empty)."},
                        "origin": origin_prop(),
                        "group_ids": {
                            "type": "array", "items": {"type": "string"}, "minItems": 1,
                            "description": "Non-empty array of groups to restrict the supersession to (required). An unscoped, all-groups call is not supported."
                        }
                    },
                    "required": ["chunk_id", "group_ids"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_supersede_document",
            description: "Record that one document replaces another (issue #707): document \
                           `superseding_doc_id` supersedes `superseded_doc_id` in `group_id`, \
                           effective at `effective_at`. Always explicit — supersession is never \
                           inferred. The old document's live episodes stop being believed at the \
                           recording instant (or at `effective_at` when that is later), but the \
                           facts are reported as in force until `effective_at`: unlike a \
                           correction, the old decision was true until it was replaced. \
                           `valid_at`/`invalid_at` are never written. Rejected, with no side \
                           effects, when the documents are the same, either is unknown in the \
                           group, the relation would form a cycle, or the old document is \
                           already superseded differently, or (issue #752) the superseding \
                           document is `derived`/`answer` and the superseded one is \
                           `primary`/`external` (`origin_conflict:`); recording the identical \
                           relation again is a no-op (`already_recorded`).",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {"type": "string", "description": "Group both documents belong to (required, non-empty)."},
                        "superseding_doc_id": {"type": "string", "description": "doc_id of the new document (required)."},
                        "superseded_doc_id": {"type": "string", "description": "doc_id of the document being replaced (required)."},
                        "effective_at": {"type": "string", "description": "When the replacement took effect: an RFC 3339 timestamp with a timezone offset (required). May be earlier than now (retroactive) or in the future."}
                    },
                    "required": ["group_id", "superseding_doc_id", "superseded_doc_id", "effective_at"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_clear_all",
            description: "Irreversibly delete the entire graph and reinitialize an empty \
                           schema. Requires explicit confirmation.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "confirm": {
                            "type": "boolean",
                            "description": "Must be true, or the call is rejected. Confirms the caller intends to destroy all graph data."
                        },
                        "preserve_wal": {
                            "type": "boolean", "default": false,
                            "description": "If true, keep the application WAL so knowledge_rebuild_from_wal can replay it afterward."
                        }
                    },
                    "required": ["confirm"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_apply_corrections",
            description: "Apply the workspace's `knowledge-corrections.yaml` file to the graph.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview without writing."}
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_merge_entities",
            description: "Merge one or more alias entities into a canonical entity, rewriting \
                           and deduplicating their edges. Merges never cross kinds (issue \
                           #615): if the canonical and any alias differ in `kind` the WHOLE call \
                           is refused with an error naming the kinds (dry_run included) and \
                           nothing is modified. `kind` optionally disambiguates \
                           `canonical_name`/`alias_names`; omitted, those names span all kinds \
                           and a `canonical_name` matching several kinds is an ambiguity error. \
                           `merge_all_by_name` is limited to the canonical's own kind.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "canonical_uuid": {"type": "string", "description": "UUID of the entity to merge into."},
                        "canonical_name": {"type": "string", "description": "Name of the entity to merge into (alternative to canonical_uuid)."},
                        "alias_uuids": {"type": "array", "items": {"type": "string"}, "description": "UUIDs of entities to merge away."},
                        "alias_names": {"type": "array", "items": {"type": "string"}, "description": "Names of entities to merge away."},
                        "merge_all_by_name": {
                            "type": "boolean", "default": false,
                            "description": "If true, merge all entities of the canonical's kind sharing its name as aliases."
                        },
                        "kind": {"type": "string", "description": "Optional kind restricting canonical_name / alias_names lookups. Omitted: those names span all kinds (a multi-kind canonical_name is an ambiguity error; an alias set spanning kinds is refused)."},
                        "group_id": {"type": "string", "default": "liminis"},
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview the merge plan without writing."}
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_reprocess_entity_types",
            description: "Reclassify entity types via the configured extraction LLM, e.g. \
                           after an ontology change.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {"type": "string", "default": "liminis"},
                        "scope": {
                            "type": "string", "enum": ["untyped", "off_ontology", "all"], "default": "untyped",
                            "description": "Which entities to reclassify: only untyped ones, only those outside the ontology, or all."
                        },
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview the reclassification plan without writing."}
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_canonicalize_relations",
            description: "Canonicalize relationship types against the workspace ontology's \
                           declared relation_types, restricted to group_id's own edges (#447) — \
                           never any other group's. group_id is required; omitted, null, or \
                           empty is rejected rather than falling back to a database-wide rewrite \
                           or the default group, since a hydrated, read-only group must never be \
                           mutated as a side effect of canonicalizing the group this node owns. \
                           Mutations are attributed to group_id's own WAL stream, never the \
                           default group's. When the ontology declares relation signatures \
                           (source_type/target_type) or inverse_aliases, also repairs edge \
                           direction (#816): edges that fit the signature only reversed are \
                           swapped, edges that fit neither way are marked signature_mismatch \
                           (never dropped); reports swapped_count, signature_mismatch_count and \
                           per-type by_type counts. Idempotent. Supports MCP progress \
                           notifications when called with a progress token.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {
                            "type": "string",
                            "description": "Restricts candidate selection and WAL attribution to this group only (required; no default — #447)."
                        },
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview without writing; the report includes the direction swap and signature-mismatch counts."},
                        "embedding_threshold": {
                            "type": "number",
                            "description": "Optional similarity threshold override for matching relation types."
                        }
                    },
                    "required": ["group_id"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_backfill_relation_types",
            description: "DEPRECATED — does not classify against the ontology. For each edge \
                           with a null/empty relation_type in group_id, this mints a pseudo-type \
                           by uppercasing and underscore-joining the first few words of the \
                           edge's fact sentence (e.g. THE_SPECIFICATION_DOCUMENT_DEFINES), \
                           producing near-unique labels rather than a real taxonomy. Running it \
                           pollutes the relation_type space and is only reversible by re-nulling \
                           the field. Use knowledge_reprocess_relation_types with scope: \
                           \"untyped\" instead — it performs genuine fact-based classification \
                           against the ontology's declared relation types, with honest \
                           UNCLASSIFIED abstention. group_id is required; omitted, null, or empty \
                           is rejected rather than falling back to a database-wide rewrite or the \
                           default group (#447), and mutations are attributed to group_id's own \
                           WAL stream. Retained for backward compatibility; supports MCP \
                           progress notifications when called with a progress token.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {
                            "type": "string",
                            "description": "Restricts candidate selection and WAL attribution to this group only (required; no default — #447)."
                        },
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview without writing."}
                    },
                    "required": ["group_id"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_backfill_summary_embeddings",
            description: "Computes summary_embedding for every entity in group_id that has a \
                           non-empty summary, so entities created before this capability existed \
                           (issue #470) become semantically retrievable by summary paraphrase, \
                           not just by name-vector or full-text match. Every candidate is \
                           unconditionally re-embedded on each call — there is no cheap way to \
                           detect which rows already carry a real embedding versus the \
                           migration's zero-vector placeholder, so re-running this is safe but \
                           not free. This operation drops and rebuilds the summary-vector index \
                           around its write phase and holds an exclusive lock for the whole \
                           run, blocking all other reads and writes for its duration — prefer \
                           running it at a low-traffic time, especially for a large group. \
                           group_id is required; omitted, null, or empty is rejected rather than \
                           falling back to a database-wide rewrite or the default group. Supports \
                           MCP progress notifications when called with a progress token.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {
                            "type": "string",
                            "description": "Restricts candidate selection and WAL attribution to this group only (required; no default)."
                        },
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview the candidate count without embedding or writing."}
                    },
                    "required": ["group_id"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_reembed_missing",
            description: "Repairs rows that were stored with a zero-vector embedding because the \
                           embedder failed during WAL replay (issue #809), which leaves them \
                           invisible to vector search. Re-embeds exactly the rows recorded as \
                           zero-vector for group_id (the per-group record `knowledge_status` \
                           reports as `zero_vector_rows`), from their stored source text: entity \
                           name (`name`), entity summary (`summary`), relationship fact (`fact`), \
                           episode content (`content`). `kinds` restricts the repair to a subset \
                           (default all four). Returns per-kind candidates, reembedded, failed \
                           and skipped_no_source; a repaired row is removed from the record so \
                           the `knowledge_status` count drops, and a failed row stays recorded for \
                           a later retry. `dry_run` only counts: no embedder call, no write, no \
                           index change. All embedding finishes before any index is dropped; then \
                           only the vector indexes of kinds that have something to write are \
                           dropped and rebuilt. Those indexes span ALL groups, so while one is \
                           down every write is blocked and vector search for every group waits \
                           for the rebuild — run `dry_run` first and prefer a low-traffic time. \
                           If a rebuild fails the result carries `index_rebuild_errors` and the \
                           next search rebuilds the indexes automatically. group_id is required. \
                           Supports MCP progress notifications when called with a progress \
                           token.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {
                            "type": "string",
                            "description": "The group whose recorded zero-vector rows are repaired (required; no default)."
                        },
                        "kinds": {
                            "type": "array",
                            "items": {"type": "string", "enum": ["name", "fact", "summary", "content"]},
                            "minItems": 1,
                            "description": "Which embedding kinds to repair. Default: all four."
                        },
                        "dry_run": {"type": "boolean", "default": false, "description": "Report per-kind candidate counts without embedding, writing or touching an index."}
                    },
                    "required": ["group_id"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_reprocess_relation_types",
            description: "Reclassify relation types via the configured extraction LLM, using \
                           each edge's fact against the ontology's declared relation types. \
                           Honestly abstains to UNCLASSIFIED rather than force-assigning the \
                           nearest type. Successful dry-run (`would_reclassify_count`, `plan`) and \
                           apply (`reclassified_count`, `unchanged_count`) responses include a \
                           `breakdown` object mapping each relation type (including UNCLASSIFIED) \
                           to the count of relations that would be written in dry-run mode or \
                           were written in apply mode, so callers can see classification quality \
                           without a second dry-run pass. Supports MCP progress notifications \
                           when called with a progress token.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {"type": "string", "default": "liminis"},
                        "scope": {
                            "type": "string", "enum": ["untyped", "off_ontology", "all"], "default": "untyped",
                            "description": "Which edges to reclassify: only untyped ones, only those outside the ontology, or all."
                        },
                        "dry_run": {"type": "boolean", "default": false, "description": "Preview the reclassification plan without writing."}
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_add_cross_group_edge",
            description: "Create an edge whose endpoint(s) may live in a group other than the \
                           edge's own `group_id` (issue #369 hub/layer-graph topology). Each \
                           endpoint is either `{\"uuid\": \"...\"}` for one already known to \
                           live in the edge's own group, or `{\"source_group_id\": \"...\", \
                           \"endpoint_name\": \"...\"}` for a foreign endpoint, which is \
                           resolved by name against that group and given resolvable pointer \
                           fields (`source_group_id`, `endpoint_name`, `resolved_uuid`, \
                           `bound_at_seq`, `binding_state`) instead of a bare UUID FK. A foreign \
                           endpoint that doesn't currently resolve is retained as `unbound` \
                           rather than dropped — the edge is still created, just missing that \
                           hop until a rebuild of the target group or a \
                           `knowledge_rebind_pointers` pass resolves it. A bare \
                           `uuid` endpoint that turns out to belong to a different group than \
                           the edge is rejected before any write. `source_kind` / `target_kind` \
                           (issue #615, optional) pin a FOREIGN endpoint's pointer to an entity \
                           kind (stored as `endpoint_kind`): a kind-pinned pointer resolves \
                           within that kind only and stays `bound` when a same-named entity of \
                           another kind appears. Omitted, the pointer resolves across ALL kinds \
                           and becomes `ambiguous` — with no hop — once a second kind shares the \
                           name. Supplying a kind for a `{uuid}` endpoint is rejected.",
            scope: Scope::Write,
            input_schema: || {
                let endpoint_schema = || {
                    json!({
                        "oneOf": [
                            {
                                "type": "object",
                                "description": "An endpoint already known to live in the edge's own group_id.",
                                "properties": {"uuid": {"type": "string"}},
                                "required": ["uuid"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "description": "A foreign endpoint, resolved by name against source_group_id.",
                                "properties": {
                                    "source_group_id": {"type": "string"},
                                    "endpoint_name": {"type": "string"}
                                },
                                "required": ["source_group_id", "endpoint_name"],
                                "additionalProperties": false
                            }
                        ]
                    })
                };
                json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Relation name, e.g. RELATES_TO subtype label."},
                        "source": endpoint_schema(),
                        "target": endpoint_schema(),
                        "source_kind": {"type": "string", "description": "Optional kind for a foreign `source` endpoint (pins the pointer to that kind). Rejected for a {uuid} endpoint."},
                        "target_kind": {"type": "string", "description": "Optional kind for a foreign `target` endpoint (pins the pointer to that kind). Rejected for a {uuid} endpoint."},
                        "group_id": {"type": "string", "default": "liminis", "description": "The edge's own (layer) group_id."},
                        "fact": {"type": "string", "description": "Natural-language fact text for the edge."},
                        "valid_at": {"type": "string", "description": "Optional ISO-8601 timestamp this fact became true."},
                        "relation_type": {"type": "string", "description": "Optional ontology relation type."}
                    },
                    "required": ["name", "source", "target", "fact"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_assert_entity",
            description: "Directly create or update a single entity by name (or by explicit \
                           entity_uuid) within group_id (issue #379) — for an agent that already \
                           knows the fact and wants to record it without a prose round-trip \
                           through episode extraction. Without entity_uuid, resolves by exact \
                           (case-insensitive, whitespace-normalized) name match within group_id \
                           — no embedding-similarity fuzzy matching — and updates that entity in \
                           place if found, or creates a new one if not; repeated calls with the \
                           same name/group_id are idempotent and always return the same \
                           entity_uuid. With entity_uuid, performs a strict group-scoped lookup \
                           only — never a fallback that mints a new entity under a caller-chosen \
                           UUID, and the call fails if that UUID doesn't exist in group_id. If \
                           resolution (by either path) lands on a Merged tombstone, the \
                           assertion forwards through merged_into to the canonical entity and \
                           updates the canonical instead. `summary` and `attributes` are always \
                           fully replaced on update, like `labels` — a re-assert that omits \
                           `summary` clears any previously-set value, not a partial merge. Its \
                           semantic-search vector is write-once: an update that changes `summary` \
                           does not recompute the entity's summary embedding, so meaning-based \
                           retrieval keeps matching the entity's original summary text until \
                           `knowledge_backfill_summary_embeddings` is run for its group_id. If the \
                           configured embedder is unavailable, the call still succeeds with a \
                           zero-vector name_embedding and a non-null `embedding_warning` in the \
                           response. Entity identity is `(group_id, kind, name)` (issue #615): \
                           `kind` is optional and WRITES ARE KIND-SCOPED — omitted, the call \
                           acts on the default kind `Entity` ONLY, so a name-only write can \
                           never reach into (or overwrite) another kind's same-named node; \
                           given, it resolves/creates within exactly that kind. `kind` is always \
                           one of the entity's `labels` (appended after `Entity` if you omit it \
                           there) and is immutable: an `entity_uuid` whose entity has a \
                           different kind than a supplied `kind` is rejected. The rename guard \
                           is per-kind.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "The entity's name (required)."},
                        "kind": {"type": "string", "description": "Optional entity kind (identity-bearing, immutable; non-empty, no U+001F, not the reserved label \"Merged\"). Omitted: acts on the default kind \"Entity\" only."},
                        "entity_uuid": {"type": "string", "description": "Optional: update this exact entity by UUID (strict group-scoped lookup, no create fallback) instead of resolving by name."},
                        "group_id": {"type": "string", "default": "liminis"},
                        "labels": {"type": "array", "items": {"type": "string"}, "default": ["Entity"], "description": "Defaults to [\"Entity\"] when omitted or empty."},
                        "summary": {"type": "string", "default": "", "description": "Fully replaces any prior summary on update; defaults to \"\" when omitted."},
                        "attributes": {"type": "object", "description": "Arbitrary JSON object, stored JSON-serialized. Fully replaces the prior value on update."}
                    },
                    "required": ["name"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_assert_relationship",
            description: "Directly create or update a single directed edge between two entities \
                           resolved by name, both strictly within this call's own group_id \
                           (issue #379) — for an agent that already knows the relationship and \
                           wants to record it without a prose round-trip through episode \
                           extraction. source_name/target_name are resolved by exact \
                           (case-insensitive, whitespace-normalized) match within group_id only \
                           — including forwarding through a Merged tombstone to its canonical, \
                           same as knowledge_assert_entity — and NEVER fall back to searching \
                           another group; if either name doesn't resolve in group_id, the call \
                           fails naming knowledge_add_cross_group_edge as the tool to use for \
                           cross-group connections instead. The upsert match is \
                           (source, predicate, target, group_id) — all four components, so \
                           asserting an edge in one group_id can never match, update, or \
                           overwrite a same-named edge between the same endpoints in a different \
                           group_id. Repeated calls with the same source/predicate/target/ \
                           group_id are idempotent: the same edge is updated in place, never \
                           duplicated. `fact` defaults to \"<source_name> <predicate> \
                           <target_name>\" when omitted. `attributes`, `valid_at`, and \
                           `relation_type` are always fully replaced on update — a re-assert \
                           that omits one of them clears any previously-set value, not a \
                           partial merge. `valid_at` accepts RFC-3339 or lbug's \
                           space-delimited read-back format and is rejected cleanly (not passed \
                           through to a Binder exception) if it matches neither. If the \
                           configured embedder is unavailable, the call still succeeds with a \
                           zero-vector fact_embedding and a non-null `embedding_warning` in the \
                           response. Endpoints are always EXISTING entities (this tool never \
                           creates one), so `source_kind` / `target_kind` (issue #615, \
                           optional) follow the READ rule: given, that endpoint resolves \
                           exactly within that kind; omitted, it spans ALL kinds and, if more \
                           than one kind has an entity of that name, the call fails with \
                           JSON-RPC error -32002 listing the candidates' kinds and uuids — it \
                           never silently picks one. `adr` (KnowledgeChannel) -> `adr` (Topic) \
                           links two distinct nodes and is not a self-loop.",
            scope: Scope::Write,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "source_name": {"type": "string", "description": "Source entity name, resolved within group_id (required)."},
                        "target_name": {"type": "string", "description": "Target entity name, resolved within group_id (required)."},
                        "source_kind": {"type": "string", "description": "Optional kind for the source endpoint. Omitted: spans all kinds; a multi-kind match is an ambiguity error (-32002)."},
                        "target_kind": {"type": "string", "description": "Optional kind for the target endpoint. Omitted: spans all kinds; a multi-kind match is an ambiguity error (-32002)."},
                        "predicate": {"type": "string", "description": "The edge's identity label — participates in the (source, predicate, target, group_id) upsert match (required)."},
                        "group_id": {"type": "string", "default": "liminis"},
                        "fact": {"type": "string", "description": "Natural-language fact text. Defaults to \"<source_name> <predicate> <target_name>\" when omitted."},
                        "attributes": {"type": "object", "description": "Arbitrary JSON object, stored JSON-serialized. Fully replaces the prior value on update."},
                        "valid_at": {"type": "string", "description": "Optional timestamp this fact became true — RFC-3339 or lbug's space-delimited read-back format. Fully replaces the prior value on update; omitting it on a re-assert clears any previously-set value, like attributes."},
                        "relation_type": {"type": "string", "description": "Optional ontology relation type. Fully replaces the prior value on update; omitting it on a re-assert clears any previously-set value, like attributes."}
                    },
                    "required": ["source_name", "target_name", "predicate"]
                })
            },
        },
        // ── cypher (1) — arbitrary query/mutation power scope ────────────────────────
        ToolSpec {
            name: "knowledge_query_cypher",
            description: "Execute raw Cypher against the graph. Can perform arbitrary reads or \
                           mutations and bypasses the WAL/embedding invariants that structured \
                           write tools maintain — use with caution. Refused entirely (JSON-RPC \
                           error -32004, reason raw_cypher_disabled) when the daemon runs with \
                           --writable-groups, because raw Cypher cannot be restricted by group.",
            scope: Scope::Cypher,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Raw Cypher query text (required)."}
                    },
                    "required": ["query"]
                })
            },
        },
        // ── admin (17) — WAL/lifecycle/recovery/index maintenance ────────────────────
        ToolSpec {
            name: "knowledge_stamp_origin",
            description: "Stamp `origin` (issue #752) onto EXISTING episodes of one group, without \
                           re-extraction — the migration path for a graph that predates origin, \
                           where every episode reads as `primary`. `rules` is an ordered list of \
                           `{origin, source_path_prefix?, attributes_kind?}`; each rule needs at \
                           least one selector, selectors within a rule are ANDed, and the first \
                           matching rule wins. `source_path_prefix` matches the start of the \
                           episode's `source_description` (`<source_file>:<chunk_id>`); \
                           `attributes_kind` matches `attributes.kind` exactly (e.g. \
                           `knowledge` → `derived`). An episode with a `doc_id` pulls in its \
                           whole document, and rules that would give one document two origins \
                           are refused. `dry_run: true` reports `{matched, changed, unchanged, \
                           changes[{uuid, doc_id, from, to}], truncated}` and writes nothing. \
                           Idempotent, recorded in the WAL (survives rebuild), makes no LLM or \
                           embedding call, and never revisits supersessions or invalidations \
                           already made.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {"type": "string", "description": "Group whose episodes to stamp (required, non-empty)."},
                        "rules": {
                            "type": "array", "minItems": 1,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "origin": origin_prop(),
                                    "source_path_prefix": {"type": "string", "description": "Match episodes whose source_description starts with this."},
                                    "attributes_kind": {"type": "string", "description": "Match episodes whose attributes.kind equals this."}
                                },
                                "required": ["origin"]
                            },
                            "description": "Ordered stamp rules; the first matching rule wins."
                        },
                        "dry_run": {"type": "boolean", "default": false, "description": "Report what would change and write nothing."}
                    },
                    "required": ["group_id", "rules"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_dump_wal",
            description: "Snapshot the current graph contents into a fresh compacted WAL \
                           directory. The output directory starts with no checkpoints — any \
                           `knowledge_wal_mark_*` checkpoints recorded against the source WAL \
                           directory are NOT carried forward, since dump_wal renumbers \
                           sequence numbers and a copied checkpoint's seq would be meaningless \
                           against the new numbering. For the same reason, the output always \
                           gets a freshly minted stream generation (issue #387) — never the \
                           source's: it is a new stream, not a copy of the source's identity.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {"type": "string", "description": "Restrict the dump to a single group. Omit to dump all groups."},
                        "target_dir": {
                            "type": "string",
                            "description": "Output directory. Must not exist or must be empty. Defaults to <workspace>/.lcg/wal-compacted/."
                        }
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_strip_wal_embeddings",
            description: "Rewrite every qualifying on-disk WAL file in place, removing \
                           embedding-vector params fields (name_embedding, fact_embedding, \
                           content_embedding, summary_embedding) while leaving every other \
                           field, record ordering, and sequence number untouched. 0.14.0 \
                           stopped writing these vectors to the WAL and made replay ignore any \
                           vector found in an older WAL — so a pre-0.14 WAL carries ~90% inert \
                           bytes (measured 89.9% on the reference corpus) this operation \
                           reclaims without changing what replay does. Idempotent: re-running \
                           on an already-stripped WAL (or one that never had vectors) is a \
                           zero-I/O no-op — zero files modified, byte-identical, mtime \
                           unchanged. Crash-safe per file: each file needing a rewrite is \
                           written to a temporary file in the same directory and atomically \
                           renamed over the original only once fully flushed; an interrupted run \
                           leaves the original file intact and a subsequent run resumes cleanly. \
                           A record whose embedding-vector value is malformed (not a well-formed \
                           JSON number array) is a per-file error: that file is left completely \
                           untouched and reported in `errors`, while every other file is still \
                           processed. Deliberately out of scope: a vector inlined as a raw \
                           Cypher literal (no params key to remove) is not reachable by this \
                           key-removal strategy and is left as-is, matching ADR-0526's existing \
                           precedent for that shape.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {
                            "type": "string",
                            "description": "Restrict stripping to a single group's own WAL \
                                             directory (issue #378). Omit to process every \
                                             group directory under the WAL root, plus any \
                                             legacy flat-layout files at the root itself."
                        },
                        "dry_run": {
                            "type": "boolean", "default": false,
                            "description": "Compute the same statistics (files that would be \
                                             rewritten, bytes that would be reclaimed, records \
                                             that would be touched) without modifying any file."
                        }
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_prepare_checkpoint",
            description: "Rotate/flush the live WAL writer so all pending mutations are on \
                           disk before an external filesystem checkpoint or backup. Unrelated \
                           to knowledge_wal_mark_create/_list/_delete: this is a disk-flush \
                           operation, not a named WAL position — the two features share the \
                           word \"checkpoint\" by coincidence, not by relation.",
            scope: Scope::Admin,
            input_schema: empty_schema,
        },
        ToolSpec {
            name: "knowledge_wal_mark_create",
            description: "Record a new named, retained WAL position — 'this graph was \
                           known-good here' — at the database's current applied_seq and \
                           current generation (issue #387) for `group_id`'s own WAL directory \
                           (default \"liminis\", issue #378). \
                           Distinct from knowledge_prepare_checkpoint (a WAL flush/rotate \
                           operation, not a named position). O(1): does not scan or replay the \
                           WAL. Fails if the current position is unknown (applied_seq is null — \
                           resolve with a full knowledge_rebuild_from_wal first) or if `name` \
                           already identifies an active checkpoint in that group. `name` must be \
                           1-200 characters of [A-Za-z0-9_-], since it becomes a single \
                           directory name under that group's .checkpoints/. The recorded `seq` \
                           is `null` for a genuinely fresh/empty group, or an integer (WAL line, \
                           inclusive) otherwise — restore a `null` checkpoint with \
                           knowledge_clear_all, or an integer checkpoint with \
                           knowledge_rebuild_from_wal \
                           {group_id, from_seq: 0, to_seq: <seq>, force_clear: true}. \
                           Exactly-one-wins under concurrent create for the same name relies on \
                           exclusive file creation, which is a local-filesystem guarantee — not \
                           reliable on an NFS-mounted WAL directory.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Checkpoint name (1-200 chars of [A-Za-z0-9_-]). \
                                             Must not already identify an active checkpoint in \
                                             this group_id's stream."
                        },
                        "group_id": {
                            "type": "string", "default": "liminis",
                            "description": "Resolves and operates against this group's own WAL \
                                             directory (issue #378). Defaults to \"liminis\", \
                                             matching a pre-378 single-stream deployment."
                        }
                    },
                    "required": ["name"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_wal_mark_list",
            description: "List every active (non-deleted) named WAL checkpoint in `group_id`'s \
                           own WAL directory (default \"liminis\" when omitted — this does NOT \
                           aggregate checkpoints across every active group, issue #378 FR-012), \
                           each with its `seq`, its recorded `generation` (issue #387), whether \
                           it is currently `reachable`, and the \
                           `wal_min_seq`/`wal_max_seq` bounds of WAL content presently on disk \
                           for that group. `reachable` requires the existing bounds check — BOTH \
                           `wal_min_seq == 0` (the WAL's own prefix has not been externally \
                           truncated, e.g. by routine retention deleting old WAL files) AND \
                           `seq <= wal_max_seq`, where a checkpoint whose seq merely falls \
                           inside `[wal_min_seq, wal_max_seq]` is still reported unreachable if \
                           `wal_min_seq > 0` since restoring it would silently omit everything \
                           before `wal_min_seq` — AND, independently, that the checkpoint's \
                           recorded `generation` matches the group's current on-disk generation \
                           whenever both are known: a checkpoint taken against a generation that \
                           has since been reset is never reachable, even when its seq still \
                           falls comfortably inside the bounds. Neither check detects a gap in \
                           the middle of that range, so `reachable: true` is a \
                           necessary, not sufficient, signal that a restore will succeed; use \
                           `wal_min_seq`/`wal_max_seq`/`generation` to diagnose an unreachable \
                           checkpoint. \
                           Works even when the database is degraded or unavailable, since it \
                           only reads the WAL-directory checkpoint store.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {
                            "type": "string", "default": "liminis",
                            "description": "Lists this group's own checkpoints only (issue \
                                             #378). Defaults to \"liminis\", matching a pre-378 \
                                             single-stream deployment; never aggregates across \
                                             every active group."
                        }
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_wal_mark_delete",
            description: "Delete an active named WAL checkpoint in `group_id`'s own WAL \
                           directory (default \"liminis\"), freeing its name for reuse by a \
                           later knowledge_wal_mark_create against that same group. Records a \
                           tombstone rather than rewriting history. Fails if `name` does not \
                           currently identify an active checkpoint in that group (never created, \
                           or already deleted). Works even when the database is degraded or \
                           unavailable, since it only touches the WAL-directory checkpoint \
                           store.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Name of the active checkpoint to delete."
                        },
                        "group_id": {
                            "type": "string", "default": "liminis",
                            "description": "Resolves and operates against this group's own WAL \
                                             directory (issue #378). Defaults to \"liminis\", \
                                             matching a pre-378 single-stream deployment."
                        }
                    },
                    "required": ["name"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_rebuild_from_wal",
            description: "Rebuild `group_id`'s own subgraph (default \"liminis\") by replaying \
                           that group's WAL files, optionally from a given sequence number \
                           and/or up to a given sequence number — never disturbing any other \
                           group's WalPosition row (issue #378 FR-006). Supports MCP progress \
                           notifications when called with a progress token. If the group already \
                           has a recorded position and its current on-disk generation is unknown \
                           (missing or corrupt .wal-generation.json), the call fails outright \
                           with an explicit error naming the group — no replay, no exceptions for \
                           dry_run, no bypass flag (issue #414); see knowledge_status's \
                           generation_status to see this coming. Otherwise, before anything \
                           else, compares the group's recorded generation against what's \
                           currently on disk (issue #387): if they differ, the caller's \
                           from_seq/to_seq/force_clear are overridden entirely and this becomes \
                           an automatic full self-heal — purge the group, replay it from scratch \
                           against the new generation, then re-bind any cross-group pointers \
                           into it — rather than silently layering new-generation mutations onto \
                           old-generation data. The result reports `reset_detected`, \
                           `previous_generation`, and `generation` so a caller can tell this \
                           apart from an ordinary incremental replay (every successful rebuild \
                           also reports `cross_group_rebind`, and rebuild order across groups \
                           does not matter: cross-group pointers are repaired after each); a \
                           `dry_run: true` call against a mismatched group reports the same \
                           fields but mutates nothing. Absent a detected reset, a `from_seq: 0` \
                           (default) full rebuild fails fast with an explicit error if that \
                           group already contains data, rather than silently producing a \
                           duplicate-primary-key failure per node — pass `force_clear: true` to \
                           clear that group's data automatically before replaying (via a \
                           group-scoped purge, not a whole-database wipe), or clear it first \
                           with `knowledge_delete_by_group`. This check does not apply to \
                           `from_seq > 0` (incremental resume against an intentionally \
                           non-empty group). A bounded rebuild (`to_seq` set) is NOT durable: \
                           WAL entries beyond `to_seq` remain on disk, unapplied — a later \
                           unbounded rebuild, or a `from_seq` resume that covers the excluded \
                           range, will reapply everything that was excluded.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {
                            "type": "string", "default": "liminis",
                            "description": "Targets this group's own WAL directory and \
                                             WalPosition row (issue #378) — no other group's \
                                             position is read or written. Defaults to \
                                             \"liminis\", matching a pre-378 single-stream \
                                             deployment."
                        },
                        "from_seq": {
                            "type": "integer", "minimum": 0, "default": 0,
                            "description": "Replay starting from this WAL sequence number."
                        },
                        "to_seq": {
                            "type": "integer", "minimum": 0,
                            "description": "Inclusive upper bound: replay only lines with \
                                             seq <= to_seq. Omit for unbounded replay to the end \
                                             of the WAL (today's default behavior). Must not be \
                                             less than from_seq. Not durable: WAL entries beyond \
                                             to_seq remain on disk and will be reapplied by a \
                                             later unbounded replay or an overlapping from_seq \
                                             resume."
                        },
                        "dry_run": {
                            "type": "boolean", "default": false,
                            "description": "Compute replay statistics without writing or touching indices. \
                                             Still fails fast against a non-empty group on a from_seq: 0 \
                                             request, regardless of force_clear, since a dry run must never \
                                             mutate the database."
                        },
                        "force_clear": {
                            "type": "boolean", "default": false,
                            "description": "When true and from_seq is 0, clear group_id's own data before \
                                             replaying if it already contains data, instead of failing fast \
                                             — never touches any other group's data. Ignored for from_seq > 0 \
                                             and for dry_run (which always fails fast on a non-empty group)."
                        }
                    }
                })
            },
        },
        ToolSpec {
            name: "knowledge_recover",
            description: "Run a single degraded-mode recovery strategy against a corrupt or \
                           unavailable database.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "strategy": {
                            "type": "string",
                            "enum": ["drop_lbug_wal", "rebuild_from_workspace_wal", "restore_from_backup"],
                            "description": "Recovery strategy to run (required)."
                        }
                    },
                    "required": ["strategy"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_recover_full",
            description: "Run the full autonomous recovery sequence (checkpoint-drop → \
                           episode-cursor resume-replay → reindex). Idempotent: a no-op if the \
                           DB is already healthy.",
            scope: Scope::Admin,
            input_schema: empty_schema,
        },
        ToolSpec {
            name: "knowledge_close",
            description: "Gracefully shut down the knowledge graph service. In standalone MCP \
                           mode, closes only this MCP process's own DB connection. In attached \
                           mode (only advertised with --allow-remote-close), forwards the \
                           shutdown to the remote service.",
            scope: Scope::Admin,
            input_schema: empty_schema,
        },
        ToolSpec {
            name: "knowledge_build_indices",
            description: "Build the full-text and vector search indices over the current \
                           graph contents.",
            scope: Scope::Admin,
            input_schema: empty_schema,
        },
        ToolSpec {
            name: "knowledge_rebind_pointers",
            description: "Re-resolve every cross-group pointer whose `source_group_id` matches \
                           the given group (issue #369) — run after that source group's \
                           purge/rehydrate refresh cycle so layer edges follow re-extracted \
                           entities, merges, and renames. Uses the same name-index resolution \
                           authority as pointer creation (ADR-0283), so results always agree \
                           with what a fresh lookup would find. A pointer currently `bound` is \
                           skipped (and counted in `staleness_skipped`) once its `bound_at_seq` \
                           is already at or past the current applied WAL position, which is what \
                           makes a second call with no intervening source change a no-op for \
                           pointers that are already correct. A pointer currently `unbound` or \
                           `ambiguous` is always re-resolved regardless of `bound_at_seq` (issue \
                           #392), since a known-broken pointer must be repairable even when its \
                           source group's position happens to look unchanged — e.g. after a \
                           purge followed by a checkpoint-bounded restore back to the position \
                           the pointer was originally bound at. A resolution that would create a \
                           self-loop or duplicate an existing directed edge invalidates the edge \
                           instead of writing a broken/redundant one, reusing the same handling \
                           `knowledge_merge_entities` uses.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "source_group_id": {"type": "string", "description": "The source group whose pointers should be re-resolved (required)."}
                    },
                    "required": ["source_group_id"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_delete_by_group",
            description: "Purge ALL data for one or more group_ids: Entity nodes, Episodic \
                           nodes, and the RELATES_TO edges among them, leaving no orphans \
                           (issue #361). Supersedes the ad hoc `knowledge_query_cypher` \
                           `DELETE ... WHERE group_id = $g` workaround, which bypasses the WAL \
                           and embedding invariants the structured write tools maintain. Never \
                           deletes a RelatesToNode_ owned by a group outside the call, even when \
                           it deletes an Entity that node is attached to via a hop relationship \
                           — the foreign edge node survives, left `unbound` (issue #369's \
                           binding_state), so a later rehydration can re-bind it with \
                           knowledge_rebind_pointers. Purging a group_id with no data is a \
                           no-op success, not an error. Does not reset the purged group's own \
                           applied WAL position (`applied_seq`, per-group since issue #378) — \
                           it is left as-is (see ADR-0361); resolve it afterward with \
                           knowledge_rebuild_from_wal targeting that group, with force_clear as \
                           needed. Pass `dry_run: true` to preview per-group entity/edge/ \
                           episode counts and the cross-group pointers that would become \
                           `unbound` (broken out by owning group_id) without mutating anything \
                           — `dry_run` takes precedence over `confirm` when both are set.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_ids": {
                            "type": "array",
                            "items": {"type": "string"},
                            "minItems": 1,
                            "description": "Required, non-empty list of group_ids to purge \
                                             completely. Unlike other tools' optional group_ids \
                                             filter, this one is never defaulted — a destructive \
                                             admin op must never silently guess which group to \
                                             purge."
                        },
                        "confirm": {
                            "type": "boolean", "default": false,
                            "description": "Must be true to perform the real (mutating) purge. \
                                             Ignored when dry_run is true."
                        },
                        "dry_run": {
                            "type": "boolean", "default": false,
                            "description": "Preview the purge's counts without mutating \
                                             anything. Takes precedence over confirm."
                        }
                    },
                    "required": ["group_ids"]
                })
            },
        },
        ToolSpec {
            name: "knowledge_reload_ontology",
            description: "Reload ONE group's ontology without a service restart (issue #627): \
                           discards the group's cached resolution and re-resolves it through the \
                           normal precedence (per-group `.lcg/ontology/<group>.yaml`, then the \
                           workspace ontology loaded at startup, then free-form), so an edited \
                           per-group file takes effect on the live service and other groups are \
                           untouched. Returns { group_id, previous_hash (null if the group was \
                           not yet resolved), new_hash (\"none\" for no ontology), changed, drift \
                           ({group_id, drifted, drift_summary}), identity_refusal }. An unchanged \
                           file returns changed: false and has no side effects. If the new \
                           ontology adds or removes `identity: true` on a type the group already \
                           holds, the group is refused exactly as a restart would refuse it: \
                           `identity_refusal` is a structured object ({message, label, count, \
                           adding}), knowledge_add_episode for the group fails with -32003, and \
                           the group is listed in knowledge_status.group_identity_refusals; \
                           restore the previous identity-bearing set in the file and reload again \
                           to clear it. Stored data is never modified. Waits for in-flight \
                           read-lock passes (reprocess/canonicalize/backfill) to finish. `drift` \
                           compares against the last-ingested ontology, not the previous cache, \
                           so it is independent of `changed`.",
            scope: Scope::Admin,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "group_id": {"type": "string", "description": "The group whose ontology should be reloaded (required, non-empty)."}
                    },
                    "required": ["group_id"]
                })
            },
        },
    ]
}

/// Returns the names from `schema`'s `"required"` array that are absent (or explicitly `null`)
/// in `params`. `handlers.rs`'s own param extraction is untouched by this issue and some
/// handlers silently fall back to an empty/default value instead of erroring when a field the
/// schema advertises as required is missing (e.g. `knowledge_delete_episode` would report
/// `"deleted"` without deleting anything) — this check makes the MCP layer honor its own
/// advertised contract (FR-008's "missing required argument" edge case) without touching the
/// out-of-scope core dispatch.
pub fn missing_required(schema: &Value, params: &Value) -> Vec<String> {
    let Some(required) = schema.get("required").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    required
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|name| matches!(params.get(*name), None | Some(Value::Null)))
        .map(|s| s.to_string())
        .collect()
}

/// Names of the seven streaming methods that emit MCP progress notifications (FR-007).
pub fn is_streaming_method(name: &str) -> bool {
    matches!(
        name,
        "knowledge_rebuild_from_wal"
            | "knowledge_canonicalize_relations"
            | "knowledge_backfill_relation_types"
            | "knowledge_backfill_summary_embeddings"
            | "knowledge_reembed_missing"
            | "knowledge_reprocess_relation_types"
            | "knowledge_reprocess_entity_types"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_has_58_unique_tools() {
        let r = registry();
        assert_eq!(r.len(), 58);
        let names: HashSet<&str> = r.iter().map(|t| t.name).collect();
        assert_eq!(names.len(), 58, "tool names must be unique");
    }

    #[test]
    fn scope_bucket_sizes_match_fr_004_table() {
        let r = registry();
        let count = |s: Scope| r.iter().filter(|t| t.scope == s).count();
        assert_eq!(count(Scope::Read), 22);
        assert_eq!(count(Scope::Write), 18);
        assert_eq!(count(Scope::Cypher), 1);
        assert_eq!(count(Scope::Admin), 17);
    }

    #[test]
    fn reload_ontology_is_admin_only_and_requires_group_id() {
        let r = registry();
        let t = r
            .iter()
            .find(|t| t.name == "knowledge_reload_ontology")
            .expect("knowledge_reload_ontology must be registered");
        assert_eq!(t.scope, Scope::Admin);
        assert_eq!((t.input_schema)()["required"], json!(["group_id"]));
    }

    #[test]
    fn cypher_scope_is_exactly_query_cypher() {
        let r = registry();
        let cypher_tools: Vec<&str> = r
            .iter()
            .filter(|t| t.scope == Scope::Cypher)
            .map(|t| t.name)
            .collect();
        assert_eq!(cypher_tools, vec!["knowledge_query_cypher"]);
    }

    /// MCP-less methods the dispatcher handles that are therefore absent from the registry.
    const NON_MCP_CLASSIFIED: &[&str] = &["health_check"];

    /// FR-013 (issue #742): every registered tool has a group-write classification, and the
    /// classification agrees with the tool's scope, so a newly added write tool that nobody
    /// classified fails here instead of silently bypassing `--writable-groups`.
    #[test]
    fn every_registered_tool_has_a_consistent_write_classification() {
        use lcg_core::write_policy::{classify, WriteClass, CLASSIFICATION};
        let reg = registry();
        for tool in &reg {
            let class = classify(tool.name).unwrap_or_else(|| {
                panic!(
                    "tool {} has no entry in lcg_core::write_policy::CLASSIFICATION — classify \
                     it (group-write check) before adding it",
                    tool.name
                )
            });
            match tool.scope {
                Scope::Read => assert_eq!(
                    class,
                    WriteClass::ReadOnly,
                    "read-scope tool {} must be ReadOnly",
                    tool.name
                ),
                _ => assert_ne!(
                    class,
                    WriteClass::ReadOnly,
                    "{:?}-scope tool {} must not be classified ReadOnly",
                    tool.scope,
                    tool.name
                ),
            }
            if tool.scope == Scope::Cypher {
                assert_eq!(class, WriteClass::RawCypher, "{}", tool.name);
            }
        }
        // No stale keys: every classified method is a registered tool or a known non-MCP one.
        let names: std::collections::HashSet<&str> = reg.iter().map(|t| t.name).collect();
        for (m, _) in CLASSIFICATION {
            assert!(
                names.contains(m) || NON_MCP_CLASSIFIED.contains(m),
                "CLASSIFICATION names {m}, which is neither a registered tool nor in \
                 NON_MCP_CLASSIFIED"
            );
        }
    }

    #[test]
    fn every_schema_is_a_valid_object_schema() {
        for tool in registry() {
            let schema = (tool.input_schema)();
            assert_eq!(
                schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "tool {} schema must have type object",
                tool.name
            );
            assert!(
                schema.get("properties").is_some(),
                "tool {} schema must have properties",
                tool.name
            );
        }
    }

    #[test]
    fn missing_required_reports_absent_and_null_fields_only() {
        let schema = json!({
            "type": "object",
            "properties": {"a": {}, "b": {}, "c": {}},
            "required": ["a", "b"]
        });
        assert_eq!(
            missing_required(&schema, &json!({"a": "x"})),
            vec!["b".to_string()]
        );
        assert_eq!(
            missing_required(&schema, &json!({"a": "x", "b": null})),
            vec!["b".to_string()]
        );
        assert!(missing_required(&schema, &json!({"a": "x", "b": "y"})).is_empty());
        // "c" isn't required, so its absence doesn't matter.
        assert!(missing_required(&schema, &json!({"a": "x", "b": "y"})).is_empty());
    }

    #[test]
    fn missing_required_is_empty_when_schema_has_no_required_array() {
        assert!(missing_required(&empty_schema(), &json!({})).is_empty());
    }

    #[test]
    fn streaming_methods_match_admin_and_write_tools() {
        assert!(is_streaming_method("knowledge_rebuild_from_wal"));
        assert!(is_streaming_method("knowledge_canonicalize_relations"));
        assert!(is_streaming_method("knowledge_backfill_relation_types"));
        assert!(is_streaming_method("knowledge_backfill_summary_embeddings"));
        assert!(is_streaming_method("knowledge_reembed_missing"));
        assert!(is_streaming_method("knowledge_reprocess_relation_types"));
        assert!(is_streaming_method("knowledge_reprocess_entity_types"));
        assert!(!is_streaming_method("knowledge_status"));
    }

    /// Issue #884: `knowledge_status` advertises the opt-in `include_episode_breakdown` flag.
    #[test]
    fn status_advertises_episode_breakdown_flag() {
        let r = registry();
        let t = r.iter().find(|t| t.name == "knowledge_status").unwrap();
        let schema = (t.input_schema)();
        assert_eq!(
            schema["properties"]["include_episode_breakdown"]["type"],
            "boolean"
        );
        assert!(schema.get("required").is_none());
        for needle in [
            "include_episode_breakdown",
            "episode_breakdown_unavailable",
            "only_superseded_entities",
            "only_superseded_edges",
        ] {
            assert!(t.description.contains(needle), "missing {needle}");
        }
    }

    /// Issue #857: `include_episodes` on `find_entities` and `uuids` on `get_episodes` are
    /// advertised, and the descriptions state when the provenance fields are empty vs filled.
    #[test]
    fn provenance_params_are_advertised_and_described() {
        let r = registry();
        let find = r
            .iter()
            .find(|t| t.name == "knowledge_find_entities")
            .unwrap();
        let props = &(find.input_schema)()["properties"];
        assert_eq!(props["include_episodes"]["type"], "boolean");
        for needle in [
            "include_episodes",
            "episode_uuids",
            "source_descriptions",
            "source_files",
            "episodes_truncated",
            "EMPTY",
        ] {
            assert!(find.description.contains(needle), "missing {needle}");
        }
        let eps = r
            .iter()
            .find(|t| t.name == "knowledge_get_episodes")
            .unwrap();
        let props = &(eps.input_schema)()["properties"];
        assert_eq!(props["uuids"]["type"], "array");
        assert_eq!(props["uuids"]["items"]["type"], "string");
        assert!(eps.description.contains("uuids"));
    }

    /// Issue #667: the two bulk reads advertise paging, projection and prefix filtering, and
    /// `knowledge_get_episodes` documents `group_ids`, the `group_id` alias and all-groups default.
    #[test]
    fn bulk_reads_advertise_paging_projection_and_prefix() {
        let r = registry();
        for name in ["knowledge_get_episodes", "knowledge_list_entities"] {
            let tool = r.iter().find(|t| t.name == name).unwrap();
            let props = &(tool.input_schema)()["properties"];
            for key in ["cursor", "fields", "name_prefix", "group_ids"] {
                assert!(props.get(key).is_some(), "{name} missing {key}");
            }
        }
        let ep = r
            .iter()
            .find(|t| t.name == "knowledge_get_episodes")
            .unwrap();
        let props = &(ep.input_schema)()["properties"];
        assert!(props.get("group_id").is_some());
        assert!(
            props["group_id"].get("default").is_none(),
            "group_id must no longer default to liminis"
        );
        assert!(ep.description.contains("EVERY group"));
    }

    #[test]
    fn find_tools_document_min_similarity_and_search_evidence() {
        let r = registry();
        for (name, evidence) in [
            ("knowledge_find_entities", "summary_similarity"),
            ("knowledge_find_relationships", "fact_similarity"),
        ] {
            let tool = r.iter().find(|t| t.name == name).unwrap();
            let prop = &(tool.input_schema)()["properties"]["min_similarity"];
            assert_eq!(prop["type"], "number", "{name}");
            assert_eq!(prop["minimum"], 0.0, "{name}");
            assert_eq!(prop["maximum"], 1.0, "{name}");
            assert!(
                prop.get("default").is_none(),
                "{name}: unset must mean no floor"
            );
            for field in [
                "`search`",
                "rrf_score",
                "text_match",
                "bm25_score",
                evidence,
            ] {
                assert!(tool.description.contains(field), "{name} missing {field}");
            }
        }
    }

    #[test]
    fn backfill_summary_embeddings_is_registered_admin_scope_with_required_group_id() {
        let r = registry();
        let tool = r
            .iter()
            .find(|t| t.name == "knowledge_backfill_summary_embeddings")
            .expect("knowledge_backfill_summary_embeddings must be registered");
        assert_eq!(
            tool.scope,
            Scope::Admin,
            "performs index drop/rebuild — must be Admin scope, not Write"
        );
        let schema = (tool.input_schema)();
        assert_eq!(
            schema.get("required").and_then(|r| r.as_array()),
            Some(&vec![json!("group_id")]),
            "group_id must be the sole required field"
        );
    }

    #[test]
    fn reembed_missing_is_registered_admin_scope_with_required_group_id() {
        let r = registry();
        let tool = r
            .iter()
            .find(|t| t.name == "knowledge_reembed_missing")
            .expect("knowledge_reembed_missing must be registered");
        assert_eq!(tool.scope, Scope::Admin);
        let schema = (tool.input_schema)();
        assert_eq!(schema["required"], json!(["group_id"]));
        assert_eq!(
            schema["properties"]["kinds"]["items"]["enum"],
            json!(["name", "fact", "summary", "content"])
        );
    }

    #[test]
    fn search_passages_documents_query_variants() {
        // Issue #802, FR-011: the schema documents `queries` and the description says when to
        // supply them and what good variants are.
        let spec = registry()
            .into_iter()
            .find(|t| t.name == "knowledge_search_passages")
            .expect("knowledge_search_passages is registered");
        let schema = (spec.input_schema)();
        let q = &schema["properties"]["queries"];
        assert_eq!(q["type"], "array");
        assert_eq!(q["maxItems"], 5);
        assert_eq!(q["items"]["type"], "string");
        assert!(
            !schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "queries"),
            "queries is optional"
        );
        for needle in [
            "queries",
            "came back thin",
            "describes something",
            "keywords the note itself",
            "likely name",
            "hypothetical answer",
            "named entities",
            "placeholders",
            "multi-part",
            "matched_queries",
        ] {
            assert!(
                spec.description.contains(needle),
                "description must mention {needle:?}: {}",
                spec.description
            );
        }
    }

    #[test]
    fn edge_tool_descriptions_state_episode_uuids_semantics_accurately() {
        let r = registry();
        let desc_of = |name: &str| {
            r.iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("tool {name} must remain registered"))
                .description
        };

        // These three read paths build `RelatesToEdge` via `..Default::default()` and
        // never call `enrich_edge_from_entity_ep_info`, so `episode_uuids` is
        // unconditionally empty on returned edges (issue #410).
        for name in [
            "knowledge_find_relationships",
            "knowledge_get_edges_by_group",
            "knowledge_get_edges_by_uuids",
        ] {
            let desc = desc_of(name);
            assert!(
                desc.contains("episode_uuids") && desc.contains("always") && desc.contains("empty"),
                "{name} description must state episode_uuids is always empty on this path, got: {desc}"
            );
            assert!(
                !desc.to_lowercase().contains("provenance"),
                "{name} description must not claim provenance for episode_uuids, got: {desc}"
            );
        }

        // These two call `enrich_edge_from_entity_ep_info`, populating `episode_uuids`
        // with either-endpoint entity mention co-occurrence (ADR-0012) — not evidence
        // for the specific relationship.
        for name in [
            "knowledge_list_relationships",
            "knowledge_get_entity_neighbors",
        ] {
            let desc = desc_of(name);
            assert!(
                desc.contains("episode_uuids") && desc.contains("either-endpoint"),
                "{name} description must state either-endpoint co-occurrence semantics, got: {desc}"
            );
            assert!(
                !desc.to_lowercase().contains("provenance"),
                "{name} description must not claim provenance for edge episode_uuids, got: {desc}"
            );
        }

        // knowledge_list_entities is entity-scoped and out of scope for issue #410:
        // "episode provenance" is accurate there and must remain untouched.
        assert!(desc_of("knowledge_list_entities").contains("episode provenance attached"));
    }

    /// Issue #796: `knowledge_search_passages` exposes the optional rerank / expansion stages,
    /// as optional parameters only — a client that sends none of them sees no change.
    #[test]
    fn search_passages_advertises_optional_stage_parameters() {
        let r = registry();
        let tool = r
            .iter()
            .find(|t| t.name == "knowledge_search_passages")
            .expect("registered");
        let schema = (tool.input_schema)();
        let props = &schema["properties"];
        assert_eq!(props["rerank"]["type"], "boolean");
        assert_eq!(props["expand"]["type"], "boolean");
        assert_eq!(props["rerank_candidates"]["type"], "integer");
        assert_eq!(
            schema["required"],
            json!(["query"]),
            "stages must stay optional"
        );
        assert_eq!(
            props["expand"]["default"],
            json!(false),
            "expansion is never a default"
        );
        assert!(
            props["rerank"].get("default").is_none(),
            "rerank defaults come from config"
        );
        assert!(tool.description.contains("rerank") && tool.description.contains("expand"));
    }

    /// SC-006 (issue #528): every episode-derived MCP tool's description must accurately state
    /// whether `attributes` is included in that tool's response, mirroring the discipline
    /// `edge_tool_descriptions_state_episode_uuids_semantics_accurately` enforces for
    /// `episode_uuids` (issue #410).
    #[test]
    fn tool_descriptions_state_attributes_inclusion_accurately() {
        let r = registry();
        let desc_of = |name: &str| {
            r.iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("tool {name} must remain registered"))
                .description
        };

        // The two episode-creation entry points write attributes onto the resulting episode
        // (FR-003) and must say so.
        for name in ["knowledge_process_chunk", "knowledge_add_episode"] {
            let desc = desc_of(name);
            assert!(
                desc.contains("attributes") && desc.contains("528"),
                "{name} description must state attributes are written to the episode, got: {desc}"
            );
        }

        // The two full-episode read paths must state attributes ARE included (FR-006/FR-010).
        for name in ["knowledge_get_episodes", "knowledge_search_passages"] {
            let desc = desc_of(name);
            assert!(
                desc.contains("attributes") && desc.contains("528"),
                "{name} description must state attributes are included in the response, got: {desc}"
            );
            assert!(
                !desc.to_lowercase().contains("does not include"),
                "{name} description must claim inclusion, not exclusion, of attributes, got: {desc}"
            );
        }

        // Every other episode-adjacent read path surfaces only partial provenance
        // (episode_uuids/source_descriptions), never a full Episodic object, and must say so
        // explicitly per FR-011 rather than leaving the omission implicit.
        for name in [
            "knowledge_find_relationships",
            "knowledge_get_edges_by_group",
            "knowledge_get_edges_by_uuids",
            "knowledge_list_relationships",
            "knowledge_get_entity_neighbors",
            "knowledge_list_entities",
            "knowledge_get_entities_by_source",
        ] {
            let desc = desc_of(name);
            assert!(
                desc.contains("Does not include episode `attributes`") && desc.contains("528"),
                "{name} description must explicitly state it does not include episode attributes, got: {desc}"
            );
        }
    }

    #[test]
    fn backfill_relation_types_description_warns_and_points_to_replacement() {
        let r = registry();
        let tool = r
            .iter()
            .find(|t| t.name == "knowledge_backfill_relation_types")
            .expect("knowledge_backfill_relation_types must remain registered");
        let desc = tool.description;
        assert!(
            desc.contains("DEPRECATED"),
            "description must mark the tool deprecated"
        );
        assert!(
            desc.contains("does not classify against the ontology"),
            "description must plainly state it does not classify against the ontology"
        );
        assert!(
            desc.contains("pollutes the relation_type space") && desc.contains("re-nulling"),
            "description must warn about pollution and reversibility only via re-nulling"
        );
        assert!(
            desc.contains("knowledge_reprocess_relation_types") && desc.contains("untyped"),
            "description must point callers to knowledge_reprocess_relation_types {{scope: \"untyped\"}}"
        );
    }
}

#[cfg(test)]
mod walk_tool_tests {
    use super::*;

    fn tool(name: &str) -> ToolSpec {
        registry()
            .into_iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} must be registered"))
    }

    #[test]
    fn walk_and_ontology_are_read_scope() {
        assert_eq!(tool("knowledge_walk").scope, Scope::Read);
        assert_eq!(tool("knowledge_get_ontology").scope, Scope::Read);
    }

    #[test]
    fn walk_description_carries_the_planning_guidance() {
        let d = tool("knowledge_walk").description;
        assert!(d.contains("knowledge_get_ontology"), "step 1");
        assert!(
            d.contains("X REPORTS_TO Y: from X to Y is `out`"),
            "step 2 example"
        );
        assert!(d.contains("answer_type"), "step 2 answer type");
        assert!(d.contains("knowledge_search_passages"), "step 3 fallback");
    }

    #[test]
    fn walk_documents_include_provenance() {
        let t = tool("knowledge_walk");
        let prop = &(t.input_schema)()["properties"]["include_provenance"];
        assert_eq!(prop["type"], "boolean");
        assert_eq!(prop["default"], false);
        let pd = prop["description"].as_str().unwrap();
        for kw in ["LIVE", "20", "episodes_truncated", "source_files"] {
            assert!(pd.contains(kw), "schema description lacks {kw}");
        }
        for kw in ["include_provenance", "LIVE", "20", "episodes_truncated"] {
            assert!(t.description.contains(kw), "tool description lacks {kw}");
        }
    }

    #[test]
    fn walk_schema_requires_only_start() {
        let schema = (tool("knowledge_walk").input_schema)();
        assert_eq!(schema["required"], json!(["start"]));
        assert_eq!(
            schema["properties"]["hops"]["items"]["properties"]["direction"]["enum"],
            json!(["out", "in", "any"])
        );
    }
}
