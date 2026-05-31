//! Agent-driven context purging.
//!
//! Unlike compaction (which summarises old messages via LLM), purge lets the
//! agent analyse the conversation history and surgically remove or rewrite
//! individual messages that are no longer needed. The agent uses the
//! `purge_context` tool to submit a list of operations; the engine validates
//! and executes them.

use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use tokio::sync::mpsc::Sender;

use crate::core::events::Event;
use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, Tool};

// ── Prompt‑building constants ──────────────────────────────────────────────

const TEXT_SNIPPET_CHARS: usize = 60;
const TOOL_RESULT_SNIPPET_CHARS: usize = 80;
const TOOL_USE_ARGS_CHARS: usize = 120;

// ── Prompt instruction template ─────────────────────────────────────────────

const PURGE_INSTRUCTIONS: &str = "\
## Context Purge

Free space in the conversation's context window. Below is the current history with stable numeric IDs.\
Identify content that is clearly no longer needed for the ongoing work.

### Operations

remove  — Delete an entire message by its ID. Example:
          {\"op\": \"remove\", \"msg\": 3}

replace — Rewrite part of a specific content block using regex substitution.
          pattern uses Rust regex syntax. Must specify both `block` and
          `pattern` and `with`. Example:
          {\"op\": \"replace\", \"msg\": 7, \"block\": 0,
           \"pattern\": \"read \\\\d+ files\", \"with\": \"read files\"}

### Pairing rule

Every ToolUse block is paired with its ToolResult. If you remove a message
containing a tool call, its result will be removed too — and vice versa. You
do not need to list both.

### What to keep

- Important decisions, architectural choices
- File paths that are still relevant
- Tool outputs that contain information not yet acted upon

### What to prune

- Verbose tool outputs whose information has been fully consumed
- Redundant confirmations (\"done\", \"ok\", \"that worked\")
- Superseded file reads (the file was later written/modified)
- Boilerplate that the model already incorporated into later work

Be conservative. When in doubt, keep the message.

### Conversation
";

// ── Purge operation types ───────────────────────────────────────────────────

/// A single purge operation submitted by the agent.
#[derive(Debug, Clone)]
pub enum PurgeOp {
    /// Remove an entire message (plus its tool-call/result counterpart).
    Remove { msg_id: usize },
    /// Regex-replace within a specific content block.
    Replace {
        msg_id: usize,
        block_idx: usize,
        pattern: Regex,
        with: String,
    },
}

/// Result of executing purge operations.
#[derive(Debug, Clone)]
pub struct PurgeResult {
    /// The remaining messages after all operations.
    pub messages: Vec<Message>,
    /// How many messages were removed.
    pub removed_count: usize,
    /// How many replace operations were applied.
    pub replaced_count: usize,
}

// ── Event emission helpers ──────────────────────────────────────────────────

/// Emit a `PurgeStarted` event to the UI.
pub async fn emit_purge_started(tx: &Sender<Event>, message: String) {
    let _ = tx.send(Event::PurgeStarted { message }).await;
}

/// Emit a `PurgeCompleted` event to the UI.
pub async fn emit_purge_completed(
    tx: &Sender<Event>,
    messages_before: usize,
    messages_after: usize,
    removed_count: usize,
    replaced_count: usize,
    message: String,
) {
    let _ = tx
        .send(Event::PurgeCompleted {
            messages_before,
            messages_after,
            removed_count,
            replaced_count,
            message,
        })
        .await;
}

/// Emit a `PurgeFailed` event to the UI.
pub async fn emit_purge_failed(tx: &Sender<Event>, message: String) {
    let _ = tx.send(Event::PurgeFailed { message }).await;
}

// ── Prompt builder ──────────────────────────────────────────────────────────

/// Build the purge request user message — a formatted listing of the current
/// conversation with ephemeral sequential IDs.
pub fn build_purge_prompt(messages: &[Message]) -> String {
    let mut buf = String::with_capacity(messages.len().saturating_mul(256));
    buf.push_str(PURGE_INSTRUCTIONS);

    for (idx, msg) in messages.iter().enumerate() {
        let msg_id = idx + 1; // 1‑based for the agent
        if msg.role == "user" {
            // User messages: always a single block — omit block index.
            format_user_message(&mut buf, msg_id, msg);
        } else {
            // Assistant messages: may be multi‑block — show block indices.
            let _ = writeln!(buf, "[{msg_id}] {role}", role = msg.role);
            for (blk_idx, block) in msg.content.iter().enumerate() {
                format_content_block(&mut buf, blk_idx, block);
            }
            buf.push('\n');
        }
    }

    buf
}

fn format_user_message(buf: &mut String, msg_id: usize, msg: &Message) {
    let block = msg.content.first();
    match block {
        Some(ContentBlock::Text { text, .. }) => {
            let snippet = truncate_str(text, TEXT_SNIPPET_CHARS);
            let _ = writeln!(
                buf,
                "[{msg_id}] user  Text ({len} chars): \"{snippet}\"",
                len = text.len()
            );
        }
        Some(ContentBlock::ToolResult {
            content,
            tool_use_id,
            ..
        }) => {
            let snippet = truncate_str(content, TOOL_RESULT_SNIPPET_CHARS);
            let _ = writeln!(
                buf,
                "[{msg_id}] user  ToolResult (id={tool_use_id}, {len} chars): \"{snippet}\"",
                len = content.len(),
            );
        }
        _ => {
            let _ = writeln!(buf, "[{msg_id}] user  (non‑text block)");
        }
    }
}

fn format_content_block(buf: &mut String, blk_idx: usize, block: &ContentBlock) {
    match block {
        ContentBlock::Text { text, .. } => {
            let snippet = truncate_str(text, TEXT_SNIPPET_CHARS);
            let _ = writeln!(
                buf,
                "  [{blk_idx}] Text ({len} chars): \"{snippet}\"",
                len = text.len(),
            );
        }
        ContentBlock::Thinking { .. } => {
            // Omit thinking blocks — API-mandated on tool-call messages;
            // the agent cannot remove them, so listing them only adds noise.
        }
        ContentBlock::ToolUse {
            name, input, id, ..
        } => {
            let args = serde_json::to_string(input).unwrap_or_default();
            let args_preview = truncate_str(&args, TOOL_USE_ARGS_CHARS);
            let _ = writeln!(
                buf,
                "  [{blk_idx}] ToolUse ({name}, id={id}, args={args_preview})"
            );
        }
        ContentBlock::ToolResult {
            content,
            tool_use_id,
            ..
        } => {
            let snippet = truncate_str(content, TOOL_RESULT_SNIPPET_CHARS);
            let _ = writeln!(
                buf,
                "  [{blk_idx}] ToolResult (id={tool_use_id}, {len} chars): \"{snippet}\"",
                len = content.len(),
            );
        }
        ContentBlock::ServerToolUse {
            name, input, id, ..
        } => {
            let args = serde_json::to_string(input).unwrap_or_default();
            let args_preview = truncate_str(&args, TOOL_USE_ARGS_CHARS);
            let _ = writeln!(
                buf,
                "  [{blk_idx}] ServerToolUse ({name}, id={id}, args={args_preview})"
            );
        }
        ContentBlock::ToolSearchToolResult {
            tool_use_id,
            content,
            ..
        } => {
            let snippet = truncate_str(&content.to_string(), TOOL_RESULT_SNIPPET_CHARS);
            let _ = writeln!(
                buf,
                "  [{blk_idx}] ToolSearchToolResult (id={tool_use_id}, content={snippet})"
            );
        }
        ContentBlock::CodeExecutionToolResult {
            tool_use_id,
            content,
            ..
        } => {
            let snippet = truncate_str(&content.to_string(), TOOL_RESULT_SNIPPET_CHARS);
            let _ = writeln!(
                buf,
                "  [{blk_idx}] CodeExecutionToolResult (id={tool_use_id}, content={snippet})"
            );
        }
    }
}

fn truncate_str(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let take = max_chars.saturating_sub(3);
    let mut out: String = text.chars().take(take).collect();
    out.push_str("...");
    out
}

// ── Operation parser ────────────────────────────────────────────────────────

/// Parse the `purge_context` tool input JSON into a list of validated
/// `PurgeOp`s. Returns an error string on invalid input.
pub fn parse_purge_operations(
    input: &serde_json::Value,
    message_count: usize,
) -> Result<Vec<PurgeOp>, String> {
    let ops = input
        .get("operations")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing or invalid 'operations' array".to_string())?;

    let mut parsed = Vec::with_capacity(ops.len());

    for (i, op) in ops.iter().enumerate() {
        let op_type = op
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("operation[{i}]: missing 'op' field"))?;

        let msg = op
            .get("msg")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("operation[{i}]: missing or invalid 'msg'"))?;

        let msg_id = usize::try_from(msg).unwrap_or(usize::MAX);
        if msg_id == 0 || msg_id > message_count {
            return Err(format!(
                "operation[{i}]: msg {msg} out of range (1–{message_count})"
            ));
        }

        match op_type {
            "remove" => {
                parsed.push(PurgeOp::Remove { msg_id });
            }
            "replace" => {
                let block_idx = op
                    .get("block")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .ok_or_else(|| format!("operation[{i}]: 'replace' requires 'block'"))?;

                let pattern_str = op
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| format!("operation[{i}]: 'replace' requires 'pattern'"))?;

                let with = op
                    .get("with")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let pattern = Regex::new(pattern_str)
                    .map_err(|e| format!("operation[{i}]: invalid regex pattern: {e}"))?;

                parsed.push(PurgeOp::Replace {
                    msg_id,
                    block_idx,
                    pattern,
                    with,
                });
            }
            other => {
                return Err(format!(
                    "operation[{i}]: unknown op '{other}' (expected 'remove' or 'replace')"
                ));
            }
        }
    }

    Ok(parsed)
}

