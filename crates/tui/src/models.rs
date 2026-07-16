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
#[cfg(test)]
const COMPACTION_THRESHOLD_PERCENT: u32 = 80;
pub const DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS: u32 = DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS;

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
    Thinking {
        thinking: String,
        /// Anthropic signed-thinking signature (#3014). Only populated on the
        /// native Messages dialect and serde-skipped when absent so OpenAI
        /// dialects are unaffected. Anthropic rejects tool loops that drop or
        /// modify signed thinking blocks, so replay this verbatim.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        signature: Option<String>,
    },
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
    /// Cache-creation / cache-write tokens (Anthropic `cache_creation_input_tokens`).
    /// Billed at the cache-write rate when the pricing row publishes one (#4318).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_write_tokens: Option<u32>,
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
    if let Some(window) = crate::model_catalog::resolved_context_window(model) {
        return Some(window);
    }
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
    if is_openai_gpt_55_api_model(&lower) || is_openai_gpt_56_api_model(&lower) {
        return Some(1_050_000);
    }
    if is_openai_codex_model(&lower) {
        return Some(400_000);
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
        // OpenAI API model docs, verified 2026-06-12:
        // https://developers.openai.com/api/docs/models/gpt-5.5
        // Family aliases and snapshots are handled by
        // `is_openai_gpt_55_api_model` before this table.
        // OpenAI Codex model docs, verified 2026-06-12:
        // https://developers.openai.com/api/docs/models/gpt-5-codex
        // https://developers.openai.com/api/docs/models/gpt-5.3-codex
        "gpt-5-codex" | "gpt-5.3-codex" => Some(400_000),
        // Anthropic 4.6+ models carry a 1M window; Haiku stays at 200K (#3014).
        "claude-opus-4-8" | "claude-sonnet-4-6" | "claude-sonnet-5" | "claude-fable-5" => {
            Some(1_000_000)
        }
        "claude-haiku-4-5" => Some(200_000),
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
        | "moonshotai/kimi-k2.7-code"
        | "moonshotai/kimi-k2.6"
        | "moonshotai/kimi-k2.6:free"
        | "kimi-k2.7-code"
        | "kimi-k2.6"
        | "kimi-for-coding" => Some(262_144),
        "minimax-m2.7"
        | "minimax/minimax-m2.7"
        | "minimax-m2.7-highspeed"
        | "minimax-m2.5"
        | "minimax-m2.5-highspeed"
        | "minimax-m2.1"
        | "minimax-m2.1-highspeed"
        | "minimax-m2" => Some(204_800),
        "z-ai/glm-5.1" | "z-ai/glm-5v-turbo" | "glm-5.1" | "glm-5v-turbo" => Some(202_752),
        "z-ai/glm-5-turbo" | "glm-5-turbo" => Some(202_752),
        "z-ai/glm-5.2" | "glm-5.2" => Some(1_000_000),
        "minimax/minimax-m3" | "minimax-m3" | "qwen/qwen3.6-flash" | "qwen/qwen3.6-plus" => {
            Some(1_000_000)
        }
        "nvidia/nemotron-3-ultra-550b-a55b" | "nvidia/nemotron-3-ultra-550b-a55b:free" => {
            Some(1_000_000)
        }
        "xiaomi/mimo-v2.5-pro"
        | "xiaomi/mimo-v2.5"
        | "mimo-v2.5-pro"
        | "mimo-v2.5-pro-ultraspeed"
        | "mimo-v2.5" => Some(1_000_000),
        "mimo-v2.5-asr"
        | "mimo-v2.5-tts"
        | "mimo-v2.5-tts-voicedesign"
        | "mimo-v2.5-tts-voiceclone"
        | "mimo-v2-tts" => Some(8_000),
        "grok-4.5" => Some(500_000),
        "grok-4.3" => Some(1_000_000),
        "grok-build" => Some(512_000),
        "grok-composer-2.5-fast" => Some(200_000),
        "grok-4.20-0309-reasoning" | "grok-4.20-0309-non-reasoning" => Some(2_000_000),
        "muse-spark-1.1" => Some(1_000_000),
        _ => None,
    }
}

