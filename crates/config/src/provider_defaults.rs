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
pub(crate) const DEFAULT_OPENAI_MODEL: &str = "deepseek-v4-pro";
pub(crate) const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/beta";
pub(crate) const DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL: &str = "https://api.deepseek.com/anthropic";
pub(crate) const DEFAULT_NVIDIA_NIM_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub(crate) const DEFAULT_OPENAI_CODEX_MODEL: &str = "gpt-5.5";
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
pub(crate) const OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL: &str =
    "arcee-ai/trinity-large-thinking";
pub(crate) const OPENROUTER_GEMMA_4_31B_MODEL: &str = "google/gemma-4-31b-it";
pub(crate) const OPENROUTER_GEMMA_4_26B_A4B_MODEL: &str = "google/gemma-4-26b-a4b-it";
pub(crate) const OPENROUTER_GLM_5_1_MODEL: &str = "z-ai/glm-5.1";
pub(crate) const OPENROUTER_GLM_5_2_MODEL: &str = "z-ai/glm-5.2";
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
pub(crate) const OPENROUTER_QWEN_3_7_MAX_MODEL: &str = "qwen/qwen3.7-max";
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
pub(crate) const DEFAULT_KIMI_CODE_MODEL: &str = "kimi-for-coding";
pub(crate) const DEFAULT_KIMI_CODE_BASE_URL: &str = "https://api.kimi.com/coding/v1";
pub(crate) const DEFAULT_SGLANG_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub(crate) const DEFAULT_SGLANG_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub(crate) const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
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
pub(crate) const DEFAULT_OLLAMA_MODEL: &str = "deepseek-coder:1.3b";
pub(crate) const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

// Z.ai (GLM Coding Plan) defaults
pub(crate) const DEFAULT_ZAI_MODEL: &str = "GLM-5.2";
pub(crate) const ZAI_GLM_5_1_MODEL: &str = "GLM-5.1";
// GLM-5.2 is both the default and a named tier; the alias arm resolves the
// `glm-5.2` spelling to DEFAULT_ZAI_MODEL directly, so this constant is
// referenced only in cfg(test) assertions (see tests.rs).
#[allow(dead_code)]
pub(crate) const ZAI_GLM_5_2_MODEL: &str = "GLM-5.2";
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
pub(crate) const DEFAULT_DEEPINFRA_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub(crate) const DEFAULT_DEEPINFRA_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
pub(crate) const DEFAULT_DEEPINFRA_BASE_URL: &str = "https://api.deepinfra.com/v1/openai";
// Sakana AI Fugu defaults
pub(crate) const DEFAULT_SAKANA_MODEL: &str = "fugu";
pub(crate) const DEFAULT_SAKANA_BASE_URL: &str = "https://api.sakana.ai/v1";
// OpenCode Zen defaults
pub(crate) const DEFAULT_OPENCODE_ZEN_MODEL: &str = "gpt-5.5";
pub(crate) const DEFAULT_OPENCODE_ZEN_BASE_URL: &str = "https://opencode.ai/zen/v1";
