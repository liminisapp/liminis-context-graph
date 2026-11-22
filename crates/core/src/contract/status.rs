//! `knowledge_status` result (handlers.rs `handle_knowledge_status`).

use super::Nullable;
use schemars::JsonSchema;
use std::collections::BTreeMap;

use super::health::Providers;

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