#[must_use]
pub fn max_output_tokens_for_model(model: &str) -> Option<u32> {
    if let Some(max_output) = crate::model_catalog::resolved_max_output(model) {
        return Some(max_output);
    }
    let lower = model.to_lowercase();
    if lower.contains("deepseek") && lower.contains("v4") {
        return Some(384_000);
    }
    if is_openai_gpt_55_api_model(&lower)
        || is_openai_gpt_56_api_model(&lower)
        || is_openai_codex_model(&lower)
    {
        return Some(128_000);
    }
    match lower.as_str() {
        "gpt-5-codex" | "gpt-5.3-codex" => Some(128_000),
        // claude-sonnet-4-6 max output raised 64K -> 128K per
        // https://platform.claude.com/docs/en/about-claude/models/overview
        // (2026-07-09 audit).
        "claude-opus-4-8" | "claude-sonnet-4-6" | "claude-sonnet-5" | "claude-fable-5" => {
            Some(128_000)
        }
        "claude-haiku-4-5" => Some(64_000),
        "arcee-ai/trinity-large-thinking" | "trinity-large-thinking" => Some(262_144),
        // Kimi K2.7 Code has a 256K context window but its documented default
        // maximum generation is 32K. Keeping those separate prevents the
        // input budget from collapsing to the 1K emergency floor (#4368).
        "moonshotai/kimi-k2.7-code"
        | "moonshotai/kimi-k2.6"
        | "kimi-k2.7-code"
        | "kimi-k2.6"
        | "kimi-for-coding" => Some(32_768),
        "minimax/minimax-m3" | "minimax-m3" => Some(524_288),
        // Alibaba's published limit is 65,536 output tokens; the earlier
        // 262,140 mirrored the context window (data-entry smell flagged by
        // MODEL_PROVIDER_AUDIT A2/D-7, vendor-verified 2026-07-12).
        "qwen/qwen3.6-35b-a3b"
        | "qwen/qwen3.6-27b"
        | "qwen/qwen3.6-flash"
        | "qwen/qwen3.6-max-preview"
        | "qwen/qwen3.6-plus" => Some(65_536),
        "z-ai/glm-5.1" | "z-ai/glm-5.2" | "z-ai/glm-5-turbo" | "glm-5.1" | "glm-5.2"
        | "glm-5-turbo" => Some(131_072),
        "xiaomi/mimo-v2.5-pro"
        | "xiaomi/mimo-v2.5"
        | "mimo-v2.5-pro"
        | "mimo-v2.5-pro-ultraspeed"
        | "mimo-v2.5" => Some(131_072),
        "mimo-v2.5-asr" => Some(2_048),
        "mimo-v2.5-tts"
        | "mimo-v2.5-tts-voicedesign"
        | "mimo-v2.5-tts-voiceclone"
        | "mimo-v2-tts" => Some(8_192),
        "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free" => Some(65_536),
        "nvidia/nemotron-3-ultra-550b-a55b" => Some(16_384),
        "nvidia/nemotron-3-ultra-550b-a55b:free" => Some(65_536),
        "google/gemma-4-31b-it" => Some(16_384),
        "google/gemma-4-31b-it:free" | "google/gemma-4-26b-a4b-it:free" => Some(32_768),
        "muse-spark-1.1" => Some(32_000),
        _ => None,
    }
}

