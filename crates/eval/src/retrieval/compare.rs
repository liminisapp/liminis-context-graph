//! `lcg-eval retrieval --compare <report.json>` (issue #872): refuse to compare runs that did not
//! measure the same thing, otherwise print per-config and per-category nDCG@k deltas.
//!
//! Mirrors `graph_integrity`'s compare (same exit codes: 1 unreadable/invalid file, 2 incompatible
//! inputs), but works on untyped JSON because `Report` is write-only. Both sides are read in the
//! same shape: the earlier file as written, the current run through `Report::to_json`.
//!
//! * **Hard inputs** — `inputs.corpus`, `inputs.questions`, `k`. Any difference refuses the
//!   comparison, naming every differing input, and no deltas are produced.
//! * **Soft inputs** — the other digests, the service build and the ontology hash. A difference is
//!   listed in a banner above the deltas; `--strict` refuses on it instead.
//! * `none` equals `none`. `unknown` equals nothing under `--strict`, and otherwise differs from
//!   everything but `unknown` (two unidentifiable inputs are not a *change*, only a gap).
//! * A report with no `inputs` block (an older run) is refused with a message to re-run it.
//! * A config or category present in one report only shows `n/a` for the missing side and is not
//!   itself a mismatch.

use std::collections::BTreeSet;
use std::fmt::Write;

use serde_json::Value;

use super::inputs::{short, KEYS, NONE, SCHEMA_VERSION, UNKNOWN};

#[derive(Debug, PartialEq, Eq)]
pub enum CompareError {
    /// The earlier report has no `inputs` block (exit 2): re-run it.
    NoInputs(String),
    /// A hard input differs, or `--strict` met a soft one (exit 2).
    Incompatible(String),
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompareError::NoInputs(m) | CompareError::Incompatible(m) => write!(f, "{m}"),
        }
    }
}

/// One soft input that differs between the two reports. Digests are stored in full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftChange {
    pub name: String,
    pub before: String,
    pub after: String,
}

/// nDCG@k for one config in one scope (`overall` or a category label).
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaRow {
    pub config: String,
    pub scope: String,
    pub before: Option<f64>,
    pub after: Option<f64>,
    /// `after - before`; `None` when either side is missing.
    pub delta: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub k: u64,
    pub soft_changes: Vec<SoftChange>,
    pub rows: Vec<DeltaRow>,
}

/// Parses an earlier `report.json`. `Err` is an unreadable/invalid file (exit 1): it must be a JSON
/// object with `meta` and a `configs` array.
pub fn parse_previous(text: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if !v.is_object() || !v["meta"].is_object() || !v["configs"].is_array() {
        return Err(
            "expected a retrieval report.json (an object with `meta` and `configs`)".into(),
        );
    }
    Ok(v)
}

/// The cheap pre-spend check: the earlier report must carry an `inputs` block of a schema this
/// build understands. Run before the service starts, so an old report costs nothing.
pub fn check_has_inputs(prev: &Value) -> Result<(), CompareError> {
    match prev.get("inputs") {
        Some(i) if i.is_object() => {
            let v = i["schema_version"].as_u64();
            if v == Some(SCHEMA_VERSION) {
                Ok(())
            } else {
                Err(CompareError::Incompatible(format!(
                    "the earlier report's `inputs` block has schema version {}, this build reads \
                     version {SCHEMA_VERSION}; re-run it with this build",
                    v.map_or("none".to_string(), |v| v.to_string())
                )))
            }
        }
        _ => Err(CompareError::NoInputs(
            "the earlier report has no `inputs` block (it predates input digests), so it cannot \
             be shown to be comparable; re-run it with this build"
                .into(),
        )),
    }
}

fn input_of<'a>(report: &'a Value, key: &str) -> &'a str {
    report["inputs"][key].as_str().unwrap_or(UNKNOWN)
}

/// Whether two soft values differ. `unknown` never matches under `strict`; otherwise two
/// `unknown`s are a gap, not a change.
fn soft_differs(a: &str, b: &str, strict: bool) -> bool {
    if a == UNKNOWN && b == UNKNOWN {
        return strict;
    }
    a != b
}

fn ontology_of(report: &Value) -> String {
    report["meta"]["ontology_hash"]
        .as_str()
        .unwrap_or(NONE)
        .to_string()
}

fn build_of(report: &Value) -> String {
    report["meta"]["lcg_build"]
        .as_str()
        .unwrap_or(UNKNOWN)
        .to_string()
}

