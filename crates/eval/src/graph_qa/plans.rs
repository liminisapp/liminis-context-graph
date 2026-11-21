//! Walk plans: the planner's output, the recorded plans file (`e13-v5-plans.json`), validation, and
//! the translation of a plan into a `knowledge_walk` request (FR-015..FR-017).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::retrieval::lcg::EVAL_GROUP;

/// One planned hop: candidate relation types, what the hop looks for, and its direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanHop {
    pub types: Vec<String>,
    pub phrase: String,
    #[serde(default = "any")]
    pub direction: String,
}

fn any() -> String {
    "any".into()
}

/// `{route, start, hops, list, history, answer_type}` (`PLAN3_SCHEMA`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// `graph` or `passages`.
    pub route: String,
    pub start: String,
    #[serde(default)]
    pub hops: Vec<PlanHop>,
    #[serde(default)]
    pub list: bool,
    #[serde(default)]
    pub history: bool,
    #[serde(default)]
    pub answer_type: Option<String>,
}

/// A recorded plans file (the probe's `v5-plans.json` format).
#[derive(Debug, Clone, Deserialize)]
pub struct PlansFile {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub ontology: Option<String>,
    #[serde(default)]
    pub planner_prompt: Option<String>,
    /// Plans keyed by question id. A `null` entry is a plan the planner failed to produce.
    pub plans: BTreeMap<String, Value>,
    /// The probe's per-question `route-planned5` scores, if recorded.
    #[serde(default)]
    pub route_planned5_scores: Option<BTreeMap<String, f64>>,
}

pub fn load_plans(path: &Path) -> Result<PlansFile, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("--plans {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("--plans {}: not a plans file: {e}", path.display()))
}

impl PlansFile {
    /// Plan ids that name no question in `ids` (ignored; reported as warnings).
    pub fn unknown_ids(&self, ids: &BTreeSet<&str>) -> Vec<String> {
        self.plans
            .keys()
            .filter(|k| !ids.contains(k.as_str()))
            .cloned()
            .collect()
    }
}

/// The walk `mode` for a plan: history if `history`, else list if `list`, else single.
pub fn walk_mode(p: &Plan) -> &'static str {
    if p.history {
        "history"
    } else if p.list {
        "list"
    } else {
        "single"
    }
}

/// Parses and validates a plan. `relation_types` and `entity_types` are the workspace ontology's
/// relation and entity type names; an empty set (a free-form workspace) skips that check. An
/// `answer_type` must be an entity type or `value` (no type filter). `Err` is a plan error: the
/// question is answered from an empty walk context and counted, never re-planned, and the run goes
/// on.
pub fn validate(
    raw: &Value,
    relation_types: &BTreeSet<String>,
    entity_types: &BTreeSet<String>,
) -> Result<Plan, String> {
    if raw.is_null() {
        return Err("no plan".into());
    }
    let p: Plan =
        serde_json::from_value(raw.clone()).map_err(|e| format!("unparseable plan: {e}"))?;
    if p.route != "graph" && p.route != "passages" {
        return Err(format!("unknown route {:?}", p.route));
    }
    if p.start.trim().is_empty() {
        return Err("empty start".into());
    }
    if let Some(at) = p.answer_type.as_deref().map(str::trim) {
        if !at.is_empty() && at != "value" && !entity_types.is_empty() && !entity_types.contains(at)
        {
            return Err(format!(
                "answer_type {at} is neither an ontology entity type nor \"value\""
            ));
        }
    }
    // A passages-routed plan may carry no hops (the walk then reads nothing, as in the probe's
    // `facts_ctx([])`); a graph-routed one must have at least one.
    if p.hops.is_empty() && p.route == "graph" {
        return Err("no hops".into());
    }
    for (i, h) in p.hops.iter().enumerate() {
        if !matches!(h.direction.as_str(), "out" | "in" | "any") {
            return Err(format!(
                "hop {}: invalid direction {:?}",
                i + 1,
                h.direction
            ));
        }
        if h.phrase.trim().is_empty() {
            return Err(format!("hop {}: empty phrase", i + 1));
        }
        if !relation_types.is_empty() {
            if let Some(t) = h.types.iter().find(|t| !relation_types.contains(*t)) {
                return Err(format!(
                    "hop {}: relation type {t} is not in the ontology",
                    i + 1
                ));
            }
        }
    }
    Ok(p)
}

