//! API request/response models for `DeepSeek` and OpenAI-compatible endpoints.

use serde::{Deserialize, Serialize};

/// Context window used only for legacy DeepSeek model IDs that do not name a
/// newer V4 alias and do not carry an explicit `*k` suffix.
pub const LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS: u32 = 128_000;
pub const DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS: u32 = 1_000_000;
/// Conservative Kimi Code K3 context baseline. The membership route's real
/// context is plan-tier dependent (verified 2026-07-20 from
/// <https://www.kimi.com/code/docs/en/kimi-code/models>): Moderato gets 256K,
/// while Allegretto and above get up to 1M. Bare `k3` therefore keeps this
/// safe floor everywhere; higher plan entitlements must come from an explicit
/// provider `context_window` configuration or fresh provider facts while
/// preserving the `k3` wire id.
pub const KIMI_CODE_K3_CONTEXT_WINDOW_TOKENS: u32 = 262_144;
/// Kimi K3 context window on the open platform (`kimi-k3` pay-as-you-go).
/// Verified 2026-07-20 from <https://platform.kimi.ai/docs/guide/kimi-k3-quickstart>
/// (1,048,576 tokens). Max output is a separate fact below and must never be
/// conflated with this window.
pub const KIMI_K3_CONTEXT_WINDOW_TOKENS: u32 = 1_048_576;
/// Conservative K3 default generation ceiling. The direct Kimi API defaults
/// `max_completion_tokens` to 131,072, while its documented route maximum is
/// a separate exact-route fact below. Membership and neighboring routes do
/// not inherit that direct-platform maximum.
pub const KIMI_K3_DEFAULT_MAX_COMPLETION_TOKENS: u32 = 131_072;
/// Documented maximum output for the exact direct Kimi K3 API route.
///
/// Source: <https://platform.kimi.ai/docs/guide/kimi-k3-quickstart> (verified 2026-07-20).
pub const DIRECT_KIMI_K3_MAX_OUTPUT_TOKENS: u32 = 1_048_576;
/// Last-resort compaction trigger when [`context_window_for_model`] returns
/// `None` (an unrecognised model id). v0.8.11 raised this from `50_000` to
/// `102_400` (80% of [`LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS`]) so unknown
/// models inherit the same late-trigger discipline as V4 instead of paying
/// the prefix-cache hit at 5% of the V4 window. Known DeepSeek / Claude
/// models resolve to their own scaled value via
/// `compaction_threshold_for_model` (#664).
pub const DEFAULT_COMPACTION_TOKEN_THRESHOLD: usize = 102_400;
#[cfg(test)]
const COMPACTION_THRESHOLD_PERCENT: u32 = 80;

// === Core Message Types ===

// Keep the historical TUI path stable while the production request DTOs are
// owned by `codewhale-core`. Existing transports and response decoders do not
// need a flag day, and headless callers can depend on core directly.
// Some process-test crates include this module privately and exercise only a
// subset of the compatibility surface, so their crate-local dead-import view
// is not evidence that a re-export can be removed.
#[allow(unused_imports)]
pub use codewhale_core::request::{
    CacheControl, ContentBlock, INTERRUPTED_ASSISTANT_CONTEXT_PREFIX, INTERRUPTED_ASSISTANT_ROLE,
    ImageUrlContent, Message, MessageRequest, OpaqueReasoningState, SystemBlock, SystemPrompt,
    Tool, ToolCaller,
};
#[allow(unused_imports)]
pub use codewhale_core::role::Role;

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

/// True when the provider ended generation because its output allowance was
/// exhausted. Providers use several wire spellings for the same condition.
#[must_use]
pub(crate) fn is_output_limit_stop_reason(reason: Option<&str>) -> bool {
    reason.is_some_and(|reason| {
        let reason = reason
            .trim()
            .strip_prefix("incomplete:")
            .unwrap_or_else(|| reason.trim());
        matches!(
            reason.to_ascii_lowercase().as_str(),
            "length" | "max_tokens" | "max_output_tokens"
        )
    })
}

