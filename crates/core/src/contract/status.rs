//! `knowledge_status` result (handlers.rs `handle_knowledge_status`).

use super::Nullable;
use schemars::JsonSchema;
use std::collections::BTreeMap;

use super::health::Providers;

/// One group's episode (and derived entity/edge) liveness counts (#884, ADR-0884).
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EpisodeBreakdownGroup {
    pub group_id: String,
    pub live: u64,
    pub superseded: u64,
    pub pending: u64,
    pub only_superseded_entities: u64,
    pub only_superseded_edges: u64,
}

/// The opt-in `episode_breakdown` block; groups are sorted by `group_id`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EpisodeBreakdown {
    /// Instant (microseconds since the epoch) liveness was evaluated at.
    pub as_of_us: i64,
    pub groups: Vec<EpisodeBreakdownGroup>,
}

/// `episode_breakdown` is *optional and nullable*: absent unless the request passed
/// `include_episode_breakdown: true`, then the block, or `null` when it is unavailable (see
/// `episode_breakdown_unavailable`). Spelled as a `oneOf` so the omit-only null stripping in
/// `contract::schema_of` leaves the `null` alternative in place.
fn breakdown_or_null(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let inner = generator.subschema_for::<EpisodeBreakdown>();
    schemars::json_schema!({ "oneOf": [inner, { "type": "null" }] })
}

/// The opt-in fields #884 adds to every `knowledge_status` shape.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EpisodeBreakdownOptIn {
    #[serde(default)]
    #[schemars(schema_with = "breakdown_or_null")]
    pub episode_breakdown: Option<EpisodeBreakdown>,
    /// Why the breakdown is unavailable; present only alongside `episode_breakdown: null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_breakdown_unavailable: Option<String>,
}

/// Rows stored with a fallback zero vector because the embedder failed (#808).
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ZeroVectorRows {
    pub total: u64,
    pub by_kind: ZeroVectorByKind,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ZeroVectorByKind {
    pub name: u64,
    pub fact: u64,
    pub summary: u64,
    pub content: u64,
}

/// The default group's WAL block (`wal`).
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Wal {
    pub exists: bool,
    pub file_count: u64,
    pub byte_size: u64,
    /// `null` = unknown, `0` = nothing applied.
    pub applied_seq: Nullable<u64>,
    pub max_seq: Nullable<u64>,
    pub generation: Nullable<String>,
    /// `known`, `unknown` or `not_applicable`.
    pub generation_status: String,
    /// `hydrated`, `wal_ahead` or `not_applicable`.
    pub hydration_status: String,
    pub embedding_model: Nullable<String>,
    pub embedding_dim: Nullable<i64>,
    pub embedding_doc_prefix: Nullable<String>,
    /// `match`, `mismatch`, `unknown` or `not_applicable`.
    pub embedding_model_status: String,
    /// Present only when a WAL root is configured (`default_group`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Present only when a WAL root is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_group: Option<String>,
    /// Present only when a WAL root is configured (`wal_groups`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub see: Option<String>,
}

/// One group's entry in `wal_groups`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct WalGroup {
    pub applied_seq: Nullable<u64>,
    pub max_seq: Nullable<u64>,
    pub generation: Nullable<String>,
    pub generation_status: String,
    pub hydration_status: String,
    pub embedding_model: Nullable<String>,
    pub embedding_dim: Nullable<i64>,
    pub embedding_doc_prefix: Nullable<String>,
    pub embedding_model_status: String,
    pub zero_vector_rows: ZeroVectorRows,
}

/// Effective embedding instruction profile (#790).
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EmbeddingInstructions {
    pub profile: String,
    pub source: String,
    pub query_prefix: String,
    pub document_prefix: String,
}

