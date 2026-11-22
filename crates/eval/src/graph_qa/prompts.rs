//! The planner and reader prompts and the reader contexts, ported word for word from the E13 probe
//! (`answer-eval.py`: `PLAN3_SYS` / `PLAN3_SCHEMA`, `READ_SYS` / `READ_SCHEMA`, `facts_ctx_typed`,
//! `pas_ctx`). Each prompt carries a version that the report records (FR-018).

use serde_json::{json, Value};

/// The v5 planner (`PLAN3_SYS` + `PLAN3_SCHEMA`).
pub const PLANNER_PROMPT_VERSION: &str = "e13-plan-v5";
/// The E13 reader (`READ_SYS` + `READ_SCHEMA`).
pub const READER_PROMPT_VERSION: &str = "e13-read-v1";
/// E13's planner and reader model.
pub const DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001";
pub const PLANNER_MAX_TOKENS: u32 = 1024;
pub const READER_MAX_TOKENS: u32 = 1024;

pub const READ_SYS: &str = "Answer the question using only the context given. Reply with the answer items only: each a short string such as a name, a value or a date, with no explanation. If the question asks for several items, list them all; if it asks for values in order, list them oldest first. If the context does not contain the answer, return an empty list.";

pub fn read_schema() -> Value {
    json!({"type": "object", "properties": {"answers": {"type": "array", "items": {"type": "string"}}}, "required": ["answers"]})
}

