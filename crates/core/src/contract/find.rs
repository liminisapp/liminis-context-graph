//! `knowledge_find_entities` and `knowledge_find_relationships` results.

use super::Nullable;
use schemars::JsonSchema;

/// `support` annotation: distinct supporting documents per origin.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Support {
    pub primary: u64,
    pub external: u64,
    pub derived: u64,
    pub answer: u64,
}

/// Why an entity was returned: the fused score plus each retrieval path's raw evidence.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EntitySearchEvidence {
    pub rrf_score: f64,
    pub text_match: bool,
    pub bm25_score: Nullable<f64>,
    pub name_similarity: Nullable<f64>,
    pub summary_similarity: Nullable<f64>,
}

/// One entity in `nodes`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EntityNode {
    pub uuid: String,
    pub name: String,
    pub group_id: String,
    pub labels: Vec<String>,
    pub kind: String,
    pub created_at: String,
    pub ingested_at: String,
    pub summary: String,
    /// JSON object serialized as a string.
    pub attributes: String,
    /// Mentioning episodes. With `include_episodes` (#857) these are the live episodes, newest
    /// first, capped; `episode_uuids` and `source_descriptions` are then index-aligned.
    pub episode_uuids: Vec<String>,
    pub source_descriptions: Vec<String>,
    /// Present only with `include_episodes` (#857): de-duplicated source files, not index-aligned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_files: Option<Vec<String>>,
    /// Present (`true`) only when `include_episodes` was capped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes_truncated: Option<bool>,
    pub search: EntitySearchEvidence,
    pub support: Support,
    pub corroboration: u64,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct FindEntitiesResult {
    pub nodes: Vec<EntityNode>,
    pub count: u64,
}

/// Why a fact was returned.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EdgeSearchEvidence {
    pub rrf_score: f64,
    pub text_match: bool,
    pub bm25_score: Nullable<f64>,
    pub fact_similarity: Nullable<f64>,
}

/// Supersession state of a fact (only when it has been superseded).
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Supersession {
    /// `corrected` or `document_superseded`.
    pub cause: String,
    pub superseded_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by_document: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_force_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
}

/// Belief annotation, present only when `believed_only` or `as_of` was passed.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Belief {
    /// `believed`, `superseded`, `legacy`, `support_deleted` or `not_yet_ingested`.
    pub state: String,
    pub believed: bool,
    pub producing_episode_uuids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warrant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_at_source: Option<String>,
    pub in_force: Nullable<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersession: Option<Supersession>,
}

/// One fact in `edges`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct RelationshipEdge {
    pub uuid: String,
    pub name: String,
    pub source_node_uuid: String,
    pub target_node_uuid: String,
    pub group_id: String,
    pub fact: String,
    pub created_at: String,
    pub ingested_at: String,
    pub valid_at: Nullable<String>,
    pub invalid_at: Nullable<String>,
    pub attributes: String,
    pub relation_type: Nullable<String>,
    pub episode_uuids: Vec<String>,
    pub source_descriptions: Vec<String>,
    /// Producing episode(s); omitted when empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodes: Option<Vec<String>>,
    /// `assertion` or `cross_group`; omitted for extracted edges.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warrant: Option<String>,
    /// `text` or `reference_time`; omitted when not applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_at_source: Option<String>,
    pub search: EdgeSearchEvidence,
    pub support: Support,
    pub corroboration: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub belief: Option<Belief>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct FindRelationshipsResult {
    pub edges: Vec<RelationshipEdge>,
    pub count: u64,
}
