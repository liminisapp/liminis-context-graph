//! Operator-tunable lbug engine settings (issue #737, ADR-0737).
//!
//! `LCG_BUFFER_POOL_BYTES` bounds lbug's buffer pool; `LCG_MAX_THREADS` bounds its worker
//! threads. Unset keeps lbug's defaults (`SystemConfig::default()`: pool ~80% of system memory,
//! one thread per CPU). Parsing is strict: an invalid value is an error, never a silent fallback,
//! because falling back to the 80%-of-memory default reintroduces the OOM this exists to prevent.

use crate::error::Error;

pub const BUFFER_POOL_ENV: &str = "LCG_BUFFER_POOL_BYTES";
pub const MAX_THREADS_ENV: &str = "LCG_MAX_THREADS";

/// Parsed lbug tuning. `None` means "use lbug's default".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LbugTuning {
    pub buffer_pool_bytes: Option<u64>,
    pub max_threads: Option<u64>,
}

fn invalid(var: &str, raw: &str, reason: &str, expected: &str) -> Error {
    Error::Config(format!(
        "invalid {var}=\"{raw}\": {reason}; expected {expected}"
    ))
}

const POOL_EXPECTED: &str =
    "a positive byte count, optionally with a KiB/MiB/GiB suffix (e.g. 2147483648 or 2GiB)";
const THREADS_EXPECTED: &str = "a positive integer (e.g. 2)";

/// Parse a byte size: positive integer with optional `KiB`/`MiB`/`GiB` suffix.
pub fn parse_byte_size(raw: &str) -> Result<u64, Error> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(invalid(BUFFER_POOL_ENV, raw, "empty value", POOL_EXPECTED));
    }
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, suffix) = s.split_at(digits_end);
    if digits.is_empty() {
        return Err(invalid(
            BUFFER_POOL_ENV,
            raw,
            "no leading integer",
            POOL_EXPECTED,
        ));
    }
    let multiplier: u64 = match suffix.trim().to_ascii_lowercase().as_str() {
        "" => 1,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        _ => {
            return Err(invalid(
                BUFFER_POOL_ENV,
                raw,
                "unknown or unsupported suffix",
                POOL_EXPECTED,
            ))
        }
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| invalid(BUFFER_POOL_ENV, raw, "number too large", POOL_EXPECTED))?;
    let bytes = n.checked_mul(multiplier).ok_or_else(|| {
        invalid(
            BUFFER_POOL_ENV,
            raw,
            "size overflows 64 bits",
            POOL_EXPECTED,
        )
    })?;
    if bytes == 0 {
        return Err(invalid(
            BUFFER_POOL_ENV,
            raw,
            "must be greater than zero",
            POOL_EXPECTED,
        ));
    }
    Ok(bytes)
}

/// Parse a positive thread count.
pub fn parse_thread_count(raw: &str) -> Result<u64, Error> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(invalid(
            MAX_THREADS_ENV,
            raw,
            "empty value",
            THREADS_EXPECTED,
        ));
    }
    let n: u64 = s.parse().map_err(|_| {
        invalid(
            MAX_THREADS_ENV,
            raw,
            "not a positive integer",
            THREADS_EXPECTED,
        )
    })?;
    if n == 0 {
        return Err(invalid(
            MAX_THREADS_ENV,
            raw,
            "must be greater than zero",
            THREADS_EXPECTED,
        ));
    }
    Ok(n)
}

fn format_bytes(b: u64) -> String {
    const GIB: u64 = 1 << 30;
    const MIB: u64 = 1 << 20;
    const KIB: u64 = 1 << 10;
    if b >= GIB {
        format!("{:.1} GiB ({b} bytes)", b as f64 / GIB as f64)
    } else if b >= MIB {
        format!("{:.1} MiB ({b} bytes)", b as f64 / MIB as f64)
    } else if b >= KIB {
        format!("{:.1} KiB ({b} bytes)", b as f64 / KIB as f64)
    } else {
        format!("{b} bytes")
    }
}

impl LbugTuning {
    /// Pure parse of the two raw variable values (`None` = unset). No env access.
    pub fn from_vars(pool: Option<&str>, threads: Option<&str>) -> Result<Self, Error> {
        Ok(Self {
            buffer_pool_bytes: pool.map(parse_byte_size).transpose()?,
            max_threads: threads.map(parse_thread_count).transpose()?,
        })
    }

    /// Read both variables from the process environment. Unset = default; empty or
    /// non-UTF-8 values are errors.
    pub fn from_env() -> Result<Self, Error> {
        fn read(var: &str) -> Result<Option<String>, Error> {
            match std::env::var(var) {
                Ok(v) => Ok(Some(v)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(v)) => Err(Error::Config(format!(
                    "invalid {var}={v:?}: value is not valid UTF-8"
                ))),
            }
        }
        let pool = read(BUFFER_POOL_ENV)?;
        let threads = read(MAX_THREADS_ENV)?;
        Self::from_vars(pool.as_deref(), threads.as_deref())
    }