// ── Operation executor ──────────────────────────────────────────────────────

/// Execute a list of purge operations against the message history.
///
/// Operations are processed in the order given but effective removal runs
/// from highest index to lowest to keep earlier indices stable. After all
/// user-requested operations, tool‑call/result pair cascading runs to
/// prevent orphaned blocks.
pub fn execute_purge_operations(messages: &[Message], ops: &[PurgeOp]) -> PurgeResult {
    let mut msgs = messages.to_vec();
    let mut msg_indices_to_remove: HashSet<usize> = HashSet::new();
    let mut replaced_count = 0usize;

    // Phase 1: collect removes and apply replaces.
    for op in ops {
        match op {
            PurgeOp::Remove { msg_id } => {
                let idx = msg_id.saturating_sub(1);
                if idx < msgs.len() {
                    msg_indices_to_remove.insert(idx);
                }
            }
            PurgeOp::Replace {
                msg_id,
                block_idx,
                pattern,
                with,
            } => {
                let idx = msg_id.saturating_sub(1);
                if idx >= msgs.len() {
                    continue;
                }
                if let Some(block) = msgs[idx].content.get_mut(*block_idx) {
                    let old_text = block_content_text(block).to_string();
                    let new_text = pattern.replace_all(&old_text, with.as_str()).to_string();
                    apply_block_replacement(block, &new_text);
                    replaced_count = replaced_count.saturating_add(1);
                }
            }
        }
    }

    // Phase 2: cascade removal to tool-call/result counterparts.
    cascade_tool_pair_removals(&msgs, &mut msg_indices_to_remove);

    // Phase 3: sort indices descending and remove.
    let mut to_remove: Vec<usize> = msg_indices_to_remove.into_iter().collect();
    to_remove.sort_unstable_by(|a, b| b.cmp(a));

    let removed_count = to_remove.len();
    for idx in to_remove {
        msgs.remove(idx);
    }

    PurgeResult {
        messages: msgs,
        removed_count,
        replaced_count,
    }
}

