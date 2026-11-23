//! `health_check` result (handlers.rs `handle_health_check`).

use schemars::JsonSchema;

/// Which providers the process runs: `test` (deterministic built-in), `real`, or `none`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Providers {
    /// `test` or `real`.
    pub embedder: String,
    /// `test`, `real` or `none`.
    pub extractor: String,
}

/// Progress of a running rebuild job (only on a `busy` result with `activity: "rebuilding"`).
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct RebuildProgress {
    pub mutations_replayed: u64,
    pub wal_files_processed: u64,
    pub wal_files_total: u64,
    pub elapsed_seconds: f64,
}

/// The three `health_check` shapes, discriminated by `state`.
#[derive(JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum HealthCheckResult {
    /// Idle and ready for writes.
    Healthy {
        ok: bool,
        healthy: bool,
        ready_for_reads: bool,
        server_version: String,
        protocol_version: u64,
        providers: Providers,
    },
    /// The database is not loaded.
    Degraded {
        ok: bool,
        healthy: bool,
        ready_for_reads: bool,
        reason: String,
        server_version: String,
        protocol_version: u64,
        providers: Providers,
    },
    /// Alive and serving reads, but a write or rebuild holds the write lock.
    Busy {
        ok: bool,
        healthy: bool,
        ready_for_reads: bool,
        /// `rebuilding` or `writing`.
        activity: String,
        /// Present with `activity: "rebuilding"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        job_id: Option<String>,
        /// Present with `activity: "rebuilding"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        progress: Option<RebuildProgress>,
        server_version: String,
        protocol_version: u64,
        providers: Providers,
    },
}
