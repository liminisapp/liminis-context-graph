//! `knowledge_search_passages` result.

use super::Nullable;
use schemars::JsonSchema;
use serde_json::Value;

/// One passage (episode) hit.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Passage {
    pub uuid: String,
    pub name: String,
    pub content: String,
    pub source_description: String,
    pub group_id: String,
    pub created_at: String,
    pub valid_at: Nullable<String>,
    pub score: f64,
    /// Episode metadata: a JSON object serialized as a string.
    pub attributes: String,
    /// `primary`, `external`, `derived` or `answer`.
    pub origin: String,
    /// Cosine similarity of a vector hit (hybrid mode only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f64>,
    /// Indices into `[query, ...queries]` that retrieved this passage; only with `queries`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_queries: Option<Vec<u64>>,
    /// Cross-encoder relevance logit; only on a reranked passage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_score: Option<f64>,
    /// `current` or `superseded`; only on a reranked passage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub belief: Option<String>,
    /// Omitted for a legacy episode without stored source identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    /// Omitted for a legacy episode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<String>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct SearchPassagesResult {
    pub passages: Vec<Passage>,
    pub count: u64,
    /// Rerank stage report; present only when reranking ran. Free-form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank: Option<Value>,
    /// Query-expansion report; present only when expansion ran. Free-form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expansion: Option<Value>,
}
