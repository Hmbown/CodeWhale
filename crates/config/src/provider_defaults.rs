//! Built-in provider default seeds: per-provider default model ids and
//! base URLs, plus the named model/tier constants the alias-normalization
//! tables resolve to. Extracted verbatim from `lib.rs` (#3311) to separate
//! these provider execution defaults from config schema/loading code; values
//! are unchanged. Re-exported `pub(crate)` at the crate root so existing
//! `crate::DEFAULT_*` references keep resolving.

pub(crate) const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-v4-pro";
pub(crate) const DEFAULT_DEEPSEEK_ANTHROPIC_MODEL: &str = DEFAULT_DEEPSEEK_MODEL;
pub(crate) const DEFAULT_NVIDIA_NIM_MODEL: &str = "deepseek-ai/deepseek-v4-pro";
pub(crate) const DEFAULT_NVIDIA_NIM_FLASH_MODEL: &str = "deepseek-ai/deepseek-v4-flash";
// A DeepSeek id here guaranteed a 404 against the default OpenAI endpoint:
// unlike the hosted-aggregator rows below, api.openai.com serves no DeepSeek
// models. Default to OpenAI's own flagship instead (#5588).
pub(crate) const DEFAULT_OPENAI_MODEL: &str = "gpt-5.6";
pub(crate) const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/beta";
pub(crate) const DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL: &str = "https://api.deepseek.com/anthropic";
pub(crate) const DEFAULT_NVIDIA_NIM_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub(crate) const DEFAULT_OPENAI_CODEX_MODEL: &str = "gpt-5.6";
pub(crate) const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
pub(crate) const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub(crate) const DEFAULT_OPENMODEL_MODEL: &str = "deepseek-v4-flash";
pub(crate) const DEFAULT_OPENMODEL_BASE_URL: &str = "https://api.openmodel.ai";
pub(crate) const DEFAULT_OPENAI_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
pub(crate) const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub(crate) const DEFAULT_ATLASCLOUD_MODEL: &str = "deepseek-ai/deepseek-v4-flash";
pub(crate) const DEFAULT_ATLASCLOUD_BASE_URL: &str = "https://api.atlascloud.ai/v1";
pub(crate) const DEFAULT_WANJIE_ARK_MODEL: &str = "deepseek-reasoner";
pub(crate) const DEFAULT_WANJIE_ARK_BASE_URL: &str = "https://maas-openapi.wanjiedata.com/api/v1";
pub(crate) const DEFAULT_VOLCENGINE_MODEL: &str = "DeepSeek-V4-Pro";
pub(crate) const DEFAULT_VOLCENGINE_BASE_URL: &str =
    "https://ark.cn-beijing.volces.com/api/coding/v3";
pub(crate) const DEFAULT_OPENROUTER_MODEL: &str = "deepseek/deepseek-v4-pro";
pub(crate) const DEFAULT_OPENROUTER_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
pub(crate) const DEFAULT_ORCAROUTER_MODEL: &str = "deepseek/deepseek-v4-pro";
pub(crate) const DEFAULT_ORCAROUTER_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
/// OrcaRouter's own auto-routing model: picks the best upstream model per
/// request. Resolved from the bare `auto` alias on the OrcaRouter provider.
pub(crate) const ORCAROUTER_AUTO_MODEL: &str = "orcarouter/auto";
pub(crate) const OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL: &str =
    "arcee-ai/trinity-large-thinking";