#[must_use]
pub fn model_supports_reasoning(model: &str) -> bool {
    if let Some(supports_reasoning) = crate::model_catalog::resolved_supports_reasoning(model) {
        return supports_reasoning;
    }
    let lower = model.to_lowercase();
    if lower.contains("deepseek") && lower.contains("v4") {
        return true;
    }
    // #3016 plus the 2026 Kimi Code K2.7 update: Moonshot-native Kimi IDs,
    // including the stable `kimi-for-coding` coding route, emit
    // reasoning_content that must stay out of answer prose.
    if lower.starts_with("kimi-") {
        return true;
    }
    matches!(
        lower.as_str(),
        "claude-opus-4-8"
            | "claude-sonnet-4-6"
            | "claude-sonnet-5"
            | "claude-fable-5"
            | "gpt-5-codex"
            | "gpt-5.3-codex"
            | "trinity-mini"
            | "arcee-ai/trinity-large-thinking"
            | "trinity-large-thinking"
            | "thinkingmachines/inkling"
            | "google/gemma-4-31b-it"
            | "google/gemma-4-31b-it:free"
            | "google/gemma-4-26b-a4b-it"
            | "google/gemma-4-26b-a4b-it:free"
            | "moonshotai/kimi-k2.7-code"
            | "moonshotai/kimi-k2.6"
            | "moonshotai/kimi-k2.6:free"
            | "kimi-k2.7-code"
            | "kimi-k2.6"
            | "kimi-for-coding"
            | "minimax/minimax-m3"
            | "minimax/minimax-m2.7"
            | "minimax-m3"
            | "minimax-m2.7"
            | "minimax-m2.7-highspeed"
            | "minimax-m2.5"
            | "minimax-m2.5-highspeed"
            | "minimax-m2.1"
            | "minimax-m2.1-highspeed"
            | "minimax-m2"
            | "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free"
            | "nvidia/nemotron-3-ultra-550b-a55b"
            | "nvidia/nemotron-3-ultra-550b-a55b:free"
            | "qwen/qwen3.6-flash"
            | "qwen/qwen3.6-35b-a3b"
            | "qwen/qwen3.6-max-preview"
            | "qwen/qwen3.6-27b"
            | "qwen/qwen3.6-plus"
            | "qwen/qwen3.7-plus"
            | "tencent/hy3-preview"
            | "xiaomi/mimo-v2.5-pro"
            | "xiaomi/mimo-v2.5"
            | "mimo-v2.5-pro"
            | "mimo-v2.5-pro-ultraspeed"
            | "mimo-v2.5"
            | "z-ai/glm-5.1"
            | "z-ai/glm-5.2"
            | "z-ai/glm-5-turbo"
            | "glm-5.1"
            | "glm-5.2"
            | "glm-5-turbo"
            | "grok-4.5"
            | "grok-4.3"
            | "grok-build"
            | "grok-4.20-0309-reasoning"
            | "muse-spark-1.1"
    ) || is_openai_gpt_55_api_model(&lower)
        || is_openai_gpt_56_api_model(&lower)
        || is_openai_codex_model(&lower)
}

#[must_use]
pub(crate) fn model_is_openai_reasoning_family(model: &str) -> bool {
    let lower = model.to_lowercase();
    is_openai_gpt_55_api_model(&lower)
        || is_openai_gpt_56_api_model(&lower)
        || is_openai_codex_model(&lower)
}

fn is_openai_gpt_55_api_model(model_lower: &str) -> bool {
    matches!(model_lower, "gpt-5.5" | "gpt-5.5-pro")
        || has_date_snapshot_suffix(model_lower, "gpt-5.5-")
        || has_date_snapshot_suffix(model_lower, "gpt-5.5-pro-")
}

pub(crate) fn is_openai_gpt_56_api_model(model_lower: &str) -> bool {
    matches!(
        model_lower,
        "gpt-5.6" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna"
    )
}

fn is_openai_codex_model(model_lower: &str) -> bool {
    matches!(
        model_lower,
        "gpt-5-codex"
            | "gpt-5.1-codex"
            | "gpt-5.1-codex-mini"
            | "gpt-5.1-codex-max"
            | "gpt-5.2-codex"
            | "gpt-5.3-codex"
            | "codex-gpt-5.5"
            | "chatgpt-gpt-5.5"
            | "gpt-5.5-codex"
            | "gpt-5.5-codex-preview"
            | "codex-gpt-5.5-preview"
            | "chatgpt-gpt-5.5-preview"
    )
}

