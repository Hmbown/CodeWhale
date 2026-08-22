//! Provider-neutral outbound model-request boundary.
//!
//! The request DTOs in this module are consumed by the TUI transport today
//! and are intentionally free of terminal, HTTP, or provider-client state.
//! Keeping the logical request in `codewhale-core` lets a headless session
//! prepare the same serializable value before the existing TUI client applies
//! provider-specific wire shaping.

use serde::{Deserialize, Serialize};

use crate::role::Role;

/// Request payload handed to the model-client preparation seam.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MessageRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPrompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<serde_json::Value>,
    /// DeepSeek reasoning-effort tier: "off" | "low" | "medium" | "high" | "max".
    /// Translated by the client into DeepSeek's `reasoning_effort` + `thinking` fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

/// Inputs that distinguish a primary agent-turn request.
///
/// Provider-neutral defaults (`stream = true`, no metadata, no provider-side
/// thinking object, and no sampling overrides) are applied once by
/// [`prepare_primary_turn_request`]. Both the production turn loop and its
/// read-only preview use this input so those defaults cannot drift.
#[derive(Debug, Clone)]
pub struct PrimaryTurnRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub system: Option<SystemPrompt>,
    pub tools: Option<Vec<Tool>>,
    pub tool_choice: Option<serde_json::Value>,
    pub reasoning_effort: Option<String>,
}

/// Prepare the provider-neutral request for a primary agent turn.
///
/// This function performs no I/O and no provider-specific transformation.
/// The existing client transport remains responsible for secret redaction,
/// protocol binding, dialect shaping, and endpoint selection.
#[must_use]
pub fn prepare_primary_turn_request(input: PrimaryTurnRequest) -> MessageRequest {
    MessageRequest {
        model: input.model,
        messages: input.messages,
        max_tokens: input.max_tokens,
        system: input.system,
        tools: input.tools,
        tool_choice: input.tool_choice,
        metadata: None,
        thinking: None,
        reasoning_effort: input.reasoning_effort,
        stream: Some(true),
        temperature: None,
        top_p: None,
    }
}

/// System prompt representation (plain text or structured blocks).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum SystemPrompt {
    Text(String),
    Blocks(Vec<SystemBlock>),
}

/// A structured system prompt block.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// OpenAI-compatible image URL payload inside a multimodal message.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ImageUrlContent {
    pub url: String,
}

/// A chat message with role and content blocks.
///
/// `role` is a closed [`Role`] rather than a free-form string. It serializes
/// to exactly the bytes the `String` field produced, so persisted sessions
/// need no schema bump, and an unfamiliar role from a newer build loads as
/// [`Role::Unrecognized`] instead of failing the session.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

/// Internal role used for assistant text that was visible before a turn was interrupted.
pub const INTERRUPTED_ASSISTANT_ROLE: &str = "assistant_interrupted";
/// Prefix attached to interrupted assistant output when it is replayed as context.
pub const INTERRUPTED_ASSISTANT_CONTEXT_PREFIX: &str = "[The following assistant output was interrupted before completion and may be incomplete or wrong]\n";

/// Provider-owned reasoning continuity that is safe to replay only on the
/// exact originating API and model. The encrypted payload is deliberately
/// separate from readable [`ContentBlock::Thinking`] text.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct OpaqueReasoningState {
    pub provider: String,
    pub api: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub encrypted_content: String,
}

/// A single content block inside a message.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlContent },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        /// Anthropic signed-thinking signature (#3014). Only populated on the
        /// native Messages dialect and serde-skipped when absent so OpenAI
        /// dialects are unaffected. Anthropic rejects tool loops that drop or
        /// modify signed thinking blocks, so replay this verbatim.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        signature: Option<String>,
        /// Opaque Responses-style continuity. Never synthesize this from the
        /// readable `thinking` text or carry it across a route/model switch.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        state: Option<OpaqueReasoningState>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        caller: Option<ToolCaller>,
        /// Google thought signature captured from the OpenAI-compat route's
        /// `extra_content.google.thought_signature` on the tool call. Google
        /// requires replaying it with the tool result for thinking models;
        /// skipped on the wire and in storage for every other provider.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        thought_signature: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_blocks: Option<Vec<serde_json::Value>>,
    },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_search_tool_result")]
    ToolSearchToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    #[serde(rename = "code_execution_tool_result")]
    CodeExecutionToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
}