/// Compares an earlier report against the current one.
pub fn compare(prev: &Value, cur: &Value, strict: bool) -> Result<Comparison, CompareError> {
    check_has_inputs(prev)?;

    // Hard inputs: all differences are named in the one error. A missing value cannot be shown
    // equal, so it differs.
    let mut hard = Vec::new();
    for key in ["corpus", "questions"] {
        let (a, b) = (input_of(prev, key), input_of(cur, key));
        if a == UNKNOWN || a != b {
            hard.push(format!(
                "{key} (earlier {}, this run {})",
                short(a),
                short(b)
            ));
        }
    }
    let (ka, kb) = (prev["meta"]["k"].as_u64(), cur["meta"]["k"].as_u64());
    if ka.is_none() || ka != kb {
        let f = |k: Option<u64>| k.map_or("missing".to_string(), |k| k.to_string());
        hard.push(format!("k (earlier {}, this run {})", f(ka), f(kb)));
    }
    if !hard.is_empty() {
        return Err(CompareError::Incompatible(format!(
            "the runs measured different things; deltas would be meaningless. Differing hard \
             input(s): {}",
            hard.join("; ")
        )));
    }

    // Soft inputs, in report order.
    let mut soft_changes = Vec::new();
    let mut push = |name: &str, a: String, b: String| {
        if soft_differs(&a, &b, strict) {
            soft_changes.push(SoftChange {
                name: name.to_string(),
                before: a,
                after: b,
            });
        }
    };
    for key in KEYS
        .iter()
        .filter(|k| !matches!(**k, "corpus" | "questions"))
    {
        push(
            key,
            input_of(prev, key).to_string(),
            input_of(cur, key).to_string(),
        );
    }
    push("service_build", build_of(prev), build_of(cur));
    push("ontology_hash", ontology_of(prev), ontology_of(cur));
    if strict && !soft_changes.is_empty() {
        return Err(CompareError::Incompatible(format!(
            "--strict: soft input(s) differ (or are unknown): {}",
            soft_changes
                .iter()
                .map(|c| format!(
                    "{} (earlier {}, this run {})",
                    c.name,
                    short(&c.before),
                    short(&c.after)
                ))
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }

    Ok(Comparison {
        k: kb.unwrap_or(0),
        soft_changes,
        rows: delta_rows(prev, cur),
    })
}

fn config_ids(report: &Value) -> Vec<String> {
    report["configs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn config_of<'a>(report: &'a Value, id: &str) -> Option<&'a Value> {
    report["configs"]
        .as_array()?
        .iter()
        .find(|c| c["id"].as_str() == Some(id))
}

/// nDCG@k of a config in a scope; `None` for a missing or skipped config or category.
fn ndcg(report: &Value, config: &str, scope: &str) -> Option<f64> {
    let c = config_of(report, config)?;
    if scope == "overall" {
        c["overall"]["ndcg"].as_f64()
    } else {
        c["by_category"][scope]["ndcg"].as_f64()
    }
}

fn categories(report: &Value, config: &str) -> Vec<String> {
    config_of(report, config)
        .and_then(|c| c["by_category"].as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn delta_rows(prev: &Value, cur: &Value) -> Vec<DeltaRow> {
    // This run's configs in order, then any that only the earlier report has.
    let mut ids = config_ids(cur);
    for id in config_ids(prev) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let mut rows = Vec::new();
    for id in ids {
        let cats: BTreeSet<String> = categories(prev, &id)
            .into_iter()
            .chain(categories(cur, &id))
            .collect();
        let scopes = std::iter::once("overall".to_string()).chain(cats);
        for scope in scopes {
            let (before, after) = (ndcg(prev, &id, &scope), ndcg(cur, &id, &scope));
            rows.push(DeltaRow {
                config: id.clone(),
                scope,
                before,
                after,
                delta: before.zip(after).map(|(b, a)| a - b),
            });
        }
    }
    rows
}

fn fmt_v(v: Option<f64>) -> String {
    v.map_or("n/a".to_string(), |v| format!("{v:.3}"))
}

pub fn render_compare_markdown(c: &Comparison) -> String {
    let mut s = String::from("# Retrieval comparison\n\n");
    if !c.soft_changes.is_empty() {
        let _ = writeln!(
            s,
            "> **Soft inputs differ.** The runs share corpus, questions and k, but not everything that can move a result; read the deltas with that in mind:"
        );
        for ch in &c.soft_changes {
            let _ = writeln!(
                s,
                "> - `{}`: `{}` → `{}`",
                ch.name,
                short(&ch.before),
                short(&ch.after)
            );
        }
        let _ = writeln!(s);
    }
    let _ = writeln!(
        s,
        "nDCG@{} per config, earlier report → this run. `n/a` means the config or category is absent or skipped on that side.\n",
        c.k
    );
    let _ = writeln!(s, "| config | scope | before | after | delta |");
    let _ = writeln!(s, "|---|---|---|---|---|");
    for r in &c.rows {
        let d = r.delta.map_or("n/a".to_string(), |d| format!("{d:+.3}"));
        let _ = writeln!(
            s,
            "| `{}` | {} | {} | {} | {} |",
            r.config,
            r.scope,
            fmt_v(r.before),
            fmt_v(r.after),
            d
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn h(c: char) -> String {
        c.to_string().repeat(64)
    }

    /// A report as `to_json` writes it, with overridable inputs.
    fn rep(inputs: Value, k: u64, configs: Value) -> Value {
        json!({"meta": {"k": k, "lcg_build": "b1", "ontology_hash": null},
               "inputs": inputs, "configs": configs})
    }

    fn inputs(over: &[(&str, &str)]) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("schema_version".into(), json!(1));
        for k in KEYS {
            m.insert(k.into(), json!(h('a')));
        }
        m.insert("variants".into(), json!("none"));
        m.insert("cassette".into(), json!("none"));
        for (k, v) in over {
            m.insert((*k).into(), json!(v));
        }
        Value::Object(m)
    }

    fn cfg(id: &str, overall: f64, para: f64) -> Value {
        json!({"id": id, "overall": {"ndcg": overall},
               "by_category": {"paraphrase": {"ndcg": para}}})
    }

    fn base(over: &[(&str, &str)]) -> Value {
        rep(
            inputs(over),
            10,
            json!([cfg("a", 0.5, 0.4), cfg("b", 0.6, 0.5)]),
        )
    }

    #[test]
    fn different_questions_hash_is_refused_with_no_deltas() {
        let e = compare(&base(&[]), &base(&[("questions", &h('z'))]), false).unwrap_err();
        let CompareError::Incompatible(m) = e else {
            panic!("{e:?}")
        };
        assert!(m.contains("questions"), "{m}");
        assert!(!m.contains("corpus"), "{m}");
    }

    #[test]
    fn every_differing_hard_input_is_named_in_one_error() {
        let cur = rep(
            inputs(&[("corpus", &h('c')), ("questions", &h('q'))]),
            20,
            json!([]),
        );
        let CompareError::Incompatible(m) = compare(&base(&[]), &cur, false).unwrap_err() else {
            panic!()
        };
        for want in ["corpus", "questions", "k (earlier 10, this run 20)"] {
            assert!(m.contains(want), "{want} missing from {m}");
        }
    }

    #[test]
    fn a_report_without_inputs_is_refused_with_a_rerun_message() {
        let old = json!({"meta": {"k": 10}, "configs": []});
        let e = compare(&old, &base(&[]), false).unwrap_err();
        let CompareError::NoInputs(m) = e else {
            panic!("{e:?}")
        };
        assert!(m.contains("re-run"), "{m}");
        assert!(check_has_inputs(&old).is_err());
        // Also refused under --strict and when it is not an object.
        assert!(compare(
            &json!({"meta": {}, "configs": [], "inputs": 3}),
            &base(&[]),
            true
        )
        .is_err());
    }

    #[test]
    fn an_unknown_inputs_schema_version_is_refused() {
        let mut p = base(&[]);
        p["inputs"]["schema_version"] = json!(2);
        assert!(matches!(
            compare(&p, &base(&[]), false),
            Err(CompareError::Incompatible(m)) if m.contains("schema version 2")
        ));
    }

    #[test]
    fn soft_only_differences_print_deltas_under_a_banner() {
        let cur = base(&[("cassette", &h('c')), ("embedder_model", &h('e'))]);
        let c = compare(&base(&[]), &cur, false).unwrap();
        let names: Vec<&str> = c.soft_changes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["cassette", "embedder_model"]);
        assert!(!c.rows.is_empty());
        let md = render_compare_markdown(&c);
        assert!(md.contains("Soft inputs differ") && md.contains("`cassette`"));
        assert!(md.contains(&format!("`none` → `{}`", &h('c')[..16])));
    }

    #[test]
    fn service_build_and_ontology_are_soft_inputs() {
        let mut cur = base(&[]);
        cur["meta"]["lcg_build"] = json!("b2");
        cur["meta"]["ontology_hash"] = json!("abc");
        let c = compare(&base(&[]), &cur, false).unwrap();
        let names: Vec<&str> = c.soft_changes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["service_build", "ontology_hash"]);
    }

    #[test]
    fn identical_soft_inputs_have_no_banner() {
        let c = compare(&base(&[]), &base(&[]), true).unwrap();
        assert!(c.soft_changes.is_empty());
        assert!(!render_compare_markdown(&c).contains("Soft inputs differ"));
    }

    #[test]
    fn strict_refuses_a_soft_difference_naming_it() {
        let cur = base(&[("service_env", &h('e'))]);
        let CompareError::Incompatible(m) = compare(&base(&[]), &cur, true).unwrap_err() else {
            panic!()
        };
        assert!(m.contains("--strict") && m.contains("service_env"), "{m}");
    }

    #[test]
    fn none_equals_none_but_unknown_is_a_mismatch_only_under_strict() {
        // none vs none: equal even under --strict (base() has none for variants and cassette).
        assert!(compare(&base(&[]), &base(&[]), true).is_ok());
        let unk = base(&[("embedder_model", "unknown")]);
        let lax = compare(&unk, &unk, false).unwrap();
        assert!(
            lax.soft_changes.is_empty(),
            "unknown vs unknown is a gap, not a change"
        );
        let strict = compare(&unk, &unk, true).unwrap_err();
        assert!(matches!(strict, CompareError::Incompatible(m) if m.contains("embedder_model")));
        // unknown against a known digest is a change either way.
        let c = compare(&unk, &base(&[]), false).unwrap();
        assert_eq!(c.soft_changes.len(), 1);
    }

    #[test]
    fn deltas_cover_overall_and_each_category() {
        let prev = base(&[]);
        let cur = rep(
            inputs(&[]),
            10,
            json!([cfg("a", 0.7, 0.4), cfg("b", 0.6, 0.45)]),
        );
        let c = compare(&prev, &cur, false).unwrap();
        let row = |cfg: &str, scope: &str| {
            c.rows
                .iter()
                .find(|r| r.config == cfg && r.scope == scope)
                .unwrap()
        };
        assert!((row("a", "overall").delta.unwrap() - 0.2).abs() < 1e-9);
        assert!((row("b", "paraphrase").delta.unwrap() + 0.05).abs() < 1e-9);
        assert_eq!(c.rows.len(), 4);
        assert_eq!(c.k, 10);
    }

    #[test]
    fn a_config_in_one_report_only_or_skipped_shows_na_and_is_not_a_mismatch() {
        let prev = rep(
            inputs(&[]),
            10,
            json!([cfg("a", 0.5, 0.4), cfg("only-before", 0.3, 0.3),
                   {"id": "qmd", "skipped": "absent"}]),
        );
        let cur = rep(
            inputs(&[]),
            10,
            json!([
                cfg("a", 0.5, 0.4),
                cfg("only-after", 0.9, 0.9),
                cfg("qmd", 0.8, 0.8)
            ]),
        );
        let c = compare(&prev, &cur, true).unwrap();
        let rows_of = |id: &str| c.rows.iter().filter(|r| r.config == id).collect::<Vec<_>>();
        assert!(rows_of("only-before")
            .iter()
            .all(|r| r.after.is_none() && r.delta.is_none()));
        assert!(rows_of("only-after")
            .iter()
            .all(|r| r.before.is_none() && r.delta.is_none()));
        // Skipped earlier, scored now: n/a on the earlier side.
        assert!(rows_of("qmd")
            .iter()
            .all(|r| r.before.is_none() && r.after.is_some()));
        let md = render_compare_markdown(&c);
        assert!(
            md.contains("| `only-before` | overall | 0.300 | n/a | n/a |"),
            "{md}"
        );
    }

    #[test]
    fn parse_previous_rejects_non_reports() {
        assert!(parse_previous("not json").is_err());
        assert!(parse_previous("[]").is_err());
        assert!(parse_previous(r#"{"meta": {}}"#).is_err());
        assert!(parse_previous(r#"{"meta": {}, "configs": []}"#).is_ok());
    }
}
