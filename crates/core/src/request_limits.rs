//! Operator-configurable request-size limits (issue #293, ADR-0293).
//!
//! * `LCG_REQUEST_MAX_BYTES` — the longest IPC request line, in bytes, the socket / named-pipe
//!   read loop will buffer. A longer line is refused with JSON-RPC error `-32005` before JSON
//!   parsing and dispatch.
//! * `LCG_CHUNK_TEXT_MAX_BYTES` — the largest `chunk_text`, in UTF-8 bytes, that
//!   `knowledge_process_chunk` accepts.
//!
//! Both follow the strict-at-startup model of `LCG_DOCUMENT_MAX_BYTES` ([`crate::document_config`],
//! ADR-0862): an invalid or empty value is an error, never a silent fallback. [`check_consistency`]
//! additionally refuses a configuration whose line limit cannot carry a maximal document.

use std::sync::OnceLock;

use crate::error::Error;
use crate::size_parse::parse_size_bytes;

pub const REQUEST_ENV_VAR: &str = "LCG_REQUEST_MAX_BYTES";
pub const CHUNK_ENV_VAR: &str = "LCG_CHUNK_TEXT_MAX_BYTES";

/// Default request-line limit: 16 MiB. Carries 2 MiB × 6 (worst-case JSON escaping) plus envelope.
pub const DEFAULT_REQUEST_MAX_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound for the request-line limit: 2 GiB. Accommodates `LCG_DOCUMENT_MAX_BYTES=256MiB`
/// (which needs about 1.5 GiB).
pub const MAX_ALLOWED_REQUEST_BYTES: usize = 2 * 1024 * 1024 * 1024;

/// Default `chunk_text` cap: 512 KiB (about 64× the 8000-char advisory threshold).
pub const DEFAULT_CHUNK_TEXT_MAX_BYTES: usize = 512 * 1024;
/// Upper bound for the `chunk_text` cap: 256 MiB.
pub const MAX_ALLOWED_CHUNK_TEXT_BYTES: usize = 256 * 1024 * 1024;

/// Worst-case JSON expansion of one body byte: a control character becomes `\u00XX` (6 bytes).
pub const JSON_ESCAPE_FACTOR: usize = 6;
/// Allowance for the JSON-RPC envelope and the other (small) parameters of a request.
pub const ENVELOPE_BYTES: usize = 64 * 1024;

/// Smallest line limit that can carry a maximal document at `doc_cap`.
pub fn min_line_limit_for(doc_cap: usize) -> usize {
    doc_cap
        .saturating_mul(JSON_ESCAPE_FACTOR)
        .saturating_add(ENVELOPE_BYTES)
}

/// Parse a raw `LCG_REQUEST_MAX_BYTES` value.
pub fn parse_request_max_bytes(raw: &str) -> Result<usize, Error> {
    parse_size_bytes(raw, REQUEST_ENV_VAR, MAX_ALLOWED_REQUEST_BYTES, "2GiB")
}

/// Parse a raw `LCG_CHUNK_TEXT_MAX_BYTES` value.
pub fn parse_chunk_text_max_bytes(raw: &str) -> Result<usize, Error> {
    parse_size_bytes(raw, CHUNK_ENV_VAR, MAX_ALLOWED_CHUNK_TEXT_BYTES, "256MiB")
}

fn read_env(var: &str) -> Result<Option<String>, Error> {
    match std::env::var(var) {
        Ok(v) => Ok(Some(v)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(v)) => Err(Error::Config(format!(
            "invalid {var}={v:?}: value is not valid UTF-8"
        ))),
    }
}

/// Resolved `LCG_REQUEST_MAX_BYTES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestLimit {
    pub max_bytes: usize,
    /// True when the value came from the environment rather than the default.
    pub from_env: bool,
}

impl RequestLimit {
    /// Pure resolution of the raw value (`None` = unset).
    pub fn from_var(raw: Option<&str>) -> Result<Self, Error> {
        match raw {
            None => Ok(Self {
                max_bytes: DEFAULT_REQUEST_MAX_BYTES,
                from_env: false,
            }),
            Some(r) => Ok(Self {
                max_bytes: parse_request_max_bytes(r)?,
                from_env: true,
            }),
        }
    }