/// True when the provider explicitly reported that it did not complete the
/// response. Responses API reasons carry an `incomplete:` prefix so unknown
/// future reasons cannot accidentally be accepted as a finished answer.
#[must_use]
pub(crate) fn is_incomplete_stop_reason(reason: Option<&str>) -> bool {
    is_output_limit_stop_reason(reason)
        || reason.is_some_and(|reason| {
            let reason = reason.trim().to_ascii_lowercase();
            reason.starts_with("incomplete:")
                || matches!(
                    reason.as_str(),
                    "content_filter" | "model_context_window_exceeded"
                )
        })
}

#[must_use]
pub(crate) fn stop_reason_detail(reason: Option<&str>) -> &str {
    reason
        .map(str::trim)
        .and_then(|reason| reason.strip_prefix("incomplete:").or(Some(reason)))
        .filter(|reason| !reason.is_empty())
        .unwrap_or("unknown")
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
    // Concrete model facts take precedence over vendor-agnostic suffix
    // inference (`k3-256k` means 262,144 tokens, not 256,000).
    if let Some(window) = known_context_window_for_model(&lower) {
        return Some(window);
    }
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
        // Opus 5 (GA 2026-07-24) is 1M / 128K per
        // https://platform.claude.com/docs/en/about-claude/models/overview.
        "claude-opus-4-8" | "claude-opus-5" | "claude-sonnet-4-6" | "claude-sonnet-5"
        | "claude-fable-5" => Some(1_000_000),
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
        | "tencent/hy3-preview" => Some(262_144),
        // Official Kimi K3 platform pricing (2026-07-20):
        // https://platform.kimi.ai/docs/guide/kimi-k3-quickstart — 1,048,576 context
        // for the open platform.
        "moonshotai/kimi-k3" | "kimi-k3" | "opencode-go/kimi-k3" => {
            Some(KIMI_K3_CONTEXT_WINDOW_TOKENS)
        }
        // Bare `k3` is plan-tier dependent; `k3-256k` is fixed at 256 KiTok.
        // Neither may fall through to the generic suffix heuristic.
        "k3" | "k3-256k" => Some(KIMI_CODE_K3_CONTEXT_WINDOW_TOKENS),
        // `kimi-k2.7-code-highspeed` is the same model on the direct
        // platform's high-speed tier (262,144 context), per
        // https://platform.kimi.ai/docs/pricing/chat-k27-code (2026-08-17).
        "moonshotai/kimi-k2.7-code"
        | "moonshotai/kimi-k2.7-code-highspeed"
        | "moonshotai/kimi-k2.6"
        | "moonshotai/kimi-k2.6:free"
        | "kimi-k2.7-code"
        | "kimi-k2.7-code-highspeed"
        | "kimi-k2.6"
        | "kimi-for-coding"
        | "kimi-for-coding-highspeed" => Some(262_144),
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
        // GLM-5.3 limits are inherited from GLM-5.2 pending official Z.ai
        // release metadata (see `INHERITED FROM glm-5.2` in config/models.rs).
        // GLM-5.3-Flash is the published 1M multimodal sibling (2026-08-26).
        "z-ai/glm-5.2" | "glm-5.2" | "z-ai/glm-5.3" | "glm-5.3" | "z-ai/glm-5.3-flash"
        | "glm-5.3-flash" => Some(1_000_000),
        "minimax/minimax-m3" | "minimax-m3" | "qwen/qwen3.8-flash" | "qwen/qwen3.6-flash"
        | "qwen/qwen3.6-plus" => Some(1_000_000),
        // Alibaba Cloud Model Studio (Token Plan console + curated catalog,
        // verified 2026-08-03): ~1M context. Never fall through to the 128K
        // legacy default — that number is the generation ceiling, not the window.
        // Bare `qwen3.8-flash` is the OpenRouter short id (verified 2026-08-26
        // against models.dev: 1M context / 131K output); Alibaba first-party
        // does not list a flash row, so this is not a Model Studio default.
        "qwen3.8-max"
        | "qwen3.8-max-preview"
        | "qwen3.8-flash"
        | "qwen3.7-plus"
        | "qwen3.7-max"
        | "qwen3.6-flash" => Some(1_000_000),
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
        "grok-4.6" | "grok-4.5" => Some(500_000),
        "grok-4.3" => Some(1_000_000),
        "grok-build" => Some(512_000),
        "grok-composer-2.5-fast" => Some(200_000),
        "grok-4.20-0309-reasoning" | "grok-4.20-0309-non-reasoning" => Some(2_000_000),
        "muse-spark-1.1" | "muse-spark-1.2" | "muse-spark-1.2-contributor" => Some(1_000_000),
        // Mistral la Plateforme text/reasoning models: all report 262144
        // (256K) tokens on /v1/models as of 2026-08-08. Codestral coding
        // model (mistral-code-latest) reports 256000 tokens on the same
        // endpoint. IDs and windows verified live against
        // https://api.mistral.ai/v1/models rather than model-card slugs.
        "mistral-medium-latest"
        | "mistral-medium-3-5"
        | "mistral-medium-2604"
        | "mistral-medium-3.5"
        | "mistral-medium-3"
        | "mistral-small-latest"
        | "mistral-small-2603"
        | "magistral-small-latest"
        | "mistral-large-latest"
        | "mistral-large-2512" => Some(262_144),
        "mistral-code-latest" | "codestral-latest" | "codestral" | "mistral-code" => Some(256_000),
        // Google Gemini API model pages (verified 2026-08-17): every current
        // Gemini 3.x / 2.5 text model lists a 1,048,576-token input limit and
        // a 65,536-token output limit.
        // https://ai.google.dev/gemini-api/docs/models/gemini-3.7-flash
        // https://ai.google.dev/gemini-api/docs/models/gemini-3.6-flash
        // https://ai.google.dev/gemini-api/docs/models/gemini-3.5-flash
        // https://ai.google.dev/gemini-api/docs/models/gemini-3.5-flash-lite
        // https://ai.google.dev/gemini-api/docs/models/gemini-3.1-pro-preview
        // https://ai.google.dev/gemini-api/docs/models/gemini-2.5-pro
        // https://ai.google.dev/gemini-api/docs/models/gemini-2.5-flash
        // (gemini-3-pro-preview's page carries the same limits but is marked
        // shut down since 2026-03-09 on the Gemini API; it stays here only
        // because other routes still name it.)
        "gemini-3.7-flash"
        | "gemini-3.6-flash"
        | "gemini-3.5-flash"
        | "gemini-3.5-flash-lite"
        | "gemini-3.1-pro-preview"
        | "gemini-3-pro-preview"
        | "gemini-2.5-pro"
        | "gemini-2.5-flash" => Some(1_048_576),
        // OpenRouter-hosted Dots Studio (RedNote) Dots3-Note preview: the only
        // hosted route (single AtlasCloud endpoint) reports 512,000 context /
        // 512,000 max completion tokens, https://openrouter.ai/api/v1/models/
        // dots-studio/dots-3-note-preview:free/endpoints (2026-08-17).
        "dots-studio/dots-3-note-preview:free" => Some(512_000),
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
        "claude-opus-4-8" | "claude-opus-5" | "claude-sonnet-4-6" | "claude-sonnet-5"
        | "claude-fable-5" => Some(128_000),
        "claude-haiku-4-5" => Some(64_000),
        "arcee-ai/trinity-large-thinking" | "trinity-large-thinking" => Some(262_144),
        // Keep the generic/model-id lookup at K3's conservative documented
        // default generation ceiling. The exact direct route's 1M maximum is
        // applied later with endpoint-aware provenance; membership and
        // neighboring routes must not inherit it.
        "moonshotai/kimi-k3" | "kimi-k3" | "k3" | "k3-256k" | "opencode-go/kimi-k3" => {
            Some(KIMI_K3_DEFAULT_MAX_COMPLETION_TOKENS)
        }
        // Kimi K2.7 Code has a 256K context window but its documented default
        // maximum generation is 32K. Keeping those separate prevents the
        // input budget from collapsing to the 1K emergency floor (#4368). The
        // direct-platform value matches the provider-reported bundled
        // catalog. The Kimi Code membership ids (`kimi-for-coding` family)
        // are deliberately absent here: the membership catalog is the source
        // of truth for their limits and no client-side output ceiling is
        // claimed, so they fall back to the generic default.
        "moonshotai/kimi-k2.7-code"
        | "moonshotai/kimi-k2.7-code-highspeed"
        | "moonshotai/kimi-k2.6"
        | "kimi-k2.7-code"
        | "kimi-k2.7-code-highspeed"
        | "kimi-k2.6" => Some(32_768),
        "minimax/minimax-m3" | "minimax-m3" => Some(524_288),
        // Alibaba's published limit is 65,536 output tokens; the earlier
        // 262,140 mirrored the context window (data-entry smell flagged by
        // MODEL_PROVIDER_AUDIT A2/D-7, vendor-verified 2026-07-12).
        "qwen/qwen3.6-35b-a3b"
        | "qwen/qwen3.6-27b"
        | "qwen/qwen3.6-flash"
        | "qwen/qwen3.6-max-preview"
        | "qwen/qwen3.6-plus" => Some(65_536),
        // Model Studio: 128K is the generation ceiling, not the context window.
        // OpenRouter qwen3.8-flash shares the 131,072 output cap (models.dev
        // 2026-08-26); do not inherit qwen3.6-flash's 65,536 ceiling.
        "qwen3.8-max" | "qwen3.8-max-preview" | "qwen/qwen3.8-flash" | "qwen3.8-flash" => {
            Some(131_072)
        }
        "qwen3.7-plus" | "qwen3.7-max" | "qwen3.6-flash" => Some(65_536),
        "z-ai/glm-5.1" | "z-ai/glm-5.2" | "z-ai/glm-5.3" | "z-ai/glm-5.3-flash"
        | "z-ai/glm-5-turbo" | "glm-5.1" | "glm-5.2" | "glm-5.3" | "glm-5.3-flash"
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
        "muse-spark-1.1" | "muse-spark-1.2" | "muse-spark-1.2-contributor" => Some(32_000),
        // Gemini API output token limit (see `known_context_window_for_model`).
        "gemini-3.7-flash"
        | "gemini-3.6-flash"
        | "gemini-3.5-flash"
        | "gemini-3.5-flash-lite"
        | "gemini-3.1-pro-preview"
        | "gemini-3-pro-preview"
        | "gemini-2.5-pro"
        | "gemini-2.5-flash" => Some(65_536),
        "dots-studio/dots-3-note-preview:free" => Some(512_000),
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
    if lower.starts_with("mistral-medium")
        || lower.starts_with("mistral-small")
        || lower.starts_with("magistral")
    {
        return true;
    }
    matches!(
        lower.as_str(),
        "claude-opus-4-8"
            | "claude-opus-5"
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
            | "moonshotai/kimi-k2.7-code-highspeed"
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
            | "qwen/qwen3.8-flash"
            | "qwen/qwen3.6-flash"
            | "qwen/qwen3.6-35b-a3b"
            | "qwen/qwen3.6-max-preview"
            | "qwen/qwen3.6-27b"
            | "qwen/qwen3.6-plus"
            | "qwen/qwen3.7-plus"
            // Bare qwen3.x ids are Alibaba Cloud Model Studio's own model ids
            // (Token Plan / Coding Plan catalogs). Per Model Studio's
            // deep-thinking docs these are hybrid-thinking models that stream
            // `reasoning_content` (OpenAI dialect) or thinking blocks
            // (Anthropic dialect); qwen3.7/3.6/3.5 families default thinking
            // ON server-side. Bare `qwen3.8-flash` is the OpenRouter short
            // id (reasoning: true on models.dev 2026-08-26), not a Model
            // Studio family default.
            | "qwen3.8-max"
            | "qwen3.8-max-preview"
            | "qwen3.8-flash"
            | "qwen3.7-max"
            | "qwen3.7-plus"
            | "qwen3.6-plus"
            | "qwen3.6-flash"
            | "qwen3.5-plus"
            | "qwen3.5-flash"
            | "tencent/hy3-preview"
            | "xiaomi/mimo-v2.5-pro"
            | "xiaomi/mimo-v2.5"
            | "mimo-v2.5-pro"
            | "mimo-v2.5-pro-ultraspeed"
            | "mimo-v2.5"
            | "z-ai/glm-5.1"
            | "z-ai/glm-5.2"
            | "z-ai/glm-5.3"
            | "z-ai/glm-5.3-flash"
            | "z-ai/glm-5-turbo"
            | "glm-5.1"
            | "glm-5.2"
            | "glm-5.3"
            | "glm-5.3-flash"
            | "glm-5-turbo"
            | "grok-4.6"
            | "grok-4.5"
            | "grok-4.3"
            | "grok-build"
            | "grok-4.20-0309-reasoning"
            | "muse-spark-1.1"
            | "muse-spark-1.2"
            | "muse-spark-1.2-contributor"
    ) || is_openai_gpt_55_api_model(&lower)
        || is_openai_gpt_56_api_model(&lower)
        || is_openai_codex_model(&lower)
}

