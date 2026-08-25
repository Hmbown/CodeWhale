//! `responseSchema` decoding: parse the subagent's reply as JSON and validate
//! it against the caller-supplied schema, with bounded repair when it fails.
//!
//! A reply that is not valid JSON, or that fails the schema, throws on the
//! awaiting `task()` call. Before that terminal throw the VM tries a bounded
//! repair (#5583): one re-ask (by default; `schemaRepairAttempts` up to 3) of
//! the same route with the schema, the invalid reply, and the decode error,
//! so a child that wrapped its JSON in prose does not abort a whole run. A
//! failed repair stays a schema failure — never a null slot or a degraded
//! success — and both attempts are reported through the driver as receipts.

/// Upper bound on `schemaRepairAttempts`. Repair is a bounded recovery, not a
/// retry loop: more attempts than this is a prompt or model-route problem that
/// re-asking cannot fix.
pub const SCHEMA_REPAIR_MAX_ATTEMPTS: u32 = 3;

/// Preview of a raw reply carried in receipts and reports. The full raw text
/// goes to a durable artifact when it is larger (the host writes it; the VM
/// only carries the bytes it was given).
pub const SCHEMA_RAW_PREVIEW_CHARS: usize = 2_000;

/// Hard cap on raw reply text carried in-memory through events. Child replies
/// beyond this are pathological; the carried text is capped with an explicit
/// marker so the receipt never pretends to be the whole reply.
pub const SCHEMA_RAW_CARRY_CHARS: usize = 64 * 1_024;

/// Compile the caller's schema. Called before spawning so a malformed schema
/// fails fast instead of burning a subagent.
pub(crate) fn compile_schema(schema: &serde_json::Value) -> Result<jsonschema::Validator, String> {
    jsonschema::validator_for(schema)
        .map_err(|err| format!("task(): invalid responseSchema: {err}"))
}

/// Why a reply failed `responseSchema` decoding.
///
/// The distinction is receipt material (#5583): a parse failure means the
/// model wrapped or broke the JSON (a repair usually fixes it); a validation
/// failure means the JSON parsed but said the wrong thing (a schema or prompt
/// problem the operator needs to see named).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplyDecodeError {
    /// The reply was not parseable as JSON at all.
    Parse(String),
    /// The reply parsed but violated the caller's schema.
    Validate(String),
}

impl ReplyDecodeError {
    /// Stable machine kind for receipts: `json_parse` or `schema_validation`.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Parse(_) => "json_parse",
            Self::Validate(_) => "schema_validation",
        }
    }

    /// The operator-facing message (byte-identical to the pre-repair strings,
    /// so existing envelopes and tests keep classifying).
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Parse(message) | Self::Validate(message) => message,
        }
    }
}

/// Parse `text` as JSON (tolerating a single Markdown code fence around the
/// payload) and validate it against `validator`.
pub(crate) fn decode_reply(
    text: &str,
    validator: &jsonschema::Validator,
) -> Result<serde_json::Value, ReplyDecodeError> {
    let candidate = strip_code_fence(text);
    let parsed: serde_json::Value = serde_json::from_str(candidate).map_err(|err| {
        ReplyDecodeError::Parse(format!(
            "task(): responseSchema was set but the reply is not valid JSON: {err}"
        ))
    })?;
    let errors = validator
        .iter_errors(&parsed)
        .map(|err| err.to_string())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(ReplyDecodeError::Validate(format!(
            "task(): reply failed responseSchema validation: {}",
            errors.join("; ")
        )));
    }
    Ok(parsed)
}

/// The raw reply text carried alongside a decode failure, capped at
/// [`SCHEMA_RAW_CARRY_CHARS`] on a char boundary with an explicit marker.
/// Returns `(carried, was_truncated)`.
pub(crate) fn carried_raw(text: &str) -> (String, bool) {
    if text.chars().count() <= SCHEMA_RAW_CARRY_CHARS {
        return (text.to_string(), false);
    }
    let kept: String = text.chars().take(SCHEMA_RAW_CARRY_CHARS).collect();
    let dropped = text.chars().count() - SCHEMA_RAW_CARRY_CHARS;
    (
        format!("{kept}\n…[raw reply truncated: {dropped} chars not carried]"),
        true,
    )
}

/// Build the repair prompt for the next attempt: the same task, the schema it
/// must satisfy, the reply that failed, and why. The repair child is a fresh
/// agent — it has no memory of the failed attempt, so everything it needs to
/// correct the reply travels in this prompt.
pub(crate) fn repair_prompt(
    original_prompt: &str,
    schema: &serde_json::Value,
    failed_raw: &str,
    error: &ReplyDecodeError,
) -> String {
    let schema_text = serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string());
    format!(
        "Your previous reply for this task failed its responseSchema and is being repaired.\n\
         Return ONLY the corrected JSON. No prose, no explanation, no Markdown code fence.\n\n\
         ## Original task\n\n{original_prompt}\n\n\
         ## responseSchema the reply must satisfy\n\n```json\n{schema_text}\n```\n\n\
         ## Your previous reply (it failed)\n\n```\n{failed_raw}\n```\n\n\
         ## Why it failed\n\n{}\n\n\
         Resend the reply as corrected JSON only.",
        error.message(),
    )
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
    fn rejects_non_json_with_the_parse_kind() {
        let err = decode_reply("definitely not json", &validator()).unwrap_err();
        assert_eq!(err.kind(), "json_parse");
        assert!(
            err.message().contains("not valid JSON"),
            "{}",
            err.message()
        );
    }

    #[test]
    fn rejects_schema_violation_with_the_validation_kind() {
        let err = decode_reply(r#"{"refuted": "yes"}"#, &validator()).unwrap_err();
        assert_eq!(err.kind(), "schema_validation");
        assert!(
            err.message().contains("responseSchema validation"),
            "{}",
            err.message()
        );
    }

    #[test]
    fn rejects_invalid_schema_before_spawn() {
        let err = compile_schema(&json!({"type": "not-a-type"})).unwrap_err();
        assert!(err.contains("invalid responseSchema"), "{err}");
    }

    #[test]
    fn carried_raw_passes_short_text_through() {
        let (carried, truncated) = carried_raw("short");
        assert_eq!(carried, "short");
        assert!(!truncated);
    }

    #[test]
    fn carried_raw_caps_long_text_on_a_char_boundary() {
        let text = "é".repeat(SCHEMA_RAW_CARRY_CHARS + 10);
        let (carried, truncated) = carried_raw(&text);
        assert!(truncated);
        assert!(carried.ends_with("chars not carried]"));
        // The cap is honored in chars: exactly SCHEMA_RAW_CARRY_CHARS kept,
        // with the marker appended as its own line.
        let kept = carried.lines().next().unwrap_or_default();
        assert_eq!(kept.chars().count(), SCHEMA_RAW_CARRY_CHARS);
    }

    #[test]
    fn repair_prompt_carries_task_schema_reply_and_reason() {
        let error = ReplyDecodeError::Parse(
            "task(): responseSchema was set but the reply is not valid JSON: expected value"
                .to_string(),
        );
        let prompt = repair_prompt(
            "Score the diff.",
            &json!({"type": "object"}),
            "Sure! Here it is: {...}",
            &error,
        );
        assert!(prompt.contains("Score the diff."));
        assert!(prompt.contains("\"type\": \"object\""));
        assert!(prompt.contains("Sure! Here it is: {...}"));
        assert!(prompt.contains("not valid JSON"));
        assert!(prompt.contains("corrected JSON only"));
    }
}
