//! `knowledge_process_chunk` result.

use schemars::JsonSchema;

/// An extracted edge dropped because an endpoint never resolved to an entity.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct DroppedEdge {
    pub source_name: String,
    pub target_name: String,
    #[schemars(required)]
    pub relation_type: Option<String>,
    pub fact: String,
    /// `source`, `target` or `both`.
    pub unresolved_endpoint: String,
}

/// Entity-resolution outcomes by path for this chunk.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct DedupPaths {
    pub exact_name: u64,
    pub embedding_merge: u64,
    pub vetoed: u64,
    pub adapter_rejected: u64,
    pub llm_confirmed: u64,
    pub llm_rejected: u64,
    pub llm_unavailable: u64,
    pub salvage_vetoed: u64,
}

/// Advisory attached when `chunk_text` exceeds the recommended size.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ChunkWarning {
    /// `chunk_text_oversized`.
    #[serde(rename = "type")]
    pub kind: String,
    pub chunk_text_chars: u64,
    pub recommended_max_chars: u64,
    pub message: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ProcessChunkResult {
    pub success: bool,
    pub chunk_id: String,
    pub superseded_episode_uuids: Vec<String>,
    pub source_file: String,
    pub episode_uuid: String,
    pub nodes_extracted: u64,
    pub edges_extracted: u64,
    pub edges_dropped_unresolvable: u64,
    pub edges_dropped_self_loop: u64,
    pub dropped_edges: Vec<DroppedEdge>,
    pub edges_reclassified_unclassified: u64,
    pub edges_swapped: u64,
    pub edges_signature_mismatch: u64,
    pub edges_literal_object: u64,
    pub edges_suspected_misattachment: u64,
    pub entities_reclassified_unclassified: u64,
    pub entities_dropped_malformed: u64,
    pub entities_dropped_source_id: u64,
    pub edges_dropped_malformed: u64,
    pub dedup_paths: DedupPaths,
    pub duration_seconds: f64,
    /// Present only when same-named episodes written by another path were left alone (#292).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_alone_episode_uuids: Option<Vec<String>>,
    /// Present only when `chunk_text` is oversized.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<ChunkWarning>,
}
