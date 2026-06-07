//! API request/response models for `DeepSeek` and OpenAI-compatible endpoints.

use serde::{Deserialize, Serialize};

/// Context window used only for legacy DeepSeek model IDs that do not name a
/// newer V4 alias and do not carry an explicit `*k` suffix.
pub const LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS: u32 = 128_000;
pub const DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS: u32 = 1_000_000;
/// Last-resort compaction trigger when [`context_window_for_model`] returns
/// `None` (an unrecognised model id). v0.8.11 raised this from `50_000` to
/// `102_400` (80% of [`LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS`]) so unknown
/// models inherit the same late-trigger discipline as V4 instead of paying
/// the prefix-cache hit at 5% of the V4 window. Known DeepSeek / Claude
/// models resolve to their own scaled value via
/// [`compaction_threshold_for_model`] (#664).
pub const DEFAULT_COMPACTION_TOKEN_THRESHOLD: usize = 102_400;
const COMPACTION_THRESHOLD_PERCENT: u32 = 80;
pub const DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS: u32 = 262_144;

// === Core Message Types ===

/// Request payload for sending a message to the API.
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
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
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
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        caller: Option<ToolCaller>,
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

/// Container metadata for code-execution style server tools.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ContainerInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Server-side tool usage counters.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct ServerToolUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_execution_requests: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_search_requests: Option<u32>,
}

/// Response payload for a message request.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MessageResponse {
    pub id: String,
    pub r#type: String,
    pub role: String,
    pub content: Vec<ContentBlock>,
    pub model: String,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<ContainerInfo>,
    pub usage: Usage,
}

/// Token usage metadata for a response.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_hit_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_miss_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    /// Approximate input tokens spent re-sending prior `reasoning_content`
    /// across user-message boundaries in DeepSeek V4 thinking-mode tool-calling
    /// turns (V4 §5.1.1 "Interleaved Thinking"). Estimated client-side at
    /// ~4 chars/token from the outgoing request body, before the model sees it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_replay_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_tool_use: Option<ServerToolUsage>,
}

/// Map known models to their approximate context window sizes.
///
/// Lookup order:
/// 1. An explicit `_Nk` suffix in the model name, for **any** vendor. This
///    lets self-hosted deployments advertise their window through the served
///    model name (e.g. a vLLM `--served-model-name qwen3-32b-256k`), which is
///    the only signal we have for non-DeepSeek/Claude models. The 1000-token
///    approximation is fine for compaction-threshold math.
/// 2. DeepSeek vendor heuristics (V4 family -> 1M, legacy -> 128K).
/// 3. Claude -> 200K.
#[must_use]
pub fn context_window_for_model(model: &str) -> Option<u32> {
    let lower = model.to_lowercase();
    if let Some(explicit_window) = explicit_context_window_hint(&lower) {
        return Some(explicit_window);
    }
    if lower.contains("deepseek") {
        if lower.contains("v4") {
            return Some(DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS);
        }
        return Some(LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS);
    }
    if let Some(window) = known_context_window_for_model(&lower) {
        return Some(window);
    }
    if lower.contains("claude") {
        return Some(200_000);
    }
    None
}

fn known_context_window_for_model(model_lower: &str) -> Option<u32> {
    match model_lower {
        "trinity-mini" => Some(128_000),
        "arcee-ai/trinity-large-thinking" | "trinity-large-thinking" | "trinity-large-preview" => {
            Some(262_144)
        }
        "google/gemma-4-31b-it"
        | "google/gemma-4-31b-it:free"
        | "google/gemma-4-26b-a4b-it"
        | "google/gemma-4-26b-a4b-it:free"
        | "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free"
        | "qwen/qwen3.6-35b-a3b"
        | "qwen/qwen3.6-max-preview"
        | "qwen/qwen3.6-27b"
        | "tencent/hy3-preview"
        | "moonshotai/kimi-k2.6"
        | "moonshotai/kimi-k2.6:free" => Some(262_144),
        "z-ai/glm-5.1" | "z-ai/glm-5v-turbo" => Some(202_752),
        "minimax/minimax-m3" | "qwen/qwen3.6-flash" | "qwen/qwen3.6-plus" => Some(1_000_000),
        "xiaomi/mimo-v2.5-pro" | "xiaomi/mimo-v2.5" | "mimo-v2.5-pro" | "mimo-v2.5" => {
            Some(1_000_000)
        }
        "mimo-v2.5-asr"
        | "mimo-v2.5-tts"
        | "mimo-v2.5-tts-voicedesign"
        | "mimo-v2.5-tts-voiceclone"
        | "mimo-v2-tts" => Some(8_000),
        _ => None,
    }
}