/// Workspace-wide ontology state.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct OntologySummary {
    pub present: bool,
    pub loaded: bool,
    pub mode: Nullable<String>,
    pub entity_type_count: u64,
    pub relation_type_count: u64,
    pub drifted: bool,
    pub drift_summary: Nullable<String>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct GroupOntologyDrift {
    pub group_id: String,
    pub drifted: bool,
    pub drift_summary: Nullable<String>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct GroupIdentityRefusal {
    pub group_id: String,
    pub message: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct CrossGroupPointers {
    pub bound: u64,
    pub unbound: u64,
    pub ambiguous: u64,
}

/// Counts are `null` when the graph is open but not queryable.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct CrossGroupPointersNullable {
    pub bound: Nullable<u64>,
    pub unbound: Nullable<u64>,
    pub ambiguous: Nullable<u64>,
}

/// Database not open (ADR-0009): counts and WAL blocks are absent.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct StatusDegraded {
    pub running: bool,
    pub degraded: bool,
    pub reason: String,
    pub context_graph_initialized: bool,
    pub connected: bool,
    pub queryable: bool,
    pub initializing: bool,
    /// Recovery strategies currently offered (`drop_lbug_wal`, `rebuild_from_workspace_wal`).
    pub recovery_available: Vec<String>,
    pub dedup_mode: String,
    pub ontology: OntologySummary,
    pub group_ontology_drift: Vec<GroupOntologyDrift>,
    pub group_identity_refusals: Vec<GroupIdentityRefusal>,
    pub indices_built: bool,
    /// Always `null` in this shape.
    pub name_index_trusted: Nullable<bool>,
    pub name_index_fallback_scans: Nullable<u64>,
    pub fts_repair_count: Nullable<u64>,
    pub fts_last_repair_unix_ms: Nullable<u64>,
    /// Always `null` in this shape.
    pub cross_group_pointers: Nullable<CrossGroupPointers>,
    pub providers: Providers,
    pub search_min_score_default: f64,
    #[serde(flatten)]
    pub episode_breakdown: EpisodeBreakdownOptIn,
}

/// Database open and every core table readable.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct StatusQueryable {
    pub database_path: String,
    pub embedding_model: String,
    pub embedding_dim: u64,
    pub embedding_instructions: EmbeddingInstructions,
    pub entity_count: u64,
    pub relationship_count: u64,
    pub legacy_edge_count: u64,
    pub episode_count: u64,
    pub last_index_time: Nullable<String>,
    /// Absent on an empty graph.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_created_at: Option<String>,
    pub context_graph_initialized: bool,
    pub connected: bool,
    pub queryable: bool,
    pub initializing: bool,
    pub wal: Wal,
    /// Per-group WAL positions keyed by group id; empty when no WAL root is configured.
    pub wal_groups: BTreeMap<String, WalGroup>,
    pub indices_built: bool,
    pub name_index_trusted: bool,
    pub name_index_fallback_scans: u64,
    pub fts_repair_count: u64,
    pub fts_last_repair_unix_ms: Nullable<u64>,
    pub cross_group_pointers: CrossGroupPointers,
    pub ontology: OntologySummary,
    pub group_ontology_drift: Vec<GroupOntologyDrift>,
    pub group_identity_refusals: Vec<GroupIdentityRefusal>,
    pub dedup_mode: String,
    pub providers: Providers,
    pub search_min_score_default: f64,
    #[serde(flatten)]
    pub episode_breakdown: EpisodeBreakdownOptIn,
}

/// Database open but a core table is missing (ADR-0325): counts are `null`, never `0`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct StatusNotQueryable {
    pub database_path: String,
    pub embedding_model: String,
    pub embedding_dim: u64,
    pub embedding_instructions: EmbeddingInstructions,
    pub entity_count: Nullable<u64>,
    pub relationship_count: Nullable<u64>,
    pub episode_count: Nullable<u64>,
    pub last_index_time: Nullable<String>,
    pub index_created_at: Nullable<String>,
    pub context_graph_initialized: bool,
    pub connected: bool,
    pub queryable: bool,
    /// Why the graph is not queryable.
    pub reason: String,
    pub initializing: bool,
    pub wal: Wal,
    pub wal_groups: BTreeMap<String, WalGroup>,
    pub indices_built: bool,
    pub name_index_trusted: bool,
    pub name_index_fallback_scans: u64,
    pub fts_repair_count: u64,
    pub fts_last_repair_unix_ms: Nullable<u64>,
    pub cross_group_pointers: CrossGroupPointersNullable,
    pub ontology: OntologySummary,
    pub group_ontology_drift: Vec<GroupOntologyDrift>,
    pub group_identity_refusals: Vec<GroupIdentityRefusal>,
    pub dedup_mode: String,
    pub providers: Providers,
    pub search_min_score_default: f64,
    #[serde(flatten)]
    pub episode_breakdown: EpisodeBreakdownOptIn,
}

/// The three `knowledge_status` shapes. They are told apart by their required keys and types
/// (`degraded`+`recovery_available`; integer counts; `reason` with `null` counts), so this is an
/// `anyOf`.
#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
pub enum StatusResult {
    Degraded(StatusDegraded),
    Queryable(StatusQueryable),
    NotQueryable(StatusNotQueryable),
}
