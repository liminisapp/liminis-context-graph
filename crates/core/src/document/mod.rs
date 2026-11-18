//! Document mode (issue #785, ADR-0785): `knowledge_process_document` ingests one whole document
//! under a completeness claim, chunks it server-side, diffs it against the previous committed
//! revision and applies ADR-0705's supersession operations itself.

pub mod chunker;
pub mod context;
pub mod diff;

/// One committed document revision (issue #785): the `DocRevision` row. Timestamps are RFC 3339.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct DocRevisionRecord {
    pub revision_id: String,
    pub group_id: String,
    pub doc_id: String,
    pub title: Option<String>,
    pub source_path: Option<String>,
    pub origin: String,
    /// Caller-supplied; recorded only, never `valid_at` (FR spec assumption).
    pub effective_at: Option<String>,
    /// The commit instant: the revision's episodes are believed from here.
    pub committed_at: String,
    pub reference_time: String,
    /// The caller's free-form `attributes`, a JSON object string.
    pub attributes: String,
    /// SHA-256 of the whole submitted document.
    pub content_hash: String,
    pub byte_len: i64,
    pub chunker_version: i64,
    /// Logical-chunk names in document order — the manifest the next diff pairs against.
    pub chunk_names: Vec<String>,
    pub previous_revision_id: Option<String>,
}
