//! Process-wide configuration and service handles for the optional passage-search stages
//! (issue #796): rerank and query expansion.
//!
//! Held in [`crate::app_state::AppState::search_stages`], which starts empty. A process that
//! configures nothing leaves it empty, and a call that opts in anyway degrades (`not_configured`)
//! rather than failing. Both stages are **off by default**: `rerank_default` only ever turns
//! rerank on as a workspace default, and expansion has no default at all.

use std::sync::Arc;

use crate::{
    error::Error, passage_rerank::BlendConfig, query_expand::QueryExpander, rerank::Reranker,
};

/// Default number of first-stage candidates the reranker rescores.
pub const DEFAULT_RERANK_CANDIDATES: usize = 50;
/// Upper bound on the rescored head, per call or via configuration.
pub const MAX_RERANK_CANDIDATES: usize = 50;

/// Workspace-level knobs read once at startup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchStagesConfig {
    /// `LCG_RERANK=on`: rerank is the default for calls that do not say otherwise.
    pub rerank_default: bool,
    /// `LCG_RERANK_CANDIDATES`, clamped to `1..=MAX_RERANK_CANDIDATES`.
    pub rerank_candidates: usize,
    pub blend: BlendConfig,
}

impl Default for SearchStagesConfig {
    fn default() -> Self {
        Self {
            rerank_default: false,
            rerank_candidates: DEFAULT_RERANK_CANDIDATES,
            blend: BlendConfig::default(),
        }
    }
}

fn parse_bool(name: &str, v: &str) -> Result<bool, Error> {
    match v.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Ok(true),
        "off" | "false" | "0" | "no" | "" => Ok(false),
        other => Err(Error::Config(format!(
            "{name}: expected on|off, got {other:?}"
        ))),
    }
}

fn parse_f64(name: &str, v: &str) -> Result<f64, Error> {
    v.trim()
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite() && *x >= 0.0)
        .ok_or_else(|| Error::Config(format!("{name}: expected a non-negative number, got {v:?}")))
}

impl SearchStagesConfig {
    /// Reads `LCG_RERANK`, `LCG_RERANK_CANDIDATES`, `LCG_RERANK_SUPERSEDED_FACTOR`,
    /// `LCG_RERANK_RECENCY_WEIGHT` and `LCG_RERANK_RANK_WEIGHT_TOP|MID|TAIL` through `get` (injectable for tests). Unset ⇒ defaults; a
    /// malformed value is a configuration error.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, Error> {
        // An empty or whitespace-only value counts as unset (a docker-compose `.env` template often
        // leaves variables set to `""`), the same rule the service applies to its other variables.
        let get = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        let mut c = Self::default();
        if let Some(v) = get("LCG_RERANK") {
            c.rerank_default = parse_bool("LCG_RERANK", &v)?;
        }
        if let Some(v) = get("LCG_RERANK_CANDIDATES") {
            let n: usize = v.trim().parse().map_err(|_| {
                Error::Config(format!(
                    "LCG_RERANK_CANDIDATES: expected an integer, got {v:?}"
                ))
            })?;
            c.rerank_candidates = n.clamp(1, MAX_RERANK_CANDIDATES);
            if c.rerank_candidates != n {
                eprintln!(
                    "liminis-context-graph: LCG_RERANK_CANDIDATES={n} is outside 1..={MAX_RERANK_CANDIDATES}; \
                     using {}",
                    c.rerank_candidates
                );
            }
        }
        if let Some(v) = get("LCG_RERANK_SUPERSEDED_FACTOR") {
            let f = parse_f64("LCG_RERANK_SUPERSEDED_FACTOR", &v)?;
            // A demotion multiplier: above 1 it would boost superseded notes over current ones.
            if f > 1.0 {
                return Err(Error::Config(format!(
                    "LCG_RERANK_SUPERSEDED_FACTOR: expected a number in 0..=1 (a demotion), got {v:?}"
                )));
            }
            c.blend.superseded_factor = f;
        }
        if let Some(v) = get("LCG_RERANK_RECENCY_WEIGHT") {
            c.blend.recency_weight = parse_f64("LCG_RERANK_RECENCY_WEIGHT", &v)?;
        }
        // First-stage weights of the position-aware blend: a mixing proportion, so 0..=1.
        for (name, slot) in [
            (
                "LCG_RERANK_RANK_WEIGHT_TOP",
                &mut c.blend.rank_weight_top as &mut f64,
            ),
            ("LCG_RERANK_RANK_WEIGHT_MID", &mut c.blend.rank_weight_mid),
            ("LCG_RERANK_RANK_WEIGHT_TAIL", &mut c.blend.rank_weight_tail),
        ] {
            if let Some(v) = get(name) {
                let w = parse_f64(name, &v)?;
                if w > 1.0 {
                    return Err(Error::Config(format!(
                        "{name}: expected a weight in 0..=1, got {v:?}"
                    )));
                }
                *slot = w;
            }
        }
        Ok(c)
    }

    pub fn from_env() -> Result<Self, Error> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }
}

