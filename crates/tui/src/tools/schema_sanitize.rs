//! Schema sanitizer for tool `input_schema` before sending to DeepSeek.
//!
//! DeepSeek's `/beta/chat/completions` strict tool mode is harsh. MCP tool
//! schemas frequently arrive with Pydantic-style `anyOf:[{type:"string"},
//! {type:"null"}]` unions, bare `{type:"object"}` with no `properties`, or
//! `required` entries that don't appear in `properties`. These dirty schemas
//! cause silent 400s that users can't diagnose.
//!
//! The sanitizer runs in-place on every schema returned by
//! `ToolRegistry::tools_for_api()` before the registry hands them off.
//! Output is cached so the per-tool overhead is paid once per registration.

use serde_json::{Map, Value};

use crate::models::Tool;

/// Sanitize a JSON Schema in-place for DeepSeek strict-tool compatibility.
///
/// Applies a sequence of normalisations chosen to be semantics-preserving:
/// - Collapse `{"anyOf":[X, {"type":"null"}]}` → `X ∪ {"nullable": true}`
/// - Inject `"properties": {}` on bare-object schemas
/// - Prune dangling `required` entries
/// - Collapse single-element `oneOf` / `allOf`
/// - Walk recursively through all subschemas
pub fn sanitize(schema: &mut Value) {
    collapse_nullable_unions(schema);
    inject_properties_on_bare_objects(schema);
    prune_dangling_required(schema);
    collapse_single_element_unions(schema);
    // Recurse into all sub-schemas
    if let Some(obj) = schema.as_object_mut() {
        for (_, v) in obj.iter_mut() {
            sanitize(v);
        }
    } else if let Some(arr) = schema.as_array_mut() {
        for v in arr.iter_mut() {
            sanitize(v);
        }
    }
}

/// Prepare a complete active tool set for DeepSeek strict function-calling.
///
/// Returns `false` and leaves the tools in non-strict mode when any root schema
/// uses conditional alternatives (`anyOf`, `oneOf`, or `allOf`). DeepSeek's
/// strict object rules make every property required, so forcing strict mode on
/// root-alternative tools such as `apply_patch` or `finance` would either 400 or
/// change their semantics. In that case callers should keep the normal
/// best-effort schema and may still use `tool_choice = "required"`.
pub fn prepare_tools_for_strict_mode(tools: &mut [Tool]) -> bool {
    if tools
        .iter()
        .any(|tool| !strict_schema_supported(&tool.input_schema))
    {
        for tool in tools {
            tool.strict = None;
        }
        return false;
    }

    for tool in tools {
        sanitize_for_strict(&mut tool.input_schema);
        tool.strict = Some(true);
    }
    true
}

/// Sanitize a schema for DeepSeek strict function-calling.
///
/// This extends the general sanitizer with the official strict-mode object
/// rules: every object must set `additionalProperties: false`, and every
/// property must be listed in `required`.
pub fn sanitize_for_strict(schema: &mut Value) {
    sanitize(schema);
    enforce_strict_subset(schema);
}

fn strict_schema_supported(schema: &Value) -> bool {
    let mut normalized = schema.clone();
    sanitize(&mut normalized);
    !has_strict_incompatible_composition(&normalized, true)
}

fn has_strict_incompatible_composition(schema: &Value, is_root: bool) -> bool {
    if let Some(obj) = schema.as_object() {
        if obj.contains_key("oneOf") || obj.contains_key("allOf") {
            return true;
        }
        if is_root && obj.contains_key("anyOf") {
            return true;
        }
        return obj
            .values()
            .any(|value| has_strict_incompatible_composition(value, false));
    }
    schema.as_array().is_some_and(|arr| {
        arr.iter()
            .any(|value| has_strict_incompatible_composition(value, false))
    })
}

