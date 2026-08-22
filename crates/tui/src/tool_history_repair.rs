//! Deterministic integrity repair for persisted model-visible tool history.
//!
//! Session JSON predates a durable per-call journal, so process exit can leave
//! a `tool_use` without its terminal `tool_result`. Provider APIs reject that
//! shape. This module repairs the existing message format without changing its
//! schema and returns a bounded diagnostic receipt for every mutation.

use std::collections::{HashMap, HashSet};

use crate::models::Role;
use crate::models::{ContentBlock, Message};

const CRASH_REPAIR_CONTENT: &str =
    "Tool call interrupted by process exit; terminal status: crashed_and_repaired.";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolRepairReceipt {
    pub(crate) repaired_call_ids: Vec<String>,
    pub(crate) duplicate_result_ids: Vec<String>,
    pub(crate) orphan_result_ids: Vec<String>,
}

impl ToolRepairReceipt {
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.repaired_call_ids.is_empty()
            && self.duplicate_result_ids.is_empty()
            && self.orphan_result_ids.is_empty()
    }

    fn visible_message(&self) -> String {
        format!(
            "[tool_history_repair] Repaired {} crashed tool call(s); quarantined {} duplicate and {} orphan terminal result(s).",
            self.repaired_call_ids.len(),
            self.duplicate_result_ids.len(),
            self.orphan_result_ids.len(),
        )
    }
}

/// Repair tool-use/result integrity in place.
///
/// The first terminal result after a known call and before the next assistant
/// turn is retained. Results that precede their call, arrive after a later
/// assistant turn, reference no call, or repeat a retained result are
/// quarantined by removing them from model-visible history. Every dangling
/// call receives a synthetic error result directly after its assistant call
/// message. A visible system receipt makes the repair apparent after resume.
pub(crate) fn repair_tool_call_pairs(messages: &mut Vec<Message>) -> ToolRepairReceipt {
    repair_tool_call_pairs_inner(messages, true)
}

/// Repair an ephemeral provider request without appending a trailing receipt.
///
/// Anthropic-style APIs interpret a final assistant message as a completion
/// prefill. Pair repair must therefore leave the synthetic user tool result as
/// the request tail; the durable session-facing path owns the visible receipt.
pub(crate) fn repair_tool_call_pairs_for_provider(
    messages: &mut Vec<Message>,
) -> ToolRepairReceipt {
    repair_tool_call_pairs_inner(messages, false)
}

fn repair_tool_call_pairs_inner(
    messages: &mut Vec<Message>,
    append_visible_receipt: bool,
) -> ToolRepairReceipt {
    let mut pending_call_message = None;
    let mut pending_call_ids = Vec::new();
    let mut retained_for_pending = HashSet::new();
    let mut missing_by_message: HashMap<usize, Vec<String>> = HashMap::new();
    let mut repaired_call_ids = Vec::new();
    let mut duplicate_result_ids = Vec::new();
    let mut orphan_result_ids = Vec::new();
    let mut keep_results = HashSet::new();
    let mut result_ordinal = 0usize;

    for (message_index, message) in messages.iter().enumerate() {
        if message.role == "assistant" || message.role == crate::models::INTERRUPTED_ASSISTANT_ROLE
        {
            record_missing_results(
                pending_call_message,
                &pending_call_ids,
                &retained_for_pending,
                &mut missing_by_message,
                &mut repaired_call_ids,
            );
            pending_call_ids = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect();
            pending_call_message = (!pending_call_ids.is_empty()).then_some(message_index);
            retained_for_pending.clear();
        }
        for block in &message.content {
            let ContentBlock::ToolResult { tool_use_id, .. } = block else {
                continue;
            };
            let ordinal = result_ordinal;
            result_ordinal = result_ordinal.saturating_add(1);

            let follows_known_call = pending_call_message
                .is_some_and(|call_index| call_index < message_index)
                && pending_call_ids.iter().any(|id| id == tool_use_id);
            if !follows_known_call {
                orphan_result_ids.push(tool_use_id.clone());
            } else if !retained_for_pending.insert(tool_use_id.clone()) {
                duplicate_result_ids.push(tool_use_id.clone());
            } else {
                keep_results.insert(ordinal);
            }
        }
    }
    record_missing_results(
        pending_call_message,
        &pending_call_ids,
        &retained_for_pending,
        &mut missing_by_message,
        &mut repaired_call_ids,
    );

    let receipt = ToolRepairReceipt {
        repaired_call_ids,
        duplicate_result_ids,
        orphan_result_ids,
    };
    if receipt.is_empty() {
        return receipt;
    }

    let original = std::mem::take(messages);
    let mut rebuilt = Vec::with_capacity(
        original
            .len()
            .saturating_add(receipt.repaired_call_ids.len()),
    );
    let mut seen_result_ordinal = 0usize;

    for (message_index, message) in original.into_iter().enumerate() {
        let missing_after_message = missing_by_message
            .remove(&message_index)
            .unwrap_or_default();
        let mut filtered = message;
        filtered.content.retain(|block| {
            if matches!(block, ContentBlock::ToolResult { .. }) {
                let keep = keep_results.contains(&seen_result_ordinal);
                seen_result_ordinal = seen_result_ordinal.saturating_add(1);
                keep
            } else {
                true
            }
        });
        if !filtered.content.is_empty() {
            rebuilt.push(filtered);
        }

        if !missing_after_message.is_empty() {
            rebuilt.push(Message {
                role: Role::User,
                content: missing_after_message
                    .into_iter()
                    .map(|tool_use_id| ContentBlock::ToolResult {
                        tool_use_id,
                        content: CRASH_REPAIR_CONTENT.to_string(),
                        is_error: Some(true),
                        content_blocks: None,
                    })
                    .collect(),
            });
        }
    }

    if append_visible_receipt {
        rebuilt.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: receipt.visible_message(),
                cache_control: None,
            }],
        });
    }
    *messages = rebuilt;
    receipt
}

