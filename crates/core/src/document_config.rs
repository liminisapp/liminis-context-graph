//! Operator-configurable size cap for `knowledge_process_document` (issue #862, ADR-0862).
//!
//! `LCG_DOCUMENT_MAX_BYTES` sets the largest document, in UTF-8 bytes, that
//! `knowledge_process_document` and `knowledge_preview_chunks` accept. Parsing is strict and
//! happens once at startup, like `LCG_SEARCH_MIN_SCORE` (ADR-0812): an invalid value is an error,
//! never a silent fallback to the default.
//!
//! Accepted grammar (after trimming outer whitespace): ASCII digits only (a byte count), or
//! digits immediately followed by a case-sensitive `KiB` or `MiB`. The value must be
//! positive and at most [`MAX_ALLOWED_BYTES`]. Everything else (signs, decimals, `_`, inner
//! whitespace, `KB`/`MB`/`GiB`, lowercase units, overflow) is rejected. (`GiB` is deliberately
//! not accepted: the upper bound is below 1 GiB, so every `GiB` value would be out of range.)

use std::sync::OnceLock;

use crate::error::Error;

pub const ENV_VAR: &str = "LCG_DOCUMENT_MAX_BYTES";

/// The default cap (2 MiB) when `LCG_DOCUMENT_MAX_BYTES` is unset, and for any process that never
/// ran the startup installer (in-process tests, attached `--connect` mode).
pub const DEFAULT_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Hard upper bound (256 MiB). The cap exists to bound memory and latency, so a value above this
/// is refused at startup rather than accepted with a warning.
pub const MAX_ALLOWED_BYTES: usize = 256 * 1024 * 1024;

const EXPECTED: &str = "a positive byte count (e.g. 4194304) or a count with a KiB or MiB \
                        suffix (e.g. 4MiB), at most 256MiB";

fn invalid(raw: &str, reason: &str) -> Error {
    Error::Config(format!(
        "invalid {ENV_VAR}=\"{raw}\": {reason}; expected {EXPECTED}"
    ))
}

/// Strictly parse a raw `LCG_DOCUMENT_MAX_BYTES` value into a byte count.
pub fn parse_max_bytes(raw: &str) -> Result<usize, Error> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(invalid(raw, "empty value"));
    }
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, unit) = s.split_at(digits_end);
    if digits.is_empty() {
        return Err(invalid(
            raw,
            "must start with a digit (no sign or fraction)",
        ));
    }
    let multiplier: usize = match unit {
        "" => 1,
        "KiB" => 1024,
        "MiB" => 1024 * 1024,
        _ => {
            return Err(invalid(
                raw,
                "unrecognised unit (only KiB and MiB are accepted; no decimals, \
                 whitespace or underscores)",
            ))
        }
    };
    let n: usize = digits
        .parse()
        .map_err(|_| invalid(raw, "number is too large"))?;
    if n == 0 {
        return Err(invalid(raw, "must be greater than zero"));
    }
    let bytes = n
        .checked_mul(multiplier)
        .ok_or_else(|| invalid(raw, "number is too large"))?;
    if bytes > MAX_ALLOWED_BYTES {
        return Err(invalid(
            raw,
            &format!("exceeds the upper bound of {MAX_ALLOWED_BYTES} bytes (256 MiB)"),
        ));
    }
    Ok(bytes)
}

/// The resolved startup configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentConfig {
    pub max_bytes: usize,
    /// True when the value came from `LCG_DOCUMENT_MAX_BYTES` rather than the built-in default.
    pub from_env: bool,
}

impl DocumentConfig {
    /// Pure resolution of the raw variable value (`None` = unset). No env access.
    pub fn from_var(raw: Option<&str>) -> Result<Self, Error> {
        match raw {
            None => Ok(Self {
                max_bytes: DEFAULT_MAX_BYTES,
                from_env: false,
            }),
            Some(r) => Ok(Self {
                max_bytes: parse_max_bytes(r)?,
                from_env: true,
            }),
        }
    }