pub(crate) const OPENROUTER_GEMMA_4_31B_MODEL: &str = "google/gemma-4-31b-it";
pub(crate) const OPENROUTER_GEMMA_4_26B_A4B_MODEL: &str = "google/gemma-4-26b-a4b-it";
pub(crate) const OPENROUTER_GLM_5_1_MODEL: &str = "z-ai/glm-5.1";
pub(crate) const OPENROUTER_GLM_5_2_MODEL: &str = "z-ai/glm-5.2";
// GLM-5.3 is live on the Z.ai Coding Plan (2026-08-13). Capability/limit
// metadata still inherits from glm-5.2 until Z.ai publishes distinct 5.3
// numbers. No USD price. The OpenRouter id is registered so the alias
// resolves to OpenRouter rather than another vendor. See
// models_dev.bundled.json `_meta.pending_release_metadata`.
pub(crate) const OPENROUTER_GLM_5_3_MODEL: &str = "z-ai/glm-5.3";
// GLM-5.3-Flash (2026-08-26): first natively multimodal GLM-5, 1M context,
// published USD list $0.15/$0.50 (50% promo until 2026-09-09 UTC+8 is not
// the durable row). OpenRouter mirror is z-ai/glm-5.3-flash.
pub(crate) const OPENROUTER_GLM_5_3_FLASH_MODEL: &str = "z-ai/glm-5.3-flash";
pub(crate) const OPENROUTER_KIMI_K2_7_CODE_MODEL: &str = "moonshotai/kimi-k2.7-code";
pub(crate) const OPENROUTER_KIMI_K2_6_MODEL: &str = "moonshotai/kimi-k2.6";
pub(crate) const OPENROUTER_MINIMAX_M3_MODEL: &str = "minimax/minimax-m3";
pub(crate) const OPENROUTER_MINIMAX_M2_7_MODEL: &str = "minimax/minimax-m2.7";
pub(crate) const OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL: &str =
    "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free";
pub(crate) const OPENROUTER_QWEN_3_6_FLASH_MODEL: &str = "qwen/qwen3.6-flash";
pub(crate) const OPENROUTER_QWEN_3_6_35B_A3B_MODEL: &str = "qwen/qwen3.6-35b-a3b";
pub(crate) const OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL: &str = "qwen/qwen3.6-max-preview";
pub(crate) const OPENROUTER_QWEN_3_6_27B_MODEL: &str = "qwen/qwen3.6-27b";
pub(crate) const OPENROUTER_QWEN_3_6_PLUS_MODEL: &str = "qwen/qwen3.6-plus";
pub(crate) const OPENROUTER_QWEN_3_7_PLUS_MODEL: &str = "qwen/qwen3.7-plus";
pub(crate) const OPENROUTER_QWEN_3_7_MAX_MODEL: &str = "qwen/qwen3.7-max";
pub(crate) const OPENROUTER_QWEN_3_8_FLASH_MODEL: &str = "qwen/qwen3.8-flash";
pub(crate) const OPENROUTER_TENCENT_HY3_PREVIEW_MODEL: &str = "tencent/hy3-preview";
pub(crate) const OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL: &str = "xiaomi/mimo-v2.5-pro";
pub(crate) const OPENROUTER_XIAOMI_MIMO_V2_5_MODEL: &str = "xiaomi/mimo-v2.5";
pub(crate) const DEFAULT_XIAOMI_MIMO_MODEL: &str = "mimo-v2.5-pro";
pub(crate) const XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL: &str = "mimo-v2.5-pro-ultraspeed";
pub(crate) const XIAOMI_MIMO_V2_5_OMNI_MODEL: &str = "mimo-v2.5";
pub(crate) const XIAOMI_MIMO_ASR_MODEL: &str = "mimo-v2.5-asr";
pub(crate) const XIAOMI_MIMO_TTS_MODEL: &str = "mimo-v2.5-tts";
pub(crate) const XIAOMI_MIMO_TTS_VOICE_DESIGN_MODEL: &str = "mimo-v2.5-tts-voicedesign";
pub(crate) const XIAOMI_MIMO_TTS_VOICE_CLONE_MODEL: &str = "mimo-v2.5-tts-voiceclone";
pub(crate) const XIAOMI_MIMO_V2_TTS_MODEL: &str = "mimo-v2-tts";
pub(crate) const DEFAULT_NOVITA_MODEL: &str = "deepseek/deepseek-v4-pro";
pub(crate) const DEFAULT_NOVITA_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
pub(crate) const DEFAULT_FIREWORKS_MODEL: &str = "accounts/fireworks/models/deepseek-v4-pro";
pub(crate) const DEFAULT_SILICONFLOW_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub(crate) const DEFAULT_SILICONFLOW_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub(crate) const DEFAULT_ARCEE_MODEL: &str = "trinity-large-thinking";
pub(crate) const ARCEE_TRINITY_LARGE_PREVIEW_MODEL: &str = "trinity-large-preview";
pub(crate) const ARCEE_TRINITY_MINI_MODEL: &str = "trinity-mini";
pub(crate) const DEFAULT_MOONSHOT_MODEL: &str = "kimi-k2.7-code";
pub(crate) const MOONSHOT_KIMI_K2_6_MODEL: &str = "kimi-k2.6";
pub(crate) const DEFAULT_MOONSHOT_BASE_URL: &str = "https://api.moonshot.ai/v1";
pub(crate) const MOONSHOT_CN_BASE_URL: &str = "https://api.moonshot.cn/v1";
pub(crate) const DEFAULT_KIMI_CODE_MODEL: &str = "kimi-for-coding";
pub(crate) const DEFAULT_KIMI_CODE_BASE_URL: &str = "https://api.kimi.com/coding/v1";
pub(crate) const DEFAULT_SGLANG_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub(crate) const DEFAULT_SGLANG_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub(crate) const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub(crate) const DEFAULT_ORCAROUTER_BASE_URL: &str = "https://api.orcarouter.ai/v1";
pub(crate) const XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
pub(crate) const DEFAULT_XIAOMI_MIMO_BASE_URL: &str = "https://token-plan-sgp.xiaomimimo.com/v1";
pub(crate) const XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL: &str =
    "https://token-plan-cn.xiaomimimo.com/v1";