/// The relation types (name, description) and entity type names of a `knowledge_get_ontology`
/// view, in file order.
pub fn ontology_menu(view: &Value) -> (Vec<(String, String)>, Vec<String>) {
    let rels = view["relation_types"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some((
                        r["name"].as_str()?.to_string(),
                        r["description"].as_str().unwrap_or("").to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let ents = view["entity_types"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (rels, ents)
}

/// `PLAN3_SYS` rendered from the workspace's ontology: the planner sees the question and this
/// menu only — no family, no gold.
pub fn planner_system(view: &Value) -> String {
    let (rels, ents) = ontology_menu(view);
    let rt: Vec<String> = rels.iter().map(|(k, v)| format!("- {k}: {v}")).collect();
    let mut s = String::from(
        "You plan how to answer a question from a team's notebook. The notebook has a knowledge graph whose \
         edges are typed with the relation types below, and a passage index over the notes themselves.\n\n\
         Relation types:\n",
    );
    s.push_str(&rt.join("\n"));
    s.push_str("\n\n");
    s.push_str(
        "Choose route 'graph' when answering means following relations from a named entity to other entities \
         (one or more hops), and 'passages' when the answer is an attribute of the named entity itself, or how \
         that attribute changed over time. For 'graph', give the start entity exactly as the question names it, \
         then the hops in order from the start towards the answer. Each hop lists 1-3 relation types that could \
         carry it (edge direction is ignored) and a short phrase for what that hop looks for. Set list=true when \
         the question expects several answers, history=true when it asks about earlier values.",
    );
    s.push_str(" Also give answer_type: the entity type of the answer, one of: ");
    s.push_str(&ents.join(", "));
    s.push_str(
        ", or 'value' when the answer is a literal such as a price, date or percentage. The hops must lead all the \
         way from the start to an entity of that type.",
    );
    s.push_str(
        " For each hop also give direction: 'out' when the current entity is the subject of the relation \
         as its type reads (X REPORTS_TO its manager: from X to the manager is 'out'), 'in' when it is the object \
         (from a manager to the people who report to them is 'in'), or 'any' when unsure.",
    );
    s
}

/// `PLAN3_SCHEMA` for the workspace's entity types.
pub fn planner_schema(view: &Value) -> Value {
    let (_, mut ents) = ontology_menu(view);
    ents.push("value".into());
    json!({"type": "object", "properties": {
        "route": {"type": "string", "enum": ["graph", "passages"]}, "start": {"type": "string"},
        "hops": {"type": "array", "items": {"type": "object", "properties": {
            "types": {"type": "array", "items": {"type": "string"}}, "phrase": {"type": "string"},
            "direction": {"type": "string", "enum": ["out", "in", "any"]}},
            "required": ["types", "phrase", "direction"]}},
        "list": {"type": "boolean"}, "history": {"type": "boolean"},
        "answer_type": {"type": "string", "enum": ents}},
        "required": ["route", "start", "hops", "list", "history", "answer_type"]})
}

pub fn planner_user(question: &str) -> String {
    format!("Question: {question}")
}

pub fn reader_user(context: &str, question: &str) -> String {
    format!("{context}\n\nQuestion: {question}")
}

/// The walk's context: the probe's header plus `facts_text` (`(none found)` when empty).
pub fn facts_context(facts_text: &str) -> String {
    let body = facts_text.trim_end_matches('\n');
    format!(
        "Facts from the knowledge graph:\n{}",
        if body.trim().is_empty() {
            "(none found)"
        } else {
            body
        }
    )
}

/// One passage for the reader: its note, the first 10 characters of its `valid_at`, and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassageItem {
    pub note: String,
    pub valid_at: String,
    pub text: String,
}

/// `pas_ctx`: `Passages from the notes:` then `[note, date]\ntext` blocks.
pub fn passages_context(items: &[PassageItem]) -> String {
    let blocks: Vec<String> = items
        .iter()
        .map(|p| {
            let date: String = p.valid_at.chars().take(10).collect();
            format!("[{}, {date}]\n{}", p.note, p.text)
        })
        .collect();
    format!("Passages from the notes:\n\n{}", blocks.join("\n\n"))
}

/// The JSON object in a model response: the whole text, else the span from the first `{` to the
/// last `}` (an unconstrained fallback response may carry prose or a fence around it).
pub fn parse_json_object(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        if v.is_object() {
            return Some(v);
        }
    }
    let (a, b) = (text.find('{')?, text.rfind('}')?);
    (a < b)
        .then(|| serde_json::from_str::<Value>(&text[a..=b]).ok())
        .flatten()
        .filter(Value::is_object)
}

/// The reader's answers, or `None` when the response does not parse.
pub fn parse_answers(text: &str) -> Option<Vec<String>> {
    let v = parse_json_object(text)?;
    let a = v["answers"].as_array()?;
    Some(
        a.iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Value {
        json!({"has_ontology": true, "entity_types": [{"name": "Person"}, {"name": "Project"}],
               "relation_types": [{"name": "OWNS", "description": "A person owns a project."},
                                  {"name": "REPORTS_TO", "description": "A person reports to another."}]})
    }

    #[test]
    fn planner_system_renders_the_v5_prompt_for_the_ontology() {
        let want = "You plan how to answer a question from a team's notebook. The notebook has a knowledge graph whose edges are typed with the relation types below, and a passage index over the notes themselves.\n\nRelation types:\n- OWNS: A person owns a project.\n- REPORTS_TO: A person reports to another.\n\nChoose route 'graph' when answering means following relations from a named entity to other entities (one or more hops), and 'passages' when the answer is an attribute of the named entity itself, or how that attribute changed over time. For 'graph', give the start entity exactly as the question names it, then the hops in order from the start towards the answer. Each hop lists 1-3 relation types that could carry it (edge direction is ignored) and a short phrase for what that hop looks for. Set list=true when the question expects several answers, history=true when it asks about earlier values. Also give answer_type: the entity type of the answer, one of: Person, Project, or 'value' when the answer is a literal such as a price, date or percentage. The hops must lead all the way from the start to an entity of that type. For each hop also give direction: 'out' when the current entity is the subject of the relation as its type reads (X REPORTS_TO its manager: from X to the manager is 'out'), 'in' when it is the object (from a manager to the people who report to them is 'in'), or 'any' when unsure.";
        assert_eq!(planner_system(&view()), want);
        let s = planner_schema(&view());
        assert_eq!(
            s["properties"]["answer_type"]["enum"],
            json!(["Person", "Project", "value"])
        );
        assert_eq!(
            s["properties"]["hops"]["items"]["required"],
            json!(["types", "phrase", "direction"])
        );
    }

    #[test]
    fn contexts_carry_the_probe_headers() {
        assert_eq!(
            facts_context(""),
            "Facts from the knowledge graph:\n(none found)"
        );
        assert_eq!(
            facts_context("- a (X [P] -> Y [Q])\n"),
            "Facts from the knowledge graph:\n- a (X [P] -> Y [Q])"
        );
        let p = vec![
            PassageItem {
                note: "n1".into(),
                valid_at: "2025-03-04T00:00:00Z".into(),
                text: "t1".into(),
            },
            PassageItem {
                note: "n2".into(),
                valid_at: String::new(),
                text: "t2".into(),
            },
        ];
        assert_eq!(
            passages_context(&p),
            "Passages from the notes:\n\n[n1, 2025-03-04]\nt1\n\n[n2, ]\nt2"
        );
        assert_eq!(passages_context(&[]), "Passages from the notes:\n\n");
        assert_eq!(reader_user("C", "Q?"), "C\n\nQuestion: Q?");
    }

    #[test]
    fn answers_parse_from_plain_or_wrapped_json() {
        assert_eq!(
            parse_answers("{\"answers\": [\"a\"]}"),
            Some(vec!["a".into()])
        );
        assert_eq!(
            parse_answers("Sure:\n```json\n{\"answers\": [\"a\", 3, \"b\"]}\n```"),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(parse_answers("no json here"), None);
        assert_eq!(parse_answers("{\"other\": 1}"), None);
    }
}