/// The `knowledge_walk` request for `plan` (soft direction, as route v5).
pub fn walk_request(plan: &Plan, question: &str) -> Value {
    let hops: Vec<Value> = plan
        .hops
        .iter()
        .map(|h| json!({"relation_types": h.types, "phrase": h.phrase, "direction": h.direction}))
        .collect();
    let mut r = json!({
        "start": plan.start,
        "group_ids": [EVAL_GROUP],
        "mode": walk_mode(plan),
        "direction_mode": "soft",
        "hops": hops,
        "query": question,
    });
    if let Some(t) = &plan.answer_type {
        r["answer_type"] = json!(t);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rels() -> BTreeSet<String> {
        ["OWNS", "REPORTS_TO"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn ents() -> BTreeSet<String> {
        ["Person", "Project"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn answer_type_must_be_an_entity_type_or_value() {
        let base = json!({"route": "graph", "start": "X", "hops": [{"types": ["OWNS"], "phrase": "p", "direction": "out"}], "list": false, "history": false});
        for ok in [json!("Person"), json!("value"), json!(null), json!("")] {
            let mut b = base.clone();
            b["answer_type"] = ok.clone();
            assert!(validate(&b, &rels(), &ents()).is_ok(), "{ok}");
        }
        let mut b = base.clone();
        b["answer_type"] = json!("Spaceship");
        assert!(validate(&b, &rels(), &ents())
            .unwrap_err()
            .contains("Spaceship"));
        // A free-form workspace has no entity types to check against.
        assert!(validate(&b, &BTreeSet::new(), &BTreeSet::new()).is_ok());
    }

    #[test]
    fn a_good_plan_becomes_a_soft_walk_request_the_service_accepts() {
        let raw = json!({"route": "graph", "start": "Tern Core", "hops": [
            {"types": ["OWNS"], "phrase": "owner", "direction": "in"},
            {"types": ["REPORTS_TO"], "phrase": "manager", "direction": "out"}],
            "list": false, "history": false, "answer_type": "Person"});
        let p = validate(&raw, &rels(), &ents()).unwrap();
        let r = walk_request(&p, "Who does the owner of Tern Core report to?");
        assert_eq!(r["mode"], "single");
        assert_eq!(r["direction_mode"], "soft");
        assert_eq!(r["hops"][0]["relation_types"], json!(["OWNS"]));
        assert_eq!(r["group_ids"], json!([EVAL_GROUP]));
        lcg_core::walk::parse_request(&r).unwrap();
        let mut h = p.clone();
        h.list = true;
        assert_eq!(walk_mode(&h), "list");
        h.history = true;
        assert_eq!(walk_mode(&h), "history");
    }

    #[test]
    fn malformed_plans_are_errors() {
        let base = json!({"route": "graph", "start": "X", "hops": [{"types": ["OWNS"], "phrase": "p", "direction": "out"}], "list": false, "history": false, "answer_type": "value"});
        assert!(validate(&base, &rels(), &ents()).is_ok());
        assert!(validate(&Value::Null, &rels(), &ents()).is_err());
        assert!(validate(&json!("nonsense"), &rels(), &ents()).is_err());
        let mut b = base.clone();
        b["route"] = json!("maybe");
        assert!(validate(&b, &rels(), &ents())
            .unwrap_err()
            .contains("route"));
        let mut b = base.clone();
        b["hops"][0]["types"] = json!(["MADE_UP"]);
        assert!(validate(&b, &rels(), &ents())
            .unwrap_err()
            .contains("MADE_UP"));
        // A free-form workspace has no types to check against.
        assert!(validate(&b, &BTreeSet::new(), &BTreeSet::new()).is_ok());
        let mut b = base.clone();
        b["hops"][0]["direction"] = json!("up");
        assert!(validate(&b, &rels(), &ents()).is_err());
        let mut b = base;
        b["hops"] = json!([]);
        assert!(validate(&b, &rels(), &ents())
            .unwrap_err()
            .contains("no hops"));
        b["route"] = json!("passages");
        assert!(
            validate(&b, &rels(), &ents()).is_ok(),
            "a passages plan needs no hops"
        );
    }
}