#[must_use]
pub fn max_output_tokens_for_model(model: &str) -> Option<u32> {
    let lower = model.to_lowercase();
    if lower.contains("deepseek") && lower.contains("v4") {
        return Some(384_000);
    }
    match lower.as_str() {
        "arcee-ai/trinity-large-thinking" | "trinity-large-thinking" | "moonshotai/kimi-k2.6" => {
            Some(262_144)
        }
        "minimax/minimax-m3" => Some(524_288),
        "qwen/qwen3.6-35b-a3b" | "qwen/qwen3.6-27b" => Some(262_140),
        "qwen/qwen3.6-flash" | "qwen/qwen3.6-max-preview" | "qwen/qwen3.6-plus" => Some(65_536),
        "xiaomi/mimo-v2.5-pro" | "xiaomi/mimo-v2.5" | "mimo-v2.5-pro" | "mimo-v2.5" => {
            Some(131_072)
        }
        "mimo-v2.5-asr" => Some(2_048),
        "mimo-v2.5-tts"
        | "mimo-v2.5-tts-voicedesign"
        | "mimo-v2.5-tts-voiceclone"
        | "mimo-v2-tts" => Some(8_192),
        "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free" => Some(65_536),
        "google/gemma-4-31b-it" => Some(16_384),
        "google/gemma-4-31b-it:free" | "google/gemma-4-26b-a4b-it:free" => Some(32_768),
        _ => None,
    }
}

#[must_use]
pub fn model_supports_reasoning(model: &str) -> bool {
    let lower = model.to_lowercase();
    if lower.contains("deepseek") && lower.contains("v4") {
        return true;
    }
    matches!(
        lower.as_str(),
        "arcee-ai/trinity-large-thinking"
            | "trinity-large-thinking"
            | "google/gemma-4-31b-it"
            | "google/gemma-4-31b-it:free"
            | "google/gemma-4-26b-a4b-it"
            | "google/gemma-4-26b-a4b-it:free"
            | "moonshotai/kimi-k2.6"
            | "moonshotai/kimi-k2.6:free"
            | "minimax/minimax-m3"
            | "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free"
            | "qwen/qwen3.6-flash"
            | "qwen/qwen3.6-35b-a3b"
            | "qwen/qwen3.6-max-preview"
            | "qwen/qwen3.6-27b"
            | "qwen/qwen3.6-plus"
            | "tencent/hy3-preview"
            | "xiaomi/mimo-v2.5-pro"
            | "xiaomi/mimo-v2.5"
            | "mimo-v2.5-pro"
            | "mimo-v2.5"
            | "z-ai/glm-5.1"
    )
}

