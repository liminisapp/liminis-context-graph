//! Published JSON Schemas for the results of the methods consumers read (issue #852, ADR-0852).
//!
//! The handlers assemble their responses with `serde_json::json!`; nothing here is on the
//! dispatch path, so this module cannot change a wire format. The types in the submodules are
//! **schema-only mirrors** of those results: they derive [`schemars::JsonSchema`] and are never
//! serialized. [`generate`] turns them into draft-07 schema documents, committed under
//! `contract/*.schema.json` and shipped as release assets.
//!
//! Drift is caught in both directions:
//!
//! * code → schema: `tests/contract_schemas.rs` validates *real* handler responses against the
//!   committed files, so a renamed/removed/retyped field fails there;
//! * source → files: the `committed_schemas_match_generated` test below fails when a committed
//!   file differs from what [`generate`] produces. Regenerate with
//!   `LCG_UPDATE_CONTRACT=1 cargo test -p lcg-core --lib contract::`.
//!
//! Policy: additional properties are tolerated (no `additionalProperties: false`), fields that
//! are always emitted are `required`, fields that are sometimes omitted are optional, and fields
//! that are always emitted as `null` are required **and** nullable. Removing or renaming a field,
//! narrowing its type, or making an optional field required is a breaking change; adding an
//! optional field is not.

mod find;
mod health;
mod lint;
mod passages;
mod process_chunk;
mod status;

use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use serde_json::{Map, Value};

/// A field that is always emitted and may be `null` (`required` and nullable). `Option<T>` alone
/// would make the field optional, which is not what the handlers do.
pub(crate) struct Nullable<T>(std::marker::PhantomData<T>);

impl<T: JsonSchema> JsonSchema for Nullable<T> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("Nullable_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let inner = serde_json::to_value(generator.subschema_for::<T>()).expect("schema");
        // `{"type": "string"}` becomes `{"type": ["string", "null"]}`, which every consumer tool
        // understands; anything else (a `$ref`) becomes `anyOf: [inner, null]`.
        if let Some(obj) = inner.as_object() {
            if let Some(Value::String(ty)) = obj.get("type") {
                let mut out = obj.clone();
                out.insert("type".into(), serde_json::json!([ty, "null"]));
                return schemars::Schema::try_from(Value::Object(out)).expect("schema");
            }
        }
        schemars::json_schema!({"anyOf": [inner, {"type": "null"}]})
    }
}

/// The JSON Schema draft every published schema uses.
pub const DRAFT: &str = "draft-07";

/// Methods with a published result schema, in manifest order.
pub const METHODS: &[&str] = &[
    "health_check",
    "knowledge_find_entities",
    "knowledge_find_relationships",
    "knowledge_lint",
    "knowledge_process_chunk",
    "knowledge_search_passages",
    "knowledge_status",
];

/// The file name a method's schema is published under.
pub fn file_name(method: &str) -> String {
    format!("{method}.schema.json")
}

fn schema_of<T: JsonSchema>(method: &str, description: &str) -> Value {
    let settings = SchemaSettings::draft07();
    let schema = settings.into_generator().into_root_schema_for::<T>();
    let mut v = serde_json::to_value(&schema).expect("schema serializes");
    if let Some(obj) = v.as_object_mut() {
        obj.insert("title".into(), Value::String(format!("{method} result")));
        obj.insert("description".into(), Value::String(description.into()));
    }
    strip_numeric_formats(&mut v);
    strip_null_from_omitted(&mut v);
    sort_keys(v)
}

/// schemars renders `Option<T>` as nullable (`["boolean", "null"]`), but every optional field in
/// the contract types is *omitted* by the handlers when absent (`skip_serializing_if`), never sent
/// as `null`. Fields that are present-but-null use [`Nullable`] and are `required`. So for every
/// property that is not `required`, drop the `null` alternative: an explicit `null` on an
/// omitted-only field then fails validation, and generated consumer types are `T | undefined`
/// rather than `T | null | undefined`.
fn strip_null_from_omitted(v: &mut Value) {
    match v {
        Value::Object(m) => {
            let required: Vec<String> = m
                .get("required")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|r| r.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(Value::Object(props)) = m.get_mut("properties") {
                for (name, prop) in props.iter_mut() {
                    if !required.contains(name) {
                        drop_null_alternative(prop);
                    }
                }
            }
            m.values_mut().for_each(strip_null_from_omitted);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_null_from_omitted),
        _ => {}
    }
}

/// Remove `null` from one property schema: `type: [T, "null"]` becomes `type: T`, and
/// `anyOf: [X, {type: null}]` collapses to `X` (merged into the property so its description and
/// other annotations are kept).
fn drop_null_alternative(prop: &mut Value) {
    let Some(obj) = prop.as_object_mut() else {
        return;
    };
    if let Some(Value::Array(types)) = obj.get_mut("type") {
        types.retain(|t| t != "null");
        if types.len() == 1 {
            let only = types.remove(0);
            obj.insert("type".into(), only);
        }
    }
    if let Some(Value::Array(alts)) = obj.get_mut("anyOf") {
        alts.retain(|a| a.get("type") != Some(&Value::String("null".into())));
        if alts.len() == 1 {
            let only = alts.remove(0);
            obj.remove("anyOf");
            if let Value::Object(inner) = only {
                for (k, val) in inner {
                    obj.insert(k, val);
                }
            }
        }
    }
}

