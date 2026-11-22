pub mod anthropic_config;
pub mod app_state;
pub mod assert;
pub mod backfill;
pub mod backfill_summary_embeddings;
pub mod belief;
pub mod canonicalize;
pub mod cassette;
pub(crate) mod checkpoint;
pub mod correction_reads;
pub mod corrections;
pub mod cross_group;
pub mod db;
pub mod dedup_adapter;
pub mod dedup_judge;
pub mod direction;
pub mod document;
pub(crate) mod dump;
pub mod embedder;
pub mod embedding_cache;
pub mod env;
pub mod episode;
pub mod error;
pub mod extraction_failures;
pub mod extractor;
pub mod group_purge;
pub mod handlers;
pub mod identifier_veto;
pub mod identity_stamp;
pub mod index_lifecycle;
pub mod ipc;
pub mod lbug_config;
pub(crate) mod lbug_extension_home;
pub(crate) mod legacy_wal;
pub mod lint;
pub mod llm_router;
pub mod ontology;
pub mod ontology_sidecar;
pub mod origin;
pub mod origin_reads;
pub mod passage_rerank;
pub mod pointer;
pub mod prompts;
pub mod query_expand;
pub mod query_variants;
pub mod read_page;
pub mod rebuild_job;
pub mod recovery;
pub mod reembed_missing;
pub mod replay;
pub mod replay_embed_retry;
pub mod reprocess_relations;
pub mod rerank;
pub mod schema;
pub mod search;
pub mod search_config;
pub mod search_stages;
pub mod sha256_hex;
pub mod sidecar_client;
pub mod source_id;
pub mod staged_search;
pub mod summary_merge;
pub mod telemetry;
pub mod token_budget;
pub mod types;
pub mod wal;
pub(crate) mod wal_exec;
pub mod wal_generation;
pub mod wal_group;
pub mod wal_strip;
pub mod walk;
pub mod write_policy;
pub mod zero_vector_record;

pub use app_state::AppState;
pub use cassette::{CassetteRecord, CassetteWriter, RecordingExtractor, ReplayingExtractor};
pub use db::{Conn, Db, WalPositionRecord};
pub use dedup_adapter::{
    build_dedup_adapter, dedup_llm_requested, dedup_mode_description, deprecated_adapter_url_set,
    effective_dedup_mode, DedupAdapter, DedupMode, ExtractorDedupAdapter, PassthroughDedupAdapter,
};
pub use embedder::{
    EffectiveInstructions, EmbedRole, Embedder, EmbedderAdvert, EmbedderInfo, InstructedEmbedder,
    InstructionConfig, InstructionProfile, InstructionSource, MockEmbedder, NameMapEmbedder,
    OaiEmbedder,
};
pub use embedding_cache::{EmbedderContext, EmbeddingCache, EmbeddingIdentity};
pub use error::Error;
pub use extraction_failures::{
    ExtractionFailureRecord, ExtractionFailureSink, ExtractionFailureWriter,
    DEFAULT_MAX_BYTES_PER_FILE,
};
pub use extractor::{
    AnthropicExtractor, ConfigurableExtractor, ExtractOptions, Extractor, MockExtractor,
    OaiExtractor,
};
pub use ipc::{IpcRequest, IpcResponse};
pub use llm_router::LlmRouter;
pub use ontology::{Ontology, OntologyMode};
pub use rebuild_job::{JobStatus, RebuildJob};
pub use replay::{
    zero_vector_embed_fn, FailureSample, RecomputeEmbedFn, ReplayOptions, ReplayProgress,
    ReplayStats, WalReplayer,
};
pub use schema::init as init_schema;
pub use telemetry::{CaptureSink, NoopSink, TeeSink, TelemetryEvent, TelemetrySink};
pub use types::{
    EntityRow, EpisodicRow, ExtractedEdge, ExtractedEntity, ExtractionOutcome, ExtractionResult,
    MentionsEdge, RelatesToEdge, SourceType,
};
pub use wal::{WalLine, WalRotationInfo, WalWriter};
pub use wal_group::DEFAULT_GROUP_ID;