    /// Read the variable from the process environment. Only a genuinely absent variable means
    /// unset: an empty value is an error, as is a non-UTF-8 one.
    pub fn from_env() -> Result<Self, Error> {
        match std::env::var(ENV_VAR) {
            Ok(v) => Self::from_var(Some(&v)),
            Err(std::env::VarError::NotPresent) => Self::from_var(None),
            Err(std::env::VarError::NotUnicode(v)) => Err(Error::Config(format!(
                "invalid {ENV_VAR}={v:?}: value is not valid UTF-8"
            ))),
        }
    }

    /// One-line startup-log description.
    pub fn describe(&self) -> String {
        if self.from_env {
            format!("document size cap: {} bytes ({ENV_VAR})", self.max_bytes)
        } else {
            format!(
                "document size cap: {} bytes ({ENV_VAR} unset)",
                self.max_bytes
            )
        }
    }
}

static INSTALLED: OnceLock<usize> = OnceLock::new();

/// Installs the process-wide cap. Called once from `main` before any database open or socket
/// bind. A second call is ignored (returns `false`).
pub fn install(max_bytes: usize) -> bool {
    INSTALLED.set(max_bytes).is_ok()
}

/// The effective document cap: the installed value, or 2 MiB if none was installed.
pub fn document_max_bytes() -> usize {
    INSTALLED.get().copied().unwrap_or(DEFAULT_MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_is_two_mib() {
        let c = DocumentConfig::from_var(None).unwrap();
        assert_eq!(c.max_bytes, 2 * 1024 * 1024);
        assert!(!c.from_env);
        assert!(c.describe().contains("2097152"));
        assert!(c.describe().contains("unset"));
    }

    #[test]
    fn valid_values() {
        assert_eq!(parse_max_bytes("1").unwrap(), 1);
        assert_eq!(parse_max_bytes("524288").unwrap(), 524_288);
        assert_eq!(parse_max_bytes("  4194304 ").unwrap(), 4_194_304);
        assert_eq!(parse_max_bytes("512KiB").unwrap(), 512 * 1024);
        assert_eq!(parse_max_bytes("4MiB").unwrap(), 4 * 1024 * 1024);
        assert_eq!(parse_max_bytes("256MiB").unwrap(), MAX_ALLOWED_BYTES);
        assert_eq!(parse_max_bytes("262144KiB").unwrap(), MAX_ALLOWED_BYTES);
        let c = DocumentConfig::from_var(Some("4MiB")).unwrap();
        assert!(c.from_env);
        assert!(c
            .describe()
            .contains("4194304 bytes (LCG_DOCUMENT_MAX_BYTES)"));
    }

    #[test]
    fn rejects_invalid_values() {
        for bad in [
            "", "   ", "abc", "0", "0MiB", "-1", "+4", "+4MiB", "1.5MiB", "4.0", "4MB", "4KB",
            "4m", "4mib", "4MIB", "4 MiB", "4_000", "4MiBx", "MiB", "1TiB", "1GiB", "0x10", "1e6",
        ] {
            let e = parse_max_bytes(bad).expect_err(bad);
            let msg = e.to_string();
            assert!(msg.contains("LCG_DOCUMENT_MAX_BYTES"), "{bad}: {msg}");
            assert!(msg.contains(&format!("\"{bad}\"")), "{bad}: {msg}");
        }
    }

    #[test]
    fn upper_bound_and_overflow_are_rejected() {
        let over = format!("{}", MAX_ALLOWED_BYTES + 1);
        let msg = parse_max_bytes(&over).unwrap_err().to_string();
        assert!(msg.contains("upper bound"), "{msg}");
        assert!(parse_max_bytes("257MiB").is_err());
        // usize overflow on the multiplication and on the digit parse itself.
        assert!(parse_max_bytes("99999999999999999999MiB").is_err());
        assert!(parse_max_bytes("18446744073709551615MiB").is_err());
        assert!(parse_max_bytes("18446744073709551615KiB").is_err());
        assert!(parse_max_bytes("99999999999999999999999999").is_err());
    }

    #[test]
    fn empty_is_error_not_unset() {
        assert!(DocumentConfig::from_var(Some("")).is_err());
    }

    #[test]
    fn accessor_falls_back_to_default_when_nothing_installed() {
        // `install` is once-per-process; this unit-test process never installs.
        assert_eq!(document_max_bytes(), DEFAULT_MAX_BYTES);
    }
}
