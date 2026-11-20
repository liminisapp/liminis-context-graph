//! Issue #812: with `LCG_SEARCH_MIN_SCORE` unset (nothing installed), an omitted `min_score`
//! filters at 0.5 exactly as before, and `knowledge_status` reports 0.5.

mod search_min_score_support;
use search_min_score_support::*;

use serde_json::json;

#[tokio::test]
async fn unset_default_is_half() {
    let (state, _db, _d) = make_state(embedder(&[("zzqueryzz", graded_query())]), &four_passages());
    for mode in ["vector", "hybrid"] {
        // Omitted: floor 0.5 -> cosines 0.75 and 0.55 pass, 0.35 / 0.11 do not.
        assert_eq!(
            uuids(&search(&state, params(mode, None)).await),
            vec!["p55", "p75"],
            "{mode}"
        );
        // Null and non-numeric fall back silently to the default (existing behaviour).
        for odd in [json!(null), json!("high")] {
            assert_eq!(
                uuids(&search(&state, params(mode, Some(odd))).await),
                vec!["p55", "p75"],
                "{mode}"
            );
        }
        // Explicit values still win.
        assert_eq!(
            search(&state, params(mode, Some(json!(0.0)))).await.len(),
            4,
            "{mode}"
        );
        assert_eq!(
            uuids(&search(&state, params(mode, Some(json!(0.3)))).await),
            vec!["p35", "p55", "p75"],
            "{mode}"
        );
    }
    let st = status(&state).await;
    assert_eq!(st["search_min_score_default"], json!(0.5));
}