pub(crate) const XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL: &str = DEFAULT_XIAOMI_MIMO_BASE_URL;
pub(crate) const XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL: &str =
    "https://token-plan-ams.xiaomimimo.com/v1";
pub(crate) const DEFAULT_NOVITA_BASE_URL: &str = "https://api.novita.ai/openai/v1";
pub(crate) const DEFAULT_FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";
pub(crate) const DEFAULT_SILICONFLOW_BASE_URL: &str = "https://api.siliconflow.com/v1";
pub(crate) const DEFAULT_SILICONFLOW_CN_BASE_URL: &str = "https://api.siliconflow.cn/v1";
pub(crate) const DEFAULT_ARCEE_BASE_URL: &str = "https://api.arcee.ai/api/v1";
pub(crate) const DEFAULT_HUGGINGFACE_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub(crate) const DEFAULT_HUGGINGFACE_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub(crate) const DEFAULT_HUGGINGFACE_BASE_URL: &str = "https://router.huggingface.co/v1";
pub(crate) const DEFAULT_TOGETHER_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub(crate) const DEFAULT_TOGETHER_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub(crate) const DEFAULT_TOGETHER_BASE_URL: &str = "https://api.together.xyz/v1";
pub(crate) const DEFAULT_QIANFAN_MODEL: &str = "ernie-4.0-turbo-8k";
pub(crate) const DEFAULT_QIANFAN_BASE_URL: &str = "https://api.baiduqianfan.ai/v1";
pub(crate) const DEFAULT_SGLANG_BASE_URL: &str = "http://localhost:30000/v1";
pub(crate) const DEFAULT_VLLM_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub(crate) const DEFAULT_VLLM_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub(crate) const DEFAULT_VLLM_BASE_URL: &str = "http://localhost:8000/v1";
/// Unresolved local-Ollama default. A live `GET /v1/models` (Ollama's
/// OpenAI-compat catalog, same tags as `/api/tags`) must supply the real id;
/// this marker must never be sent as a model name.
pub(crate) const DEFAULT_OLLAMA_MODEL: &str = "unknown";
pub(crate) const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
pub(crate) const DEFAULT_OLLAMA_CLOUD_MODEL: &str = "gpt-oss:120b";
pub(crate) const DEFAULT_OLLAMA_CLOUD_BASE_URL: &str = "https://ollama.com/v1";