    /// Read from the process environment. An empty value is an error, not "unset".
    pub fn from_env() -> Result<Self, Error> {
        Self::from_var(read_env(REQUEST_ENV_VAR)?.as_deref())
    }

    /// One-line startup-log description.
    pub fn describe(&self) -> String {
        if self.from_env {
            format!(
                "request line limit: {} bytes ({REQUEST_ENV_VAR})",
                self.max_bytes
            )
        } else {
            format!(
                "request line limit: {} bytes ({REQUEST_ENV_VAR} unset)",
                self.max_bytes
            )
        }
    }
}

/// Resolved `LCG_CHUNK_TEXT_MAX_BYTES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkTextLimit {
    pub max_bytes: usize,
    pub from_env: bool,
}

impl ChunkTextLimit {
    pub fn from_var(raw: Option<&str>) -> Result<Self, Error> {
        match raw {
            None => Ok(Self {
                max_bytes: DEFAULT_CHUNK_TEXT_MAX_BYTES,
                from_env: false,
            }),
            Some(r) => Ok(Self {
                max_bytes: parse_chunk_text_max_bytes(r)?,
                from_env: true,
            }),
        }
    }

    pub fn from_env() -> Result<Self, Error> {
        Self::from_var(read_env(CHUNK_ENV_VAR)?.as_deref())
    }

    pub fn describe(&self) -> String {
        if self.from_env {
            format!(
                "chunk_text size cap: {} bytes ({CHUNK_ENV_VAR})",
                self.max_bytes
            )
        } else {
            format!(
                "chunk_text size cap: {} bytes ({CHUNK_ENV_VAR} unset)",
                self.max_bytes
            )
        }
    }
}

/// Refuse a configuration whose request-line limit cannot hold a maximal document at `doc_cap`
/// (`line_limit >= 6 * doc_cap + 64 KiB`).
pub fn check_consistency(line_limit: usize, doc_cap: usize) -> Result<(), Error> {
    let min = min_line_limit_for(doc_cap);
    if line_limit >= min {
        return Ok(());
    }
    Err(Error::Config(format!(
        "{REQUEST_ENV_VAR}={line_limit} bytes is too small to carry a maximal document at \
         LCG_DOCUMENT_MAX_BYTES={doc_cap} bytes: a JSON-escaped document can be up to \
         {JSON_ESCAPE_FACTOR}x its size, so {REQUEST_ENV_VAR} must be at least {min} bytes \
         ({JSON_ESCAPE_FACTOR} x LCG_DOCUMENT_MAX_BYTES + {} KiB envelope); raise \
         {REQUEST_ENV_VAR} to at least {min} or lower LCG_DOCUMENT_MAX_BYTES",
        ENVELOPE_BYTES / 1024
    )))
}

static INSTALLED_REQUEST: OnceLock<usize> = OnceLock::new();
static INSTALLED_CHUNK: OnceLock<usize> = OnceLock::new();

/// Install the process-wide request-line limit (once, from `main`). A second call is ignored.
pub fn install_request_max_bytes(max_bytes: usize) -> bool {
    INSTALLED_REQUEST.set(max_bytes).is_ok()
}

/// Install the process-wide `chunk_text` cap (once, from `main`). A second call is ignored.
pub fn install_chunk_text_max_bytes(max_bytes: usize) -> bool {
    INSTALLED_CHUNK.set(max_bytes).is_ok()
}

/// The effective request-line limit: installed value, or 16 MiB.
pub fn request_line_max_bytes() -> usize {
    INSTALLED_REQUEST
        .get()
        .copied()
        .unwrap_or(DEFAULT_REQUEST_MAX_BYTES)
}

