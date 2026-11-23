//! `knowledge_search_passages` result.

use super::Nullable;
use schemars::JsonSchema;
use serde_json::Value;

/// A free-form JSON object (`{"type": "object"}`): the stage reports are always objects, but
/// their members are deliberately left unpinned. Plain `Value` would generate a schema with no
/// `type` at all, which accepts an explicit `null` and yields an `unknown` consumer type.
fn free_form_object(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": "object" })
}

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
    /// Which hybrid retrieval legs found the passage (#871): `dense`, `lexical` or `both`.
    /// Only on hybrid passage search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legs: Option<PassageLegs>,
    /// Raw BM25 value of the lexical leg (#871); only on single-query hybrid search where the
    /// lexical leg matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lexical_score: Option<f64>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum PassageLegs {
    Dense,
    Lexical,
    Both,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct SearchPassagesResult {
    pub passages: Vec<Passage>,
    pub count: u64,
    /// Rerank stage report; present only when reranking ran. Free-form.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    #[schemars(schema_with = "free_form_object")]
    pub rerank: Option<Value>,
    /// Query-expansion report; present only when expansion ran. Free-form.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    #[schemars(schema_with = "free_form_object")]
    pub expansion: Option<Value>,
}