// Z.ai (GLM Coding Plan) defaults. GLM-5.3 is live on the Z.ai Coding Plan
// (2026-08-13) and is the default for new Z.ai routes. Capability/limit
// metadata still inherits from glm-5.2 until Z.ai publishes distinct 5.3
// numbers; no USD price is claimed. See models_dev.bundled.json
// `_meta.pending_release_metadata`. Explicit GLM-5.2 selections keep their
// own id: only the default moved.
pub(crate) const DEFAULT_ZAI_MODEL: &str = ZAI_GLM_5_3_MODEL;
pub(crate) const ZAI_GLM_5_3_MODEL: &str = "GLM-5.3";
pub(crate) const ZAI_GLM_5_3_FLASH_MODEL: &str = "GLM-5.3-Flash";
pub(crate) const ZAI_GLM_5_2_MODEL: &str = "GLM-5.2";
pub(crate) const ZAI_GLM_5_1_MODEL: &str = "GLM-5.1";
pub(crate) const ZAI_GLM_5_TURBO_MODEL: &str = "GLM-5-Turbo";
pub(crate) const DEFAULT_ZAI_BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";
// StepFun / StepFlash defaults
pub(crate) const DEFAULT_STEPFUN_MODEL: &str = "step-3.7-flash";
pub(crate) const DEFAULT_STEPFUN_BASE_URL: &str = "https://api.stepfun.ai/v1";
// MiniMax defaults
pub(crate) const DEFAULT_MINIMAX_MODEL: &str = "MiniMax-M3";
pub(crate) const MINIMAX_M2_7_MODEL: &str = "MiniMax-M2.7";
pub(crate) const MINIMAX_M2_7_HIGHSPEED_MODEL: &str = "MiniMax-M2.7-highspeed";
pub(crate) const MINIMAX_M2_5_MODEL: &str = "MiniMax-M2.5";
pub(crate) const MINIMAX_M2_5_HIGHSPEED_MODEL: &str = "MiniMax-M2.5-highspeed";
pub(crate) const MINIMAX_M2_1_MODEL: &str = "MiniMax-M2.1";
pub(crate) const MINIMAX_M2_1_HIGHSPEED_MODEL: &str = "MiniMax-M2.1-highspeed";
pub(crate) const MINIMAX_M2_MODEL: &str = "MiniMax-M2";
pub(crate) const DEFAULT_MINIMAX_BASE_URL: &str = "https://api.minimax.io/v1";
pub(crate) const DEFAULT_MINIMAX_ANTHROPIC_BASE_URL: &str = "https://api.minimax.io/anthropic";
pub(crate) const DEFAULT_DEEPINFRA_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub(crate) const DEFAULT_DEEPINFRA_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub(crate) const DEFAULT_DEEPINFRA_BASE_URL: &str = "https://api.deepinfra.com/v1/openai";
// Sakana AI Fugu defaults
pub(crate) const DEFAULT_SAKANA_MODEL: &str = "fugu";
pub(crate) const DEFAULT_SAKANA_BASE_URL: &str = "https://api.sakana.ai/v1";
// Meituan LongCat defaults
pub(crate) const DEFAULT_LONGCAT_MODEL: &str = "LongCat-2.0";
pub(crate) const DEFAULT_LONGCAT_BASE_URL: &str = "https://api.longcat.chat/openai/v1";
// OpenCode Go Chat Completions defaults. The Go catalog also contains models
// served only through Anthropic Messages; those are deliberately not listed by
// this provider until Codewhale can route wire formats per model.
pub(crate) const DEFAULT_OPENCODE_GO_MODEL: &str = "deepseek-v4-pro";
pub(crate) const DEFAULT_OPENCODE_GO_BASE_URL: &str = "https://opencode.ai/zen/go/v1";
pub(crate) const OPENCODE_GO_GROK_4_5_MODEL: &str = "grok-4.5";
pub(crate) const OPENCODE_GO_GLM_5_2_MODEL: &str = "glm-5.2";
pub(crate) const OPENCODE_GO_GLM_5_1_MODEL: &str = "glm-5.1";
pub(crate) const OPENCODE_GO_KIMI_K3_MODEL: &str = "kimi-k3";
pub(crate) const OPENCODE_GO_KIMI_K2_7_CODE_MODEL: &str = "kimi-k2.7-code";
pub(crate) const OPENCODE_GO_KIMI_K2_6_MODEL: &str = "kimi-k2.6";
pub(crate) const OPENCODE_GO_DEEPSEEK_V4_FLASH_MODEL: &str = "deepseek-v4-flash";
pub(crate) const OPENCODE_GO_MIMO_V2_5_MODEL: &str = "mimo-v2.5";
pub(crate) const OPENCODE_GO_MIMO_V2_5_PRO_MODEL: &str = "mimo-v2.5-pro";

