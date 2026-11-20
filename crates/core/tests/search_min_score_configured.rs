//! Issue #812: `LCG_SEARCH_MIN_SCORE=0.7` (installed once, as `main` does at startup) becomes the
//! default for requests that omit `min_score`, in both `vector` and `hybrid` modes. An explicit
//! request value — including `0.0` — always wins.
//!
//! Own integration binary: `install` is once-per-process. Clamping and parse cases live in the
//! `search_config` unit tests.

mod search_min_score_support;
use search_min_score_support::*;

use serde_json::json;

#[tokio::test]
async fn configured_default_applies_and_explicit_wins() {
    let cfg = lcg_core::search_config::SearchConfig::from_var(Some("0.7")).unwrap();
    assert!(lcg_core::search_config::install(cfg.min_score));

    let (state, _db, _d) = make_state(embedder(&[("zzqueryzz", graded_query())]), &four_passages());
    for mode in ["vector", "hybrid"] {
        // Omitted: floor 0.7 -> only the 0.75 passage (the 0.55 one passed at 0.5).
        assert_eq!(
            uuids(&search(&state, params(mode, None)).await),
            vec!["p75"],
            "{mode}"
        );
        // Explicit 0.0 returns everything (not treated as omitted).
        assert_eq!(
            search(&state, params(mode, Some(json!(0.0)))).await.len(),
            4,
            "{mode}"
        );
        // Explicit 0.3 uses 0.3.
        assert_eq!(
            uuids(&search(&state, params(mode, Some(json!(0.3)))).await),
            vec!["p35", "p55", "p75"],
            "{mode}"
        );
        // Explicit values above the default also win.
        assert!(
            search(&state, params(mode, Some(json!(0.9))))
                .await
                .is_empty(),
            "{mode}"
        );
    }
    let st = status(&state).await;
    assert_eq!(st["search_min_score_default"], json!(0.7));
}