/// Parse an explicit `_Nk` context-window hint from a model name (vendor
/// agnostic). Returns the window in tokens for `N` in `8..=1024`.
fn explicit_context_window_hint(model_lower: &str) -> Option<u32> {
    let bytes = model_lower.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i >= bytes.len() || bytes[i] != b'k' {
                continue;
            }

            let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            let after_ok = i + 1 >= bytes.len() || !bytes[i + 1].is_ascii_alphanumeric();
            if !before_ok || !after_ok {
                continue;
            }

            if let Ok(kilo_tokens) = model_lower[start..i].parse::<u32>()
                && (8..=1024).contains(&kilo_tokens)
            {
                return Some(kilo_tokens.saturating_mul(1000));
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Derive a compaction token threshold from model context and a caller-supplied
/// percentage.
#[must_use]
pub fn compaction_threshold_for_model_at_percent(model: &str, percent: f64) -> usize {
    let Some(window) = context_window_for_model(model) else {
        return DEFAULT_COMPACTION_TOKEN_THRESHOLD;
    };

    let percent = percent.clamp(10.0, 100.0);
    let threshold = (f64::from(window) * percent / 100.0).round();
    let threshold = if threshold.is_finite() && threshold > 0.0 {
        threshold as u64
    } else {
        u64::from(window) * u64::from(COMPACTION_THRESHOLD_PERCENT) / 100
    };
    usize::try_from(threshold).unwrap_or(DEFAULT_COMPACTION_TOKEN_THRESHOLD)
}

/// Whether auto-compaction should be enabled when the user did not explicitly
/// configure it. V4-class 1M models keep the prefix-cache-friendly opt-in
/// behavior; 256K-class and smaller known models need automatic pressure
/// relief near the context wall.
#[must_use]
pub fn auto_compact_default_for_model(model: &str) -> bool {
    context_window_for_model(model)
        .is_some_and(|window| window <= DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS)
}

// === Streaming Structures ===

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
/// Streaming event types for SSE responses.
pub enum StreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageResponse },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: ContentBlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: Delta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDelta,
        usage: Option<Usage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
/// Content block types used in streaming starts.
pub enum ContentBlockStart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value, // usually empty or partial
        #[serde(skip_serializing_if = "Option::is_none")]
        caller: Option<ToolCaller>,
    },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

// Variant names match legacy streaming spec, suppressing style warning
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
/// Delta events emitted during streaming responses.
pub enum Delta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
/// Delta payload for message-level updates.
pub struct MessageDelta {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4_snapshots_preserve_context_window() {
        // v-series snapshots get 1M context since they contain "v4"
        assert_eq!(
            context_window_for_model("deepseek-v4-flash-20260423"),
            Some(DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS)
        );
        assert_eq!(
            context_window_for_model("deepseek-v4-pro-20260423"),
            Some(DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS)
        );
    }

    #[test]
    fn unknown_legacy_deepseek_models_map_to_128k_context_window() {
        assert_eq!(
            context_window_for_model("deepseek-coder"),
            Some(LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS)
        );
        assert_eq!(
            context_window_for_model("deepseek-v3.2-0324"),
            Some(LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS)
        );
    }

    #[test]
    fn deepseek_v4_models_map_to_1m_context_window() {
        assert_eq!(
            context_window_for_model("deepseek-v4-pro"),
            Some(DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS)
        );
        assert_eq!(
            context_window_for_model("deepseek-v4-flash"),
            Some(DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS)
        );
        assert_eq!(
            context_window_for_model("deepseek-ai/deepseek-v4-pro"),
            Some(DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS)
        );
    }

    #[test]
    fn recent_openrouter_large_models_have_static_windows() {
        for (model, expected_window) in [
            ("arcee-ai/trinity-large-thinking", 262_144),
            ("trinity-large-thinking", 262_144),
            (concat!("qwen/", "qwen3.6-flash"), 1_000_000),
            (concat!("qwen/", "qwen3.6-35b-a3b"), 262_144),
            (concat!("qwen/", "qwen3.6-max-preview"), 262_144),
            (concat!("qwen/", "qwen3.6-plus"), 1_000_000),
            (concat!("xiaomi/", "mimo-v2.5-pro"), 1_000_000),
            ("mimo-v2.5-pro", 1_000_000),
            ("mimo-v2.5", 1_000_000),
            ("minimax/minimax-m3", 1_000_000),
            ("moonshotai/kimi-k2.6", 262_144),
            ("google/gemma-4-31b-it", 262_144),
            ("z-ai/glm-5.1", 202_752),
        ] {
            assert_eq!(context_window_for_model(model), Some(expected_window));
            assert!(model_supports_reasoning(model));
        }
    }