/// When a message containing a ToolUse or ToolResult is marked for removal,
/// cascade that removal to its counterpart so the API never sees orphaned
/// blocks. Runs a fixpoint loop until the remove set is closed under pairing.
fn cascade_tool_pair_removals(messages: &[Message], remove_set: &mut HashSet<usize>) {
    if remove_set.is_empty() {
        return;
    }

    // Build lookup maps: tool_use id → message index, tool_result id → message index.
    let mut call_id_to_idx: HashMap<String, usize> = HashMap::new();
    let mut result_id_to_idx: HashMap<String, usize> = HashMap::new();

    for (idx, msg) in messages.iter().enumerate() {
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { id, .. } => {
                    call_id_to_idx.insert(id.clone(), idx);
                }
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    result_id_to_idx.insert(tool_use_id.clone(), idx);
                }
                _ => {}
            }
        }
    }

    // Fixpoint: when a tool-call is removed, also remove its result (and vice versa).
    let max_iters = messages.len().max(10);
    for _ in 0..max_iters {
        let snapshot: Vec<usize> = remove_set.iter().copied().collect();
        let mut changed = false;

        for idx in snapshot {
            let msg = &messages[idx];
            for block in &msg.content {
                match block {
                    ContentBlock::ToolUse { id, .. } => {
                        if let Some(&result_idx) = result_id_to_idx.get(id)
                            && remove_set.insert(result_idx)
                        {
                            changed = true;
                        }
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        if let Some(&call_idx) = call_id_to_idx.get(tool_use_id)
                            && remove_set.insert(call_idx)
                        {
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        if !changed {
            break;
        }
    }
}

fn block_content_text(block: &ContentBlock) -> &str {
    match block {
        ContentBlock::Text { text, .. } => text,
        ContentBlock::ToolResult { content, .. } => content,
        _ => "",
    }
}

fn apply_block_replacement(block: &mut ContentBlock, new_text: &str) {
    match block {
        ContentBlock::Text { text, .. } => {
            *text = new_text.to_string();
        }
        ContentBlock::ToolResult { content, .. } => {
            *content = new_text.to_string();
        }
        _ => {}
    }
}

// ── Tool definition builder ──────────────────────────────────────────────────

/// Build the `purge_context` tool definition sent to the model during a purge
/// turn. This tool is ad-hoc — it is not registered in the normal tool catalog
/// and has no dispatch handler.
pub fn build_purge_tool() -> Tool {
    Tool {
        tool_type: None,
        name: "purge_context".to_string(),
        description: "Remove or condense conversation history to free context window space."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "operations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {"type": "string", "enum": ["remove", "replace"]},
                            "msg": {"type": "integer"},
                            "block": {"type": "integer"},
                            "pattern": {"type": "string"},
                            "with": {"type": "string"}
                        },
                        "required": ["op", "msg"]
                    }
                }
            },
            "required": ["operations"]
        }),
        allowed_callers: None,
        defer_loading: None,
        input_examples: None,
        strict: Some(true),
        cache_control: None,
    }
}