/// Collapse `{"anyOf":[X, {"type":"null"}]}` → `X ∪ {"nullable": true}`.
///
/// Same treatment for `oneOf`. Only collapses when exactly one non-null
/// member and exactly one null-type member are present.
fn collapse_nullable_unions(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    for key in ["anyOf", "oneOf"] {
        let members: Vec<Value> = match obj.get(key).and_then(|v| v.as_array()) {
            Some(arr) => arr.clone(),
            None => continue,
        };
        let (nulls, nons): (Vec<_>, Vec<_>) = members.into_iter().partition(is_null_type);
        if nulls.len() == 1 && nons.len() == 1 {
            obj.remove(key);
            if let Value::Object(non_obj) = nons.into_iter().next().unwrap() {
                for (k, v) in non_obj {
                    if k != "type" || v != "null" {
                        obj.insert(k, v);
                    }
                }
            }
            obj.insert("nullable".into(), Value::Bool(true));
        }
    }
}

fn is_null_type(v: &Value) -> bool {
    v.as_object()
        .and_then(|o| o.get("type"))
        .and_then(|t| t.as_str())
        == Some("null")
}

/// Bare `{"type": "object"}` (no `properties`, no `additionalProperties`)
/// → inject `"properties": {}` so DeepSeek's strict validator doesn't 400.
fn inject_properties_on_bare_objects(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    if obj.get("type").and_then(|t| t.as_str()) != Some("object") {
        return;
    }
    if obj.contains_key("properties") || obj.contains_key("additionalProperties") {
        return;
    }
    obj.insert("properties".into(), Value::Object(Map::new()));
}

/// Remove entries from `required` that aren't keys in `properties`.
fn prune_dangling_required(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    // Collect known property names first (immutable borrow), then prune.
    let known_keys: Vec<String> = obj
        .get("properties")
        .and_then(|v| v.as_object())
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default();
    let Some(required) = obj.get_mut("required").and_then(|v| v.as_array_mut()) else {
        return;
    };
    required.retain(|entry| {
        entry
            .as_str()
            .is_some_and(|k| known_keys.iter().any(|known| known == k))
    });
    if required.is_empty() {
        obj.remove("required");
    }
}

/// Collapse `{"oneOf": [X]}` → X, same for `allOf`.
///
/// Single-element unions are semantically equivalent to the element itself;
/// DeepSeek's strict validator doesn't always flatten them.
fn collapse_single_element_unions(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    for key in ["oneOf", "allOf", "anyOf"] {
        let single = match obj.get(key).and_then(|v| v.as_array()) {
            Some(arr) if arr.len() == 1 => arr[0].clone(),
            _ => continue,
        };
        obj.remove(key);
        if let Value::Object(inner) = single {
            for (k, v) in inner {
                if !obj.contains_key(&k) {
                    obj.insert(k, v);
                }
            }
        }
    }
}

fn enforce_strict_subset(schema: &mut Value) {
    if let Some(obj) = schema.as_object_mut() {
        strip_unsupported_strict_keywords(obj);
        if is_object_schema(obj) {
            let mut property_names: Vec<Value> = ensure_properties_object(obj)
                .keys()
                .cloned()
                .map(Value::String)
                .collect();
            property_names.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            obj.insert("required".into(), Value::Array(property_names));
            obj.insert("additionalProperties".into(), Value::Bool(false));
        }

        for value in obj.values_mut() {
            enforce_strict_subset(value);
        }
    } else if let Some(arr) = schema.as_array_mut() {
        for value in arr {
            enforce_strict_subset(value);
        }
    }
}

fn strip_unsupported_strict_keywords(obj: &mut Map<String, Value>) {
    obj.remove("patternProperties");
    match obj.get("type").and_then(Value::as_str) {
        Some("string") => {
            obj.remove("minLength");
            obj.remove("maxLength");
        }
        Some("array") => {
            obj.remove("minItems");
            obj.remove("maxItems");
        }
        _ => {}
    }
}

fn is_object_schema(obj: &Map<String, Value>) -> bool {
    obj.get("type").and_then(Value::as_str) == Some("object") || obj.contains_key("properties")
}