/// The configured stages: workspace config plus whichever service handles resolved at startup.
pub struct SearchStages {
    pub config: SearchStagesConfig,
    pub reranker: Option<Arc<dyn Reranker>>,
    pub expander: Option<Arc<dyn QueryExpander>>,
}

impl SearchStages {
    pub fn new(
        config: SearchStagesConfig,
        reranker: Option<Arc<dyn Reranker>>,
        expander: Option<Arc<dyn QueryExpander>>,
    ) -> Self {
        Self {
            config,
            reranker,
            expander,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> Result<SearchStagesConfig, Error> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        SearchStagesConfig::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn empty_or_blank_values_count_as_unset() {
        let c = cfg(&[
            ("LCG_RERANK", ""),
            ("LCG_RERANK_CANDIDATES", "  "),
            ("LCG_RERANK_SUPERSEDED_FACTOR", ""),
            ("LCG_RERANK_RECENCY_WEIGHT", ""),
            ("LCG_RERANK_RANK_WEIGHT_TOP", ""),
        ])
        .unwrap();
        assert!(!c.rerank_default);
        assert_eq!(c.rerank_candidates, DEFAULT_RERANK_CANDIDATES);
        assert_eq!(c.blend, BlendConfig::default());
    }

    #[test]
    fn defaults_are_off() {
        let c = cfg(&[]).unwrap();
        assert!(!c.rerank_default);
        assert_eq!(c.rerank_candidates, DEFAULT_RERANK_CANDIDATES);
        assert_eq!(c.blend, BlendConfig::default());
    }

    #[test]
    fn parses_and_clamps() {
        let c = cfg(&[
            ("LCG_RERANK", "on"),
            ("LCG_RERANK_CANDIDATES", "500"),
            ("LCG_RERANK_SUPERSEDED_FACTOR", "0.25"),
            ("LCG_RERANK_RECENCY_WEIGHT", "0"),
        ])
        .unwrap();
        assert!(c.rerank_default);
        assert_eq!(c.rerank_candidates, MAX_RERANK_CANDIDATES);
        assert_eq!(c.blend.superseded_factor, 0.25);
        assert_eq!(c.blend.recency_weight, 0.0);
        assert_eq!(
            cfg(&[("LCG_RERANK_CANDIDATES", "0")])
                .unwrap()
                .rerank_candidates,
            1
        );
    }

    #[test]
    fn rejects_malformed_values() {
        assert!(cfg(&[("LCG_RERANK", "maybe")]).is_err());
        assert!(cfg(&[("LCG_RERANK_CANDIDATES", "many")]).is_err());
        assert!(cfg(&[("LCG_RERANK_SUPERSEDED_FACTOR", "-1")]).is_err());
        // Above 1 would promote superseded notes rather than demote them.
        assert!(cfg(&[("LCG_RERANK_SUPERSEDED_FACTOR", "2")]).is_err());
        assert!(cfg(&[("LCG_RERANK_SUPERSEDED_FACTOR", "1")]).is_ok());
        assert!(cfg(&[("LCG_RERANK_RECENCY_WEIGHT", "nan")]).is_err());
        assert!(cfg(&[("LCG_RERANK_RANK_WEIGHT_TOP", "1.5")]).is_err());
        assert!(cfg(&[("LCG_RERANK_RANK_WEIGHT_MID", "-0.1")]).is_err());
    }

    #[test]
    fn rank_weights_are_configurable() {
        let c = cfg(&[
            ("LCG_RERANK_RANK_WEIGHT_TOP", "0.9"),
            ("LCG_RERANK_RANK_WEIGHT_TAIL", "0"),
        ])
        .unwrap();
        assert_eq!(c.blend.rank_weight_top, 0.9);
        assert_eq!(
            c.blend.rank_weight_mid,
            BlendConfig::default().rank_weight_mid
        );
        assert_eq!(c.blend.rank_weight_tail, 0.0);
    }
}