// ── Orchestration ────────────────────────────────────────────────────────────

/// Run a full purge cycle: build the prompt, call the model with the
/// `purge_context` tool, parse the response, and execute the operations.
///
/// Returns the `PurgeResult` with the modified message list on success,
/// or a human-readable error string on failure.
///
/// Cost reporting is handled internally as a side-effect of the API call.
/// The caller is responsible for emitting start/completed/failed events
/// and for replacing the session message list with `PurgeResult.messages`.
pub async fn run_purge(
    client: &impl LlmClient,
    messages: &[Message],
    model: &str,
    reasoning_effort: Option<String>,
    max_tokens: u32,
) -> Result<PurgeResult, String> {
    // 1. Build the purge prompt from the current conversation.
    let prompt = build_purge_prompt(messages);

    // 2. Clone messages and inject the prompt as a user message.
    let mut request_messages = messages.to_vec();
    request_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: prompt,
            cache_control: None,
        }],
    });

    // 3. Build the tool definition and the request.
    let purge_tool = build_purge_tool();
    let request = MessageRequest {
        model: model.to_string(),
        messages: request_messages,
        max_tokens,
        system: None,
        tools: Some(vec![purge_tool]),
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort,
        stream: Some(false),
        temperature: Some(0.2),
        top_p: None,
    };

    // 4. Send to the model.
    let response = client
        .create_message(request)
        .await
        .map_err(|e| format!("Purge API error: {e}"))?;

    crate::cost_status::report(&response.model, &response.usage);

    // 5. Find the `purge_context` tool call in the response.
    let tool_input = response.content.iter().find_map(|block| {
        if let ContentBlock::ToolUse { name, input, .. } = block
            && name == "purge_context"
        {
            return Some(input.clone());
        }
        None
    });

    match tool_input {
        Some(input) => {
            let ops = parse_purge_operations(&input, messages.len())
                .map_err(|e| format!("Purge parse error: {e}"))?;
            Ok(execute_purge_operations(messages, &ops))
        }
        None => Err("Purge: model did not call purge_context tool".to_string()),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg_text(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn msg_tool_use(id: &str, name: &str, input: serde_json::Value) -> Message {
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
                caller: None,
            }],
        }
    }

    fn msg_tool_result(id: &str, content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    #[test]
    fn parse_remove_operations() {
        let input = json!({
            "operations": [
                {"op": "remove", "msg": 1},
                {"op": "remove", "msg": 3}
            ]
        });
        let ops = parse_purge_operations(&input, 5).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], PurgeOp::Remove { msg_id: 1 }));
        assert!(matches!(ops[1], PurgeOp::Remove { msg_id: 3 }));
    }

    #[test]
    fn parse_replace_operation() {
        let input = json!({
            "operations": [
                {"op": "replace", "msg": 2, "block": 0, "pattern": "hello", "with": "hi"}
            ]
        });
        let ops = parse_purge_operations(&input, 5).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], PurgeOp::Replace { msg_id: 2, .. }));
    }

    #[test]
    fn parse_rejects_out_of_range_msg() {
        let input = json!({"operations": [{"op": "remove", "msg": 10}]});
        assert!(parse_purge_operations(&input, 5).is_err());
    }

    #[test]
    fn parse_rejects_invalid_regex() {
        let input = json!({
            "operations": [{"op": "replace", "msg": 1, "block": 0, "pattern": "[", "with": "x"}]
        });
        assert!(parse_purge_operations(&input, 5).is_err());
    }

    #[test]
    fn execute_remove_works() {
        let msgs = vec![
            msg_text("user", "hello"),
            msg_text("assistant", "hi there"),
            msg_text("user", "bye"),
        ];
        let ops = vec![PurgeOp::Remove { msg_id: 2 }];
        let result = execute_purge_operations(&msgs, &ops);
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.messages.len(), 2);
    }

    #[test]
    fn execute_replace_text_block() {
        let msgs = vec![msg_text("assistant", "Hello world! Hello again!")];
        let pattern = Regex::new("Hello").unwrap();
        let ops = vec![PurgeOp::Replace {
            msg_id: 1,
            block_idx: 0,
            pattern,
            with: "Hi".to_string(),
        }];
        let result = execute_purge_operations(&msgs, &ops);
        assert_eq!(result.replaced_count, 1);

        if let ContentBlock::Text { text, .. } = &result.messages[0].content[0] {
            assert_eq!(text, "Hi world! Hi again!");
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn tool_call_result_pairing_cascaded() {
        // Message 2 (idx 1) is a tool call. Message 3 (idx 2) is its result.
        // Removing the tool call should cascade to remove the result too.
        let msgs = vec![
            msg_text("user", "read a file"),
            msg_tool_use("call_01", "read_file", json!({"path": "x.rs"})),
            msg_tool_result("call_01", "fn main() {}"),
        ];
        let ops = vec![PurgeOp::Remove { msg_id: 2 }]; // remove tool call only
        let result = execute_purge_operations(&msgs, &ops);
        // Both tool call and its result should be gone (cascaded).
        assert_eq!(
            result.removed_count, 2,
            "tool call + its result should both be removed"
        );
        assert_eq!(result.messages.len(), 1);
    }

    #[test]
    fn tool_result_removal_cascades_to_call() {
        // Removing the result should cascade to remove the call.
        let msgs = vec![
            msg_text("user", "read a file"),
            msg_tool_use("call_01", "read_file", json!({"path": "x.rs"})),
            msg_tool_result("call_01", "fn main() {}"),
        ];
        let ops = vec![PurgeOp::Remove { msg_id: 3 }]; // remove result only
        let result = execute_purge_operations(&msgs, &ops);
        assert_eq!(
            result.removed_count, 2,
            "tool result + its call should both be removed"
        );
        assert_eq!(result.messages.len(), 1);
    }

    #[test]
    fn prompt_truncates_long_content() {
        let long_text = "x".repeat(200);
        let msgs = vec![msg_text("user", &long_text)];
        let prompt = build_purge_prompt(&msgs);
        assert!(prompt.contains("(200 chars)"));
        assert!(prompt.contains("xxx...")); // truncated
        assert!(!prompt.contains(&long_text));
    }

    #[test]
    fn prompt_shows_full_short_content() {
        let msgs = vec![msg_text("user", "hi")];
        let prompt = build_purge_prompt(&msgs);
        assert!(prompt.contains("\"hi\""));
        assert!(!prompt.contains("..."));
    }

    #[test]
    fn prompt_omits_thinking_blocks() {
        let msgs = vec![Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Thinking {
                    thinking: "let me think...".to_string(),
                },
                ContentBlock::Text {
                    text: "done".to_string(),
                    cache_control: None,
                },
            ],
        }];
        let prompt = build_purge_prompt(&msgs);
        assert!(!prompt.contains("let me think"));
        assert!(prompt.contains("Text (4 chars)"));
    }

    #[test]
    fn build_purge_tool_has_correct_shape() {
        let tool = build_purge_tool();
        assert_eq!(tool.name, "purge_context");
        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["operations"]["type"] == "array");
        let ops_item = &schema["properties"]["operations"]["items"];
        assert_eq!(ops_item["type"], "object");
        let required = ops_item["required"].as_array().unwrap();
        assert!(required.contains(&json!("op")));
        assert!(required.contains(&json!("msg")));
    }

    use crate::llm_client::mock::MockLlmClient;
    use crate::models::{MessageResponse, Usage};

    fn msg_response_with_tool_call(operations: serde_json::Value) -> MessageResponse {
        MessageResponse {
            id: "resp_test".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "call_purge".to_string(),
                name: "purge_context".to_string(),
                input: json!({"operations": operations}),
                caller: None,
            }],
            model: "mock-model".to_string(),
            stop_reason: None,
            stop_sequence: None,
            container: None,
            usage: Usage::default(),
        }
    }

    fn msg_response_without_tool_call(text: &str) -> MessageResponse {
        MessageResponse {
            id: "resp_plain".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            model: "mock".to_string(),
            stop_reason: None,
            stop_sequence: None,
            container: None,
            usage: Usage::default(),
        }
    }

    #[tokio::test]
    async fn run_purge_removes_message() {
        let mock = MockLlmClient::new(vec![]);
        mock.push_message_response(msg_response_with_tool_call(json!([
            {"op": "remove", "msg": 2}
        ])));

        let messages = vec![
            msg_text("user", "hello"),
            msg_text("assistant", "remove me"),
            msg_text("user", "bye"),
        ];

        let result = run_purge(&mock, &messages, "mock", None, 4096)
            .await
            .unwrap();
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.replaced_count, 0);
        assert_eq!(result.messages.len(), 2);

        if let ContentBlock::Text { text, .. } = &result.messages[0].content[0] {
            assert_eq!(text, "hello");
        } else {
            panic!(
                "expected text block, got {:?}",
                &result.messages[0].content[0]
            );
        }
        if let ContentBlock::Text { text, .. } = &result.messages[1].content[0] {
            assert_eq!(text, "bye");
        } else {
            panic!(
                "expected text block, got {:?}",
                &result.messages[1].content[0]
            );
        }
    }

    #[tokio::test]
    async fn run_purge_replace_condenses_text() {
        let mock = MockLlmClient::new(vec![]);
        mock.push_message_response(msg_response_with_tool_call(json!([
            {"op": "replace", "msg": 1, "block": 0, "pattern": "very long and verbose", "with": "short"}
        ])));

        let messages = vec![msg_text("assistant", "this is very long and verbose text")];

        let result = run_purge(&mock, &messages, "mock", None, 4096)
            .await
            .unwrap();
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.replaced_count, 1);

        if let ContentBlock::Text { text, .. } = &result.messages[0].content[0] {
            assert_eq!(text, "this is short text");
        } else {
            panic!(
                "expected text block, got {:?}",
                &result.messages[0].content[0]
            );
        }
    }

    #[tokio::test]
    async fn run_purge_errors_when_no_tool_call() {
        let mock = MockLlmClient::new(vec![]);
        mock.push_message_response(msg_response_without_tool_call("nothing to clean up"));

        let messages = vec![msg_text("user", "hi")];
        let err = run_purge(&mock, &messages, "mock", None, 4096)
            .await
            .unwrap_err();
        assert!(err.contains("did not call purge_context"));
    }

    #[tokio::test]
    async fn run_purge_errors_on_api_failure() {
        // No canned response — MockLlmClient returns an error.
        let mock = MockLlmClient::new(vec![]);
        let messages = vec![msg_text("user", "hi")];
        let err = run_purge(&mock, &messages, "mock", None, 4096)
            .await
            .unwrap_err();
        assert!(err.contains("Purge API error"));
    }
}