// OpenCode Zen is a model-aware gateway. The default is a documented
// Responses model, but every executable route must still obtain its protocol
// from a provider-scoped catalog offering.
pub(crate) const DEFAULT_OPENCODE_ZEN_MODEL: &str = "gpt-5.6";
pub(crate) const DEFAULT_OPENCODE_ZEN_BASE_URL: &str = "https://opencode.ai/zen/v1";
// Meta Model API / Muse Spark defaults
pub(crate) const DEFAULT_META_MODEL: &str = "muse-spark-1.2";
pub(crate) const DEFAULT_META_BASE_URL: &str = "https://api.meta.ai/v1";
// xAI / Grok API-key route defaults
pub(crate) const DEFAULT_XAI_MODEL: &str = "grok-4.6";
pub(crate) const DEFAULT_XAI_BASE_URL: &str = "https://api.x.ai/v1";
// Mistral AI (la Plateforme) defaults
pub(crate) const DEFAULT_MISTRAL_MODEL: &str = "mistral-code-latest";
pub(crate) const DEFAULT_MISTRAL_BASE_URL: &str = "https://api.mistral.ai/v1";
// TelecomJS (Jiangsu Telecom TokenHub) defaults
pub(crate) const DEFAULT_TELECOMJS_MODEL: &str = "deepseek-v4-pro";
pub(crate) const DEFAULT_TELECOMJS_BASE_URL: &str = "https://aigw.telecomjs.com/v1";
// Eden AI (OpenAI-compatible AI gateway) defaults
pub(crate) const DEFAULT_EDENAI_MODEL: &str = "deepseek/deepseek-v4-pro";
pub(crate) const DEFAULT_EDENAI_BASE_URL: &str = "https://api.edenai.run/v3";
// Concentrate (OpenAI Responses-compatible AI gateway) defaults. Contract:
// https://concentrate.ai/docs/api-reference/introduction — base URL, bearer
// Universal API key, `POST /v1/responses`, unauthenticated `GET /v1/models`.
// The default is a plain catalog id (the gateway picks the upstream provider);
// `provider/model` pins a provider and `concentrate/auto` is the gateway router.
pub(crate) const DEFAULT_CONCENTRATE_MODEL: &str = "deepseek-v4-pro";
pub(crate) const DEFAULT_CONCENTRATE_BASE_URL: &str = "https://api.concentrate.ai/v1";
// Alibaba Cloud Model Studio (DashScope) defaults
// Token Plan (Personal / Team): shared endpoint, OpenAI + Anthropic dialects
pub(crate) const DEFAULT_MODELSTUDIO_TOKEN_PLAN_MODEL: &str = "qwen3.8-max";
pub(crate) const DEFAULT_MODELSTUDIO_TOKEN_PLAN_BASE_URL: &str =
    "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1";
pub(crate) const MODELSTUDIO_TOKEN_PLAN_ANTHROPIC_BASE_URL: &str =
    "https://token-plan.ap-southeast-1.maas.aliyuncs.com/apps/anthropic";
// Coding Plan: separate endpoint, OpenAI + Anthropic dialects
pub(crate) const DEFAULT_MODELSTUDIO_CODING_PLAN_BASE_URL: &str =
    "https://coding-intl.dashscope.aliyuncs.com/v1";
pub(crate) const MODELSTUDIO_CODING_PLAN_ANTHROPIC_BASE_URL: &str =
    "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic";

/// Google Gemini OpenAI-compatible Chat Completions base URL.
pub const DEFAULT_GOOGLE_BASE_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/openai/";
/// Default Gemini model for the Google provider (preview flagship, 2026-08).
pub const DEFAULT_GOOGLE_MODEL: &str = "gemini-3.1-pro-preview";

/// Antigravity cloud-code internal endpoint (credential plane only; the
/// wire protocol is not implemented and sends fail closed).
pub const DEFAULT_ANTIGRAVITY_BASE_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal";
/// Placeholder model id; never sent — the route fails closed before transport.
pub const DEFAULT_ANTIGRAVITY_MODEL: &str = "gemini-3-pro-preview";