fn ensure_properties_object(obj: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let needs_replacement = !matches!(obj.get("properties"), Some(Value::Object(_)));
    if needs_replacement {
        obj.insert("properties".into(), Value::Object(Map::new()));
    }
    obj.get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("properties was just ensured as object")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collapses_nullable_anyof() {
        let mut schema = json!({
            "anyOf": [
                {"type": "string"},
                {"type": "null"}
            ]
        });
        sanitize(&mut schema);
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["nullable"], true);
        assert!(schema.get("anyOf").is_none());
    }

    #[test]
    fn collapses_nullable_oneof() {
        let mut schema = json!({
            "oneOf": [
                {"type": "null"},
                {"type": "integer", "minimum": 0}
            ]
        });
        sanitize(&mut schema);
        assert_eq!(schema["type"], "integer");
        assert_eq!(schema["minimum"], 0);
        assert_eq!(schema["nullable"], true);
    }

    #[test]
    fn preserves_non_null_anyof() {
        let original = json!({
            "anyOf": [
                {"type": "string"},
                {"type": "integer"}
            ]
        });
        let mut schema = original.clone();
        sanitize(&mut schema);
        // Multi-typed anyOf should collapse to single element after
        // recursive walk — but here neither is null so the collapse
        // doesn't trigger. The anyOf array itself remains.
        assert!(schema.get("anyOf").is_some());
    }

    #[test]
    fn injects_properties_on_bare_object() {
        let mut schema = json!({"type": "object"});
        sanitize(&mut schema);
        assert!(schema.get("properties").is_some());
        assert_eq!(schema["properties"], json!({}));
    }

    #[test]
    fn does_not_inject_properties_when_present() {
        let mut schema = json!({
            "type": "object",
            "properties": {"name": {"type": "string"}}
        });
        let expected = schema.clone();
        sanitize(&mut schema);
        assert_eq!(schema, expected);
    }

    #[test]
    fn prunes_dangling_required() {
        let mut schema = json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name", "email"]
        });
        sanitize(&mut schema);
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "name");
    }

    #[test]
    fn removes_required_when_all_pruned() {
        let mut schema = json!({
            "type": "object",
            "properties": {},
            "required": ["ghost"]
        });
        sanitize(&mut schema);
        assert!(schema.get("required").is_none());
    }

    #[test]
    fn collapses_single_element_oneof() {
        let mut schema = json!({
            "oneOf": [{"type": "string", "minLength": 1}]
        });
        sanitize(&mut schema);
        assert!(schema.get("oneOf").is_none());
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["minLength"], 1);
    }

    #[test]
    fn collapses_single_element_anyof() {
        let mut schema = json!({
            "anyOf": [{"type": "boolean"}]
        });
        sanitize(&mut schema);
        assert!(schema.get("anyOf").is_none());
        assert_eq!(schema["type"], "boolean");
    }

    #[test]
    fn recursive_walk_into_properties() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "opt_name": {
                    "anyOf": [
                        {"type": "string"},
                        {"type": "null"}
                    ]
                }
            }
        });
        sanitize(&mut schema);
        let prop = &schema["properties"]["opt_name"];
        assert_eq!(prop["type"], "string");
        assert_eq!(prop["nullable"], true);
    }

    #[test]
    fn recursive_walk_into_items() {
        let mut schema = json!({
            "type": "array",
            "items": {
                "anyOf": [
                    {"type": "integer"},
                    {"type": "null"}
                ]
            }
        });
        sanitize(&mut schema);
        let items = &schema["items"];
        assert_eq!(items["type"], "integer");
        assert_eq!(items["nullable"], true);
    }

    #[test]
    fn nested_anyof_in_anyof_collapses() {
        // Pydantic can nest unions: Optional[Union[str, int]].
        let mut schema = json!({
            "anyOf": [
                {
                    "anyOf": [
                        {"type": "string"},
                        {"type": "integer"}
                    ]
                },
                {"type": "null"}
            ]
        });
        sanitize(&mut schema);
        // Outer anyOf is single non-null → collapsed. Inner anyOf is
        // multi-typed → preserved, but the outer null is handled.
        assert_eq!(schema["nullable"], true);
        assert!(schema.get("anyOf").is_some());
    }

    #[test]
    fn idempotent() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "maybe": {
                    "anyOf": [{"type": "integer"}, {"type": "null"}]
                }
            },
            "required": ["name", "missing_field"]
        });
        sanitize(&mut schema);
        let after_first = schema.clone();
        sanitize(&mut schema);
        assert_eq!(schema, after_first, "sanitize must be idempotent");
    }

    #[test]
    fn strict_sanitize_requires_all_object_properties_and_closes_extra_keys() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "count": {"type": "integer"}
            },
            "required": ["name"],
            "additionalProperties": {"type": "string"}
        });

        sanitize_for_strict(&mut schema);

        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["count", "name"]));
    }

    #[test]
    fn strict_sanitize_applies_object_rules_recursively() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "inner": {"type": "string"}
                    },
                    "required": []
                }
            },
            "required": []
        });

        sanitize_for_strict(&mut schema);

        assert_eq!(schema["required"], json!(["outer"]));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["outer"]["required"], json!(["inner"]));
        assert_eq!(schema["properties"]["outer"]["additionalProperties"], false);
    }

    #[test]
    fn strict_sanitize_removes_unsupported_string_and_array_bounds() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 64,
                    "pattern": "^[a-z]+$"
                },
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 5,
                    "items": {"type": "string"}
                },
                "score": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 5
                }
            }
        });

        sanitize_for_strict(&mut schema);

        let name = &schema["properties"]["name"];
        assert!(name.get("minLength").is_none());
        assert!(name.get("maxLength").is_none());
        assert_eq!(name["pattern"], "^[a-z]+$");

        let items = &schema["properties"]["items"];
        assert!(items.get("minItems").is_none());
        assert!(items.get("maxItems").is_none());

        let score = &schema["properties"]["score"];
        assert_eq!(score["minimum"], 1);
        assert_eq!(score["maximum"], 5);
    }

    #[test]
    fn strict_mode_rejects_root_composition_for_whole_tool_set() {
        let mut tools = vec![Tool {
            tool_type: None,
            name: "either".to_string(),
            description: "Either input shape".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "a": {"type": "string"},
                    "b": {"type": "string"}
                },
                "anyOf": [
                    {"required": ["a"]},
                    {"required": ["b"]}
                ]
            }),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: Some(true),
            cache_control: None,
        }];

        assert!(!prepare_tools_for_strict_mode(&mut tools));
        assert_eq!(tools[0].strict, None);
        assert!(tools[0].input_schema.get("anyOf").is_some());
    }

    #[test]
    fn strict_mode_rejects_nested_unsupported_composition() {
        let mut tools = vec![Tool {
            tool_type: None,
            name: "nested".to_string(),
            description: "Nested oneOf".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "value": {
                        "oneOf": [
                            {"type": "string"},
                            {"type": "integer"}
                        ]
                    }
                }
            }),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: None,
            cache_control: None,
        }];

        assert!(!prepare_tools_for_strict_mode(&mut tools));
        assert_eq!(tools[0].strict, None);
    }

    #[test]
    fn strict_mode_marks_compatible_tools_strict() {
        let mut tools = vec![Tool {
            tool_type: None,
            name: "lookup".to_string(),
            description: "Lookup".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": []
            }),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: None,
            cache_control: None,
        }];

        assert!(prepare_tools_for_strict_mode(&mut tools));
        assert_eq!(tools[0].strict, Some(true));
        assert_eq!(tools[0].input_schema["required"], json!(["query"]));
        assert_eq!(tools[0].input_schema["additionalProperties"], false);
    }
}
