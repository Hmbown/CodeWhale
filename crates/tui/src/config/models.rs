//! Static provider model-name and base-URL constants.
//!
//! These are pure data tables (default model identifiers, base URLs, and
//! curated model lists) extracted verbatim from `config.rs` to keep the
//! configuration monolith focused on loading/normalization logic. They are
//! re-exported from `crate::config` via `pub use models::*;`, so every existing
//! `crate::config::<CONST>` path keeps resolving unchanged (#3311).

pub const DEFAULT_TEXT_MODEL: &str = "deepseek-v4-pro";
pub const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/beta";
pub const DEFAULT_DEEPSEEK_ANTHROPIC_MODEL: &str = DEFAULT_TEXT_MODEL;
pub const DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL: &str = "https://api.deepseek.com/anthropic";
pub const DEFAULT_NVIDIA_NIM_MODEL: &str = "deepseek-ai/deepseek-v4-pro";
pub const DEFAULT_NVIDIA_NIM_FLASH_MODEL: &str = "deepseek-ai/deepseek-v4-flash";
pub const DEFAULT_NVIDIA_NIM_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub const DEFAULT_OPENAI_MODEL: &str = "deepseek-v4-pro";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_ATLASCLOUD_MODEL: &str = "deepseek-ai/deepseek-v4-flash";
pub const DEFAULT_ATLASCLOUD_BASE_URL: &str = "https://api.atlascloud.ai/v1";
pub const DEFAULT_WANJIE_ARK_MODEL: &str = "deepseek-reasoner";
pub const DEFAULT_VOLCENGINE_MODEL: &str = "DeepSeek-V4-Pro";
pub const DEFAULT_VOLCENGINE_FLASH_MODEL: &str = "DeepSeek-V4-Flash";
pub const DEFAULT_VOLCENGINE_BASE_URL: &str = "https://ark.cn-beijing.volces.com/api/coding/v3";
pub const DEFAULT_WANJIE_ARK_BASE_URL: &str = "https://maas-openapi.wanjiedata.com/api/v1";
pub const DEFAULT_OPENROUTER_MODEL: &str = "deepseek/deepseek-v4-pro";
pub const DEFAULT_OPENROUTER_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
pub const OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL: &str = "arcee-ai/trinity-large-thinking";
pub const OPENROUTER_GEMMA_4_31B_MODEL: &str = "google/gemma-4-31b-it";
pub const OPENROUTER_GEMMA_4_26B_A4B_MODEL: &str = "google/gemma-4-26b-a4b-it";
pub const OPENROUTER_GLM_5_1_MODEL: &str = "z-ai/glm-5.1";
pub const OPENROUTER_GLM_5_2_MODEL: &str = "z-ai/glm-5.2";
pub const OPENROUTER_GLM_5_TURBO_MODEL: &str = "z-ai/glm-5-turbo";
pub const OPENROUTER_KIMI_K2_7_CODE_MODEL: &str = "moonshotai/kimi-k2.7-code";
pub const OPENROUTER_KIMI_K2_6_MODEL: &str = "moonshotai/kimi-k2.6";
pub const OPENROUTER_MINIMAX_M3_MODEL: &str = "minimax/minimax-m3";
pub const OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL: &str =
    "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free";