/// schemars annotates integers with `format: uint64` etc., which no consumer tool needs and some
/// warn about; the `integer` type and `minimum` carry the meaning.
fn strip_numeric_formats(v: &mut Value) {
    match v {
        Value::Object(m) => {
            let numeric = matches!(
                m.get("format").and_then(Value::as_str),
                Some(
                    "uint"
                        | "uint8"
                        | "uint16"
                        | "uint32"
                        | "uint64"
                        | "int8"
                        | "int16"
                        | "int32"
                        | "int64"
                        | "float"
                        | "double"
                )
            );
            if numeric {
                m.remove("format");
            }
            m.values_mut().for_each(strip_numeric_formats);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_numeric_formats),
        _ => {}
    }
}

/// `serde_json` is built with `preserve_order`; sort keys so output is deterministic.
fn sort_keys(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut entries: Vec<(String, Value)> = m.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = Map::new();
            for (k, val) in entries {
                out.insert(k, sort_keys(val));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sort_keys).collect()),
        other => other,
    }
}

/// Every published schema as `(method, schema)`, in [`METHODS`] order.
pub fn generate() -> Vec<(&'static str, Value)> {
    vec![
        (
            "health_check",
            schema_of::<health::HealthCheckResult>(
                "health_check",
                "Result of the health_check method: healthy, degraded or busy.",
            ),
        ),
        (
            "knowledge_find_entities",
            schema_of::<find::FindEntitiesResult>(
                "knowledge_find_entities",
                "Result of knowledge_find_entities.",
            ),
        ),
        (
            "knowledge_find_relationships",
            schema_of::<find::FindRelationshipsResult>(
                "knowledge_find_relationships",
                "Result of knowledge_find_relationships.",
            ),
        ),
        (
            "knowledge_lint",
            schema_of::<lint::LintResult>(
                "knowledge_lint",
                "Result of knowledge_lint: findings discriminated by category (and reason).",
            ),
        ),
        (
            "knowledge_process_chunk",
            schema_of::<process_chunk::ProcessChunkResult>(
                "knowledge_process_chunk",
                "Result of knowledge_process_chunk.",
            ),
        ),
        (
            "knowledge_search_passages",
            schema_of::<passages::SearchPassagesResult>(
                "knowledge_search_passages",
                "Result of knowledge_search_passages.",
            ),
        ),
        (
            "knowledge_status",
            schema_of::<status::StatusResult>(
                "knowledge_status",
                "Result of knowledge_status: degraded (database not open), queryable, or \
                 open-but-not-queryable.",
            ),
        ),
    ]
}

/// The committed file content for a schema: 2-space pretty JSON with a trailing newline.
pub fn render(schema: &Value) -> String {
    let mut s = serde_json::to_string_pretty(schema).expect("schema serializes");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn contract_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contract")
    }

    #[test]
    fn generated_methods_match_methods_list() {
        let names: Vec<&str> = generate().iter().map(|(m, _)| *m).collect();
        assert_eq!(names, METHODS);
    }

    #[test]
    fn schemas_are_self_contained_draft07() {
        for (m, schema) in generate() {
            assert_eq!(
                schema["$schema"], "http://json-schema.org/draft-07/schema#",
                "{m}"
            );
            let text = render(&schema);
            for line in text.lines().filter(|l| l.contains("\"$ref\"")) {
                assert!(
                    line.contains("\"$ref\": \"#/definitions/"),
                    "{m}: non-local $ref: {line}"
                );
            }
        }
    }

    /// FR-007: the committed files are exactly what the contract types generate.
    /// `LCG_UPDATE_CONTRACT=1` rewrites them instead of failing.
    #[test]
    fn committed_schemas_match_generated() {
        let dir = contract_dir();
        let update = std::env::var("LCG_UPDATE_CONTRACT").is_ok_and(|v| v == "1");
        if update {
            std::fs::create_dir_all(&dir).unwrap();
        }
        let mut stale = Vec::new();
        for (method, schema) in generate() {
            let path = dir.join(file_name(method));
            let want = render(&schema);
            if update {
                std::fs::write(&path, &want).unwrap();
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(have) if have == want => {}
                _ => stale.push(path.display().to_string()),
            }
        }
        assert!(
            stale.is_empty(),
            "contract schemas are stale or missing: {stale:?}\n\
             regenerate with: LCG_UPDATE_CONTRACT=1 cargo test -p lcg-core --lib contract::"
        );
        // No orphan schema files for methods that are no longer published.
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(m) = name.strip_suffix(".schema.json") {
                assert!(METHODS.contains(&m), "orphan schema file {name}");
            }
        }
    }
}