pub(crate) fn has_date_snapshot_suffix(model_lower: &str, prefix: &str) -> bool {
    let Some(rest) = model_lower.strip_prefix(prefix) else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(idx, byte)| idx == 4 || idx == 7 || byte.is_ascii_digit())
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
#[cfg(test)]
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
/// configure it. v0.8.64 defaults automatic continuity on for known model
/// windows up to the V4 1M class while keeping unknown model ids opt-in.
#[must_use]
#[cfg(test)]
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
    /// Anthropic SSE error event (#3014).
    #[serde(rename = "error")]
    Error { error: serde_json::Value },
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
    /// Anthropic signed-thinking signature delta (#3014); arrives at the end
    /// of a thinking block on the native Messages stream.
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
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
    use std::collections::BTreeMap;

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
            ("mimo-v2.5-pro-ultraspeed", 1_000_000),
            ("mimo-v2.5", 1_000_000),
            ("minimax/minimax-m3", 1_000_000),
            ("minimax/minimax-m2.7", 204_800),
            ("moonshotai/kimi-k2.7-code", 262_144),
            ("moonshotai/kimi-k2.6", 262_144),
            ("google/gemma-4-31b-it", 262_144),
            ("z-ai/glm-5.1", 202_752),
            ("z-ai/glm-5.2", 1_000_000),
        ] {
            assert_eq!(context_window_for_model(model), Some(expected_window));
            assert!(model_supports_reasoning(model));
        }
    }

    #[test]
    fn openai_api_and_codex_models_have_verified_context_metadata() {
        for model in ["gpt-5.6", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            assert_eq!(context_window_for_model(model), Some(1_050_000));
            assert_eq!(max_output_tokens_for_model(model), Some(128_000));
            assert!(model_supports_reasoning(model));
            assert_eq!(
                compaction_threshold_for_model_at_percent(model, 80.0),
                840_000
            );
        }

        for model in [
            "gpt-5.5",
            "gpt-5.5-pro",
            "gpt-5.5-2026-04-23",
            "gpt-5.5-pro-2026-04-23",
        ] {
            assert_eq!(context_window_for_model(model), Some(1_050_000));
            assert_eq!(max_output_tokens_for_model(model), Some(128_000));
            assert!(model_supports_reasoning(model));
            assert_eq!(
                compaction_threshold_for_model_at_percent(model, 80.0),
                840_000
            );
        }

        for model in [
            "gpt-5-codex",
            "gpt-5.1-codex",
            "gpt-5.1-codex-mini",
            "gpt-5.1-codex-max",
            "gpt-5.2-codex",
            "gpt-5.3-codex",
            "codex-gpt-5.5",
            "chatgpt-gpt-5.5",
            "gpt-5.5-codex",
            "gpt-5.5-codex-preview",
        ] {
            assert_eq!(context_window_for_model(model), Some(400_000));
            assert_eq!(max_output_tokens_for_model(model), Some(128_000));
            assert!(model_supports_reasoning(model));
            assert_eq!(
                compaction_threshold_for_model_at_percent(model, 80.0),
                320_000
            );
        }

        assert_eq!(context_window_for_model("gpt-5.5-nano"), None);
        assert_eq!(max_output_tokens_for_model("gpt-5.5-nano"), None);
        assert!(!model_supports_reasoning("gpt-5.5-nano"));
    }

    #[test]
    fn anthropic_stepfun_and_sakana_limits_match_2026_07_09_audit() {
        // Sonnet 4.6 output cap raised 64K -> 128K per
        // https://platform.claude.com/docs/en/about-claude/models/overview;
        // Haiku stays at 64K.
        assert_eq!(
            max_output_tokens_for_model("claude-sonnet-4-6"),
            Some(128_000)
        );
        assert_eq!(
            max_output_tokens_for_model("claude-haiku-4-5"),
            Some(64_000)
        );
        // step-3.7-flash max output is third-party sourced (models.dev +
        // Artificial Analysis; the official StepFun page is silent):
        // https://models.dev/models/stepfun/step-3.7-flash/
        assert_eq!(max_output_tokens_for_model("step-3.7-flash"), Some(256_000));
        assert_eq!(context_window_for_model("step-3.7-flash"), Some(256_000));
        // fugu-ultra limits are third-party sourced (Requesty; Sakana's own
        // >272K price tier at https://console.sakana.ai/pricing confirms the
        // context window exceeds 272K).
        for model in ["fugu-ultra", "fugu-ultra-20260615"] {
            assert_eq!(context_window_for_model(model), Some(1_000_000), "{model}");
            assert_eq!(max_output_tokens_for_model(model), Some(131_000), "{model}");
        }
    }

    #[test]
    fn claude_fable_5_and_sonnet_5_have_verified_metadata() {
        // 1M context / 128K output per
        // https://platform.claude.com/docs/en/about-claude/pricing (2026-07-09).
        for model in ["claude-fable-5", "claude-sonnet-5"] {
            assert_eq!(context_window_for_model(model), Some(1_000_000), "{model}");
            assert_eq!(max_output_tokens_for_model(model), Some(128_000), "{model}");
            assert!(model_supports_reasoning(model), "{model}");
        }
    }

    #[test]
    fn muse_spark_has_verified_context_and_reasoning_metadata() {
        assert_eq!(context_window_for_model("muse-spark-1.1"), Some(1_000_000));
        assert_eq!(max_output_tokens_for_model("muse-spark-1.1"), Some(32_000));
        assert!(model_supports_reasoning("muse-spark-1.1"));
    }

    #[test]
    fn model_metadata_catalog_override_flows_through_models_chokepoint() {
        let _lock = crate::model_catalog::test_catalog_lock();
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "catalog-only-model".to_string(),
            crate::model_catalog::CatalogEntry {
                id: "catalog-only-model".to_string(),
                context_window: Some(777_000),
                max_output: Some(55_000),
                supports_reasoning: Some(true),
                input_usd_per_million: None,
                output_usd_per_million: None,
                modalities: Vec::new(),
                supported_parameters: Vec::new(),
                provider_model_id: None,
                provenance: crate::model_catalog::MetadataProvenance::UserOverride,
            },
        );
        let catalog = crate::model_catalog::MergedCatalog::from_sources(
            overrides,
            None,
            crate::model_catalog::bundled_catalog(),
            chrono::Utc::now(),
        );
        let _guard = crate::model_catalog::replace_active_catalog_for_test(catalog);

        assert_eq!(
            context_window_for_model("catalog-only-model"),
            Some(777_000)
        );
        assert_eq!(
            max_output_tokens_for_model("catalog-only-model"),
            Some(55_000)
        );
        assert!(model_supports_reasoning("catalog-only-model"));
    }

    #[test]
    fn moonshot_native_kimi_ids_support_reasoning_including_coding_route() {
        // #3016: bare Moonshot ids (no moonshotai/ prefix) emit
        // reasoning_content; kimi-for-coding currently rides the K2.7 Code path.
        assert!(model_supports_reasoning("kimi-k2.7-code"));
        assert!(model_supports_reasoning("kimi-k2.6"));
        assert!(model_supports_reasoning("kimi-for-coding"));
        assert!(model_supports_reasoning("kimi-k2.5"));
    }

    #[test]
    fn xai_grok_models_have_static_context_metadata() {
        for (model, expected_window, supports_reasoning) in [
            ("grok-4.5", 500_000, true),
            ("grok-4.3", 1_000_000, true),
            ("grok-build", 512_000, true),
            ("grok-composer-2.5-fast", 200_000, false),
            ("grok-4.20-0309-reasoning", 2_000_000, true),
            ("grok-4.20-0309-non-reasoning", 2_000_000, false),
        ] {
            assert_eq!(context_window_for_model(model), Some(expected_window));
            assert_eq!(max_output_tokens_for_model(model), None);
            assert_eq!(model_supports_reasoning(model), supports_reasoning);
        }
    }

    #[test]
    fn arcee_direct_models_preserve_verified_capabilities_only() {
        assert_eq!(
            context_window_for_model("trinity-large-preview"),
            Some(262_144)
        );
        assert!(!model_supports_reasoning("trinity-large-preview"));
        assert_eq!(context_window_for_model("trinity-mini"), Some(128_000));
        assert_eq!(max_output_tokens_for_model("trinity-mini"), None);
        assert!(model_supports_reasoning("trinity-mini"));
    }

    #[test]
    fn qwen37_plus_and_inkling_reasoning_do_not_invent_limits() {
        for model in ["qwen/qwen3.7-plus", "thinkingmachines/inkling"] {
            assert_eq!(context_window_for_model(model), None, "{model}");
            assert_eq!(max_output_tokens_for_model(model), None, "{model}");
            assert!(model_supports_reasoning(model), "{model}");
        }
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
        assert_eq!(
            max_output_tokens_for_model("mimo-v2.5-pro-ultraspeed"),
            Some(131_072)
        );
        assert_eq!(max_output_tokens_for_model("mimo-v2.5"), Some(131_072));
        assert_eq!(
            max_output_tokens_for_model("minimax/minimax-m3"),
            Some(524_288)
        );
        assert_eq!(max_output_tokens_for_model("z-ai/glm-5.1"), Some(131_072));
        assert_eq!(max_output_tokens_for_model("z-ai/glm-5.2"), Some(131_072));
        assert_eq!(
            max_output_tokens_for_model("z-ai/glm-5-turbo"),
            Some(131_072)
        );
        assert_eq!(max_output_tokens_for_model("glm-5-turbo"), Some(131_072));
    }

    #[test]
    fn bare_provider_model_ids_mirror_vendor_prefixed_rows() {
        // Direct-provider routes (Moonshot, MiniMax, Z.ai) serve bare model
        // ids without the OpenRouter vendor prefix; both spellings must
        // resolve identical metadata (#1310 ride-along on #3023).
        for (model, expected_window) in [
            ("kimi-k2.7-code", 262_144),
            ("kimi-k2.6", 262_144),
            ("minimax-m3", 1_000_000),
            ("minimax-m2.7", 204_800),
            ("minimax-m2.5-highspeed", 204_800),
            ("minimax-m2", 204_800),
            ("glm-5.1", 202_752),
            ("glm-5.2", 1_000_000),
            ("glm-5-turbo", 202_752),
        ] {
            assert_eq!(context_window_for_model(model), Some(expected_window));
            assert!(model_supports_reasoning(model));
        }
        assert_eq!(context_window_for_model("kimi-for-coding"), Some(262_144));
        assert!(model_supports_reasoning("kimi-for-coding"));
        assert_eq!(context_window_for_model("glm-5v-turbo"), Some(202_752));
        assert!(!model_supports_reasoning("glm-5v-turbo"));
        // GLM-5-Turbo is a fast text sibling (distinct from the glm-5v-turbo
        // vision model): same compact window as 5.1 but reasoning-capable.
        assert_eq!(context_window_for_model("z-ai/glm-5-turbo"), Some(202_752));
        assert!(model_supports_reasoning("z-ai/glm-5-turbo"));
        assert_eq!(
            crate::model_catalog::resolved_max_output("kimi-k2.7-code"),
            Some(32_768)
        );
        assert_eq!(max_output_tokens_for_model("kimi-k2.7-code"), Some(32_768));
        assert_eq!(max_output_tokens_for_model("kimi-k2.6"), Some(32_768));
        assert_eq!(max_output_tokens_for_model("kimi-for-coding"), Some(32_768));
        assert_eq!(max_output_tokens_for_model("minimax-m3"), Some(524_288));
        assert_eq!(max_output_tokens_for_model("glm-5.1"), Some(131_072));
        assert_eq!(max_output_tokens_for_model("glm-5.2"), Some(131_072));
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
    fn auto_compaction_defaults_on_for_known_supported_model_windows() {
        assert!(auto_compact_default_for_model("trinity-large-thinking"));
        assert!(auto_compact_default_for_model("deepseek-v3.2-128k"));
        assert!(auto_compact_default_for_model("deepseek-v4-pro"));
        assert!(auto_compact_default_for_model("mimo-v2.5-pro"));
        assert!(!auto_compact_default_for_model("unknown-model"));
    }
}
