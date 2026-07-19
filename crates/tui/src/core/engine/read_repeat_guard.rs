//! Per-user-turn protection against repeated identical read-only tool calls.
//!
//! The ordinary stuck guard detects consecutive no-progress steps. This guard
//! is narrower and deliberately non-consecutive: it keys finalized read-only
//! calls by resolved tool name plus canonical arguments, survives interleaved
//! model steps, and resets when `handle_deepseek_turn` returns.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::tools::spec::{ToolError, ToolResult};

pub(super) const NUDGE_THRESHOLD: usize = 3;
pub(super) const RECEIPT_THRESHOLD: usize = 5;
pub(super) const STOP_THRESHOLD: usize = 8;

const CORRECTIVE_NUDGE: &str = "[codewhale read-repeat guard] This identical read-only call has already been requested multiple times in the current user turn. Reuse the result already in context, change the arguments, or switch methods; do not issue the same read unchanged.";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ReadRepeatKey {
    tool_name: String,
    arguments_sha256: String,
}

impl ReadRepeatKey {
    pub(super) fn tool_name(&self) -> &str {
        &self.tool_name
    }

    pub(super) fn arguments_sha256(&self) -> &str {
        &self.arguments_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadRepeatOccurrence {
    pub(super) key: ReadRepeatKey,
    pub(super) count: usize,
}

#[derive(Debug, Clone)]
struct PriorSuccess {
    tool_use_id: String,
    content_sha256: String,
}

#[derive(Debug, Default)]
pub(super) struct ReadRepeatGuard {
    counts: HashMap<ReadRepeatKey, usize>,
    prior_successes: HashMap<ReadRepeatKey, PriorSuccess>,
}

impl ReadRepeatGuard {
    pub(super) fn register(&mut self, tool_name: &str, arguments: &Value) -> ReadRepeatOccurrence {
        let key = ReadRepeatKey {
            tool_name: tool_name.to_string(),
            arguments_sha256: crate::hashing::sha256_hex(
                canonical_json(arguments).to_string().as_bytes(),
            ),
        };
        let count = self.counts.entry(key.clone()).or_default();
        *count = count.saturating_add(1);
        ReadRepeatOccurrence { key, count: *count }
    }

    pub(super) fn prior_receipt(&self, occurrence: &ReadRepeatOccurrence) -> Option<ToolResult> {
        if occurrence.count < RECEIPT_THRESHOLD {
            return None;
        }
        let prior = self.prior_successes.get(&occurrence.key)?;
        Some(receipt_result(
            occurrence,
            &prior.tool_use_id,
            &prior.content_sha256,
            "prior_result",
        ))
    }

    pub(super) fn remember_success(
        &mut self,
        occurrence: &ReadRepeatOccurrence,
        tool_use_id: &str,
        result: &ToolResult,
    ) {
        if !result.success || !result_was_executed(result) {
            return;
        }
        self.prior_successes.insert(
            occurrence.key.clone(),
            PriorSuccess {
                tool_use_id: tool_use_id.to_string(),
                content_sha256: crate::hashing::sha256_hex(result.content.as_bytes()),
            },
        );
    }

    pub(super) fn coalesced_result(
        &self,
        occurrence: &ReadRepeatOccurrence,
        leader_tool_use_id: &str,
        leader_result: &Result<ToolResult, ToolError>,
    ) -> Result<ToolResult, ToolError> {
        let Ok(leader_result) = leader_result else {
            return leader_result.clone();
        };
        if !leader_result.success {
            let mut result = leader_result.clone();
            stamp_repeat_metadata(
                &mut result,
                occurrence,
                false,
                "coalesced_error",
                Some(leader_tool_use_id),
                None,
            );
            return Ok(result);
        }

        let content_sha256 = crate::hashing::sha256_hex(leader_result.content.as_bytes());
        if occurrence.count >= RECEIPT_THRESHOLD {
            return Ok(receipt_result(
                occurrence,
                leader_tool_use_id,
                &content_sha256,
                "same_batch_receipt",
            ));
        }

        let mut result = leader_result.clone();
        stamp_repeat_metadata(
            &mut result,
            occurrence,
            false,
            "same_batch_subscription",
            Some(leader_tool_use_id),
            Some(&content_sha256),
        );
        Ok(result)
    }

    pub(super) fn decorate_model_result(
        &self,
        occurrence: &ReadRepeatOccurrence,
        result: &mut ToolResult,
    ) {
        if occurrence.count < NUDGE_THRESHOLD {
            return;
        }
        if !result.content.contains("[codewhale read-repeat guard]") {
            if !result.content.is_empty() {
                result.content.push_str("\n\n");
            }
            result.content.push_str(CORRECTIVE_NUDGE);
        }
        if let Some(repeat) = result
            .metadata
            .as_mut()
            .and_then(Value::as_object_mut)
            .and_then(|metadata| metadata.get_mut("read_repeat"))
            .and_then(Value::as_object_mut)
        {
            repeat.insert("nudged".to_string(), Value::Bool(true));
            return;
        }
        let executed = result_was_executed(result);
        stamp_repeat_metadata(result, occurrence, executed, "nudge", None, None);
    }

    pub(super) fn corrective_nudge(occurrence: &ReadRepeatOccurrence) -> Option<&'static str> {
        (occurrence.count >= NUDGE_THRESHOLD).then_some(CORRECTIVE_NUDGE)
    }

    pub(super) fn should_stop(occurrence: &ReadRepeatOccurrence) -> bool {
        occurrence.count >= STOP_THRESHOLD
    }
}

fn receipt_result(
    occurrence: &ReadRepeatOccurrence,
    source_tool_use_id: &str,
    source_content_sha256: &str,
    action: &str,
) -> ToolResult {
    let mut result = ToolResult::success(format!(
        "[read-only result receipt]\nIdentical call occurrence {} was not executed. Reuse tool result `{source_tool_use_id}` (content sha256 `{source_content_sha256}`).",
        occurrence.count
    ));
    stamp_repeat_metadata(
        &mut result,
        occurrence,
        false,
        action,
        Some(source_tool_use_id),
        Some(source_content_sha256),
    );
    result
}

fn result_was_executed(result: &ToolResult) -> bool {
    result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("executed"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn stamp_repeat_metadata(
    result: &mut ToolResult,
    occurrence: &ReadRepeatOccurrence,
    executed: bool,
    action: &str,
    source_tool_use_id: Option<&str>,
    source_content_sha256: Option<&str>,
) {
    let repeat = json!({
        "tool_name": occurrence.key.tool_name(),
        "arguments_sha256": occurrence.key.arguments_sha256(),
        "count": occurrence.count,
        "action": action,
        "source_tool_use_id": source_tool_use_id,
        "source_content_sha256": source_content_sha256,
    });
    let metadata = result.metadata.get_or_insert_with(|| json!({}));
    if let Some(object) = metadata.as_object_mut() {
        object.insert("executed".to_string(), Value::Bool(executed));
        object.insert("read_repeat".to_string(), repeat);
    } else {
        let prior = std::mem::replace(metadata, json!({}));
        let object = metadata
            .as_object_mut()
            .expect("replacement metadata is an object");
        object.insert("_prior".to_string(), prior);
        object.insert("executed".to_string(), Value::Bool(executed));
        object.insert("read_repeat".to_string(), repeat);
    }
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            let mut canonical = serde_json::Map::new();
            for (key, value) in entries {
                canonical.insert(key.clone(), canonical_json(value));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_argument_order_shares_one_counter() {
        let mut guard = ReadRepeatGuard::default();
        let first = guard.register("read_file", &json!({"path": "a", "limit": 10}));
        let second = guard.register("read_file", &json!({"limit": 10, "path": "a"}));

        assert_eq!(first.key, second.key);
        assert_eq!(second.count, 2);
    }

    #[test]
    fn counts_survive_interleaving_with_other_reads() {
        let mut guard = ReadRepeatGuard::default();
        let a1 = guard.register("read_file", &json!({"path": "a"}));
        let _b1 = guard.register("read_file", &json!({"path": "b"}));
        let a2 = guard.register("read_file", &json!({"path": "a"}));
        let _b2 = guard.register("grep_files", &json!({"pattern": "needle"}));
        let a3 = guard.register("read_file", &json!({"path": "a"}));

        assert_eq!(a1.count, 1);
        assert_eq!(a2.count, 2);
        assert_eq!(a3.count, NUDGE_THRESHOLD);
    }

    #[test]
    fn fifth_call_reuses_prior_success_and_eighth_stops() {
        let mut guard = ReadRepeatGuard::default();
        let arguments = json!({"path": "a"});
        let first = guard.register("read_file", &arguments);
        guard.remember_success(&first, "tool-1", &ToolResult::success("contents"));

        for expected in 2..RECEIPT_THRESHOLD {
            assert_eq!(guard.register("read_file", &arguments).count, expected);
        }
        let fifth = guard.register("read_file", &arguments);
        let mut receipt = guard
            .prior_receipt(&fifth)
            .expect("fifth identical read should reuse the prior result");
        guard.decorate_model_result(&fifth, &mut receipt);
        assert_eq!(receipt.metadata.as_ref().unwrap()["executed"], false);
        assert_eq!(
            receipt.metadata.as_ref().unwrap()["read_repeat"]["action"],
            "prior_result"
        );
        assert_eq!(
            receipt.metadata.as_ref().unwrap()["read_repeat"]["nudged"],
            true
        );
        assert!(receipt.content.contains("tool-1"));

        let _sixth = guard.register("read_file", &arguments);
        let _seventh = guard.register("read_file", &arguments);
        let eighth = guard.register("read_file", &arguments);
        assert!(ReadRepeatGuard::should_stop(&eighth));
    }

    #[test]
    fn nudge_is_added_once_and_is_replay_stable() {
        let mut guard = ReadRepeatGuard::default();
        let arguments = json!({"path": "a"});
        let _first = guard.register("read_file", &arguments);
        let _second = guard.register("read_file", &arguments);
        let third = guard.register("read_file", &arguments);
        let mut result = ToolResult::success("contents");

        guard.decorate_model_result(&third, &mut result);
        guard.decorate_model_result(&third, &mut result);

        assert_eq!(result.content.matches(CORRECTIVE_NUDGE).count(), 1);
        assert_eq!(result.metadata.as_ref().unwrap()["read_repeat"]["count"], 3);
    }
}
