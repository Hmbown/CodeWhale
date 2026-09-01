//! Last-round coverage floor for compaction replacement history.
//!
//! Compaction may summarize older turns, but the latest user round (user
//! text plus following assistant/tool results) must survive verbatim,
//! bounded, or the pass is refused. See [`SURVIVAL_CONTRACT.md`].

use anyhow::Result;

use crate::models::{ContentBlock, Message, SystemPrompt};

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

fn is_plain_user_text(message: &Message) -> bool {
    !is_compaction_checkpoint_message(message) && user_text_of(message).is_some()
}

fn last_plain_user_index(messages: &[Message], end: usize) -> Option<usize> {
    messages[..end]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(idx, message)| is_plain_user_text(message).then_some(idx))
}

fn slice_has_tool_result(messages: &[Message], start: usize) -> bool {
    messages[start..].iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    })
}

#[must_use]
pub(crate) fn last_round_start(messages: &[Message]) -> usize {
    let Some(last_user) = last_plain_user_index(messages, messages.len()) else {
        return 0;
    };
    if slice_has_tool_result(messages, last_user) {
        return last_user;
    }
    // Trailing toolless user/assistant turns still need the previous
    // tool-bearing round; otherwise those results vanish behind the summary.
    // If no tool round exists, keep only the latest user turn so chat-only
    // sessions can still summarize older text.
    let mut candidate = last_user;
    loop {
        let Some(prev) = last_plain_user_index(messages, candidate) else {
            return last_user;
        };
        if slice_has_tool_result(messages, prev) {
            return prev;
        }
        candidate = prev;
    }
}