impl ContentBlock {
    /// Build readable reasoning with no provider-owned continuity state.
    #[must_use]
    pub fn thinking(thinking: impl Into<String>) -> Self {
        Self::Thinking {
            thinking: thinking.into(),
            signature: None,
            state: None,
        }
    }
}

/// Cache control metadata for tool definitions and blocks.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
}

/// Metadata describing who invoked a tool call.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ToolCaller {
    #[serde(rename = "type")]
    pub caller_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
}

/// Tool definition exposed to the model.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Tool {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub tool_type: Option<String>,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_callers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_examples: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn primary_turn() -> PrimaryTurnRequest {
        PrimaryTurnRequest {
            model: "deepseek-v4-flash".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "inspect the request".to_string(),
                    cache_control: None,
                }],
            }],
            max_tokens: 4096,
            system: Some(SystemPrompt::Text("system".to_string())),
            tools: Some(vec![Tool {
                tool_type: None,
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({"zeta": 1, "alpha": 2, "type": "object"}),
                allowed_callers: None,
                defer_loading: None,
                input_examples: None,
                strict: None,
                cache_control: None,
            }]),
            tool_choice: Some(json!({"type": "auto"})),
            reasoning_effort: Some("high".to_string()),
        }
    }

    #[test]
    fn primary_turn_preparation_has_stable_serialized_bytes() {
        let first = prepare_primary_turn_request(primary_turn());
        let second = prepare_primary_turn_request(primary_turn());
        let first_bytes = serde_json::to_vec(&first).expect("serialize first request");
        let second_bytes = serde_json::to_vec(&second).expect("serialize second request");

        assert_eq!(first_bytes, second_bytes);
        assert_eq!(
            first_bytes,
            br#"{"model":"deepseek-v4-flash","messages":[{"role":"user","content":[{"type":"text","text":"inspect the request"}]}],"max_tokens":4096,"system":"system","tools":[{"name":"read_file","description":"Read a file","input_schema":{"zeta":1,"alpha":2,"type":"object"}}],"tool_choice":{"type":"auto"},"reasoning_effort":"high","stream":true}"#
        );
    }

    #[test]
    fn persisted_messages_keep_their_pre_typed_role_bytes() {
        // `Message::role` became a closed `Role` enum. Saved transcripts are
        // plain JSON with a free-form role string, so the typed field has to
        // produce byte-identical output and accept every string it used to —
        // otherwise every session on disk would need a schema bump, and
        // `session_manager` refuses a session whose schema_version exceeds
        // CURRENT with no migration ladder to climb back down.
        let persisted = br#"[{"role":"user","content":[{"type":"text","text":"a"}]},{"role":"assistant","content":[{"type":"text","text":"b"}]},{"role":"system","content":[{"type":"text","text":"c"}]},{"role":"assistant_interrupted","content":[{"type":"text","text":"d"}]},{"role":"developer","content":[{"type":"text","text":"e"}]}]"#;
        let decoded: Vec<Message> = serde_json::from_slice(persisted).expect("load transcript");
        assert_eq!(
            decoded.iter().map(|m| m.role.clone()).collect::<Vec<_>>(),
            vec![
                Role::User,
                Role::Assistant,
                Role::System,
                Role::InterruptedAssistant,
                Role::Developer,
            ]
        );
        assert_eq!(
            serde_json::to_vec(&decoded).expect("re-save transcript"),
            persisted.to_vec(),
            "re-saving a loaded transcript must not change a single byte",
        );
    }

    #[test]
    fn primary_turn_preparation_owns_shared_defaults() {
        let request = prepare_primary_turn_request(primary_turn());
        assert_eq!(request.stream, Some(true));
        assert!(request.metadata.is_none());
        assert!(request.thinking.is_none());
        assert!(request.temperature.is_none());
        assert!(request.top_p.is_none());
    }
}