pub const OPENROUTER_QWEN_3_6_FLASH_MODEL: &str = "qwen/qwen3.6-flash";
pub const OPENROUTER_QWEN_3_6_35B_A3B_MODEL: &str = "qwen/qwen3.6-35b-a3b";
pub const OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL: &str = "qwen/qwen3.6-max-preview";
pub const OPENROUTER_QWEN_3_6_27B_MODEL: &str = "qwen/qwen3.6-27b";
pub const OPENROUTER_QWEN_3_6_PLUS_MODEL: &str = "qwen/qwen3.6-plus";
pub const OPENROUTER_QWEN_3_7_PLUS_MODEL: &str = "qwen/qwen3.7-plus";
pub const OPENROUTER_QWEN_3_7_MAX_MODEL: &str = "qwen/qwen3.7-max";
pub const OPENROUTER_MINIMAX_M2_7_MODEL: &str = "minimax/minimax-m2.7";
pub const OPENROUTER_NEMOTRON_3_ULTRA_MODEL: &str = "nvidia/nemotron-3-ultra-550b-a55b";
pub const OPENROUTER_TENCENT_HY3_PREVIEW_MODEL: &str = "tencent/hy3-preview";
pub const OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL: &str = "xiaomi/mimo-v2.5-pro";
pub const OPENROUTER_XIAOMI_MIMO_V2_5_MODEL: &str = "xiaomi/mimo-v2.5";
pub const RECENT_OPENROUTER_LARGE_MODELS: &[&str] = &[
    OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL,
    OPENROUTER_MINIMAX_M3_MODEL,
    OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL,
    OPENROUTER_XIAOMI_MIMO_V2_5_MODEL,
    OPENROUTER_QWEN_3_6_FLASH_MODEL,
    OPENROUTER_QWEN_3_6_35B_A3B_MODEL,
    OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL,
    OPENROUTER_QWEN_3_6_27B_MODEL,
    OPENROUTER_QWEN_3_6_PLUS_MODEL,
    OPENROUTER_QWEN_3_7_PLUS_MODEL,
    OPENROUTER_QWEN_3_7_MAX_MODEL,
    OPENROUTER_MINIMAX_M2_7_MODEL,
    OPENROUTER_NEMOTRON_3_ULTRA_MODEL,
    OPENROUTER_KIMI_K2_7_CODE_MODEL,
    OPENROUTER_KIMI_K2_6_MODEL,
    OPENROUTER_GLM_5_1_MODEL,
    OPENROUTER_GLM_5_2_MODEL,
    OPENROUTER_TENCENT_HY3_PREVIEW_MODEL,
    OPENROUTER_GEMMA_4_31B_MODEL,
    OPENROUTER_GEMMA_4_26B_A4B_MODEL,
    OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL,
];
pub const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const DEFAULT_XIAOMI_MIMO_MODEL: &str = "mimo-v2.5-pro";
pub const XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL: &str = "mimo-v2.5-pro-ultraspeed";
pub const XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
pub const DEFAULT_XIAOMI_MIMO_BASE_URL: &str = "https://token-plan-sgp.xiaomimimo.com/v1";
pub const XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL: &str = "https://token-plan-cn.xiaomimimo.com/v1";
pub const XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL: &str = DEFAULT_XIAOMI_MIMO_BASE_URL;
pub const XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL: &str = "https://token-plan-ams.xiaomimimo.com/v1";
pub const XIAOMI_MIMO_V2_5_OMNI_MODEL: &str = "mimo-v2.5";
pub const XIAOMI_MIMO_ASR_MODEL: &str = "mimo-v2.5-asr";
pub const XIAOMI_MIMO_TTS_MODEL: &str = "mimo-v2.5-tts";
pub const XIAOMI_MIMO_TTS_VOICE_DESIGN_MODEL: &str = "mimo-v2.5-tts-voicedesign";
pub const XIAOMI_MIMO_TTS_VOICE_CLONE_MODEL: &str = "mimo-v2.5-tts-voiceclone";
pub const XIAOMI_MIMO_V2_TTS_MODEL: &str = "mimo-v2-tts";
pub const DEFAULT_NOVITA_MODEL: &str = "deepseek/deepseek-v4-pro";
pub const DEFAULT_NOVITA_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
pub const DEFAULT_NOVITA_BASE_URL: &str = "https://api.novita.ai/openai/v1";
pub const DEFAULT_FIREWORKS_MODEL: &str = "accounts/fireworks/models/deepseek-v4-pro";
pub const DEFAULT_FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";
pub const DEFAULT_SILICONFLOW_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const DEFAULT_SILICONFLOW_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const DEFAULT_SILICONFLOW_BASE_URL: &str = "https://api.siliconflow.com/v1";
pub const DEFAULT_SILICONFLOW_CN_BASE_URL: &str = "https://api.siliconflow.cn/v1";
pub const DEFAULT_ARCEE_MODEL: &str = "trinity-large-thinking";
pub const ARCEE_TRINITY_LARGE_PREVIEW_MODEL: &str = "trinity-large-preview";
pub const ARCEE_TRINITY_MINI_MODEL: &str = "trinity-mini";
pub const DEFAULT_ARCEE_BASE_URL: &str = "https://api.arcee.ai/api/v1";
pub const DEFAULT_MOONSHOT_MODEL: &str = "kimi-k2.7-code";
pub const MOONSHOT_KIMI_K2_6_MODEL: &str = "kimi-k2.6";
pub const DEFAULT_MOONSHOT_BASE_URL: &str = "https://api.moonshot.ai/v1";
pub const DEFAULT_KIMI_CODE_MODEL: &str = "kimi-for-coding";
pub const DEFAULT_KIMI_CODE_BASE_URL: &str = "https://api.kimi.com/coding/v1";
pub const KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL: &str =
    codewhale_config::provider::KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL;