    #[test]
    fn arcee_direct_models_have_static_windows_without_reasoning_flag() {
        assert_eq!(
            context_window_for_model("trinity-large-preview"),
            Some(262_144)
        );
        assert!(!model_supports_reasoning("trinity-large-preview"));
        assert_eq!(context_window_for_model("trinity-mini"), Some(128_000));
        assert!(!model_supports_reasoning("trinity-mini"));
    }

    #[test]
    fn recent_openrouter_large_models_have_known_output_caps() {
        assert_eq!(
            max_output_tokens_for_model("arcee-ai/trinity-large-thinking"),
            Some(262_144)
        );
        assert_eq!(
            max_output_tokens_for_model("trinity-large-thinking"),
            Some(262_144)
        );
        assert_eq!(
            max_output_tokens_for_model(concat!("qwen/", "qwen3.6-flash")),
            Some(65_536)
        );
        assert_eq!(
            max_output_tokens_for_model(concat!("qwen/", "qwen3.6-max-preview")),
            Some(65_536)
        );
        assert_eq!(
            max_output_tokens_for_model(concat!("qwen/", "qwen3.6-plus")),
            Some(65_536)
        );
        assert_eq!(
            max_output_tokens_for_model(concat!("xiaomi/", "mimo-v2.5-pro")),
            Some(131_072)
        );
        assert_eq!(max_output_tokens_for_model("mimo-v2.5-pro"), Some(131_072));
        assert_eq!(max_output_tokens_for_model("mimo-v2.5"), Some(131_072));
        assert_eq!(
            max_output_tokens_for_model("minimax/minimax-m3"),
            Some(524_288)
        );
    }

    #[test]
    fn deepseek_models_with_k_suffix_use_hint() {
        assert_eq!(context_window_for_model("deepseek-v3.2-32k"), Some(32_000));
        assert_eq!(
            context_window_for_model("deepseek-v3.2-256k-preview"),
            Some(256_000)
        );
        assert_eq!(
            context_window_for_model("deepseek-v3.2-2k-preview"),
            Some(LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS)
        );
    }

    #[test]
    fn compaction_threshold_scales_with_context_window() {
        assert_eq!(
            compaction_threshold_for_model_at_percent("deepseek-v3.2-128k", 80.0),
            102_400
        );
        // v0.8.11 (#664): unknown-model fallback also resolves to 80% of
        // `LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS` (128K legacy DeepSeek
        // fallback) — same late-trigger discipline as the V4 path. Was
        // `50_000` pre-v0.8.11; that hardcoded value compacted at ~5% of a
        // 1M window when model detection silently fell through, which is
        // exactly the prefix-cache-burning behaviour we're getting away from.
        assert_eq!(
            compaction_threshold_for_model_at_percent("unknown-model", 80.0),
            102_400
        );
    }

    #[test]
    fn compaction_scales_for_deepseek_v4_1m_context() {
        assert_eq!(
            compaction_threshold_for_model_at_percent("deepseek-v4-pro", 80.0),
            800_000
        );
    }

    #[test]
    fn compaction_threshold_honors_configured_percent() {
        assert_eq!(
            compaction_threshold_for_model_at_percent("deepseek-v4-pro", 75.0),
            750_000
        );
        assert_eq!(
            compaction_threshold_for_model_at_percent("trinity-large-thinking", 80.0),
            209_715
        );
    }

    #[test]
    fn auto_compaction_defaults_on_for_256k_class_models_only() {
        assert!(auto_compact_default_for_model("trinity-large-thinking"));
        assert!(auto_compact_default_for_model("deepseek-v3.2-128k"));
        assert!(!auto_compact_default_for_model("deepseek-v4-pro"));
        assert!(!auto_compact_default_for_model("mimo-v2.5-pro"));
        assert!(!auto_compact_default_for_model("unknown-model"));
    }
}
