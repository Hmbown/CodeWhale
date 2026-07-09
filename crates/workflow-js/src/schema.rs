//! `responseSchema` decoding: parse the subagent's reply as JSON and validate
//! it against the caller-supplied schema.
//!
//! Retry semantics live on the driver side (it owns the child and its
//! prompt); the VM only parses and validates — a reply that is not valid
//! JSON, or that fails the schema, throws on the awaiting `task()` call.

/// Compile the caller's schema. Called before spawning so a malformed schema
/// fails fast instead of burning a subagent.
pub(crate) fn compile_schema(schema: &serde_json::Value) -> Result<jsonschema::Validator, String> {
    jsonschema::validator_for(schema)
        .map_err(|err| format!("task(): invalid responseSchema: {err}"))
}

/// Parse `text` as JSON (tolerating a single Markdown code fence around the
/// payload) and validate it against `validator`.
pub(crate) fn decode_reply(
    text: &str,
    validator: &jsonschema::Validator,
) -> Result<serde_json::Value, String> {
    let candidate = strip_code_fence(text);
    let parsed: serde_json::Value = serde_json::from_str(candidate).map_err(|err| {
        format!("task(): responseSchema was set but the reply is not valid JSON: {err}")
    })?;
    let errors = validator
        .iter_errors(&parsed)
        .map(|err| err.to_string())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(format!(
            "task(): reply failed responseSchema validation: {}",
            errors.join("; ")
        ));
    }
    Ok(parsed)
}

/// If the whole reply is wrapped in one Markdown code fence (``` or ```json),
/// return the fenced body; otherwise return the trimmed reply unchanged.
fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(body) = rest.strip_suffix("```") else {
        return trimmed;
    };
    // Drop an optional language tag on the opening fence line.
    match body.split_once('\n') {
        Some((first_line, tail)) if !first_line.trim().is_empty() => tail.trim(),
        _ => body.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn validator() -> jsonschema::Validator {
        compile_schema(&json!({
            "type": "object",
            "properties": { "refuted": { "type": "boolean" } },
            "required": ["refuted"],
        }))
        .expect("schema compiles")
    }

    #[test]
    fn decodes_plain_json() {
        let value = decode_reply(r#"{"refuted": true}"#, &validator()).unwrap();
        assert_eq!(value, json!({"refuted": true}));
    }

    #[test]
    fn decodes_fenced_json() {
        let text = "```json\n{\"refuted\": false}\n```";
        let value = decode_reply(text, &validator()).unwrap();
        assert_eq!(value, json!({"refuted": false}));
    }

    #[test]
    fn rejects_non_json() {
        let err = decode_reply("definitely not json", &validator()).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
    }

    #[test]
    fn rejects_schema_violation() {
        let err = decode_reply(r#"{"refuted": "yes"}"#, &validator()).unwrap_err();
        assert!(err.contains("responseSchema validation"), "{err}");
    }

    #[test]
    fn rejects_invalid_schema_before_spawn() {
        let err = compile_schema(&json!({"type": "not-a-type"})).unwrap_err();
        assert!(err.contains("invalid responseSchema"), "{err}");
    }
}