/// Official Kimi Code route model id. It is deliberately distinct from
/// Moonshot's pay-as-you-go `kimi-k3` catalog id.
pub const KIMI_CODE_K3_MODEL: &str = "k3";
/// Kimi K3 context window for every genuine K3 route (direct Moonshot
/// `kimi-k3`, Kimi Code membership bare `k3`, and OpenCode Go `kimi-k3`).
/// Official Kimi platform pricing documents 1,048,576 tokens for `kimi-k3`
/// (verified 2026-07-20 from https://platform.kimi.ai/docs/pricing/chat-k3).
/// Max output remains 131_072 and must not be conflated with this window.
pub const KIMI_CODE_K3_CONTEXT_WINDOW_TOKENS: u32 = 1_048_576;
/// Documented K3 max generation tokens; never use this as a context window.
#[allow(dead_code)] // Public contract constant for route/budget call sites.
pub const KIMI_K3_MAX_OUTPUT_TOKENS: u32 = 131_072;
pub const DEFAULT_SGLANG_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const DEFAULT_SGLANG_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const DEFAULT_SGLANG_BASE_URL: &str = "http://localhost:30000/v1";
pub const DEFAULT_VLLM_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const DEFAULT_VLLM_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const DEFAULT_VLLM_BASE_URL: &str = "http://localhost:8000/v1";
pub const DEFAULT_OLLAMA_MODEL: &str = "deepseek-v4-flash";
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
pub const DEFAULT_HUGGINGFACE_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const DEFAULT_HUGGINGFACE_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const DEFAULT_HUGGINGFACE_BASE_URL: &str = "https://router.huggingface.co/v1";
pub const DEFAULT_DEEPINFRA_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const DEFAULT_DEEPINFRA_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const DEFAULT_DEEPINFRA_BASE_URL: &str = "https://api.deepinfra.com/v1/openai";
pub const DEFAULT_TOGETHER_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const DEFAULT_TOGETHER_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub const TOGETHER_INKLING_MODEL: &str = "thinkingmachines/inkling";
pub const DEFAULT_TOGETHER_BASE_URL: &str = "https://api.together.xyz/v1";
pub const DEFAULT_QIANFAN_MODEL: &str = "ernie-4.0-turbo-8k";
pub const DEFAULT_QIANFAN_BASE_URL: &str = "https://api.baiduqianfan.ai/v1";
pub const DEFAULT_OPENAI_CODEX_MODEL: &str = "gpt-5.5";
pub const DEFAULT_OPENAI_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
/// Conservative offline floor for an OAuth model absent from a fresh Codex
/// roster. Fresh account-scoped cache metadata overrides this in route_runtime.
pub const OPENAI_CODEX_EFFECTIVE_CONTEXT_WINDOW_TOKENS: u32 = 128_000;
/// Legacy `deepseek-cn` provider alias.
///
/// DeepSeek's official API host is the same worldwide. Keep this alias for
/// old configs, but route it through the normal beta-enabled DeepSeek default.
/// Legacy typo hostname `api.deepseeki.com` remains recognized in URL
/// heuristics for backward compatibility.
pub const DEFAULT_DEEPSEEKCN_BASE_URL: &str = DEFAULT_DEEPSEEK_BASE_URL;
pub const COMMON_DEEPSEEK_MODELS: &[&str] = &[
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "deepseek-ai/deepseek-v4-pro",
    "deepseek-ai/deepseek-v4-flash",
    "deepseek/deepseek-v4-pro",
    "deepseek/deepseek-v4-flash",
];
pub const OFFICIAL_DEEPSEEK_MODELS: &[&str] = &["deepseek-v4-pro", "deepseek-v4-flash"];
pub const DEFAULT_ZAI_MODEL: &str = "GLM-5.2";
pub const ZAI_GLM_5_1_MODEL: &str = "GLM-5.1";
pub const ZAI_GLM_5_2_MODEL: &str = "GLM-5.2";
pub const ZAI_GLM_5_TURBO_MODEL: &str = "GLM-5-Turbo";
pub const DEFAULT_ZAI_BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";
pub const DEFAULT_STEPFUN_MODEL: &str = "step-3.7-flash";
pub const DEFAULT_STEPFUN_BASE_URL: &str = "https://api.stepfun.ai/v1";
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
pub const ANTHROPIC_OPUS_MODEL: &str = "claude-opus-4-8";
pub const ANTHROPIC_HAIKU_MODEL: &str = "claude-haiku-4-5";
pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_OPENMODEL_MODEL: &str = "deepseek-v4-flash";
pub const DEFAULT_OPENMODEL_BASE_URL: &str = "https://api.openmodel.ai";
pub const DEFAULT_MINIMAX_MODEL: &str = "MiniMax-M3";
pub const MINIMAX_M2_7_MODEL: &str = "MiniMax-M2.7";
pub const MINIMAX_M2_7_HIGHSPEED_MODEL: &str = "MiniMax-M2.7-highspeed";
pub const MINIMAX_M2_5_MODEL: &str = "MiniMax-M2.5";
pub const MINIMAX_M2_5_HIGHSPEED_MODEL: &str = "MiniMax-M2.5-highspeed";
pub const MINIMAX_M2_1_MODEL: &str = "MiniMax-M2.1";
pub const MINIMAX_M2_1_HIGHSPEED_MODEL: &str = "MiniMax-M2.1-highspeed";
pub const MINIMAX_M2_MODEL: &str = "MiniMax-M2";
pub const DEFAULT_MINIMAX_BASE_URL: &str = "https://api.minimax.io/v1";
pub const DEFAULT_MINIMAX_ANTHROPIC_BASE_URL: &str = "https://api.minimax.io/anthropic";
pub const DEFAULT_SAKANA_MODEL: &str = "fugu";
pub const SAKANA_FUGU_ULTRA_MODEL: &str = "fugu-ultra-20260615";
pub const DEFAULT_SAKANA_BASE_URL: &str = "https://api.sakana.ai/v1";
pub const DEFAULT_LONGCAT_MODEL: &str = "LongCat-2.0";
pub const DEFAULT_LONGCAT_BASE_URL: &str = "https://api.longcat.chat/openai/v1";
pub const DEFAULT_OPENCODE_GO_MODEL: &str = "deepseek-v4-pro";
pub const DEFAULT_OPENCODE_GO_BASE_URL: &str = "https://opencode.ai/zen/go/v1";
pub use codewhale_config::OPENCODE_GO_CHAT_MODELS;
pub const DEFAULT_META_MODEL: &str = "muse-spark-1.1";
pub const DEFAULT_META_BASE_URL: &str = "https://api.meta.ai/v1";
pub const DEFAULT_XAI_MODEL: &str = "grok-4.5";
pub const XAI_GROK_4_3_MODEL: &str = "grok-4.3";
pub const XAI_GROK_BUILD_MODEL: &str = "grok-build";
pub const XAI_GROK_COMPOSER_2_5_FAST_MODEL: &str = "grok-composer-2.5-fast";
pub const XAI_GROK_4_20_0309_REASONING_MODEL: &str = "grok-4.20-0309-reasoning";
pub const XAI_GROK_4_20_0309_NON_REASONING_MODEL: &str = "grok-4.20-0309-non-reasoning";
pub const DEFAULT_XAI_BASE_URL: &str = "https://api.x.ai/v1";
