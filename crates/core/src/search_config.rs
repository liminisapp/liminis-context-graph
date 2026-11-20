//! Operator-configurable default `min_score` for `knowledge_search_passages` (issue #812,
//! ADR-0812).
//!
//! `LCG_SEARCH_MIN_SCORE` sets the floor used when a request omits `min_score`. Unlike the
//! lenient per-request readers (`LCG_PASSAGE_SEARCH_MODE`, `LCG_PASSAGE_BELIEF_AWARE`), parsing is
//! strict and happens once at startup: a non-numeric, empty, NaN or infinite value is an error so
//! a typo can never silently leave the wrong floor in place. Finite out-of-range values are
//! clamped to `[0.0, 1.0]`, exactly as the request parameter is.

use std::sync::OnceLock;

use crate::error::Error;

pub const ENV_VAR: &str = "LCG_SEARCH_MIN_SCORE";

/// The default floor when `LCG_SEARCH_MIN_SCORE` is unset (and for any process that never ran
/// the startup installer, e.g. in-process tests and attached `--connect` mode).
pub const DEFAULT_MIN_SCORE: f64 = 0.5;

const EXPECTED: &str = "a finite decimal number (e.g. 0.7); values outside 0.0-1.0 are clamped";

fn invalid(raw: &str, reason: &str) -> Error {
    Error::Config(format!(
        "invalid {ENV_VAR}=\"{raw}\": {reason}; expected {EXPECTED}"
    ))
}

/// Strictly parse a raw `LCG_SEARCH_MIN_SCORE` value and clamp it to `[0.0, 1.0]`.
pub fn parse_min_score(raw: &str) -> Result<f64, Error> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(invalid(raw, "empty value"));
    }
    let v: f64 = s.parse().map_err(|_| invalid(raw, "not a number"))?;
    // Rust's f64 parser accepts nan / inf / -inf / infinity (case-insensitive).
    if !v.is_finite() {
        return Err(invalid(
            raw,
            "must be finite (NaN and infinity are not allowed)",
        ));
    }
    Ok(v.clamp(0.0, 1.0))
}

/// The resolved startup configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchConfig {
    pub min_score: f64,
    /// True when the value came from `LCG_SEARCH_MIN_SCORE` rather than the built-in default.
    pub from_env: bool,
}

impl SearchConfig {
    /// Pure resolution of the raw variable value (`None` = unset). No env access.
    pub fn from_var(raw: Option<&str>) -> Result<Self, Error> {
        match raw {
            None => Ok(Self {
                min_score: DEFAULT_MIN_SCORE,
                from_env: false,
            }),
            Some(r) => Ok(Self {
                min_score: parse_min_score(r)?,
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
            format!(
                "passage search default min_score: {} ({ENV_VAR})",
                self.min_score
            )
        } else {
            format!(
                "passage search default min_score: {} ({ENV_VAR} unset)",
                self.min_score
            )
        }
    }
}

static DEFAULT: OnceLock<f64> = OnceLock::new();

/// Installs the process-wide default. Called once from `main` before the socket binds. A second
/// call is ignored (returns `false`).
pub fn install(min_score: f64) -> bool {
    DEFAULT.set(min_score).is_ok()
}

/// The effective default `min_score`: the installed value, or 0.5 if none was installed.
pub fn default_min_score() -> f64 {
    DEFAULT.get().copied().unwrap_or(DEFAULT_MIN_SCORE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_is_half() {
        let c = SearchConfig::from_var(None).unwrap();
        assert_eq!(c.min_score, 0.5);
        assert!(!c.from_env);
        assert!(c.describe().contains("0.5"));
        assert!(c.describe().contains("unset"));
    }

    #[test]
    fn valid_values() {
        assert_eq!(parse_min_score("0.7").unwrap(), 0.7);
        assert_eq!(parse_min_score("0").unwrap(), 0.0);
        assert_eq!(parse_min_score("1").unwrap(), 1.0);
        assert_eq!(parse_min_score("  0.65 ").unwrap(), 0.65);
        let c = SearchConfig::from_var(Some("0.7")).unwrap();
        assert!(c.from_env);
        assert!(c.describe().contains("0.7 (LCG_SEARCH_MIN_SCORE)"));
    }

    #[test]
    fn clamps_out_of_range() {
        assert_eq!(parse_min_score("1.5").unwrap(), 1.0);
        assert_eq!(parse_min_score("-0.2").unwrap(), 0.0);
        assert_eq!(parse_min_score("1e9").unwrap(), 1.0);
    }

    #[test]
    fn rejects_invalid_values() {
        for bad in [
            "", "   ", "abc", "NaN", "nan", "inf", "-inf", "Infinity", "infinity", "0.5x", "1,5",
        ] {
            let e = parse_min_score(bad).expect_err(bad);
            let msg = e.to_string();
            assert!(msg.contains("LCG_SEARCH_MIN_SCORE"), "{bad}: {msg}");
            assert!(msg.contains(&format!("\"{bad}\"")), "{bad}: {msg}");
        }
    }

    #[test]
    fn empty_is_error_not_unset() {
        assert!(SearchConfig::from_var(Some("")).is_err());
    }
}