fn record_missing_results(
    call_message: Option<usize>,
    call_ids: &[String],
    retained_results: &HashSet<String>,
    missing_by_message: &mut HashMap<usize, Vec<String>>,
    repaired_call_ids: &mut Vec<String>,
) {
    let Some(message_index) = call_message else {
        return;
    };
    let missing = call_ids
        .iter()
        .filter(|id| !retained_results.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return;
    }
    repaired_call_ids.extend(missing.iter().cloned());
    missing_by_message.insert(message_index, missing);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool_call(id: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "README.md"}),
                caller: None,
                thought_signature: None,
            }],
        }
    }

    fn tool_result(id: &str, content: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    fn text(role: &str, content: &str) -> Message {
        Message {
            role: Role::from(role),
            content: vec![ContentBlock::Text {
                text: content.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn well_formed_history_is_unchanged() {
        let mut messages = vec![tool_call("call-1"), tool_result("call-1", "ok")];
        let before = messages.clone();

        let receipt = repair_tool_call_pairs(&mut messages);

        assert!(receipt.is_empty());
        assert_eq!(messages, before);
    }

    #[test]
    fn repeated_provider_call_id_is_scoped_to_each_assistant_turn() {
        let mut messages = vec![
            tool_call("call-reused"),
            tool_result("call-reused", "hydrated"),
            tool_call("call-reused"),
            tool_result("call-reused", "executed"),
        ];
        let before = messages.clone();

        let receipt = repair_tool_call_pairs_for_provider(&mut messages);

        assert!(receipt.is_empty());
        assert_eq!(messages, before);
    }

    #[test]
    fn repairs_dangling_calls_beside_their_assistant_message() {
        let mut messages = vec![
            tool_call("call-1"),
            text("assistant", "later assistant text"),
        ];

        let receipt = repair_tool_call_pairs(&mut messages);

        assert_eq!(receipt.repaired_call_ids, vec!["call-1"]);
        assert_eq!(messages[1].role, "user");
        assert!(matches!(
            &messages[1].content[0],
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error: Some(true),
                ..
            } if tool_use_id == "call-1" && content.contains("crashed_and_repaired")
        ));
        assert_eq!(messages.last().expect("receipt").role, "assistant");
    }

    #[test]
    fn provider_repair_never_appends_an_assistant_prefill_receipt() {
        let mut messages = vec![tool_call("call-1")];

        let receipt = repair_tool_call_pairs_for_provider(&mut messages);

        assert_eq!(receipt.repaired_call_ids, vec!["call-1"]);
        assert_eq!(messages.last().expect("synthetic result").role, "user");
        assert!(!messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Text { text, .. }
                        if text.contains("[tool_history_repair]")
                )
            })
        }));
    }

    #[test]
    fn quarantines_orphan_and_duplicate_results_without_losing_other_blocks() {
        let mut mixed_result = tool_result("call-1", "duplicate");
        mixed_result.content.push(ContentBlock::Text {
            text: "keep me".to_string(),
            cache_control: None,
        });
        let mut messages = vec![
            tool_result("orphan", "bad"),
            tool_call("call-1"),
            tool_result("call-1", "first"),
            mixed_result,
        ];

        let receipt = repair_tool_call_pairs(&mut messages);

        assert_eq!(receipt.orphan_result_ids, vec!["orphan"]);
        assert_eq!(receipt.duplicate_result_ids, vec!["call-1"]);
        let result_contents: Vec<_> = messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(result_contents, vec!["first"]);
        assert!(messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text { text, .. } if text == "keep me"))
        }));
    }

    #[test]
    fn repair_is_idempotent() {
        let mut messages = vec![tool_call("call-1"), tool_result("orphan", "bad")];

        let first = repair_tool_call_pairs(&mut messages);
        let after_first = messages.clone();
        let second = repair_tool_call_pairs(&mut messages);

        assert!(!first.is_empty());
        assert!(second.is_empty());
        assert_eq!(messages, after_first);
    }

    #[test]
    fn result_preceding_its_call_is_orphaned_and_call_is_repaired() {
        let mut messages = vec![tool_result("call-1", "too early"), tool_call("call-1")];

        let receipt = repair_tool_call_pairs(&mut messages);

        assert_eq!(receipt.orphan_result_ids, vec!["call-1"]);
        assert_eq!(receipt.repaired_call_ids, vec!["call-1"]);
        assert!(messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { content, .. }
                        if content.contains("crashed_and_repaired")
                )
            })
        }));
    }

    #[test]
    fn result_after_a_later_assistant_turn_is_quarantined_as_too_late() {
        let mut messages = vec![
            tool_call("call-1"),
            text("assistant", "a later model turn"),
            tool_result("call-1", "too late"),
        ];

        let receipt = repair_tool_call_pairs(&mut messages);

        assert_eq!(receipt.orphan_result_ids, vec!["call-1"]);
        assert_eq!(receipt.repaired_call_ids, vec!["call-1"]);
        assert!(!messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(block, ContentBlock::ToolResult { content, .. } if content == "too late")
            })
        }));
    }
}