    /// True when at least one knob overrides lbug's default.
    pub fn is_customized(&self) -> bool {
        self.buffer_pool_bytes.is_some() || self.max_threads.is_some()
    }

    /// Build the lbug `SystemConfig`; unset knobs keep lbug's defaults.
    pub fn system_config(&self) -> lbug::SystemConfig {
        let mut cfg = lbug::SystemConfig::default();
        if let Some(n) = self.buffer_pool_bytes {
            cfg = cfg.buffer_pool_size(n);
        }
        if let Some(n) = self.max_threads {
            cfg = cfg.max_num_threads(n);
        }
        cfg
    }

    /// Human-readable startup log line.
    pub fn describe(&self) -> String {
        let pool = match self.buffer_pool_bytes {
            Some(b) => format!("{} ({BUFFER_POOL_ENV})", format_bytes(b)),
            None => "default (lbug auto: ~80% of system memory)".to_string(),
        };
        let threads = match self.max_threads {
            Some(n) => format!("{n} ({MAX_THREADS_ENV})"),
            None => "default (lbug auto: one per CPU)".to_string(),
        };
        format!("lbug buffer pool: {pool}; max threads: {threads}")
    }

    /// Annotation appended to an lbug open failure when a knob is set.
    pub fn open_failure_context(&self) -> String {
        let pool = self
            .buffer_pool_bytes
            .map_or("default".to_string(), |b| format!("{b} bytes"));
        let threads = self
            .max_threads
            .map_or("default".to_string(), |n| n.to_string());
        format!(
            "failed to open lbug database with {BUFFER_POOL_ENV}={pool} / {MAX_THREADS_ENV}={threads}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(s: &str) -> Result<u64, Error> {
        parse_byte_size(s)
    }

    #[test]
    fn plain_bytes() {
        assert_eq!(pool("2147483648").unwrap(), 2_147_483_648);
        assert_eq!(pool("  1024  ").unwrap(), 1024);
    }

    #[test]
    fn binary_suffixes_case_insensitive_and_spaced() {
        assert_eq!(pool("2GiB").unwrap(), 2_147_483_648);
        assert_eq!(pool("2gib").unwrap(), 2_147_483_648);
        assert_eq!(pool("2 GIB").unwrap(), 2_147_483_648);
        assert_eq!(pool(" 512MiB ").unwrap(), 512 << 20);
        assert_eq!(pool("64kib").unwrap(), 64 << 10);
        assert_eq!(pool("2GiB").unwrap(), pool("2147483648").unwrap());
    }

    #[test]
    fn rejects_invalid_pool_values() {
        for bad in [
            "", "   ", "0", "0GiB", "-1", "2.5GiB", "2GB", "2 gigs", "banana", "GiB", "1TiB", "+1",
            "1 2",
        ] {
            let e = pool(bad).expect_err(bad);
            let msg = e.to_string();
            assert!(msg.contains("LCG_BUFFER_POOL_BYTES"), "{bad}: {msg}");
            assert!(msg.contains(&format!("\"{bad}\"")), "{bad}: {msg}");
        }
    }

    #[test]
    fn rejects_overflow() {
        assert!(pool("18446744073709551615GiB").is_err());
        assert!(pool("99999999999999999999999").is_err());
        assert_eq!(pool("18446744073709551615").unwrap(), u64::MAX);
    }

    #[test]
    fn thread_count() {
        assert_eq!(parse_thread_count("2").unwrap(), 2);
        assert_eq!(parse_thread_count(" 16 ").unwrap(), 16);
        for bad in ["", "0", "-1", "two", "2.5", "1GiB"] {
            let msg = parse_thread_count(bad).expect_err(bad).to_string();
            assert!(msg.contains("LCG_MAX_THREADS"), "{bad}: {msg}");
            assert!(msg.contains(&format!("\"{bad}\"")), "{bad}: {msg}");
        }
    }

    #[test]
    fn from_vars_unset_is_default() {
        let t = LbugTuning::from_vars(None, None).unwrap();
        assert_eq!(t, LbugTuning::default());
        assert!(!t.is_customized());
    }

    #[test]
    fn from_vars_propagates_errors() {
        assert!(LbugTuning::from_vars(Some("banana"), None).is_err());
        assert!(LbugTuning::from_vars(None, Some("0")).is_err());
        let t = LbugTuning::from_vars(Some("1GiB"), Some("2")).unwrap();
        assert_eq!(t.buffer_pool_bytes, Some(1 << 30));
        assert_eq!(t.max_threads, Some(2));
        assert!(t.is_customized());
    }

    #[test]
    fn describe_default_and_set() {
        let d = LbugTuning::default().describe();
        assert!(d.contains("buffer pool: default"), "{d}");
        assert!(d.contains("max threads: default"), "{d}");
        let s = LbugTuning::from_vars(Some("2GiB"), Some("2"))
            .unwrap()
            .describe();
        assert!(s.contains("2.0 GiB (2147483648 bytes)"), "{s}");
        assert!(s.contains("LCG_BUFFER_POOL_BYTES"), "{s}");
        assert!(s.contains("max threads: 2 (LCG_MAX_THREADS)"), "{s}");
    }
}