/// The effective `chunk_text` cap: installed value, or 512 KiB.
pub fn chunk_text_max_bytes() -> usize {
    INSTALLED_CHUNK
        .get()
        .copied()
        .unwrap_or(DEFAULT_CHUNK_TEXT_MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_unset() {
        let r = RequestLimit::from_var(None).unwrap();
        assert_eq!(r.max_bytes, 16 * 1024 * 1024);
        assert!(!r.from_env);
        assert!(r.describe().contains("unset"));
        let c = ChunkTextLimit::from_var(None).unwrap();
        assert_eq!(c.max_bytes, 512 * 1024);
        assert!(c.describe().contains("unset"));
    }

    #[test]
    fn valid_values() {
        assert_eq!(parse_request_max_bytes("32MiB").unwrap(), 32 * 1024 * 1024);
        assert_eq!(
            parse_request_max_bytes("2GiB").unwrap(),
            2 * 1024 * 1024 * 1024
        );
        assert_eq!(parse_request_max_bytes("1").unwrap(), 1);
        assert_eq!(parse_chunk_text_max_bytes("256KiB").unwrap(), 256 * 1024);
        assert_eq!(
            parse_chunk_text_max_bytes("256MiB").unwrap(),
            MAX_ALLOWED_CHUNK_TEXT_BYTES
        );
        assert!(RequestLimit::from_var(Some("4MiB")).unwrap().from_env);
        assert!(ChunkTextLimit::from_var(Some("4MiB")).unwrap().from_env);
    }

    #[test]
    fn rejects_invalid_values() {
        for bad in [
            "", "  ", "abc", "0", "-1", "+4", "1.5MiB", "4MB", "4mib", "4 MiB", "4_000", "1TiB",
        ] {
            let m = parse_request_max_bytes(bad).expect_err(bad).to_string();
            assert!(m.contains("LCG_REQUEST_MAX_BYTES"), "{bad}: {m}");
            assert!(m.contains(&format!("\"{bad}\"")), "{bad}: {m}");
            let m = parse_chunk_text_max_bytes(bad).expect_err(bad).to_string();
            assert!(m.contains("LCG_CHUNK_TEXT_MAX_BYTES"), "{bad}: {m}");
        }
        assert!(RequestLimit::from_var(Some("")).is_err());
        assert!(ChunkTextLimit::from_var(Some("")).is_err());
    }

    #[test]
    fn upper_bounds_and_overflow() {
        assert!(parse_request_max_bytes("3GiB").is_err());
        assert!(parse_request_max_bytes("2147483649").is_err());
        assert!(parse_request_max_bytes("99999999999999999999GiB").is_err());
        assert!(parse_chunk_text_max_bytes("257MiB").is_err());
        // GiB is only a unit where the bound is at least 1 GiB.
        assert!(parse_chunk_text_max_bytes("1GiB").is_err());
    }

    #[test]
    fn default_pair_carries_a_maximal_default_document() {
        check_consistency(
            DEFAULT_REQUEST_MAX_BYTES,
            crate::document_config::DEFAULT_MAX_BYTES,
        )
        .unwrap();
    }

    #[test]
    fn small_line_limit_with_big_document_names_both_settings() {
        let e = check_consistency(DEFAULT_REQUEST_MAX_BYTES, 4 * 1024 * 1024)
            .unwrap_err()
            .to_string();
        assert!(e.contains("LCG_REQUEST_MAX_BYTES"), "{e}");
        assert!(e.contains("LCG_DOCUMENT_MAX_BYTES"), "{e}");
        assert!(
            e.contains(&min_line_limit_for(4 * 1024 * 1024).to_string()),
            "{e}"
        );
    }

    #[test]
    fn max_document_cap_fits_in_max_line_limit() {
        check_consistency(
            MAX_ALLOWED_REQUEST_BYTES,
            crate::document_config::MAX_ALLOWED_BYTES,
        )
        .unwrap();
    }

    #[test]
    fn boundary_is_inclusive() {
        let doc = 1024 * 1024;
        let min = min_line_limit_for(doc);
        assert!(check_consistency(min, doc).is_ok());
        assert!(check_consistency(min - 1, doc).is_err());
    }

    #[test]
    fn accessors_fall_back_to_defaults() {
        assert_eq!(request_line_max_bytes(), DEFAULT_REQUEST_MAX_BYTES);
        assert_eq!(chunk_text_max_bytes(), DEFAULT_CHUNK_TEXT_MAX_BYTES);
    }
}