/// Contributor tier of Muse Spark 1.2 is a distinct selectable id with
/// its own wire model (`muse-spark-1.2-contributor`) and cheaper billing in
/// exchange for training-data opt-in. Do not collapse it to the standard tier.
#[must_use]
pub fn effective_muse_wire_id(model: &str) -> &str {
    model
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

/// The context window a model name's `_Nk` suffix advertises, when the
/// catalog does not already describe the model (#5441).
///
/// Exposed separately from [`explicit_context_window_hint`] because the
/// honesty surfaces need to know *whether the number they are holding came
/// from the name* — a naming convention the serving engine may ignore is not
/// a fact about the route, and every surface that shows such a window must
/// mark it unverified.
#[must_use]
pub(crate) fn name_suffix_context_window_hint(model: &str) -> Option<u32> {
    if crate::model_catalog::resolved_context_window(model).is_some() {
        return None;
    }
    explicit_context_window_hint(&model.to_lowercase())
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
/// configure it. Known model windows default automatic continuity on; an
/// explicit `auto_compact = false` remains authoritative at the call sites.
#[must_use]
#[cfg(test)]
pub fn auto_compact_default_for_model(model: &str) -> bool {
    context_window_for_model(model).is_some()
}

// === Streaming Structures ===

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
/// Streaming event types for SSE responses.
pub enum StreamEvent {
    /// Local pre-stream receipt: the provider request was sent with a reduced
    /// tool surface. This is not provider SSE and must not count as content.
    #[serde(rename = "tool_projection_warning")]
    ToolProjectionWarning {
        provider: String,
        omitted_tool_names: Vec<String>,
        omitted_tool_count: usize,
    },
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
        /// Google thought signature, when the first streaming chunk of this
        /// tool call carried `extra_content.google.thought_signature`.
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
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
    /// Opaque Responses reasoning continuity, attached only when the provider
    /// returns an encrypted item on the exact originating route.
    #[serde(rename = "reasoning_state_delta")]
    ReasoningStateDelta { state: OpaqueReasoningState },
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
    use std::any::TypeId;
    use std::collections::BTreeMap;

    #[test]
    fn output_limit_stop_reason_accepts_provider_aliases_only() {
        for reason in [
            "length",
            "max_tokens",
            "max_output_tokens",
            " MAX_TOKENS ",
            "incomplete:max_output_tokens",
        ] {
            assert!(is_output_limit_stop_reason(Some(reason)), "{reason}");
        }
        for reason in [None, Some("end_turn"), Some("tool_use"), Some("")] {
            assert!(!is_output_limit_stop_reason(reason), "{reason:?}");
        }
    }

    #[test]
    fn incomplete_stop_reason_never_accepts_unknown_responses_failures() {
        assert!(is_incomplete_stop_reason(Some("incomplete:content_filter")));
        assert!(is_incomplete_stop_reason(Some("content_filter")));
        assert!(is_incomplete_stop_reason(Some(
            "model_context_window_exceeded"
        )));
        assert!(is_incomplete_stop_reason(Some("max_tokens")));
        assert!(!is_incomplete_stop_reason(Some("end_turn")));
        assert_eq!(
            stop_reason_detail(Some("incomplete:content_filter")),
            "content_filter"
        );
    }

    #[test]
    fn historical_tui_request_path_is_the_core_request_type() {
        assert_eq!(
            TypeId::of::<MessageRequest>(),
            TypeId::of::<codewhale_core::request::MessageRequest>()
        );

        let via_tui_path = MessageRequest {
            model: "model".to_string(),
            messages: vec![],
            max_tokens: 1024,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(true),
            temperature: None,
            top_p: None,
        };
        let via_core_path: codewhale_core::request::MessageRequest = via_tui_path.clone();
        assert_eq!(
            serde_json::to_vec(&via_tui_path).expect("serialize TUI path"),
            serde_json::to_vec(&via_core_path).expect("serialize core path")
        );
    }

    #[test]
    fn interrupted_assistant_role_round_trips_as_distinct_session_item() {
        let message = Message {
            role: Role::InterruptedAssistant,
            content: vec![ContentBlock::Text {
                text: "partial output".to_string(),
                cache_control: None,
            }],
        };
        let encoded = serde_json::to_string(&message).expect("message should serialize");
        let decoded: Message = serde_json::from_str(&encoded).expect("message should deserialize");
        assert_eq!(decoded, message);
        assert_ne!(decoded.role, "assistant");
    }

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
            (concat!("qwen/", "qwen3.8-flash"), 1_000_000),
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
            ("z-ai/glm-5.3", 1_000_000),
            ("z-ai/glm-5.3-flash", 1_000_000),
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
    fn claude_opus_5_has_verified_metadata() {
        // 1M context / 128K output, adaptive thinking, per
        // https://platform.claude.com/docs/en/about-claude/models/overview
        // (2026-08-17).
        assert_eq!(context_window_for_model("claude-opus-5"), Some(1_000_000));
        assert_eq!(max_output_tokens_for_model("claude-opus-5"), Some(128_000));
        assert!(model_supports_reasoning("claude-opus-5"));
    }

    #[test]
    fn kimi_k2_7_code_highspeed_shares_the_k2_7_code_limits() {
        // https://platform.kimi.ai/docs/pricing/chat-k27-code (2026-08-17):
        // same model as kimi-k2.7-code, 262,144 context.
        for model in [
            "kimi-k2.7-code-highspeed",
            "moonshotai/kimi-k2.7-code-highspeed",
        ] {
            assert_eq!(context_window_for_model(model), Some(262_144), "{model}");
            assert_eq!(max_output_tokens_for_model(model), Some(32_768), "{model}");
            assert!(model_supports_reasoning(model), "{model}");
        }
    }

    #[test]
    fn gemini_api_models_have_documented_token_limits() {
        // Every current Gemini API text model page lists 1,048,576 input /
        // 65,536 output (verified 2026-08-17, see
        // `known_context_window_for_model`).
        for model in [
            "gemini-3.7-flash",
            "gemini-3.6-flash",
            "gemini-3.5-flash",
            "gemini-3.5-flash-lite",
            "gemini-3.1-pro-preview",
            "gemini-3-pro-preview",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
        ] {
            assert_eq!(context_window_for_model(model), Some(1_048_576), "{model}");
            assert_eq!(max_output_tokens_for_model(model), Some(65_536), "{model}");
        }
    }

    #[test]
    fn muse_spark_has_verified_context_and_reasoning_metadata() {
        assert_eq!(context_window_for_model("muse-spark-1.1"), Some(1_000_000));
        assert_eq!(max_output_tokens_for_model("muse-spark-1.1"), Some(32_000));
        assert!(model_supports_reasoning("muse-spark-1.1"));
        // Muse Spark 1.2 standard: 1M context, $1.25/$4.25 + $0.15 cache (Artificial Analysis).
        assert_eq!(context_window_for_model("muse-spark-1.2"), Some(1_000_000));
        assert_eq!(max_output_tokens_for_model("muse-spark-1.2"), Some(32_000));
        assert!(model_supports_reasoning("muse-spark-1.2"));
        // Contributor tier: same model/limits, ~12×/21× cheaper in exchange for training-data opt-in.
        assert_eq!(
            context_window_for_model("muse-spark-1.2-contributor"),
            Some(1_000_000)
        );
        assert_eq!(
            max_output_tokens_for_model("muse-spark-1.2-contributor"),
            Some(32_000)
        );
        assert!(model_supports_reasoning("muse-spark-1.2-contributor"));
    }

    #[test]
    fn modelstudio_qwen38_max_is_1m_context_not_128k() {
        // Owner Token Plan console + curated catalog (2026-08-03). The 128K
        // figure is max output, not the window — never collapse them.
        for model in ["qwen3.8-max", "qwen3.8-max-preview"] {
            assert_eq!(context_window_for_model(model), Some(1_000_000), "{model}");
            assert_eq!(max_output_tokens_for_model(model), Some(131_072), "{model}");
        }
    }

    #[test]
    fn openrouter_qwen38_flash_is_1m_context_with_128k_output() {
        // models.dev OpenRouter listing 2026-08-26: 1,000,000 / 131,072.
        // Both the namespaced wire id and the bare short id must resolve.
        for model in ["qwen/qwen3.8-flash", "qwen3.8-flash"] {
            assert_eq!(context_window_for_model(model), Some(1_000_000), "{model}");
            assert_eq!(max_output_tokens_for_model(model), Some(131_072), "{model}");
            assert!(model_supports_reasoning(model), "{model}");
        }
    }

    #[test]
    fn modelstudio_bare_qwen_models_support_reasoning() {
        // Model Studio's deep-thinking docs: every qwen3.x family the Token /
        // Coding Plan catalogs carry is hybrid-thinking (reasoning_content on
        // the OpenAI dialect, thinking blocks on the Anthropic dialect).
        for model in [
            "qwen3.8-max",
            "qwen3.8-max-preview",
            "qwen3.7-max",
            "qwen3.7-plus",
            "qwen3.6-plus",
            "qwen3.6-flash",
            "qwen3.5-plus",
            "qwen3.5-flash",
        ] {
            assert!(model_supports_reasoning(model), "{model}");
        }
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
        assert!(model_supports_reasoning("kimi-for-coding-highspeed"));
        assert!(model_supports_reasoning("kimi-k2.5"));
    }

    #[test]
    fn xai_grok_models_have_static_context_metadata() {
        for (model, expected_window, supports_reasoning) in [
            ("grok-4.6", 500_000, true),
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
            max_output_tokens_for_model(concat!("qwen/", "qwen3.8-flash")),
            Some(131_072)
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
        assert_eq!(max_output_tokens_for_model("z-ai/glm-5.3"), Some(131_072));
        assert_eq!(
            max_output_tokens_for_model("z-ai/glm-5-turbo"),
            Some(131_072)
        );
        assert_eq!(max_output_tokens_for_model("glm-5-turbo"), Some(131_072));
    }

    #[test]
    fn k3_route_ids_use_verified_contracts_not_legacy_128k() {
        // Open-platform K3 carries the verified 1M contract.
        assert_eq!(context_window_for_model("kimi-k3"), Some(1_048_576));
        assert_eq!(
            context_window_for_model("opencode-go/kimi-k3"),
            Some(1_048_576)
        );
        // Bare `k3` (Kimi Code membership) is plan-tier dependent, so it
        // keeps the documented safe floor — and must never fall through to
        // the 128K legacy default.
        assert_eq!(context_window_for_model("k3"), Some(262_144));
        assert_eq!(context_window_for_model("k3-256k"), Some(262_144));
        assert_eq!(max_output_tokens_for_model("k3"), Some(131_072));
        assert_eq!(max_output_tokens_for_model("k3-256k"), Some(131_072));
        assert_eq!(max_output_tokens_for_model("kimi-k3"), Some(131_072));
        // Never project max output as the context window.
        assert_ne!(
            context_window_for_model("k3"),
            max_output_tokens_for_model("k3")
        );
        assert_ne!(
            context_window_for_model("kimi-k3"),
            max_output_tokens_for_model("kimi-k3")
        );
    }

    #[test]
    fn kimi_code_membership_ids_mirror_their_family_facts() {
        // The high-speed membership id rides the kimi-for-coding family
        // context fact (256K) and reasoning support via the same `kimi-`
        // native-id rule as `kimi-for-coding`. No client-side output ceiling
        // is claimed for the membership ids — the membership catalog is the
        // source of truth, so the generic lookup returns None.
        assert_eq!(
            context_window_for_model("kimi-for-coding-highspeed"),
            Some(262_144)
        );
        assert_eq!(
            max_output_tokens_for_model("kimi-for-coding-highspeed"),
            None
        );
        assert_eq!(max_output_tokens_for_model("kimi-for-coding"), None);
        assert!(model_supports_reasoning("kimi-for-coding-highspeed"));
    }

    #[test]
    fn bare_provider_model_ids_mirror_vendor_prefixed_rows() {
        // Direct-provider routes (Moonshot, MiniMax, Z.ai) serve bare model
        // ids without the OpenRouter vendor prefix; both spellings must
        // resolve identical metadata (#1310 ride-along on #3023).
        for (model, expected_window) in [
            ("kimi-k3", 1_048_576),
            ("kimi-k2.7-code", 262_144),
            ("kimi-k2.6", 262_144),
            ("minimax-m3", 1_000_000),
            ("minimax-m2.7", 204_800),
            ("minimax-m2.5-highspeed", 204_800),
            ("minimax-m2", 204_800),
            ("glm-5.1", 202_752),
            ("glm-5.2", 1_000_000),
            // Inherited from glm-5.2 pending official Z.ai release metadata.
            ("glm-5.3", 1_000_000),
            ("glm-5.3-flash", 1_000_000),
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
        assert_eq!(max_output_tokens_for_model("kimi-for-coding"), None);
        assert_eq!(max_output_tokens_for_model("kimi-k3"), Some(131_072));
        assert_eq!(max_output_tokens_for_model("minimax-m3"), Some(524_288));
        assert_eq!(max_output_tokens_for_model("glm-5.1"), Some(131_072));
        assert_eq!(max_output_tokens_for_model("glm-5.2"), Some(131_072));
        assert_eq!(max_output_tokens_for_model("glm-5.3"), Some(131_072));
        assert_eq!(max_output_tokens_for_model("glm-5.3-flash"), Some(131_072));
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
