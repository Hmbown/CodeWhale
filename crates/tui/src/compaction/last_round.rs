//! Last-round coverage floor for compaction replacement history.
//!
//! Compaction may summarize older turns, but the latest user round (user
//! text plus following assistant/tool results) must survive verbatim,
//! bounded, or the pass is refused.

use anyhow::Result;

use crate::models::{ContentBlock, Message, Role, SystemPrompt};

use super::{
    COMPACT_RETAINED_USER_MESSAGE_MAX_TOKENS, compaction_checkpoint_message,
    is_compaction_checkpoint_message, retained_user_messages, truncate_retained_block,
    user_text_of,
};

const LAST_ROUND_TOOL_RESULT_MAX_CHARS: usize = 8 * 1024;
const LAST_ROUND_THINKING_MAX_CHARS: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompactionPath {
    #[default]
    Summary,
    PruneOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompactionCoverage {
    pub path: CompactionPath,
    pub last_round_messages: usize,
    pub last_round_tool_results: usize,
    pub last_round_assistant: bool,
    pub dropped_messages: usize,
    pub anchors_chars: usize,
}

impl CompactionCoverage {
    #[must_use]
    pub fn receipt_clause(&self) -> String {
        let path = match self.path {
            CompactionPath::Summary => "summary",
            CompactionPath::PruneOnly => "prune-only",
        };
        let assistant = if self.last_round_assistant {
            ", assistant"
        } else {
            ""
        };
        let mut clause = format!(
            "{path}; last round kept: {} messages ({} tool results{assistant})",
            self.last_round_messages, self.last_round_tool_results
        );
        if self.anchors_chars > 0 {
            clause.push_str(&format!("; anchors {} chars", self.anchors_chars));
        }
        clause
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastCompactionSnapshot {
    pub auto: bool,
    pub coverage: CompactionCoverage,
    pub messages_before: usize,
    pub messages_after: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompactionKeep {
    pub has_checkpoint: bool,
    pub last_round_messages: usize,
    pub last_round_tool_results: usize,
    pub last_round_assistant: bool,
}

#[must_use]
pub fn inspect_compaction_keep(messages: &[Message]) -> CompactionKeep {
    let last_round = last_round_slice(messages);
    CompactionKeep {
        has_checkpoint: messages.iter().any(is_compaction_checkpoint_message),
        last_round_messages: last_round.len(),
        last_round_tool_results: last_round.iter().flat_map(tool_result_ids).count(),
        last_round_assistant: last_round
            .iter()
            .any(|message| message.role.is_assistant_like()),
    }
}

#[must_use]
pub fn pinned_anchors_text(workspace: Option<&std::path::Path>) -> Option<String> {
    let workspace = workspace?;
    let primary = workspace.join(".codewhale").join("anchors.md");
    let path = if primary.exists() {
        primary
    } else {
        workspace.join(".deepseek").join("anchors.md")
    };
    std::fs::read_to_string(path)
        .ok()
        .map(|contents| contents.trim().to_string())
        .filter(|contents| !contents.is_empty())
}

#[must_use]
pub(crate) fn last_round_start(messages: &[Message]) -> usize {
    let last_user = messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(idx, message)| {
            if is_compaction_checkpoint_message(message) {
                return None;
            }
            user_text_of(message).map(|_| idx)
        })
        .unwrap_or(0);
    let tail_has_tools = messages[last_user..].iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    });
    if tail_has_tools {
        return last_user;
    }
    // A trailing user/assistant pair with no tools still needs the previous
    // tool-bearing round; otherwise the last results vanish behind the summary.
    messages[..last_user]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(idx, message)| {
            if is_compaction_checkpoint_message(message) {
                return None;
            }
            user_text_of(message).map(|_| idx)
        })
        .unwrap_or(last_user)
}

/// How many messages of the open round sit in `messages` before a checkpoint.
#[must_use]
pub fn last_round_kept_count(messages: &[Message]) -> Option<usize> {
    let checkpoint = messages
        .iter()
        .rposition(is_compaction_checkpoint_message)?;
    if checkpoint == 0 {
        return None;
    }
    let start = last_round_start(&messages[..checkpoint]);
    Some(checkpoint.saturating_sub(start))
}

fn last_round_slice(messages: &[Message]) -> &[Message] {
    let start = last_round_start(messages).min(messages.len());
    &messages[start..]
}

pub(super) fn bound_last_round(messages: &[Message]) -> Vec<Message> {
    let mut round = messages.to_vec();
    for message in &mut round {
        for block in &mut message.content {
            match block {
                ContentBlock::ToolResult {
                    content,
                    content_blocks,
                    ..
                } => {
                    if truncate_retained_block(
                        "tool result",
                        content,
                        LAST_ROUND_TOOL_RESULT_MAX_CHARS,
                    ) {
                        *content_blocks = None;
                    }
                }
                ContentBlock::Thinking {
                    thinking,
                    signature,
                    ..
                } if signature.is_none() => {
                    truncate_retained_block(
                        "thinking block",
                        thinking,
                        LAST_ROUND_THINKING_MAX_CHARS,
                    );
                }
                _ => {}
            }
        }
    }
    round
}