#[must_use]
pub(crate) fn last_round_range(messages: &[Message]) -> (usize, usize) {
    let start = last_round_start(messages).min(messages.len());
    let end = messages[start..]
        .iter()
        .position(is_compaction_checkpoint_message)
        .map_or(messages.len(), |rel| start + rel);
    (start, end)
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
    let (start, end) = last_round_range(messages);
    &messages[start..end]
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

fn tool_use_ids(message: &Message) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn has_tool_use_id(message: &Message, id: &str) -> bool {
    message.content.iter().any(|block| {
        matches!(
            block,
            ContentBlock::ToolUse { id: seen, .. } if seen == id
        )
    })
}

fn assistant_text_of(message: &Message) -> Option<String> {
    if !message.role.is_assistant_like() {
        return None;
    }
    let text = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A retained copy may be truncated (`bound_last_round` caps oversized blocks),
/// so a prefix either way counts as survival -- but nothing weaker does.
fn survives(text: &str, replacement: &[Message], of: fn(&Message) -> Option<String>) -> bool {
    replacement.iter().any(|message| {
        of(message)
            .is_some_and(|kept| kept == text || text.starts_with(&kept) || kept.starts_with(text))
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
    // Every user turn in the round, not the first one `find_map` happens to
    // reach. `last_round_start` walks back past a toolless tail to the previous
    // tool-bearing turn, so the round routinely spans two user messages -- and
    // checking only the earliest let a rewrite drop the *latest* one, which is
    // the turn this whole contract exists to keep.
    for text in last_round.iter().filter_map(user_text_of) {
        if !survives(&text, replacement, user_text_of) {
            anyhow::bail!(
                "Compaction coverage floor: a last-round user message was dropped; history was not replaced."
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
    // The call, not just its result. Keeping a tool_result whose tool_use was
    // summarized away leaves an orphaned result that providers reject outright.
    for id in last_round.iter().flat_map(tool_use_ids) {
        if !replacement
            .iter()
            .any(|message| has_tool_use_id(message, &id))
        {
            anyhow::bail!(
                "Compaction coverage floor: last-round tool call {id} was dropped; history was not replaced."
            );
        }
    }
    // Match the assistant's actual output. An existential "some assistant
    // message survived" check passed on a replacement whose only assistant
    // message was the summary the rewrite had just written.
    for text in last_round.iter().filter_map(assistant_text_of) {
        if !survives(&text, replacement, assistant_text_of) {
            anyhow::bail!(
                "Compaction coverage floor: last-round assistant output was dropped; history was not replaced."
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

pub(crate) fn require_text_survives(
    replacement: &[Message],
    needle: &str,
    label: &str,
) -> Result<()> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Ok(());
    }
    let kept = replacement.iter().any(|message| {
        message.content.iter().any(|block| match block {
            ContentBlock::Text { text, .. } => text.contains(needle),
            ContentBlock::ToolResult { content, .. } => content.contains(needle),
            _ => false,
        })
    });
    if !kept {
        anyhow::bail!("Compaction coverage floor: {label} was dropped; history was not replaced.");
    }
    Ok(())
}

pub(crate) fn validate_survival_contract(
    original: &[Message],
    replacement: &[Message],
    anchors: Option<&str>,
) -> Result<()> {
    validate_last_round_coverage(original, replacement)?;
    let checkpoints = replacement
        .iter()
        .filter(|message| is_compaction_checkpoint_message(message))
        .count();
    if checkpoints == 0 {
        anyhow::bail!(
            "Compaction coverage floor: checkpoint receipt was dropped; history was not replaced."
        );
    }
    if checkpoints > 1 {
        anyhow::bail!(
            "Compaction coverage floor: prior summaries were duplicated; history was not replaced."
        );
    }
    if let Some(anchors) = anchors {
        require_text_survives(replacement, anchors, "pinned /anchor text")?;
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
    anchors: Option<&str>,
) -> Result<Vec<Message>> {
    let (start, end) = last_round_range(messages);
    let mut retained =
        retained_user_messages(&messages[..start], COMPACT_RETAINED_USER_MESSAGE_MAX_TOKENS);
    retained.extend(bound_last_round(&messages[start..end]));
    retained.push(compaction_checkpoint_message(&SystemPrompt::Text(
        checkpoint_text.to_string(),
    )));
    validate_survival_contract(messages, &retained, anchors)?;
    Ok(retained)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::{COMPACTION_SUMMARY_MARKER, compaction_checkpoint_message};
    use crate::models::{ContentBlock, Role};
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

    fn checkpoint(summary: &str) -> Message {
        compaction_checkpoint_message(&SystemPrompt::Text(format!(
            "{COMPACTION_SUMMARY_MARKER}: {summary}"
        )))
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
            checkpoint("and kept going"),
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

    /// The round spans the tool-bearing turn *and* the toolless tail after it,
    /// because `last_round_start` walks back for the tools. Checking only the
    /// first user text it found meant a rewrite could keep the older question
    /// and drop the one the person actually just asked.
    #[test]
    fn coverage_floor_rejects_a_replacement_that_drops_the_latest_user_turn() {
        let original = vec![
            msg("user", "Run the suite."),
            msg("assistant", "Running."),
            tool_use("live", "Bash", json!({"command": "cargo test"})),
            tool_result("live", "ok"),
            msg("user", "Now ship it."),
            msg("assistant", "Shipping."),
        ];
        assert_eq!(last_round_start(&original), 0, "round must span both turns");

        let drops_latest = vec![
            msg("user", "Run the suite."),
            msg("assistant", "Running."),
            tool_use("live", "Bash", json!({"command": "cargo test"})),
            tool_result("live", "ok"),
            checkpoint("then shipped"),
        ];
        let error = validate_last_round_coverage(&original, &drops_latest)
            .expect_err("dropping the latest user turn must fail the coverage floor");
        assert!(error.to_string().contains("user message"), "{error}");
        assert!(validate_last_round_coverage(&original, &original).is_ok());
    }

    /// A surviving `tool_result` whose `tool_use` was summarized away is an
    /// orphan the provider rejects, so the floor must cover the call too.
    #[test]
    fn coverage_floor_rejects_a_replacement_that_drops_the_tool_call() {
        let original = vec![
            msg("user", "Run the failing test."),
            msg("assistant", "Running."),
            tool_use("live", "Bash", json!({"command": "cargo test"})),
            tool_result("live", "FAILED"),
        ];
        let orphaned = vec![
            msg("user", "Run the failing test."),
            msg("assistant", "Running."),
            tool_result("live", "FAILED"),
            checkpoint("and it failed"),
        ];
        let error = validate_last_round_coverage(&original, &orphaned)
            .expect_err("dropping the tool call must fail the coverage floor");
        assert!(error.to_string().contains("tool call live"), "{error}");
    }

    /// "Some assistant message survived" was satisfied by the summary the
    /// rewrite had just written, so the round's real output could vanish.
    #[test]
    fn coverage_floor_rejects_assistant_output_replaced_by_a_summary() {
        let original = vec![
            msg("user", "What failed?"),
            msg("assistant", "session_store::roundtrip panics on reload."),
        ];
        let summarized = vec![
            msg("user", "What failed?"),
            msg("assistant", "Earlier we discussed several test failures."),
        ];
        let error = validate_last_round_coverage(&original, &summarized)
            .expect_err("substituting a summary for the round's output must fail closed");
        assert!(error.to_string().contains("assistant"), "{error}");
    }

    #[test]
    fn survival_contract_rejects_dropped_anchors_and_receipts() {
        let original = vec![msg("user", "Keep the pin."), msg("assistant", "Anchored.")];
        let without_receipt = vec![msg("user", "Keep the pin."), msg("assistant", "Anchored.")];
        let error = validate_survival_contract(&original, &without_receipt, Some("ship 0.9.12"))
            .expect_err("missing checkpoint receipt must fail closed");
        assert!(error.to_string().contains("receipt"), "{error}");

        let without_anchor = vec![
            msg("user", "Keep the pin."),
            msg("assistant", "Anchored."),
            checkpoint("progress without the pin"),
        ];
        let error = validate_survival_contract(&original, &without_anchor, Some("ship 0.9.12"))
            .expect_err("dropped /anchor text must fail closed");
        assert!(error.to_string().contains("anchor"), "{error}");
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
        let (start, end) = last_round_range(&messages);
        let kept = bound_last_round(&messages[start..end]);
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

    #[test]
    fn last_round_walks_back_through_toolless_tails_to_the_tool_round() {
        let original = vec![
            msg("user", "Run the failing test."),
            msg("assistant", "Running."),
            tool_use("live", "Bash", json!({"command": "cargo test"})),
            tool_result("live", "test session_store::roundtrip ... FAILED"),
            msg("user", "ok thanks"),
            msg("assistant", "you're welcome"),
            msg("user", "one more thing"),
            msg("assistant", "sure"),
        ];
        assert_eq!(last_round_start(&original), 0);
        let next = format!("{COMPACTION_SUMMARY_MARKER}: keep the failing test result");
        let replaced = build_replacement_history(&original, &next, None)
            .expect("toolless tails must not drop the last tool result");
        assert!(replaced.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { tool_use_id, content, .. }
                        if tool_use_id == "live" && content.contains("FAILED")
                )
            })
        }));
    }

    #[test]
    fn chat_only_history_keeps_the_latest_user_round() {
        let messages = vec![
            msg("user", "hello"),
            msg("assistant", "hi"),
            msg("user", "how are you"),
            msg("assistant", "fine"),
        ];
        assert_eq!(last_round_start(&messages), 2);
    }

    #[derive(serde::Deserialize)]
    struct FixtureMatrix {
        schema_version: u32,
        cases: Vec<FixtureCase>,
    }

    #[derive(serde::Deserialize)]
    struct FixtureCase {
        id: String,
        expect: String,
        #[serde(default)]
        anchors: Option<String>,
        original: Vec<Message>,
        replacement: Vec<Message>,
        #[serde(default)]
        last_round_start: Option<usize>,
    }

    #[test]
    fn fixture_matrix_enforces_survival_contract() {
        let matrix: FixtureMatrix =
            serde_json::from_str(include_str!("fixtures/matrix.json")).expect("matrix.json");
        assert_eq!(matrix.schema_version, 1);
        assert!(
            matrix.cases.len() >= 8,
            "fixture matrix must cover last-round, toolless-tail, chat-only, anchor, and receipt cases"
        );
        for case in &matrix.cases {
            if let Some(start) = case.last_round_start {
                assert_eq!(
                    last_round_start(&case.original),
                    start,
                    "{} last_round_start",
                    case.id
                );
            }
            let result = validate_survival_contract(
                &case.original,
                &case.replacement,
                case.anchors.as_deref(),
            );
            match case.expect.as_str() {
                "pass" => {
                    result.unwrap_or_else(|error| panic!("{} should pass: {error}", case.id));
                }
                "fail" => {
                    result.expect_err(&format!("{} should fail closed", case.id));
                }
                other => panic!("{}: unknown expect {other}", case.id),
            }
        }
    }

    #[test]
    fn second_compact_does_not_duplicate_prior_summaries() {
        let first = vec![
            msg("user", "older"),
            msg("user", "Run the suite now."),
            msg("assistant", "Rerunning."),
            tool_use("live", "Bash", json!({"command": "cargo test"})),
            tool_result("live", "ok"),
            checkpoint("first handoff: suite still running"),
        ];
        let next = format!(
            "{COMPACTION_SUMMARY_MARKER}: second handoff with User-pinned anchors (verbatim):\nship 0.9.12"
        );
        let replaced = build_replacement_history(&first, &next, Some("ship 0.9.12"))
            .expect("second compact must keep last round and one receipt");
        let checkpoints = replaced
            .iter()
            .filter(|message| is_compaction_checkpoint_message(message))
            .count();
        assert_eq!(checkpoints, 1, "{replaced:?}");
        assert!(replaced.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "live"
                )
            })
        }));
        require_text_survives(&replaced, "ship 0.9.12", "pinned /anchor text").unwrap();
    }
}