fn tool_result_ids(message: &Message) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect()
}

fn has_tool_result_id(message: &Message, id: &str) -> bool {
    message.content.iter().any(|block| {
        matches!(
            block,
            ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id
        )
    })
}

pub(crate) fn validate_last_round_coverage(
    original: &[Message],
    replacement: &[Message],
) -> Result<()> {
    let last_round = last_round_slice(original);
    if last_round.is_empty() {
        return Ok(());
    }
    if let Some(text) = last_round.iter().find_map(user_text_of) {
        let kept = replacement.iter().any(|message| {
            user_text_of(message).is_some_and(|kept| {
                kept == text || text.starts_with(&kept) || kept.starts_with(&text)
            })
        });
        if !kept {
            anyhow::bail!(
                "Compaction coverage floor: the last user message was dropped; history was not replaced."
            );
        }
    }
    for id in last_round.iter().flat_map(tool_result_ids) {
        if !replacement
            .iter()
            .any(|message| has_tool_result_id(message, &id))
        {
            anyhow::bail!(
                "Compaction coverage floor: last-round tool result {id} was dropped; history was not replaced."
            );
        }
    }
    if last_round
        .iter()
        .any(|message| message.role.is_assistant_like())
        && !replacement
            .iter()
            .any(|message| message.role.is_assistant_like())
    {
        anyhow::bail!(
            "Compaction coverage floor: last-round assistant output was dropped; history was not replaced."
        );
    }
    Ok(())
}

pub(super) fn measure_coverage(
    original: &[Message],
    replacement: &[Message],
    path: CompactionPath,
    anchors_chars: usize,
) -> CompactionCoverage {
    let last_round = last_round_slice(replacement);
    CompactionCoverage {
        path,
        last_round_messages: last_round.len(),
        last_round_tool_results: last_round.iter().flat_map(tool_result_ids).count(),
        last_round_assistant: last_round
            .iter()
            .any(|message| message.role.is_assistant_like()),
        dropped_messages: original.len().saturating_sub(replacement.len()),
        anchors_chars,
    }
}

pub(super) fn build_replacement_history(
    messages: &[Message],
    checkpoint_text: &str,
) -> Result<Vec<Message>> {
    let start = last_round_start(messages);
    let mut retained =
        retained_user_messages(&messages[..start], COMPACT_RETAINED_USER_MESSAGE_MAX_TOKENS);
    retained.extend(bound_last_round(&messages[start..]));
    retained.push(compaction_checkpoint_message(&SystemPrompt::Text(
        checkpoint_text.to_string(),
    )));
    validate_last_round_coverage(messages, &retained)?;
    Ok(retained)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::{COMPACTION_SUMMARY_MARKER, compaction_checkpoint_message};
    use crate::models::ContentBlock;
    use serde_json::json;

    fn msg(role: &str, text: &str) -> Message {
        Message {
            role: Role::from(role),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn tool_use(id: &str, name: &str, input: serde_json::Value) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
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

    #[test]
    fn coverage_floor_rejects_a_replacement_that_drops_last_round_tools() {
        let original = vec![
            msg("user", "Run the failing test."),
            msg("assistant", "Running."),
            tool_use("live", "Bash", json!({"command": "cargo test"})),
            tool_result("live", "test session_store::roundtrip ... FAILED"),
        ];
        let gutting = vec![
            msg("user", "Run the failing test."),
            compaction_checkpoint_message(&SystemPrompt::Text(format!(
                "{COMPACTION_SUMMARY_MARKER} and kept going"
            ))),
        ];
        let error = validate_last_round_coverage(&original, &gutting)
            .expect_err("dropping the last tool result must fail the coverage floor");
        assert!(error.to_string().contains("tool result live"), "{error}");
        assert!(validate_last_round_coverage(&original, &original).is_ok());
    }

    #[test]
    fn coverage_floor_rejects_a_replacement_that_drops_last_round_assistant() {
        let original = vec![
            msg("user", "What failed?"),
            msg("assistant", "session_store::roundtrip panics on reload."),
        ];
        let error = validate_last_round_coverage(&original, &[msg("user", "What failed?")])
            .expect_err("dropping last-round assistant text must fail closed");
        assert!(error.to_string().contains("assistant"), "{error}");
    }

    #[test]
    fn last_round_starts_at_the_latest_plain_user_message() {
        let messages = vec![
            msg("user", "older"),
            msg("assistant", "working"),
            tool_result("old", "stale"),
            msg("user", "Run the suite now."),
            msg("assistant", "Rerunning."),
            tool_use("live", "Bash", json!({"command": "cargo test"})),
            tool_result("live", "ok"),
        ];
        assert_eq!(last_round_start(&messages), 3); // last user with tools
        let kept = bound_last_round(&messages[last_round_start(&messages)..]);
        assert!(kept.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { tool_use_id, content, .. }
                        if tool_use_id == "live" && content == "ok"
                )
            })
        }));
    }
}
