pub mod provider;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
pub use codewhale_execpolicy::ToolAskRule;
use codewhale_execpolicy::{ExecPolicyEngine, Ruleset};
use codewhale_secrets::SecretSource;
pub use codewhale_secrets::Secrets;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const PERMISSIONS_FILE_NAME: &str = "permissions.toml";
const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-v4-pro";
const DEFAULT_NVIDIA_NIM_MODEL: &str = "deepseek-ai/deepseek-v4-pro";
const DEFAULT_NVIDIA_NIM_FLASH_MODEL: &str = "deepseek-ai/deepseek-v4-flash";
const DEFAULT_OPENAI_MODEL: &str = "deepseek-v4-pro";
const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/beta";
const DEFAULT_NVIDIA_NIM_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const DEFAULT_OPENAI_CODEX_MODEL: &str = "gpt-5.5";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_OPENAI_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_ATLASCLOUD_MODEL: &str = "deepseek-ai/deepseek-v4-flash";
const DEFAULT_ATLASCLOUD_BASE_URL: &str = "https://api.atlascloud.ai/v1";
const DEFAULT_WANJIE_ARK_MODEL: &str = "deepseek-reasoner";
const DEFAULT_WANJIE_ARK_BASE_URL: &str = "https://maas-openapi.wanjiedata.com/api/v1";
const DEFAULT_VOLCENGINE_MODEL: &str = "DeepSeek-V4-Pro";
const DEFAULT_VOLCENGINE_BASE_URL: &str = "https://ark.cn-beijing.volces.com/api/coding/v3";
const DEFAULT_OPENROUTER_MODEL: &str = "deepseek/deepseek-v4-pro";
const DEFAULT_OPENROUTER_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
const OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL: &str = "arcee-ai/trinity-large-thinking";
const OPENROUTER_GEMMA_4_31B_MODEL: &str = "google/gemma-4-31b-it";
const OPENROUTER_GEMMA_4_26B_A4B_MODEL: &str = "google/gemma-4-26b-a4b-it";
const OPENROUTER_GLM_5_1_MODEL: &str = "z-ai/glm-5.1";
const OPENROUTER_GLM_5_2_MODEL: &str = "z-ai/glm-5.2";
const OPENROUTER_KIMI_K2_7_CODE_MODEL: &str = "moonshotai/kimi-k2.7-code";
const OPENROUTER_KIMI_K2_6_MODEL: &str = "moonshotai/kimi-k2.6";
const OPENROUTER_MINIMAX_M3_MODEL: &str = "minimax/minimax-m3";
const OPENROUTER_MINIMAX_2_7_MODEL: &str = "minimax/minimax-2.7";
const OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL: &str =
    "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free";
const OPENROUTER_QWEN_3_6_FLASH_MODEL: &str = "qwen/qwen3.6-flash";
const OPENROUTER_QWEN_3_6_35B_A3B_MODEL: &str = "qwen/qwen3.6-35b-a3b";
const OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL: &str = "qwen/qwen3.6-max-preview";
const OPENROUTER_QWEN_3_6_27B_MODEL: &str = "qwen/qwen3.6-27b";
const OPENROUTER_QWEN_3_6_PLUS_MODEL: &str = "qwen/qwen3.6-plus";
const OPENROUTER_QWEN_3_7_MAX_MODEL: &str = "qwen/qwen3.7-max";
const OPENROUTER_TENCENT_HY3_PREVIEW_MODEL: &str = "tencent/hy3-preview";
const OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL: &str = "xiaomi/mimo-v2.5-pro";
const OPENROUTER_XIAOMI_MIMO_V2_5_MODEL: &str = "xiaomi/mimo-v2.5";
const DEFAULT_XIAOMI_MIMO_MODEL: &str = "mimo-v2.5-pro";
const XIAOMI_MIMO_V2_5_OMNI_MODEL: &str = "mimo-v2.5";
const XIAOMI_MIMO_ASR_MODEL: &str = "mimo-v2.5-asr";
const XIAOMI_MIMO_TTS_MODEL: &str = "mimo-v2.5-tts";
const XIAOMI_MIMO_TTS_VOICE_DESIGN_MODEL: &str = "mimo-v2.5-tts-voicedesign";
const XIAOMI_MIMO_TTS_VOICE_CLONE_MODEL: &str = "mimo-v2.5-tts-voiceclone";
const XIAOMI_MIMO_V2_TTS_MODEL: &str = "mimo-v2-tts";
const DEFAULT_NOVITA_MODEL: &str = "deepseek/deepseek-v4-pro";
const DEFAULT_NOVITA_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
const DEFAULT_FIREWORKS_MODEL: &str = "accounts/fireworks/models/deepseek-v4-pro";
const DEFAULT_SILICONFLOW_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_SILICONFLOW_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_ARCEE_MODEL: &str = "trinity-large-thinking";
const ARCEE_TRINITY_LARGE_PREVIEW_MODEL: &str = "trinity-large-preview";
const ARCEE_TRINITY_MINI_MODEL: &str = "trinity-mini";
const DEFAULT_MOONSHOT_MODEL: &str = "kimi-k2.7-code";
const MOONSHOT_KIMI_K2_6_MODEL: &str = "kimi-k2.6";
const DEFAULT_MOONSHOT_BASE_URL: &str = "https://api.moonshot.ai/v1";
const DEFAULT_KIMI_CODE_MODEL: &str = "kimi-for-coding";
const DEFAULT_KIMI_CODE_BASE_URL: &str = "https://api.kimi.com/coding/v1";
const DEFAULT_SGLANG_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_SGLANG_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
const DEFAULT_XIAOMI_MIMO_BASE_URL: &str = "https://token-plan-sgp.xiaomimimo.com/v1";
const XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL: &str = "https://token-plan-cn.xiaomimimo.com/v1";
const XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL: &str = DEFAULT_XIAOMI_MIMO_BASE_URL;
const XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL: &str = "https://token-plan-ams.xiaomimimo.com/v1";
const DEFAULT_NOVITA_BASE_URL: &str = "https://api.novita.ai/openai/v1";
const DEFAULT_FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";
const DEFAULT_SILICONFLOW_BASE_URL: &str = "https://api.siliconflow.com/v1";
const DEFAULT_SILICONFLOW_CN_BASE_URL: &str = "https://api.siliconflow.cn/v1";
const DEFAULT_ARCEE_BASE_URL: &str = "https://api.arcee.ai/api/v1";
const DEFAULT_HUGGINGFACE_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_HUGGINGFACE_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_HUGGINGFACE_BASE_URL: &str = "https://router.huggingface.co/v1";
const DEFAULT_TOGETHER_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_TOGETHER_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_TOGETHER_BASE_URL: &str = "https://api.together.xyz/v1";
const DEFAULT_SGLANG_BASE_URL: &str = "http://localhost:30000/v1";
const DEFAULT_VLLM_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_VLLM_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_VLLM_BASE_URL: &str = "http://localhost:8000/v1";
const DEFAULT_OLLAMA_MODEL: &str = "deepseek-coder:1.3b";
const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

// Z.ai (GLM Coding Plan) defaults
const DEFAULT_ZAI_MODEL: &str = "GLM-5.2";
const ZAI_GLM_5_1_MODEL: &str = "GLM-5.1";
// GLM-5.2 is both the default and a named tier; the alias arm resolves the
// `glm-5.2` spelling to DEFAULT_ZAI_MODEL directly, so this constant is only
// referenced by the invariant test below.
#[allow(dead_code)]
const ZAI_GLM_5_2_MODEL: &str = "GLM-5.2";
const ZAI_GLM_5_TURBO_MODEL: &str = "GLM-5-Turbo";
const DEFAULT_ZAI_BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";
// StepFun / StepFlash defaults
const DEFAULT_STEPFUN_MODEL: &str = "step-3.7-flash";
const DEFAULT_STEPFUN_BASE_URL: &str = "https://api.stepfun.ai/v1";
// MiniMax defaults
const DEFAULT_MINIMAX_MODEL: &str = "MiniMax-M3";
const MINIMAX_M2_7_MODEL: &str = "MiniMax-M2.7";
const MINIMAX_M2_7_HIGHSPEED_MODEL: &str = "MiniMax-M2.7-highspeed";
const MINIMAX_M2_5_MODEL: &str = "MiniMax-M2.5";
const MINIMAX_M2_5_HIGHSPEED_MODEL: &str = "MiniMax-M2.5-highspeed";
const MINIMAX_M2_1_MODEL: &str = "MiniMax-M2.1";
const MINIMAX_M2_1_HIGHSPEED_MODEL: &str = "MiniMax-M2.1-highspeed";
const MINIMAX_M2_MODEL: &str = "MiniMax-M2";
const DEFAULT_MINIMAX_BASE_URL: &str = "https://api.minimax.io/v1";
const DEFAULT_DEEPINFRA_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_DEEPINFRA_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_DEEPINFRA_BASE_URL: &str = "https://api.deepinfra.com/v1/openai";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    #[default]
    #[serde(
        alias = "deepseek-cn",
        alias = "deepseek_china",
        alias = "deepseekcn",
        alias = "deepseek-china"
    )]
    Deepseek,
    NvidiaNim,
    #[serde(alias = "open-ai")]
    Openai,
    Atlascloud,
    #[serde(
        alias = "wanjie",
        alias = "wanjie_ark",
        alias = "ark-wanjie",
        alias = "ark_wanjie",
        alias = "wanjie-maas",
        alias = "wanjie_maas"
    )]
    WanjieArk,
    #[serde(alias = "volcengine-ark", alias = "volcengine_ark", alias = "ark")]
    Volcengine,
    Openrouter,
    #[serde(alias = "mimo", alias = "xiaomi", alias = "xiaomi_mimo")]
    XiaomiMimo,
    Novita,
    Fireworks,
    #[serde(alias = "silicon-flow", alias = "silicon_flow")]
    Siliconflow,
    #[serde(alias = "arcee-ai", alias = "arcee_ai")]
    Arcee,
    #[serde(alias = "siliconflow-cn", alias = "siliconflow-CN")]
    SiliconflowCN,
    Moonshot,
    Sglang,
    Vllm,
    Ollama,
    #[serde(alias = "hugging-face", alias = "hugging_face", alias = "hf")]
    Huggingface,
    #[serde(alias = "together-ai", alias = "together_ai")]
    Together,
    #[serde(
        alias = "openai-codex",
        alias = "openai_codex",
        alias = "codex",
        alias = "chatgpt",
        alias = "chatgpt-codex",
        alias = "chatgpt_codex"
    )]
    OpenaiCodex,
    #[serde(alias = "claude")]
    Anthropic,
    #[serde(alias = "z-ai", alias = "z_ai", alias = "z.ai")]
    Zai,
    #[serde(
        alias = "step-fun",
        alias = "step_fun",
        alias = "stepfun",
        alias = "stepflash",
        alias = "step-flash",
        alias = "step_flash"
    )]
    Stepfun,
    #[serde(alias = "mini-max", alias = "mini_max", alias = "minimax")]
    Minimax,
    #[serde(alias = "deep-infra", alias = "deep_infra")]
    Deepinfra,
}

impl ProviderKind {
    pub const ALL: [Self; 25] = [
        Self::Deepseek,
        Self::NvidiaNim,
        Self::Openai,
        Self::Atlascloud,
        Self::WanjieArk,
        Self::Volcengine,
        Self::Openrouter,
        Self::XiaomiMimo,
        Self::Novita,
        Self::Fireworks,
        Self::Siliconflow,
        Self::Arcee,
        Self::SiliconflowCN,
        Self::Moonshot,
        Self::Sglang,
        Self::Vllm,
        Self::Ollama,
        Self::Huggingface,
        Self::Together,
        Self::OpenaiCodex,
        Self::Anthropic,
        Self::Zai,
        Self::Stepfun,
        Self::Minimax,
        Self::Deepinfra,
    ];

    #[must_use]
    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    #[must_use]
    pub fn names_hint() -> String {
        Self::all()
            .iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.provider().id()
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        provider::all_providers()
            .iter()
            .find(|p| {
                trimmed.eq_ignore_ascii_case(p.id())
                    || p.aliases().iter().any(|a| trimmed.eq_ignore_ascii_case(a))
            })
            .map(|p| p.kind())
    }

    #[must_use]
    pub fn is_siliconflow(self) -> bool {
        matches!(self, Self::Siliconflow | Self::SiliconflowCN)
    }

    /// Return the built-in metadata entry for this provider.
    ///
    /// This is a metadata foundation only; runtime routing still resolves
    /// through [`ConfigToml::resolve_runtime_options`].
    #[must_use]
    pub fn provider(self) -> &'static dyn provider::Provider {
        provider::provider_for_kind(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfigToml {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub auth_mode: Option<String>,
    pub insecure_skip_tls_verify: Option<bool>,
    #[serde(default)]
    pub http_headers: BTreeMap<String, String>,
    pub path_suffix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersToml {
    #[serde(default)]
    pub deepseek: ProviderConfigToml,
    #[serde(default)]
    pub nvidia_nim: ProviderConfigToml,
    #[serde(default)]
    pub openai: ProviderConfigToml,
    #[serde(default)]
    pub atlascloud: ProviderConfigToml,
    #[serde(default)]
    pub wanjie_ark: ProviderConfigToml,
    #[serde(default)]
    pub volcengine: ProviderConfigToml,
    #[serde(default)]
    pub openrouter: ProviderConfigToml,
    #[serde(default, alias = "xiaomi", alias = "mimo", alias = "xiaomimimo")]
    pub xiaomi_mimo: ProviderConfigToml,
    #[serde(default)]
    pub novita: ProviderConfigToml,
    #[serde(default)]
    pub fireworks: ProviderConfigToml,
    #[serde(default)]
    pub siliconflow: ProviderConfigToml,
    #[serde(default, alias = "siliconflow-CN", alias = "siliconflow-cn")]
    pub siliconflow_cn: ProviderConfigToml,
    #[serde(default)]
    pub arcee: ProviderConfigToml,
    #[serde(default)]
    pub moonshot: ProviderConfigToml,
    #[serde(default)]
    pub sglang: ProviderConfigToml,
    #[serde(default)]
    pub vllm: ProviderConfigToml,
    #[serde(default)]
    pub ollama: ProviderConfigToml,
    #[serde(default)]
    pub huggingface: ProviderConfigToml,
    #[serde(default)]
    pub together: ProviderConfigToml,
    #[serde(
        default,
        alias = "openai-codex",
        alias = "openai_codex",
        alias = "codex",
        alias = "chatgpt",
        alias = "chatgpt-codex"
    )]
    pub openai_codex: ProviderConfigToml,
    #[serde(default)]
    pub anthropic: ProviderConfigToml,
    #[serde(default, alias = "z-ai", alias = "z_ai", alias = "z.ai")]
    pub zai: ProviderConfigToml,
    #[serde(
        default,
        alias = "step-fun",
        alias = "step_fun",
        alias = "stepfun",
        alias = "stepflash",
        alias = "step-flash",
        alias = "step_flash"
    )]
    pub stepfun: ProviderConfigToml,
    #[serde(default, alias = "mini-max", alias = "mini_max", alias = "minimax")]
    pub minimax: ProviderConfigToml,
    #[serde(default, alias = "deep-infra", alias = "deep_infra")]
    pub deepinfra: ProviderConfigToml,
}

/// Sibling `permissions.toml` schema.
///
/// This slice is intentionally ask-only: each rule is a typed condition that
/// means "ask before this tool invocation." Typed allow/deny records and UI
/// actions are expected to land in follow-up PRs.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PermissionsToml {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<ToolAskRule>,
}

impl PermissionsToml {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    #[must_use]
    pub fn ruleset(&self) -> Ruleset {
        Ruleset::user(Vec::new(), Vec::new()).with_ask_rules(self.rules.clone())
    }
}

impl ProvidersToml {
    #[must_use]
    pub fn for_provider(&self, provider: ProviderKind) -> &ProviderConfigToml {
        match provider {
            ProviderKind::Deepseek => &self.deepseek,
            ProviderKind::NvidiaNim => &self.nvidia_nim,
            ProviderKind::Openai => &self.openai,
            ProviderKind::Atlascloud => &self.atlascloud,
            ProviderKind::WanjieArk => &self.wanjie_ark,
            ProviderKind::Volcengine => &self.volcengine,
            ProviderKind::Openrouter => &self.openrouter,
            ProviderKind::XiaomiMimo => &self.xiaomi_mimo,
            ProviderKind::Novita => &self.novita,
            ProviderKind::Fireworks => &self.fireworks,
            ProviderKind::Siliconflow => &self.siliconflow,
            ProviderKind::SiliconflowCN => &self.siliconflow_cn,
            ProviderKind::Arcee => &self.arcee,
            ProviderKind::Moonshot => &self.moonshot,
            ProviderKind::Sglang => &self.sglang,
            ProviderKind::Vllm => &self.vllm,
            ProviderKind::Ollama => &self.ollama,
            ProviderKind::Huggingface => &self.huggingface,
            ProviderKind::Together => &self.together,
            ProviderKind::OpenaiCodex => &self.openai_codex,
            ProviderKind::Anthropic => &self.anthropic,
            ProviderKind::Zai => &self.zai,
            ProviderKind::Stepfun => &self.stepfun,
            ProviderKind::Minimax => &self.minimax,
            ProviderKind::Deepinfra => &self.deepinfra,
        }
    }

    pub fn for_provider_mut(&mut self, provider: ProviderKind) -> &mut ProviderConfigToml {
        match provider {
            ProviderKind::Deepseek => &mut self.deepseek,
            ProviderKind::NvidiaNim => &mut self.nvidia_nim,
            ProviderKind::Openai => &mut self.openai,
            ProviderKind::Atlascloud => &mut self.atlascloud,
            ProviderKind::WanjieArk => &mut self.wanjie_ark,
            ProviderKind::Volcengine => &mut self.volcengine,
            ProviderKind::Openrouter => &mut self.openrouter,
            ProviderKind::XiaomiMimo => &mut self.xiaomi_mimo,
            ProviderKind::Novita => &mut self.novita,
            ProviderKind::Fireworks => &mut self.fireworks,
            ProviderKind::Siliconflow => &mut self.siliconflow,
            ProviderKind::SiliconflowCN => &mut self.siliconflow_cn,
            ProviderKind::Arcee => &mut self.arcee,
            ProviderKind::Moonshot => &mut self.moonshot,
            ProviderKind::Sglang => &mut self.sglang,
            ProviderKind::Vllm => &mut self.vllm,
            ProviderKind::Ollama => &mut self.ollama,
            ProviderKind::Huggingface => &mut self.huggingface,
            ProviderKind::Together => &mut self.together,
            ProviderKind::OpenaiCodex => &mut self.openai_codex,
            ProviderKind::Anthropic => &mut self.anthropic,
            ProviderKind::Zai => &mut self.zai,
            ProviderKind::Stepfun => &mut self.stepfun,
            ProviderKind::Minimax => &mut self.minimax,
            ProviderKind::Deepinfra => &mut self.deepinfra,
        }
    }
}

/// Kinds of built-in harness postures.
///
/// A posture names the runtime strategy CodeWhale should use for a
/// provider/model route: how much context to preload, how aggressively to lean
/// on sub-agents, and how to balance prompt-cache stability against quick
/// exploration. Runtime selection is wired in later v0.9 slices; this config
/// model intentionally keeps the policy data explicit first.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessPostureKind {
    /// Full-featured default: rich constitution, broad tool catalog, and normal
    /// sub-agent posture.
    #[default]
    Standard,
    /// Cache-heavy: deeper prompt layering and prefix-cache-oriented context.
    CacheHeavy,
    /// Lean: smaller starting context, faster compaction, and stronger
    /// exploration/delegation bias.
    Lean,
    /// User-defined posture assembled from explicit knobs below.
    Custom,
}

/// How this posture should approach compaction and prompt-cache stability.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessCompactionStrategy {
    #[default]
    Default,
    PrefixCache,
    Aggressive,
}

/// Which tool catalog shape this posture prefers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessToolSurface {
    #[default]
    Full,
    ReadOnly,
    Auto,
}

/// Safety posture applied when the runtime consumes a harness profile.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessSafetyPosture {
    #[default]
    Standard,
    Strict,
    Permissive,
}

/// A concrete harness posture with policy knobs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HarnessPosture {
    /// Named posture kind.
    #[serde(default)]
    pub kind: HarnessPostureKind,
    /// Maximum number of concurrent sub-agents (0 = runtime default).
    #[serde(default)]
    pub max_subagents: usize,
    /// Prefer search-based/on-demand context over always-on documentation.
    #[serde(default)]
    pub prefer_codebase_search: bool,
    /// Compaction and prompt-cache strategy.
    #[serde(default)]
    pub compaction_strategy: HarnessCompactionStrategy,
    /// Preferred tool catalog shape.
    #[serde(default)]
    pub tool_surface: HarnessToolSurface,
    /// Safety posture for runtime consumers.
    #[serde(default)]
    pub safety_posture: HarnessSafetyPosture,
}

impl Default for HarnessPosture {
    fn default() -> Self {
        Self {
            kind: HarnessPostureKind::Standard,
            max_subagents: 0,
            prefer_codebase_search: false,
            compaction_strategy: HarnessCompactionStrategy::default(),
            tool_surface: HarnessToolSurface::default(),
            safety_posture: HarnessSafetyPosture::default(),
        }
    }
}

impl HarnessPosture {
    /// A cache-heavy posture tuned for DeepSeek V4 / MiMo-style models.
    #[must_use]
    pub fn cache_heavy() -> Self {
        Self {
            kind: HarnessPostureKind::CacheHeavy,
            max_subagents: 10,
            prefer_codebase_search: false,
            compaction_strategy: HarnessCompactionStrategy::PrefixCache,
            tool_surface: HarnessToolSurface::Full,
            safety_posture: HarnessSafetyPosture::Standard,
        }
    }

    /// A lean posture for smaller-context or weaker tool-use models.
    #[must_use]
    pub fn lean() -> Self {
        Self {
            kind: HarnessPostureKind::Lean,
            max_subagents: 20,
            prefer_codebase_search: true,
            compaction_strategy: HarnessCompactionStrategy::Aggressive,
            tool_surface: HarnessToolSurface::Full,
            safety_posture: HarnessSafetyPosture::Standard,
        }
    }
}

/// A harness profile binds a posture to a provider route and model pattern.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HarnessProfile {
    /// Provider route this profile applies to, e.g. "deepseek" or
    /// "xiaomi-mimo".
    pub provider_route: String,
    /// Regex or glob pattern for model names, e.g. "deepseek-v4.*".
    pub model_pattern: String,
    /// The posture to apply.
    #[serde(default)]
    pub posture: HarnessPosture,
}

impl HarnessProfile {
    /// Return true when this profile applies to the provider/model route.
    ///
    /// This is a pure config helper: matching a profile must not mutate runtime
    /// provider selection, prompts, auth, tools, context, or persisted config.
    #[must_use]
    pub fn matches_route(&self, provider_route: &str, model: &str) -> bool {
        provider_routes_equal(&self.provider_route, provider_route)
            && wildcard_pattern_matches(&self.model_pattern, model)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigToml {
    /// TUI-compatible DeepSeek API key. Kept at the root so both `deepseek`
    /// and `codewhale-tui` can share a single config file.
    pub api_key: Option<String>,
    /// TUI-compatible DeepSeek base URL.
    pub base_url: Option<String>,
    /// Optional extra HTTP headers forwarded to model API requests.
    #[serde(default)]
    pub http_headers: BTreeMap<String, String>,
    /// TUI-compatible default DeepSeek model.
    pub default_text_model: Option<String>,
    #[serde(default)]
    pub provider: ProviderKind,
    pub model: Option<String>,
    pub auth_mode: Option<String>,
    pub output_mode: Option<String>,
    pub verbosity: Option<String>,
    pub log_level: Option<String>,
    pub telemetry: Option<bool>,
    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
    /// Native tool catalog controls shared with `codewhale-tui`.
    #[serde(default)]
    pub tools: Option<ToolsToml>,
    #[serde(default)]
    pub providers: ProvidersToml,
    /// Provider fallback chain (#2574). TUI runtime code may advance through
    /// these providers after recoverable provider errors; config resolution
    /// itself still reports the selected primary provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_providers: Vec<ProviderKind>,
    /// Per-domain network policy (#135). When absent, network tools fall back
    /// to a permissive default that mirrors pre-v0.7.0 behavior.
    #[serde(default)]
    pub network: Option<NetworkPolicyToml>,
    /// Community skill installer settings (#140). Mirrors
    /// [`SkillsToml`] from the TUI side; the dispatcher consults
    /// `registry_url` when running `deepseek skill install`.
    #[serde(default)]
    pub skills: Option<SkillsToml>,
    /// Workspace side-git snapshots (#137). The live TUI defaults this to
    /// enabled with 7-day retention when absent.
    #[serde(default)]
    pub snapshots: Option<SnapshotsToml>,
    /// Post-edit LSP diagnostics injection (#136). When absent, the engine
    /// applies the defaults documented in [`LspConfigToml`].
    #[serde(default)]
    pub lsp: Option<LspConfigToml>,
    /// Per-model harness profiles (#2693). Runtime wiring lands in follow-up
    /// v0.9 slices; this is the durable config data model.
    #[serde(default)]
    pub harness_profiles: Vec<HarnessProfile>,
    /// Optional 1-8 hotbar slot bindings (#2064). When absent, the TUI falls
    /// back to the built-in default slots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotbar: Option<Vec<HotbarBindingToml>>,
    /// App-server hook sink configuration. Kept separate from the TUI
    /// lifecycle `[hooks]` table so config rewrites preserve existing hooks.
    #[serde(default)]
    pub hook_sinks: Option<HookSinksToml>,
    /// Agent Fleet trust and security policy (#3165). When absent, fleet
    /// workers inherit conservative Sandbox defaults.
    #[serde(default)]
    pub fleet: Option<FleetConfigToml>,
    #[serde(flatten)]
    pub extras: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderConfigField {
    ApiKey,
    BaseUrl,
    Model,
    Mode,
    AuthMode,
    InsecureSkipTlsVerify,
    HttpHeaders,
    PathSuffix,
}

impl ProviderConfigField {
    fn parse(key: &str) -> Option<Self> {
        Some(match key {
            "api_key" => Self::ApiKey,
            "base_url" => Self::BaseUrl,
            "model" => Self::Model,
            "mode" => Self::Mode,
            "auth_mode" => Self::AuthMode,
            "insecure_skip_tls_verify" => Self::InsecureSkipTlsVerify,
            "http_headers" => Self::HttpHeaders,
            "path_suffix" => Self::PathSuffix,
            _ => return None,
        })
    }

    fn key(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::BaseUrl => "base_url",
            Self::Model => "model",
            Self::Mode => "mode",
            Self::AuthMode => "auth_mode",
            Self::InsecureSkipTlsVerify => "insecure_skip_tls_verify",
            Self::HttpHeaders => "http_headers",
            Self::PathSuffix => "path_suffix",
        }
    }
}

fn parse_provider_config_key(key: &str) -> Option<(ProviderKind, ProviderConfigField)> {
    let suffix = key.strip_prefix("providers.")?;
    let (provider_key, field_key) = suffix.split_once('.')?;
    let field = ProviderConfigField::parse(field_key)?;
    let provider = ProviderKind::ALL
        .iter()
        .copied()
        .find(|kind| kind.provider().provider_config_key() == provider_key)?;
    Some((provider, field))
}

fn provider_config_key(provider: ProviderKind, field: ProviderConfigField) -> String {
    format!(
        "providers.{}.{}",
        provider.provider().provider_config_key(),
        field.key()
    )
}

fn get_provider_config_value(
    config: &ProviderConfigToml,
    field: ProviderConfigField,
) -> Option<String> {
    match field {
        ProviderConfigField::ApiKey => config.api_key.clone(),
        ProviderConfigField::BaseUrl => config.base_url.clone(),
        ProviderConfigField::Model => config.model.clone(),
        ProviderConfigField::Mode => config.mode.clone(),
        ProviderConfigField::AuthMode => config.auth_mode.clone(),
        ProviderConfigField::InsecureSkipTlsVerify => config
            .insecure_skip_tls_verify
            .map(|value| value.to_string()),
        ProviderConfigField::HttpHeaders => serialize_http_headers(&config.http_headers),
        ProviderConfigField::PathSuffix => config.path_suffix.clone(),
    }
}

fn get_provider_config_display_value(
    config: &ProviderConfigToml,
    field: ProviderConfigField,
) -> Option<String> {
    match field {
        ProviderConfigField::ApiKey => config.api_key.as_deref().map(redact_secret),
        ProviderConfigField::HttpHeaders => {
            serialize_http_headers_for_display(&config.http_headers)
        }
        _ => get_provider_config_value(config, field),
    }
}

fn set_provider_config_value(
    config: &mut ConfigToml,
    provider: ProviderKind,
    field: ProviderConfigField,
    value: &str,
) -> Result<()> {
    match field {
        ProviderConfigField::ApiKey => {
            let value = value.to_string();
            config.providers.for_provider_mut(provider).api_key = Some(value.clone());
            if provider == ProviderKind::Deepseek {
                config.api_key = Some(value);
            }
        }
        ProviderConfigField::BaseUrl => {
            let value = value.to_string();
            config.providers.for_provider_mut(provider).base_url = Some(value.clone());
            if provider == ProviderKind::Deepseek {
                config.base_url = Some(value);
            }
        }
        ProviderConfigField::Model => {
            let value = value.to_string();
            config.providers.for_provider_mut(provider).model = Some(value.clone());
            if provider == ProviderKind::Deepseek {
                config.default_text_model = Some(value);
            }
        }
        ProviderConfigField::Mode => {
            config.providers.for_provider_mut(provider).mode = Some(value.to_string());
        }
        ProviderConfigField::AuthMode => {
            config.providers.for_provider_mut(provider).auth_mode = Some(value.to_string());
        }
        ProviderConfigField::InsecureSkipTlsVerify => {
            config
                .providers
                .for_provider_mut(provider)
                .insecure_skip_tls_verify = Some(parse_bool(value)?);
        }
        ProviderConfigField::HttpHeaders => {
            let headers = parse_http_headers(value)?;
            config.providers.for_provider_mut(provider).http_headers = headers.clone();
            if provider == ProviderKind::Deepseek {
                config.http_headers = headers;
            }
        }
        ProviderConfigField::PathSuffix => {
            config.providers.for_provider_mut(provider).path_suffix = Some(value.to_string());
        }
    }
    Ok(())
}

fn unset_provider_config_value(
    config: &mut ConfigToml,
    provider: ProviderKind,
    field: ProviderConfigField,
) {
    match field {
        ProviderConfigField::ApiKey => {
            config.providers.for_provider_mut(provider).api_key = None;
            if provider == ProviderKind::Deepseek {
                config.api_key = None;
            }
        }
        ProviderConfigField::BaseUrl => {
            config.providers.for_provider_mut(provider).base_url = None;
            if provider == ProviderKind::Deepseek {
                config.base_url = None;
            }
        }
        ProviderConfigField::Model => {
            config.providers.for_provider_mut(provider).model = None;
            if provider == ProviderKind::Deepseek {
                config.default_text_model = None;
            }
        }
        ProviderConfigField::Mode => {
            config.providers.for_provider_mut(provider).mode = None;
        }
        ProviderConfigField::AuthMode => {
            config.providers.for_provider_mut(provider).auth_mode = None;
        }
        ProviderConfigField::InsecureSkipTlsVerify => {
            config
                .providers
                .for_provider_mut(provider)
                .insecure_skip_tls_verify = None;
        }
        ProviderConfigField::HttpHeaders => {
            config
                .providers
                .for_provider_mut(provider)
                .http_headers
                .clear();
            if provider == ProviderKind::Deepseek {
                config.http_headers.clear();
            }
        }
        ProviderConfigField::PathSuffix => {
            config.providers.for_provider_mut(provider).path_suffix = None;
        }
    }
}

fn insert_provider_config_values(
    out: &mut BTreeMap<String, String>,
    provider: ProviderKind,
    config: &ProviderConfigToml,
) {
    if let Some(v) = config.api_key.as_ref() {
        out.insert(
            provider_config_key(provider, ProviderConfigField::ApiKey),
            redact_secret(v),
        );
    }
    if let Some(v) = config.base_url.as_ref() {
        out.insert(
            provider_config_key(provider, ProviderConfigField::BaseUrl),
            v.clone(),
        );
    }
    if let Some(v) = config.model.as_ref() {
        out.insert(
            provider_config_key(provider, ProviderConfigField::Model),
            v.clone(),
        );
    }
    if let Some(v) = config.mode.as_ref() {
        out.insert(
            provider_config_key(provider, ProviderConfigField::Mode),
            v.clone(),
        );
    }
    if let Some(v) = config.auth_mode.as_ref() {
        out.insert(
            provider_config_key(provider, ProviderConfigField::AuthMode),
            v.clone(),
        );
    }
    if let Some(v) = config.insecure_skip_tls_verify {
        out.insert(
            provider_config_key(provider, ProviderConfigField::InsecureSkipTlsVerify),
            v.to_string(),
        );
    }
    if let Some(v) = serialize_http_headers_for_display(&config.http_headers) {
        out.insert(
            provider_config_key(provider, ProviderConfigField::HttpHeaders),
            v,
        );
    }
    if let Some(v) = config.path_suffix.as_ref() {
        out.insert(
            provider_config_key(provider, ProviderConfigField::PathSuffix),
            v.clone(),
        );
    }
}

impl ConfigToml {
    /// Resolve the first configured harness profile for a provider/model route.
    ///
    /// This helper is deliberately dormant for v0.9: callers may display or
    /// test the resolved profile, but runtime provider/model routing and prompt
    /// shaping remain unchanged until a later, explicit integration slice.
    #[must_use]
    pub fn resolve_harness_profile(
        &self,
        provider_route: &str,
        model: &str,
    ) -> Option<&HarnessProfile> {
        self.harness_profiles
            .iter()
            .chain(built_in_harness_profiles().iter())
            .find(|profile| profile.matches_route(provider_route, model))
    }

    /// Resolve durable hotbar config into normalized 1-8 slot bindings.
    ///
    /// `known_action_ids` is supplied by the TUI action registry in later
    /// slices. Unknown actions are preserved so the UI can render a disabled
    /// `?` cell instead of silently deleting user config.
    #[must_use]
    pub fn resolve_hotbar_bindings(&self, known_action_ids: &[&str]) -> HotbarConfigResolution {
        resolve_hotbar_bindings(self.hotbar.as_deref(), known_action_ids)
    }
}

/// Built-in profile seeds for common provider/model families.
///
/// User-configured profiles are always checked first; these seeds only provide
/// a stable resolver result when config has no narrower match.
#[must_use]
pub fn built_in_harness_profiles() -> &'static [HarnessProfile] {
    static PROFILES: OnceLock<Vec<HarnessProfile>> = OnceLock::new();
    PROFILES.get_or_init(|| {
        vec![
            HarnessProfile {
                provider_route: "deepseek".to_string(),
                model_pattern: "deepseek-v4*".to_string(),
                posture: HarnessPosture::cache_heavy(),
            },
            HarnessProfile {
                provider_route: "xiaomi-mimo".to_string(),
                model_pattern: "mimo-v2.5*".to_string(),
                posture: HarnessPosture::cache_heavy(),
            },
            HarnessProfile {
                provider_route: "arcee".to_string(),
                model_pattern: "trinity-large-thinking".to_string(),
                posture: HarnessPosture::cache_heavy(),
            },
            HarnessProfile {
                provider_route: "huggingface".to_string(),
                model_pattern: "*".to_string(),
                posture: HarnessPosture::lean(),
            },
            HarnessProfile {
                provider_route: "sglang".to_string(),
                model_pattern: "*".to_string(),
                posture: HarnessPosture::lean(),
            },
            HarnessProfile {
                provider_route: "vllm".to_string(),
                model_pattern: "*".to_string(),
                posture: HarnessPosture::lean(),
            },
            HarnessProfile {
                provider_route: "ollama".to_string(),
                model_pattern: "*".to_string(),
                posture: HarnessPosture::lean(),
            },
        ]
    })
}

fn provider_routes_equal(expected: &str, actual: &str) -> bool {
    match (ProviderKind::parse(expected), ProviderKind::parse(actual)) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => expected.trim().eq_ignore_ascii_case(actual.trim()),
    }
}

fn wildcard_pattern_matches(pattern: &str, value: &str) -> bool {
    wildcard_chars_match(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
    )
}

fn wildcard_chars_match(pattern: &[char], value: &[char]) -> bool {
    let (mut pattern_idx, mut value_idx) = (0, 0);
    let mut star_idx: Option<usize> = None;
    let mut star_value_idx = 0;

    while value_idx < value.len() {
        if pattern_idx < pattern.len()
            && (pattern[pattern_idx] == '?' || pattern[pattern_idx] == value[value_idx])
        {
            pattern_idx += 1;
            value_idx += 1;
        } else if pattern_idx < pattern.len() && pattern[pattern_idx] == '*' {
            star_idx = Some(pattern_idx);
            pattern_idx += 1;
            star_value_idx = value_idx;
        } else if let Some(star) = star_idx {
            pattern_idx = star + 1;
            star_value_idx += 1;
            value_idx = star_value_idx;
        } else {
            return false;
        }
    }

    pattern[pattern_idx..].iter().all(|ch| *ch == '*')
}

/// Ordered primary-plus-fallback provider list for future provider routing.
///
/// The helper is intentionally dormant: constructing or parsing a chain does
/// not change [`ConfigToml::resolve_runtime_options`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChain {
    providers: Vec<ProviderKind>,
    position: usize,
}

pub const HOTBAR_SLOT_COUNT: u8 = 8;

pub const DEFAULT_HOTBAR_ACTIONS: [&str; HOTBAR_SLOT_COUNT as usize] = [
    "voice.toggle",
    "session.compact",
    "mode.plan",
    "mode.agent",
    "mode.yolo",
    "palette.open",
    "sidebar.toggle",
    "trust.toggle",
];

/// On-disk schema for one `[[hotbar]]` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HotbarBindingToml {
    pub slot: u8,
    pub action: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// Validated hotbar binding used by future render/dispatch layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotbarBinding {
    pub slot: u8,
    pub action: String,
    pub label: Option<String>,
}

/// Non-fatal hotbar config issue. Invalid slots are skipped; duplicate slots
/// use the last binding; unknown actions are kept for UI feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotbarConfigWarning {
    SlotOutOfRange {
        slot: u8,
        action: String,
    },
    DuplicateSlot {
        slot: u8,
        previous_action: String,
        replacement_action: String,
    },
    UnknownAction {
        slot: u8,
        action: String,
    },
}

impl fmt::Display for HotbarConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SlotOutOfRange { slot, action } => write!(
                f,
                "hotbar slot {slot} for action '{action}' is outside 1-{HOTBAR_SLOT_COUNT}; skipped"
            ),
            Self::DuplicateSlot {
                slot,
                previous_action,
                replacement_action,
            } => write!(
                f,
                "hotbar slot {slot} was bound to '{previous_action}' more than once; using '{replacement_action}'"
            ),
            Self::UnknownAction { slot, action } => write!(
                f,
                "hotbar slot {slot} references unknown action '{action}'; keeping binding"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotbarConfigResolution {
    pub bindings: Vec<HotbarBinding>,
    pub warnings: Vec<HotbarConfigWarning>,
}

#[must_use]
pub fn default_hotbar_bindings() -> Vec<HotbarBinding> {
    DEFAULT_HOTBAR_ACTIONS
        .iter()
        .enumerate()
        .map(|(idx, action)| HotbarBinding {
            slot: u8::try_from(idx + 1).expect("default hotbar slot fits in u8"),
            action: (*action).to_string(),
            label: None,
        })
        .collect()
}

#[must_use]
pub fn resolve_hotbar_bindings(
    configured: Option<&[HotbarBindingToml]>,
    known_action_ids: &[&str],
) -> HotbarConfigResolution {
    let known = known_action_ids.iter().copied().collect::<BTreeSet<&str>>();
    let mut warnings = Vec::new();

    let source = match configured {
        Some(bindings) => bindings
            .iter()
            .map(|binding| HotbarBinding {
                slot: binding.slot,
                action: binding.action.clone(),
                label: binding.label.clone(),
            })
            .collect::<Vec<_>>(),
        None => default_hotbar_bindings(),
    };

    let mut by_slot: BTreeMap<u8, HotbarBinding> = BTreeMap::new();
    for binding in source {
        if !(1..=HOTBAR_SLOT_COUNT).contains(&binding.slot) {
            warnings.push(HotbarConfigWarning::SlotOutOfRange {
                slot: binding.slot,
                action: binding.action,
            });
            continue;
        }
        if !known.is_empty() && !known.contains(binding.action.as_str()) {
            warnings.push(HotbarConfigWarning::UnknownAction {
                slot: binding.slot,
                action: binding.action.clone(),
            });
        }
        if let Some(previous) = by_slot.insert(binding.slot, binding.clone()) {
            warnings.push(HotbarConfigWarning::DuplicateSlot {
                slot: binding.slot,
                previous_action: previous.action,
                replacement_action: binding.action,
            });
        }
    }

    HotbarConfigResolution {
        bindings: by_slot.into_values().collect(),
        warnings,
    }
}

impl ProviderChain {
    #[must_use]
    pub fn new(active: ProviderKind, fallbacks: &[ProviderKind]) -> Self {
        let mut providers = vec![active];
        for fallback in fallbacks {
            if *fallback != active && !providers.contains(fallback) {
                providers.push(*fallback);
            }
        }
        Self {
            providers,
            position: 0,
        }
    }

    #[must_use]
    pub fn providers(&self) -> &[ProviderKind] {
        &self.providers
    }

    #[must_use]
    pub fn position(&self) -> usize {
        self.position
    }

    #[must_use]
    pub fn current(&self) -> ProviderKind {
        self.providers
            .get(self.position)
            .copied()
            .unwrap_or(self.providers[0])
    }

    #[must_use]
    pub fn has_next(&self) -> bool {
        self.position + 1 < self.providers.len()
    }

    pub fn advance(&mut self) -> Option<ProviderKind> {
        if !self.has_next() {
            return None;
        }
        self.position += 1;
        Some(self.current())
    }

    pub fn reset(&mut self) {
        self.position = 0;
    }

    #[must_use]
    pub fn is_fallback_active(&self) -> bool {
        self.position > 0
    }

    /// Count the current provider plus untried chain entries.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.providers.len() - self.position
    }
}

/// On-disk schema for the `[hook_sinks]` table.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookSinksToml {
    /// Unix domain socket path used by the app-server event sink.
    ///
    /// When unset, no Unix socket sink is registered. There is deliberately no
    /// shared `/tmp` default because socket ownership should be explicit.
    #[serde(default)]
    pub unix_socket_path: Option<PathBuf>,
}

/// On-disk schema for the `[skills]` table (#140). See `config.example.toml`
/// for documentation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillsToml {
    /// Curated registry index URL. When unset, the TUI falls back to the
    /// bundled default (community-curated GitHub raw).
    #[serde(default)]
    pub registry_url: Option<String>,
    /// Per-skill maximum *uncompressed* size in bytes. When unset, the TUI
    /// uses 5 MiB.
    #[serde(default)]
    pub max_install_size_bytes: Option<u64>,
}

/// On-disk schema for the `[tools]` table (#2076).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsToml {
    /// Native tool names to keep loaded outside the default core catalog.
    #[serde(default)]
    pub always_load: Vec<String>,
}

/// On-disk schema for the `[snapshots]` table (#137). See
/// `config.example.toml` for documentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotsToml {
    #[serde(default = "default_snapshots_enabled")]
    pub enabled: bool,
    #[serde(default = "default_snapshot_max_age_days")]
    pub max_age_days: u64,
}

fn default_snapshots_enabled() -> bool {
    true
}

fn default_snapshot_max_age_days() -> u64 {
    7
}

impl Default for SnapshotsToml {
    fn default() -> Self {
        Self {
            enabled: default_snapshots_enabled(),
            max_age_days: default_snapshot_max_age_days(),
        }
    }
}

/// On-disk schema for the `[fleet]` table (#3165). See `config.example.toml`
/// and `docs/FLEET.md` for documentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetConfigToml {
    /// Default trust level for fleet workers. One of `"sandbox"`, `"local"`,
    /// `"remote-verified"`, or `"operator"`. Defaults to `"sandbox"`.
    #[serde(default = "default_fleet_trust_level_str")]
    pub default_trust_level: String,
    /// Require identity verification for remote (SSH) workers before
    /// granting them `remote-verified` trust. Defaults to true.
    #[serde(default = "default_fleet_require_identity")]
    pub require_identity_verification: bool,
    /// Maximum trust level any worker may have (`"sandbox"`, `"local"`,
    /// `"remote-verified"`, or `"operator"`). Defaults to `"operator"`.
    #[serde(default = "default_fleet_max_trust_level_str")]
    pub max_trust_level: String,
    /// User-defined and built-in role presets.
    ///
    /// Each role defines default tool profiles, capabilities, budgets, and
    /// trust settings that task specs can reference by name. Built-in roles
    /// (`smoke-runner`, `reviewer`, `builder`, `read-only`) are always
    /// available; user-defined roles in config override or extend them.
    #[serde(default)]
    pub roles: BTreeMap<String, FleetRolePreset>,
    /// Headless worker execution hardening (#3027).
    #[serde(default)]
    pub exec: FleetExecConfig,
}

/// Canonical recursion-depth policy for the headless worker runtime.
///
/// Single source of truth shared by BOTH standalone sub-agents and fleet
/// workers so the two cannot drift into "two moving targets":
/// - [`DEFAULT_SPAWN_DEPTH`] is the default recursion budget (the sub-agent
///   runtime's `DEFAULT_MAX_SPAWN_DEPTH` is defined as this value).
/// - [`MAX_SPAWN_DEPTH_CEILING`] is the opt-in safety cap; every configured
///   value (fleet `max_spawn_depth`, the `agent` tool's `max_depth`) clamps to it.
///
/// A worker runs at `spawn_depth = 0` and may spawn while
/// `spawn_depth + 1 <= max_spawn_depth`, so a depth of N affords N nested
/// delegation levels below the root worker. The default of 3 affords at least
/// three recursion levels out of the box; the root worker still runs at
/// depth 0 even when the budget is 0.
pub const DEFAULT_SPAWN_DEPTH: u32 = 3;

/// Hard ceiling on recursion depth for any worker/sub-agent. The default stays
/// conservative at [`DEFAULT_SPAWN_DEPTH`], while explicit config can opt into
/// deeper trees for direct-API providers that can tolerate the fanout.
/// Raising this single constant lifts the limit everywhere (the fleet clamp
/// and `agent` validation both read it).
pub const MAX_SPAWN_DEPTH_CEILING: u32 = 8;

/// Headless worker execution constraints (#3027).
///
/// These limits apply to all fleet workers and sub-agents spawned through
/// the headless worker runtime. Task specs can tighten but not loosen them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetExecConfig {
    /// Tools that are always allowed regardless of role or task spec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Tools that are always disallowed, overriding role and task spec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disallowed_tools: Vec<String>,
    /// Hard ceiling on sub-agent steps (tool calls + model turns).
    /// Workers that exceed this are terminated. Default: unbounded (u32::MAX).
    #[serde(default = "default_fleet_max_turns")]
    pub max_turns: u32,
    /// Recursive child-agent budget for headless fleet workers.
    /// Defaults to [`DEFAULT_SPAWN_DEPTH`] (3) so a fleet worker has the SAME
    /// recursion budget as a standalone sub-agent — fleet and sub-agents are one
    /// substrate, not two. Set 0 to block child `agent` calls (the root worker
    /// still runs); the value is clamped to [`MAX_SPAWN_DEPTH_CEILING`].
    #[serde(default = "default_fleet_max_spawn_depth")]
    pub max_spawn_depth: u32,
    /// Extra system prompt text appended to every headless worker.
    /// Useful for injecting org-wide policy or behavior constraints.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub append_system_prompt: String,
    /// Output format for fleet worker results.
    /// `"text"` (default) or `"stream-json"` for newline-delimited JSON events.
    #[serde(default = "default_fleet_output_format")]
    pub output_format: String,
}

fn default_fleet_max_turns() -> u32 {
    u32::MAX
}

fn default_fleet_max_spawn_depth() -> u32 {
    DEFAULT_SPAWN_DEPTH
}

fn default_fleet_output_format() -> String {
    "text".to_string()
}

impl Default for FleetExecConfig {
    fn default() -> Self {
        Self {
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            max_turns: default_fleet_max_turns(),
            max_spawn_depth: default_fleet_max_spawn_depth(),
            append_system_prompt: String::new(),
            output_format: default_fleet_output_format(),
        }
    }
}

/// A named role preset that bundles common worker settings.
///
/// Task specs reference a role name (e.g. `"role": "reviewer"`), and the
/// fleet manager fills in any missing fields from the preset. User-defined
/// roles in `[fleet.roles]` override built-in defaults with the same name.
///
/// Token budgets and tool-call limits are task-level decisions — they don't
/// belong on role presets. Use `timeout_seconds` as the safety bound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetRolePreset {
    /// Short description of what this role is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Default tool profile (`"read-only"`, `"read-write"`, or `"custom"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_profile: Option<String>,
    /// Default set of tool names available to this role.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Default capability tags (e.g. `"rust"`, `"git"`, `"gh"`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Default timeout in seconds for tasks using this role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Default trust level override for this role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_level: Option<String>,
}

fn default_fleet_trust_level_str() -> String {
    "sandbox".to_string()
}

fn default_fleet_require_identity() -> bool {
    true
}

fn default_fleet_max_trust_level_str() -> String {
    "operator".to_string()
}

impl Default for FleetConfigToml {
    fn default() -> Self {
        Self {
            default_trust_level: default_fleet_trust_level_str(),
            require_identity_verification: default_fleet_require_identity(),
            max_trust_level: default_fleet_max_trust_level_str(),
            roles: BTreeMap::new(),
            exec: FleetExecConfig::default(),
        }
    }
}

impl FleetConfigToml {
    /// Resolve a role preset by name. Checks user-defined roles first,
    /// then falls back to built-in role defaults.
    #[must_use]
    pub fn resolve_role(&self, name: &str) -> Option<FleetRolePreset> {
        self.roles
            .get(name)
            .cloned()
            .or_else(|| built_in_role_presets().get(name).cloned())
    }
}

/// Built-in role presets that are always available without config.
#[must_use]
pub fn built_in_role_presets() -> BTreeMap<String, FleetRolePreset> {
    [
        (
            "smoke-runner".to_string(),
            FleetRolePreset {
                description: Some("Lightweight read-only smoke check worker".to_string()),
                tool_profile: Some("read-only".to_string()),
                tools: vec![],
                capabilities: vec![],
                timeout_seconds: Some(300),
                trust_level: Some("local".to_string()),
            },
        ),
        (
            "reviewer".to_string(),
            FleetRolePreset {
                description: Some("Read-only code and documentation review".to_string()),
                tool_profile: Some("read-only".to_string()),
                tools: vec![],
                capabilities: vec![],
                timeout_seconds: Some(600),
                trust_level: None,
            },
        ),
        (
            "builder".to_string(),
            FleetRolePreset {
                description: Some(
                    "Read-write builder with compilation and test access".to_string(),
                ),
                tool_profile: Some("read-write".to_string()),
                tools: vec![],
                capabilities: vec![],
                timeout_seconds: Some(1800),
                trust_level: Some("local".to_string()),
            },
        ),
        (
            "read-only".to_string(),
            FleetRolePreset {
                description: Some(
                    "Minimal read-only observer with no writes or secrets".to_string(),
                ),
                tool_profile: Some("read-only".to_string()),
                tools: vec![],
                capabilities: vec![],
                timeout_seconds: Some(300),
                trust_level: Some("sandbox".to_string()),
            },
        ),
    ]
    .into()
}

/// On-disk schema for the `[network]` table (#135). See `config.example.toml`
/// for documentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicyToml {
    /// Decision for hosts that are not in `allow` or `deny`. One of
    /// `"allow" | "deny" | "prompt"`. Defaults to `"prompt"`.
    #[serde(default = "default_network_decision")]
    pub default: String,
    /// Hosts that are always allowed. Subdomain rules: a leading dot
    /// (`.example.com`) matches subdomains but not the apex.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Hosts that are always denied. Deny entries win over allow entries.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Hostnames whose DNS may resolve to fake-IP/private proxy ranges in an
    /// explicitly trusted proxy setup. Literal IP URLs remain blocked.
    #[serde(default)]
    pub proxy: Vec<String>,
    /// Whether to record one audit-log line per outbound network call.
    #[serde(default = "default_network_audit")]
    pub audit: bool,
}

fn default_network_decision() -> String {
    "prompt".to_string()
}

fn default_network_audit() -> bool {
    true
}

impl Default for NetworkPolicyToml {
    fn default() -> Self {
        Self {
            default: default_network_decision(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: Vec::new(),
            audit: default_network_audit(),
        }
    }
}

/// On-disk schema for the `[lsp]` table (#136). See `config.example.toml`
/// for documentation. All fields are optional so the TUI runtime can fall
/// back to its own defaults when keys are absent.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LspConfigToml {
    /// Master switch.
    pub enabled: Option<bool>,
    /// Maximum time to wait for diagnostics after an edit, in milliseconds.
    pub poll_after_edit_ms: Option<u64>,
    /// Cap on diagnostics surfaced per file.
    pub max_diagnostics_per_file: Option<usize>,
    /// When `true`, warnings (severity 2) are surfaced in addition to errors.
    pub include_warnings: Option<bool>,
    /// Optional override for the `language -> [cmd, ...args]` table.
    pub servers: Option<BTreeMap<String, Vec<String>>>,
}

impl ConfigToml {
    /// Merge safe project-level overrides from `$WORKSPACE/.codewhale/config.toml`
    /// or legacy `$WORKSPACE/.deepseek/config.toml`.
    ///
    /// Repo-local config is untrusted input. This helper intentionally ignores
    /// credentials, endpoints, provider selection, auth/session values, telemetry,
    /// network policy, skill registry, LSP command tables, and unknown extras.
    /// Approval and sandbox values may only tighten the existing user/global
    /// posture.
    pub fn merge_project_overrides(&mut self, project: ConfigToml) {
        if project.default_text_model.is_some() {
            self.default_text_model = project.default_text_model;
        }
        if project.model.is_some() {
            self.model = project.model;
        }
        if project.output_mode.is_some() {
            self.output_mode = project.output_mode;
        }
        if project.verbosity.is_some() {
            self.verbosity = project.verbosity;
        }
        if project.log_level.is_some() {
            self.log_level = project.log_level;
        }
        if let Some(policy) = project.approval_policy
            && project_approval_policy_is_allowed(self.approval_policy.as_deref(), &policy)
        {
            self.approval_policy = Some(policy);
        }
        if let Some(mode) = project.sandbox_mode
            && project_sandbox_mode_is_allowed(self.sandbox_mode.as_deref(), &mode)
        {
            self.sandbox_mode = Some(mode);
        }
        if project.tools.is_some() {
            self.tools = project.tools;
        }
        for provider in ProviderKind::ALL {
            merge_project_provider_config(
                self.providers.for_provider_mut(provider),
                project.providers.for_provider(provider),
            );
        }
    }

    #[must_use]
    pub fn get_value(&self, key: &str) -> Option<String> {
        if let Some((provider, field)) = parse_provider_config_key(key) {
            return get_provider_config_value(self.providers.for_provider(provider), field);
        }

        match key {
            "provider" => Some(self.provider.as_str().to_string()),
            "api_key" => self.api_key.clone(),
            "base_url" => self.base_url.clone(),
            "http_headers" => serialize_http_headers(&self.http_headers),
            "default_text_model" => self.default_text_model.clone(),
            "model" => self.model.clone(),
            "auth.mode" => self.auth_mode.clone(),
            "output_mode" => self.output_mode.clone(),
            "verbosity" => self.verbosity.clone(),
            "log_level" => self.log_level.clone(),
            "telemetry" => self.telemetry.map(|v| v.to_string()),
            "approval_policy" => self.approval_policy.clone(),
            "sandbox_mode" => self.sandbox_mode.clone(),
            "tools.always_load" => self.tools.as_ref().map(|tools| tools.always_load.join(",")),
            "hook_sinks.unix_socket_path" => self
                .hook_sinks
                .as_ref()
                .and_then(|sinks| sinks.unix_socket_path.as_ref())
                .map(|path| path.display().to_string()),
            _ => self.extras.get(key).map(toml::Value::to_string),
        }
    }

    #[must_use]
    pub fn get_display_value(&self, key: &str) -> Option<String> {
        if let Some((provider, field)) = parse_provider_config_key(key) {
            return get_provider_config_display_value(self.providers.for_provider(provider), field);
        }

        if key == "http_headers" {
            return serialize_http_headers_for_display(&self.http_headers);
        }

        if let Some(value) = self.extras.get(key) {
            return Some(redact_toml_value_for_display(key, value));
        }

        self.get_value(key).map(|value| {
            if is_sensitive_config_key(key) {
                redact_secret(&value)
            } else {
                value
            }
        })
    }

    pub fn set_value(&mut self, key: &str, value: &str) -> Result<()> {
        if let Some((provider, field)) = parse_provider_config_key(key) {
            return set_provider_config_value(self, provider, field, value);
        }

        match key {
            "provider" => {
                self.provider = ProviderKind::parse(value).with_context(|| {
                    format!(
                        "unknown provider '{value}': expected {}",
                        ProviderKind::names_hint()
                    )
                })?;
            }
            "api_key" => self.api_key = Some(value.to_string()),
            "base_url" => self.base_url = Some(value.to_string()),
            "http_headers" => self.http_headers = parse_http_headers(value)?,
            "default_text_model" => self.default_text_model = Some(value.to_string()),
            "model" => self.model = Some(value.to_string()),
            "auth.mode" => self.auth_mode = Some(value.to_string()),
            "output_mode" => self.output_mode = Some(value.to_string()),
            "verbosity" => self.verbosity = Some(value.to_string()),
            "log_level" => self.log_level = Some(value.to_string()),
            "telemetry" => {
                self.telemetry = Some(parse_bool(value)?);
            }
            "approval_policy" => self.approval_policy = Some(value.to_string()),
            "sandbox_mode" => self.sandbox_mode = Some(value.to_string()),
            "hook_sinks.unix_socket_path" => {
                self.hook_sinks
                    .get_or_insert_with(HookSinksToml::default)
                    .unix_socket_path = Some(PathBuf::from(value));
            }
            _ => {
                self.extras
                    .insert(key.to_string(), toml::Value::String(value.to_string()));
            }
        }
        Ok(())
    }

    pub fn unset_value(&mut self, key: &str) -> Result<()> {
        if let Some((provider, field)) = parse_provider_config_key(key) {
            unset_provider_config_value(self, provider, field);
            return Ok(());
        }

        match key {
            "provider" => self.provider = ProviderKind::Deepseek,
            "api_key" => self.api_key = None,
            "base_url" => self.base_url = None,
            "http_headers" => self.http_headers.clear(),
            "default_text_model" => self.default_text_model = None,
            "model" => self.model = None,
            "auth.mode" => self.auth_mode = None,
            "output_mode" => self.output_mode = None,
            "verbosity" => self.verbosity = None,
            "log_level" => self.log_level = None,
            "telemetry" => self.telemetry = None,
            "approval_policy" => self.approval_policy = None,
            "sandbox_mode" => self.sandbox_mode = None,
            "hook_sinks.unix_socket_path" => {
                if let Some(sinks) = self.hook_sinks.as_mut() {
                    sinks.unix_socket_path = None;
                }
            }
            _ => {
                self.extras.remove(key);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn list_values(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        out.insert("provider".to_string(), self.provider.as_str().to_string());

        if let Some(v) = self.api_key.as_ref() {
            out.insert("api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.base_url.as_ref() {
            out.insert("base_url".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers_for_display(&self.http_headers) {
            out.insert("http_headers".to_string(), v);
        }
        if let Some(v) = self.default_text_model.as_ref() {
            out.insert("default_text_model".to_string(), v.clone());
        }
        if let Some(v) = self.model.as_ref() {
            out.insert("model".to_string(), v.clone());
        }
        if let Some(v) = self.auth_mode.as_ref() {
            out.insert("auth.mode".to_string(), v.clone());
        }
        if let Some(v) = self.output_mode.as_ref() {
            out.insert("output_mode".to_string(), v.clone());
        }
        if let Some(v) = self.verbosity.as_ref() {
            out.insert("verbosity".to_string(), v.clone());
        }
        if let Some(v) = self.log_level.as_ref() {
            out.insert("log_level".to_string(), v.clone());
        }
        if let Some(v) = self.telemetry {
            out.insert("telemetry".to_string(), v.to_string());
        }
        if let Some(v) = self.approval_policy.as_ref() {
            out.insert("approval_policy".to_string(), v.clone());
        }
        if let Some(v) = self.sandbox_mode.as_ref() {
            out.insert("sandbox_mode".to_string(), v.clone());
        }
        if let Some(v) = self
            .hook_sinks
            .as_ref()
            .and_then(|sinks| sinks.unix_socket_path.as_ref())
        {
            out.insert(
                "hook_sinks.unix_socket_path".to_string(),
                v.display().to_string(),
            );
        }

        for provider in ProviderKind::ALL {
            insert_provider_config_values(
                &mut out,
                provider,
                self.providers.for_provider(provider),
            );
        }

        for (k, v) in &self.extras {
            out.insert(k.clone(), redact_toml_value_for_display(k, v));
        }
        out
    }

    /// Resolve runtime options without touching platform credential stores.
    ///
    /// This method keeps library callers prompt-free: CLI flag → config file
    /// → environment. Call `resolve_runtime_options_with_secrets` when a
    /// user-facing dispatcher should recover credentials from the configured
    /// secret store.
    #[must_use]
    pub fn resolve_runtime_options(&self, cli: &CliRuntimeOverrides) -> ResolvedRuntimeOptions {
        let no_keyring = Secrets::new(std::sync::Arc::new(
            codewhale_secrets::InMemoryKeyringStore::new(),
        ));
        self.resolve_runtime_options_with_secrets(cli, &no_keyring)
    }

    /// Resolve runtime options using an explicit secrets façade.
    ///
    /// API-key precedence is **CLI flag → config-file → secret store → environment**.
    #[must_use]
    pub fn resolve_runtime_options_with_secrets(
        &self,
        cli: &CliRuntimeOverrides,
        secrets: &Secrets,
    ) -> ResolvedRuntimeOptions {
        let env = EnvRuntimeOverrides::load();
        let (provider, provider_source) = if let Some(provider) = cli.provider {
            (provider, ProviderSource::Cli)
        } else if let Some(provider) = env.provider {
            (
                provider,
                ProviderSource::Env(env.provider_source.unwrap_or("CODEWHALE_PROVIDER")),
            )
        } else {
            (self.provider, ProviderSource::Config)
        };

        let mut provider_cfg = self.providers.for_provider(provider).clone();
        if provider == ProviderKind::SiliconflowCN {
            let fb = &self.providers.siliconflow;
            if provider_cfg.api_key.is_none() {
                provider_cfg.api_key = fb.api_key.clone();
            }
            if provider_cfg.base_url.is_none() {
                provider_cfg.base_url = fb.base_url.clone();
            }
            if provider_cfg.model.is_none() {
                provider_cfg.model = fb.model.clone();
            }
        }
        let root_deepseek_api_key = (provider == ProviderKind::Deepseek)
            .then(|| self.api_key.clone())
            .flatten();
        let root_deepseek_base_url = (provider == ProviderKind::Deepseek)
            .then(|| self.base_url.clone())
            .flatten();
        let root_deepseek_model = (provider == ProviderKind::Deepseek)
            .then(|| self.default_text_model.clone())
            .flatten();
        let auth_mode = cli
            .auth_mode
            .clone()
            .or_else(|| env.auth_mode.clone())
            .or_else(|| provider_cfg.auth_mode.clone())
            .or_else(|| self.auth_mode.clone());
        let from_file = provider_cfg.api_key.clone().or(root_deepseek_api_key);
        let configured_base_url = cli
            .base_url
            .clone()
            .or_else(|| env.base_url_for(provider))
            .or_else(|| provider_cfg.base_url.clone())
            .or(root_deepseek_base_url);
        let xiaomi_mimo_mode = if provider == ProviderKind::XiaomiMimo {
            env.xiaomi_mimo_mode
                .clone()
                .or_else(|| provider_cfg.mode.clone())
        } else {
            None
        };
        let xiaomi_mimo_env_api_key = if provider == ProviderKind::XiaomiMimo {
            xiaomi_mimo_env_api_key_for_runtime(
                xiaomi_mimo_mode.as_deref(),
                configured_base_url.as_deref(),
            )
        } else {
            None
        };
        let explicit_api_key_for_endpoint = cli
            .api_key
            .as_deref()
            .or(from_file.as_deref())
            .or(xiaomi_mimo_env_api_key.as_deref());
        let base_url = if provider == ProviderKind::XiaomiMimo {
            resolve_xiaomi_mimo_base_url(
                configured_base_url,
                explicit_api_key_for_endpoint,
                xiaomi_mimo_mode.as_deref(),
            )
        } else {
            configured_base_url.unwrap_or_else(|| match provider {
                ProviderKind::Deepseek => DEFAULT_DEEPSEEK_BASE_URL.to_string(),
                ProviderKind::NvidiaNim => DEFAULT_NVIDIA_NIM_BASE_URL.to_string(),
                ProviderKind::Openai => DEFAULT_OPENAI_BASE_URL.to_string(),
                ProviderKind::Atlascloud => DEFAULT_ATLASCLOUD_BASE_URL.to_string(),
                ProviderKind::WanjieArk => DEFAULT_WANJIE_ARK_BASE_URL.to_string(),
                ProviderKind::Volcengine => DEFAULT_VOLCENGINE_BASE_URL.to_string(),
                ProviderKind::Openrouter => DEFAULT_OPENROUTER_BASE_URL.to_string(),
                ProviderKind::XiaomiMimo => DEFAULT_XIAOMI_MIMO_BASE_URL.to_string(),
                ProviderKind::Novita => DEFAULT_NOVITA_BASE_URL.to_string(),
                ProviderKind::Fireworks => DEFAULT_FIREWORKS_BASE_URL.to_string(),
                ProviderKind::Siliconflow => DEFAULT_SILICONFLOW_BASE_URL.to_string(),
                ProviderKind::SiliconflowCN => DEFAULT_SILICONFLOW_CN_BASE_URL.to_string(),
                ProviderKind::Arcee => DEFAULT_ARCEE_BASE_URL.to_string(),
                ProviderKind::Moonshot => {
                    if auth_mode.as_deref().is_some_and(auth_mode_uses_kimi_oauth) {
                        DEFAULT_KIMI_CODE_BASE_URL.to_string()
                    } else {
                        DEFAULT_MOONSHOT_BASE_URL.to_string()
                    }
                }
                ProviderKind::Sglang => DEFAULT_SGLANG_BASE_URL.to_string(),
                ProviderKind::Vllm => DEFAULT_VLLM_BASE_URL.to_string(),
                ProviderKind::Ollama => DEFAULT_OLLAMA_BASE_URL.to_string(),
                ProviderKind::Huggingface => DEFAULT_HUGGINGFACE_BASE_URL.to_string(),
                ProviderKind::Together => DEFAULT_TOGETHER_BASE_URL.to_string(),
                ProviderKind::OpenaiCodex => DEFAULT_OPENAI_CODEX_BASE_URL.to_string(),
                ProviderKind::Anthropic => DEFAULT_ANTHROPIC_BASE_URL.to_string(),
                ProviderKind::Zai => DEFAULT_ZAI_BASE_URL.to_string(),
                ProviderKind::Stepfun => DEFAULT_STEPFUN_BASE_URL.to_string(),
                ProviderKind::Minimax => DEFAULT_MINIMAX_BASE_URL.to_string(),
                ProviderKind::Deepinfra => DEFAULT_DEEPINFRA_BASE_URL.to_string(),
            })
        };
        // CLI flag wins outright. Otherwise: config-file → injected secrets/env.
        // This makes `deepseek auth set` a reliable fix even when the user's
        // shell still exports an old key. When the file is empty, the injected
        // secrets façade recovers configured secret-store credentials before
        // falling back to ambient env.
        let uses_kimi_oauth = provider == ProviderKind::Moonshot
            && auth_mode.as_deref().is_some_and(auth_mode_uses_kimi_oauth);
        let (api_key, api_key_source) = if let Some(value) = cli.api_key.clone() {
            (Some(value), Some(RuntimeApiKeySource::Cli))
        } else if uses_kimi_oauth {
            (None, None)
        } else if let Some(value) = from_file.clone().filter(|v| !v.trim().is_empty()) {
            (Some(value), Some(RuntimeApiKeySource::ConfigFile))
        } else if let Some(value) = xiaomi_mimo_env_api_key.filter(|v| !v.trim().is_empty()) {
            (Some(value), Some(RuntimeApiKeySource::Env))
        } else if should_skip_secret_store_for_provider(provider, &base_url, auth_mode.as_deref()) {
            match env_api_key_for_provider(provider) {
                Some(value) => (Some(value), Some(RuntimeApiKeySource::Env)),
                None => (None, None),
            }
        } else {
            match secrets.resolve_with_source(provider.as_str()) {
                Some((value, source)) => {
                    let source = match source {
                        SecretSource::Keyring => RuntimeApiKeySource::Keyring,
                        SecretSource::Env => RuntimeApiKeySource::Env,
                    };
                    (Some(value), Some(source))
                }
                None => match env_api_key_for_provider(provider) {
                    Some(value) => (Some(value), Some(RuntimeApiKeySource::Env)),
                    None => (None, None),
                },
            }
        };

        let env_provider_model = env.model_for(provider, &base_url);
        let explicit_model = cli.model.is_some()
            || env.model.is_some()
            || env_provider_model.is_some()
            || provider_cfg.model.is_some()
            || root_deepseek_model.is_some()
            || self.model.is_some();
        let model = cli
            .model
            .clone()
            .or_else(|| env.model.clone())
            .or(env_provider_model)
            .or_else(|| provider_cfg.model.clone())
            .or(root_deepseek_model)
            .or_else(|| self.model.clone())
            .unwrap_or_else(|| {
                if provider == ProviderKind::Moonshot
                    && (auth_mode.as_deref().is_some_and(auth_mode_uses_kimi_oauth)
                        || moonshot_base_url_uses_kimi_code(&base_url))
                {
                    DEFAULT_KIMI_CODE_MODEL.to_string()
                } else {
                    default_model_for_provider(provider).to_string()
                }
            });
        let model =
            if explicit_model && provider_preserves_custom_base_url_model(provider, &base_url) {
                model.trim().to_string()
            } else {
                normalize_model_for_provider(provider, &model)
            };

        let mut http_headers = self.http_headers.clone();
        http_headers.extend(provider_cfg.http_headers.clone());
        if let Some(env_headers) = env.http_headers {
            http_headers.extend(env_headers);
        }
        http_headers.retain(|name, value| !name.trim().is_empty() && !value.trim().is_empty());

        let output_mode = cli
            .output_mode
            .clone()
            .or_else(|| env.output_mode.clone())
            .or_else(|| self.output_mode.clone());
        let log_level = cli
            .log_level
            .clone()
            .or_else(|| env.log_level.clone())
            .or_else(|| self.log_level.clone());
        let telemetry = cli
            .telemetry
            .or(env.telemetry)
            .or(self.telemetry)
            .unwrap_or(false);
        let approval_policy = cli
            .approval_policy
            .clone()
            .or_else(|| env.approval_policy.clone())
            .or_else(|| self.approval_policy.clone());
        let sandbox_mode = cli
            .sandbox_mode
            .clone()
            .or_else(|| env.sandbox_mode.clone())
            .or_else(|| self.sandbox_mode.clone());
        let yolo = cli.yolo.or(env.yolo);
        let verbosity = cli
            .verbosity
            .clone()
            .or_else(|| env.verbosity.clone())
            .or_else(|| self.verbosity.clone());

        ResolvedRuntimeOptions {
            provider,
            provider_source,
            model,
            api_key,
            api_key_source,
            base_url,
            auth_mode,
            insecure_skip_tls_verify: provider_cfg.insecure_skip_tls_verify.unwrap_or(false),
            output_mode,
            log_level,
            telemetry,
            approval_policy,
            sandbox_mode,
            yolo,
            verbosity,
            http_headers,
        }
    }
}

fn merge_project_provider_config(target: &mut ProviderConfigToml, source: &ProviderConfigToml) {
    if source.model.is_some() {
        target.model = source.model.clone();
    }
}

#[must_use]
pub fn project_approval_policy_is_allowed(current: Option<&str>, project: &str) -> bool {
    let Some(project_rank) = approval_policy_rank(project) else {
        return false;
    };
    match current.and_then(approval_policy_rank) {
        Some(current_rank) => project_rank >= current_rank,
        None => project_rank >= 2,
    }
}

#[must_use]
pub fn project_sandbox_mode_is_allowed(current: Option<&str>, project: &str) -> bool {
    let normalized_project = project.trim().to_ascii_lowercase();
    if normalized_project == "external-sandbox" {
        return current
            .map(|value| value.trim().eq_ignore_ascii_case("external-sandbox"))
            .unwrap_or(false);
    }

    let Some(project_rank) = sandbox_mode_rank(project) else {
        return false;
    };
    match current.and_then(sandbox_mode_rank) {
        Some(current_rank) => project_rank >= current_rank,
        None => project_rank >= 2,
    }
}

fn approval_policy_rank(value: &str) -> Option<u8> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(0),
        "suggest" | "suggested" | "on-request" | "untrusted" => Some(1),
        "never" | "deny" | "denied" => Some(2),
        _ => None,
    }
}

fn sandbox_mode_rank(value: &str) -> Option<u8> {
    match value.trim().to_ascii_lowercase().as_str() {
        "danger-full-access" => Some(0),
        "external-sandbox" => Some(0),
        "workspace-write" => Some(1),
        "read-only" => Some(2),
        _ => None,
    }
}

/// Load a project-level config from the workspace.
///
/// Checks `$WORKSPACE/.codewhale/config.toml` first, falling back to
/// `$WORKSPACE/.deepseek/config.toml` for backward compatibility.
/// Returns `None` if neither file exists or can't be parsed.
pub fn load_project_config(workspace: &Path) -> Option<ConfigToml> {
    for dir in [CODEWHALE_APP_DIR, LEGACY_APP_DIR] {
        let path = workspace.join(dir).join(CONFIG_FILE_NAME);
        if path.exists()
            && let Ok(raw) = fs::read_to_string(&path)
        {
            match toml::from_str(&raw) {
                Ok(config) => return Some(config),
                Err(e) => {
                    tracing::warn!("Failed to parse project config {}: {e}", path.display());
                    return None;
                }
            }
        }
    }
    None
}

fn normalize_model_for_provider(provider: ProviderKind, model: &str) -> String {
    if matches!(provider, ProviderKind::XiaomiMimo)
        && let Some(canonical) = canonical_xiaomi_mimo_model_id(model)
    {
        return canonical.to_string();
    }
    if matches!(provider, ProviderKind::Minimax)
        && let Some(canonical) = canonical_minimax_model_id(model)
    {
        return canonical.to_string();
    }
    if matches!(provider, ProviderKind::Zai)
        && let Some(canonical) = canonical_zai_model_id(model)
    {
        return canonical.to_string();
    }

    if matches!(
        provider,
        ProviderKind::Atlascloud
            | ProviderKind::WanjieArk
            | ProviderKind::Volcengine
            | ProviderKind::XiaomiMimo
            | ProviderKind::Zai
            | ProviderKind::Stepfun
            | ProviderKind::Minimax
            | ProviderKind::Ollama
    ) {
        return model.to_string();
    }

    let normalized = model.trim().to_ascii_lowercase();
    if provider == ProviderKind::Openrouter
        && let Some(canonical) = canonical_openrouter_recent_model_id(&normalized)
    {
        return canonical.to_string();
    }
    match (provider, normalized.as_str()) {
        (ProviderKind::NvidiaNim, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_NVIDIA_NIM_MODEL.to_string()
        }
        (
            ProviderKind::NvidiaNim,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner"
            | "deepseek-r1" | "deepseek-v3" | "deepseek-v3.2",
        ) => DEFAULT_NVIDIA_NIM_FLASH_MODEL.to_string(),
        (ProviderKind::Openrouter, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_OPENROUTER_MODEL.to_string()
        }
        (
            ProviderKind::Openrouter,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner"
            | "deepseek-r1" | "deepseek-v3" | "deepseek-v3.2",
        ) => DEFAULT_OPENROUTER_FLASH_MODEL.to_string(),
        (ProviderKind::Novita, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_NOVITA_MODEL.to_string()
        }
        (
            ProviderKind::Novita,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner"
            | "deepseek-r1" | "deepseek-v3" | "deepseek-v3.2",
        ) => DEFAULT_NOVITA_FLASH_MODEL.to_string(),
        (ProviderKind::Fireworks, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_FIREWORKS_MODEL.to_string()
        }
        (
            ProviderKind::Siliconflow | ProviderKind::SiliconflowCN,
            "deepseek-v4-pro" | "deepseek-v4pro" | "deepseek-reasoner" | "deepseek-r1",
        ) => DEFAULT_SILICONFLOW_MODEL.to_string(),
        (
            ProviderKind::Siliconflow | ProviderKind::SiliconflowCN,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-v3",
        ) => DEFAULT_SILICONFLOW_FLASH_MODEL.to_string(),
        (
            ProviderKind::Arcee,
            "trinity" | "arcee-trinity" | "trinity-large-thinking" | "arcee-trinity-large-thinking",
        ) => DEFAULT_ARCEE_MODEL.to_string(),
        (ProviderKind::Arcee, "trinity-mini" | "arcee-trinity-mini") => {
            ARCEE_TRINITY_MINI_MODEL.to_string()
        }
        (ProviderKind::Arcee, "arcee-trinity-large-preview") => {
            ARCEE_TRINITY_LARGE_PREVIEW_MODEL.to_string()
        }
        (
            ProviderKind::Moonshot,
            "kimi"
            | "kimi-k2"
            | "kimi-k2.7"
            | "kimi-k2-7"
            | "kimi-k2.7-code"
            | "kimi-k2-7-code"
            | "kimi-code"
            | "moonshot-kimi-k2.7-code",
        ) => DEFAULT_MOONSHOT_MODEL.to_string(),
        (ProviderKind::Moonshot, "kimi-k2.6" | "kimi-k2-6" | "moonshot-kimi-k2.6") => {
            MOONSHOT_KIMI_K2_6_MODEL.to_string()
        }
        (ProviderKind::Sglang, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_SGLANG_MODEL.to_string()
        }
        (
            ProviderKind::Sglang,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner"
            | "deepseek-r1" | "deepseek-v3" | "deepseek-v3.2",
        ) => DEFAULT_SGLANG_FLASH_MODEL.to_string(),
        (ProviderKind::Vllm, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_VLLM_MODEL.to_string()
        }
        (
            ProviderKind::Vllm,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner"
            | "deepseek-r1" | "deepseek-v3" | "deepseek-v3.2",
        ) => DEFAULT_VLLM_FLASH_MODEL.to_string(),
        (ProviderKind::Huggingface, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_HUGGINGFACE_MODEL.to_string()
        }
        (
            ProviderKind::Huggingface,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner"
            | "deepseek-r1" | "deepseek-v3" | "deepseek-v3.2",
        ) => DEFAULT_HUGGINGFACE_FLASH_MODEL.to_string(),
        (ProviderKind::Together, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_TOGETHER_MODEL.to_string()
        }
        (
            ProviderKind::Together,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner"
            | "deepseek-r1" | "deepseek-v3" | "deepseek-v3.2",
        ) => DEFAULT_TOGETHER_FLASH_MODEL.to_string(),
        (ProviderKind::Deepinfra, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_DEEPINFRA_MODEL.to_string()
        }
        (
            ProviderKind::Deepinfra,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner"
            | "deepseek-r1" | "deepseek-v3" | "deepseek-v3.2",
        ) => DEFAULT_DEEPINFRA_FLASH_MODEL.to_string(),
        _ => model.to_string(),
    }
}

fn canonical_xiaomi_mimo_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "mimo"
        | DEFAULT_XIAOMI_MIMO_MODEL
        | "mimo-v2-5-pro"
        | "xiaomi-mimo-v2.5-pro"
        | "xiaomi-mimo-v2-5-pro" => Some(DEFAULT_XIAOMI_MIMO_MODEL),
        "omni"
        | "mimo-omni"
        | "v2.5-omni"
        | "v25-omni"
        | "mimo-v2.5"
        | "mimo-v25"
        | "mimo-v2-5"
        | "mimo-v2.5-omni"
        | "mimo-v25-omni"
        | "mimo-v2-5-omni"
        | "xiaomi-mimo-v2.5"
        | "xiaomi-mimo-v2-5"
        | "xiaomi-mimo-v2.5-omni"
        | "xiaomi-mimo-v2-5-omni" => Some(XIAOMI_MIMO_V2_5_OMNI_MODEL),
        "asr" | "mimo-asr" | "mimo-v2.5-asr" | "speech-to-text" | "transcribe" => {
            Some(XIAOMI_MIMO_ASR_MODEL)
        }
        "mimo-tts" | "mimo-v25-tts" | "mimo-v2.5-tts" | "tts" | "speech" => {
            Some(XIAOMI_MIMO_TTS_MODEL)
        }
        "mimo-tts-voicedesign"
        | "mimo-voice-design"
        | "mimo-v25-tts-voicedesign"
        | "mimo-v2.5-tts-voicedesign"
        | "voicedesign"
        | "voice-design" => Some(XIAOMI_MIMO_TTS_VOICE_DESIGN_MODEL),
        "mimo-tts-voiceclone"
        | "mimo-voice-clone"
        | "mimo-v25-tts-voiceclone"
        | "mimo-v2.5-tts-voiceclone"
        | "voiceclone"
        | "voice-clone" => Some(XIAOMI_MIMO_TTS_VOICE_CLONE_MODEL),
        "mimo-v2-tts" => Some(XIAOMI_MIMO_V2_TTS_MODEL),
        _ => None,
    }
}

fn canonical_minimax_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "minimax" | "minimax-m3" | "minimax-m-3" | "minimax-m-3-thinking" => {
            Some(DEFAULT_MINIMAX_MODEL)
        }
        "minimax-m2.7" | "minimax-m2-7" | "minimax-m-2.7" | "minimax-m-2-7" => {
            Some(MINIMAX_M2_7_MODEL)
        }
        "minimax-m2.7-highspeed"
        | "minimax-m2-7-highspeed"
        | "minimax-m-2.7-highspeed"
        | "minimax-m-2-7-highspeed" => Some(MINIMAX_M2_7_HIGHSPEED_MODEL),
        "minimax-m2.5" | "minimax-m2-5" | "minimax-m-2.5" | "minimax-m-2-5" => {
            Some(MINIMAX_M2_5_MODEL)
        }
        "minimax-m2.5-highspeed"
        | "minimax-m2-5-highspeed"
        | "minimax-m-2.5-highspeed"
        | "minimax-m-2-5-highspeed" => Some(MINIMAX_M2_5_HIGHSPEED_MODEL),
        "minimax-m2.1" | "minimax-m2-1" | "minimax-m-2.1" | "minimax-m-2-1" => {
            Some(MINIMAX_M2_1_MODEL)
        }
        "minimax-m2.1-highspeed"
        | "minimax-m2-1-highspeed"
        | "minimax-m-2.1-highspeed"
        | "minimax-m-2-1-highspeed" => Some(MINIMAX_M2_1_HIGHSPEED_MODEL),
        "minimax-m2" | "minimax-m-2" => Some(MINIMAX_M2_MODEL),
        _ => None,
    }
}

fn canonical_zai_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "glm-5.1" | "glm-5-1" | "zai-glm-5.1" | "zai-glm-5-1" => Some(ZAI_GLM_5_1_MODEL),
        "glm-5.2" | "glm-5-2" | "zai-glm-5.2" | "zai-glm-5-2" => Some(DEFAULT_ZAI_MODEL),
        "glm-5-turbo" | "glm-5turbo" | "zai-glm-5-turbo" => Some(ZAI_GLM_5_TURBO_MODEL),
        _ => None,
    }
}

fn canonical_openrouter_recent_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL
        | "trinity"
        | "trinity-large-thinking"
        | "arcee-trinity"
        | "arcee-trinity-large-thinking" => Some(OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL),
        OPENROUTER_GEMMA_4_31B_MODEL | "gemma-4-31b" | "gemma-4-31b-it" => {
            Some(OPENROUTER_GEMMA_4_31B_MODEL)
        }
        OPENROUTER_GEMMA_4_26B_A4B_MODEL | "gemma-4-26b-a4b" | "gemma-4-26b-a4b-it" => {
            Some(OPENROUTER_GEMMA_4_26B_A4B_MODEL)
        }
        OPENROUTER_GLM_5_1_MODEL | "glm-5.1" | "glm-5-1" | "zai-glm-5.1" | "zai-glm-5-1" => {
            Some(OPENROUTER_GLM_5_1_MODEL)
        }
        OPENROUTER_GLM_5_2_MODEL | "glm-5.2" | "glm-5-2" | "zai-glm-5.2" | "zai-glm-5-2" => {
            Some(OPENROUTER_GLM_5_2_MODEL)
        }
        OPENROUTER_KIMI_K2_7_CODE_MODEL
        | "kimi"
        | "kimi-k2"
        | "kimi-k2.7"
        | "kimi-k2-7"
        | "kimi-k2.7-code"
        | "kimi-k2-7-code"
        | "kimi-code"
        | "moonshot-kimi-k2.7-code"
        | "openrouter-kimi-k2.7-code" => Some(OPENROUTER_KIMI_K2_7_CODE_MODEL),
        OPENROUTER_KIMI_K2_6_MODEL | "kimi-k2.6" | "kimi-k2-6" | "moonshot-kimi-k2.6" => {
            Some(OPENROUTER_KIMI_K2_6_MODEL)
        }
        OPENROUTER_MINIMAX_M3_MODEL | "minimax-m3" | "minimax-m-3" => {
            Some(OPENROUTER_MINIMAX_M3_MODEL)
        }
        OPENROUTER_MINIMAX_2_7_MODEL
        | "minimax-2.7"
        | "minimax-2-7"
        | "minimax-m2.7"
        | "minimax-m2-7"
        | "minimax-m-2.7"
        | "minimax-m-2-7" => Some(OPENROUTER_MINIMAX_2_7_MODEL),
        OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL
        | "nemotron-3-nano-omni"
        | "nemotron-3-nano-omni-reasoning" => Some(OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL),
        OPENROUTER_QWEN_3_6_35B_A3B_MODEL
        | "qwen3.6-35b-a3b"
        | "qwen-3.6-35b-a3b"
        | "qwen3-6-35b-a3b" => Some(OPENROUTER_QWEN_3_6_35B_A3B_MODEL),
        OPENROUTER_QWEN_3_6_FLASH_MODEL | "qwen3.6-flash" | "qwen-3.6-flash" => {
            Some(OPENROUTER_QWEN_3_6_FLASH_MODEL)
        }
        OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL
        | "qwen3.6-max-preview"
        | "qwen-3.6-max-preview"
        | "qwen-max-preview" => Some(OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL),
        OPENROUTER_QWEN_3_6_27B_MODEL | "qwen3.6-27b" | "qwen-3.6-27b" | "qwen3-6-27b" => {
            Some(OPENROUTER_QWEN_3_6_27B_MODEL)
        }
        OPENROUTER_QWEN_3_6_PLUS_MODEL | "qwen3.6-plus" | "qwen-3.6-plus" => {
            Some(OPENROUTER_QWEN_3_6_PLUS_MODEL)
        }
        OPENROUTER_QWEN_3_7_MAX_MODEL | "qwen3.7-max" | "qwen-3.7-max" => {
            Some(OPENROUTER_QWEN_3_7_MAX_MODEL)
        }
        OPENROUTER_TENCENT_HY3_PREVIEW_MODEL | "hy3-preview" | "tencent-hy3-preview" => {
            Some(OPENROUTER_TENCENT_HY3_PREVIEW_MODEL)
        }
        OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL
        | "mimo-v2.5-pro"
        | "mimo-v2-5-pro"
        | "xiaomi-mimo-v2.5-pro"
        | "xiaomi-mimo-v2-5-pro" => Some(OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL),
        OPENROUTER_XIAOMI_MIMO_V2_5_MODEL
        | "mimo-v2.5"
        | "mimo-v2-5"
        | "xiaomi-mimo-v2.5"
        | "xiaomi-mimo-v2-5" => Some(OPENROUTER_XIAOMI_MIMO_V2_5_MODEL),
        _ => None,
    }
}

fn default_model_for_provider(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Deepseek => DEFAULT_DEEPSEEK_MODEL,
        ProviderKind::NvidiaNim => DEFAULT_NVIDIA_NIM_MODEL,
        ProviderKind::Openai => DEFAULT_OPENAI_MODEL,
        ProviderKind::Atlascloud => DEFAULT_ATLASCLOUD_MODEL,
        ProviderKind::WanjieArk => DEFAULT_WANJIE_ARK_MODEL,
        ProviderKind::Volcengine => DEFAULT_VOLCENGINE_MODEL,
        ProviderKind::Openrouter => DEFAULT_OPENROUTER_MODEL,
        ProviderKind::XiaomiMimo => DEFAULT_XIAOMI_MIMO_MODEL,
        ProviderKind::Novita => DEFAULT_NOVITA_MODEL,
        ProviderKind::Fireworks => DEFAULT_FIREWORKS_MODEL,
        ProviderKind::Siliconflow | ProviderKind::SiliconflowCN => DEFAULT_SILICONFLOW_MODEL,
        ProviderKind::Arcee => DEFAULT_ARCEE_MODEL,
        ProviderKind::Moonshot => DEFAULT_MOONSHOT_MODEL,
        ProviderKind::Sglang => DEFAULT_SGLANG_MODEL,
        ProviderKind::Vllm => DEFAULT_VLLM_MODEL,
        ProviderKind::Ollama => DEFAULT_OLLAMA_MODEL,
        ProviderKind::Huggingface => DEFAULT_HUGGINGFACE_MODEL,
        ProviderKind::Together => DEFAULT_TOGETHER_MODEL,
        ProviderKind::OpenaiCodex => DEFAULT_OPENAI_CODEX_MODEL,
        ProviderKind::Anthropic => DEFAULT_ANTHROPIC_MODEL,
        ProviderKind::Zai => DEFAULT_ZAI_MODEL,
        ProviderKind::Stepfun => DEFAULT_STEPFUN_MODEL,
        ProviderKind::Minimax => DEFAULT_MINIMAX_MODEL,
        ProviderKind::Deepinfra => DEFAULT_DEEPINFRA_MODEL,
    }
}

fn default_base_url_for_provider(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Deepseek => DEFAULT_DEEPSEEK_BASE_URL,
        ProviderKind::NvidiaNim => DEFAULT_NVIDIA_NIM_BASE_URL,
        ProviderKind::Openai => DEFAULT_OPENAI_BASE_URL,
        ProviderKind::Atlascloud => DEFAULT_ATLASCLOUD_BASE_URL,
        ProviderKind::WanjieArk => DEFAULT_WANJIE_ARK_BASE_URL,
        ProviderKind::Volcengine => DEFAULT_VOLCENGINE_BASE_URL,
        ProviderKind::Openrouter => DEFAULT_OPENROUTER_BASE_URL,
        ProviderKind::XiaomiMimo => DEFAULT_XIAOMI_MIMO_BASE_URL,
        ProviderKind::Novita => DEFAULT_NOVITA_BASE_URL,
        ProviderKind::Fireworks => DEFAULT_FIREWORKS_BASE_URL,
        ProviderKind::Siliconflow => DEFAULT_SILICONFLOW_BASE_URL,
        ProviderKind::SiliconflowCN => DEFAULT_SILICONFLOW_CN_BASE_URL,
        ProviderKind::Arcee => DEFAULT_ARCEE_BASE_URL,
        ProviderKind::Moonshot => DEFAULT_MOONSHOT_BASE_URL,
        ProviderKind::Sglang => DEFAULT_SGLANG_BASE_URL,
        ProviderKind::Vllm => DEFAULT_VLLM_BASE_URL,
        ProviderKind::Ollama => DEFAULT_OLLAMA_BASE_URL,
        ProviderKind::Huggingface => DEFAULT_HUGGINGFACE_BASE_URL,
        ProviderKind::Together => DEFAULT_TOGETHER_BASE_URL,
        ProviderKind::OpenaiCodex => DEFAULT_OPENAI_CODEX_BASE_URL,
        ProviderKind::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
        ProviderKind::Zai => DEFAULT_ZAI_BASE_URL,
        ProviderKind::Stepfun => DEFAULT_STEPFUN_BASE_URL,
        ProviderKind::Minimax => DEFAULT_MINIMAX_BASE_URL,
        ProviderKind::Deepinfra => DEFAULT_DEEPINFRA_BASE_URL,
    }
}

fn moonshot_base_url_uses_kimi_code(base_url: &str) -> bool {
    let normalized = base_url.trim_end_matches('/').to_ascii_lowercase();
    normalized == DEFAULT_KIMI_CODE_BASE_URL
        || normalized == "https://api.kimi.com/coding"
        || normalized.starts_with("https://api.kimi.com/coding/")
}

fn xiaomi_mimo_base_url_for_mode(mode: &str) -> Option<&'static str> {
    let normalized = mode.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    if normalized.is_empty() || xiaomi_mimo_mode_uses_standard_endpoint(&normalized) {
        return None;
    }
    Some(match normalized.as_str() {
        "token-plan" | "tokenplan" | "subscription" | "subscribed" | "plan" => {
            DEFAULT_XIAOMI_MIMO_BASE_URL
        }
        "token-plan-cn"
        | "token-plan-china"
        | "token-plan-mainland"
        | "token-plan-mainland-china"
        | "cn"
        | "china" => XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL,
        "token-plan-sgp"
        | "token-plan-sg"
        | "token-plan-singapore"
        | "sgp"
        | "sg"
        | "singapore" => XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL,
        "token-plan-ams"
        | "token-plan-eu"
        | "token-plan-europe"
        | "token-plan-amsterdam"
        | "ams"
        | "eu"
        | "europe"
        | "amsterdam" => XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL,
        _ => DEFAULT_XIAOMI_MIMO_BASE_URL,
    })
}

fn xiaomi_mimo_mode_uses_standard_endpoint(normalized_mode: &str) -> bool {
    matches!(
        normalized_mode,
        "standard" | "default" | "payg" | "paygo" | "pay-as-you-go" | "pay-as-go"
    )
}

fn xiaomi_mimo_base_url_uses_token_plan(base_url: &str) -> bool {
    let normalized = base_url.trim_end_matches('/').to_ascii_lowercase();
    normalized == XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL
        || normalized == XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL
        || normalized == XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL
}

fn xiaomi_mimo_env_var(candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

fn xiaomi_mimo_env_api_key_for_runtime(
    mode: Option<&str>,
    base_url: Option<&str>,
) -> Option<String> {
    const TOKEN_PLAN_ENV_VARS: &[&str] =
        &["XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "MIMO_TOKEN_PLAN_API_KEY"];
    const STANDARD_ENV_VARS: &[&str] = &["XIAOMI_MIMO_API_KEY", "XIAOMI_API_KEY", "MIMO_API_KEY"];

    let normalized_mode =
        mode.map(|value| value.trim().to_ascii_lowercase().replace(['_', ' '], "-"));
    let standard_selected = normalized_mode
        .as_deref()
        .is_some_and(xiaomi_mimo_mode_uses_standard_endpoint)
        || base_url.is_some_and(xiaomi_mimo_base_url_is_pay_as_you_go);
    if standard_selected {
        return xiaomi_mimo_env_var(STANDARD_ENV_VARS);
    }

    let token_plan_selected = normalized_mode
        .as_deref()
        .and_then(xiaomi_mimo_base_url_for_mode)
        .is_some()
        || base_url.is_some_and(xiaomi_mimo_base_url_uses_token_plan);
    if token_plan_selected {
        return xiaomi_mimo_env_var(TOKEN_PLAN_ENV_VARS);
    }

    xiaomi_mimo_env_var(TOKEN_PLAN_ENV_VARS).or_else(|| xiaomi_mimo_env_var(STANDARD_ENV_VARS))
}

fn resolve_xiaomi_mimo_base_url(
    configured: Option<String>,
    api_key: Option<&str>,
    mode: Option<&str>,
) -> String {
    let normalized_mode =
        mode.map(|value| value.trim().to_ascii_lowercase().replace(['_', ' '], "-"));
    let uses_standard_mode = normalized_mode
        .as_deref()
        .is_some_and(xiaomi_mimo_mode_uses_standard_endpoint);
    let mode_base_url = normalized_mode
        .as_deref()
        .and_then(xiaomi_mimo_base_url_for_mode);
    let uses_token_plan = xiaomi_mimo_api_key_uses_token_plan(api_key);
    match configured {
        Some(base_url) if uses_standard_mode => base_url,
        Some(base_url) if uses_token_plan && xiaomi_mimo_base_url_is_pay_as_you_go(&base_url) => {
            mode_base_url
                .unwrap_or(DEFAULT_XIAOMI_MIMO_BASE_URL)
                .to_string()
        }
        Some(base_url) => base_url,
        None => {
            if let Some(base_url) = mode_base_url {
                base_url.to_string()
            } else if uses_standard_mode {
                XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL.to_string()
            } else if uses_token_plan || api_key.is_none() {
                DEFAULT_XIAOMI_MIMO_BASE_URL.to_string()
            } else {
                XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL.to_string()
            }
        }
    }
}

fn xiaomi_mimo_api_key_uses_token_plan(api_key: Option<&str>) -> bool {
    api_key.is_some_and(|key| key.trim_start().starts_with("tp-"))
}

fn xiaomi_mimo_base_url_is_pay_as_you_go(base_url: &str) -> bool {
    matches!(
        base_url.trim_end_matches('/').to_ascii_lowercase().as_str(),
        "https://api.xiaomimimo.com" | "https://api.xiaomimimo.com/v1"
    )
}

fn base_url_is_custom_for_provider(provider: ProviderKind, base_url: &str) -> bool {
    if provider.is_siliconflow() && siliconflow_base_url_is_official(base_url) {
        return false;
    }
    if provider == ProviderKind::XiaomiMimo
        && (xiaomi_mimo_base_url_uses_token_plan(base_url)
            || xiaomi_mimo_base_url_is_pay_as_you_go(base_url))
    {
        return false;
    }
    let actual = base_url.trim_end_matches('/');
    let default = default_base_url_for_provider(provider).trim_end_matches('/');
    actual != default
}

fn siliconflow_base_url_is_official(base_url: &str) -> bool {
    matches!(
        base_url.trim_end_matches('/').to_ascii_lowercase().as_str(),
        "https://api.siliconflow.com/v1" | "https://api.siliconflow.cn/v1"
    )
}

fn provider_preserves_custom_base_url_model(provider: ProviderKind, base_url: &str) -> bool {
    base_url_is_custom_for_provider(provider, base_url)
}

fn should_skip_secret_store_for_provider(
    provider: ProviderKind,
    base_url: &str,
    auth_mode: Option<&str>,
) -> bool {
    if auth_mode_requires_api_key(auth_mode) {
        return false;
    }
    if auth_mode_disables_api_key(auth_mode) {
        return true;
    }

    matches!(
        provider,
        ProviderKind::Sglang | ProviderKind::Vllm | ProviderKind::Ollama
    ) || base_url_uses_local_host(base_url)
}

fn env_api_key_for_provider(provider: ProviderKind) -> Option<String> {
    if provider == ProviderKind::Huggingface {
        return std::env::var("HUGGINGFACE_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var("HF_TOKEN")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            });
    }

    codewhale_secrets::env_for(provider.as_str())
}

fn auth_mode_requires_api_key(auth_mode: Option<&str>) -> bool {
    matches!(
        auth_mode
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
        Some(value)
            if matches!(
                value.as_str(),
                "api_key" | "api-key" | "apikey" | "bearer" | "bearer-token"
            )
    )
}

fn auth_mode_disables_api_key(auth_mode: Option<&str>) -> bool {
    matches!(
        auth_mode
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase()),
        Some(value)
            if matches!(
                value.as_str(),
                "none" | "off" | "disabled" | "no_auth" | "no-auth" | "anonymous"
            )
    )
}

fn auth_mode_uses_kimi_oauth(auth_mode: &str) -> bool {
    matches!(
        auth_mode
            .trim()
            .to_ascii_lowercase()
            .replace('-', "_")
            .as_str(),
        "kimi" | "kimi_oauth" | "kimi_cli" | "oauth"
    )
}

fn base_url_uses_local_host(base_url: &str) -> bool {
    let Some(host) = base_url_host(base_url) else {
        return false;
    };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if matches!(host.as_str(), "localhost" | "0.0.0.0") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|addr| addr.is_loopback() || addr.is_unspecified())
}

fn base_url_host(base_url: &str) -> Option<&str> {
    let without_scheme = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let authority = without_scheme.split('/').next()?.rsplit('@').next()?;
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split_once(']').map(|(host, _)| host);
    }
    authority.split(':').next().filter(|host| !host.is_empty())
}

#[derive(Debug, Clone, Default)]
pub struct CliRuntimeOverrides {
    pub provider: Option<ProviderKind>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub auth_mode: Option<String>,
    pub output_mode: Option<String>,
    pub log_level: Option<String>,
    pub telemetry: Option<bool>,
    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
    pub yolo: Option<bool>,
    pub verbosity: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeApiKeySource {
    Cli,
    ConfigFile,
    Keyring,
    Env,
}

impl RuntimeApiKeySource {
    #[must_use]
    pub fn as_env_value(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::ConfigFile => "config",
            Self::Keyring => "keyring",
            Self::Env => "env",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSource {
    Cli,
    Env(&'static str),
    Config,
}

#[derive(Debug, Clone)]
pub struct ResolvedRuntimeOptions {
    pub provider: ProviderKind,
    pub provider_source: ProviderSource,
    pub model: String,
    pub api_key: Option<String>,
    pub api_key_source: Option<RuntimeApiKeySource>,
    pub base_url: String,
    pub auth_mode: Option<String>,
    pub insecure_skip_tls_verify: bool,
    pub output_mode: Option<String>,
    pub log_level: Option<String>,
    pub telemetry: bool,
    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
    pub yolo: Option<bool>,
    pub verbosity: Option<String>,
    pub http_headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    pub config: ConfigToml,
    permissions: PermissionsToml,
    /// Original file text, retained so [`save`](Self::save) can merge
    /// comments back after serialisation.
    original_raw: Option<String>,
}

impl ConfigStore {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = resolve_config_path(path)?;
        let (config, original_raw) = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read config at {}", path.display()))?;
            let parsed: ConfigToml = toml::from_str(&raw)
                .with_context(|| format!("failed to parse config at {}", path.display()))?;
            (parsed, Some(raw))
        } else {
            (ConfigToml::default(), None)
        };
        let permissions = load_sibling_permissions(&path)?;

        Ok(Self {
            path,
            config,
            permissions,
            original_raw,
        })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }
        let body = if let Some(ref original_raw) = self.original_raw {
            let serialized =
                toml::to_string_pretty(&self.config).context("failed to serialize config")?;
            merge_and_preserve_comments(&serialized, original_raw).unwrap_or_else(|e| {
                tracing::warn!("failed to merge config comments, saving without them: {e:#}");
                serialized
            })
        } else {
            toml::to_string_pretty(&self.config).context("failed to serialize config")?
        };
        match fs::read_to_string(&self.path) {
            Ok(existing) => {
                if existing == body {
                    return Ok(());
                }
                write_one_time_config_backup(&self.path)?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read config at {}", self.path.display()));
            }
        }
        #[cfg(unix)]
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&self.path)
                .with_context(|| format!("failed to write config at {}", self.path.display()))?;
            file.write_all(body.as_bytes())
                .with_context(|| format!("failed to write config at {}", self.path.display()))?;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .with_context(|| {
                    format!(
                        "failed to set config permissions at {}",
                        self.path.display()
                    )
                })?;
        }
        #[cfg(not(unix))]
        {
            fs::write(&self.path, body)
                .with_context(|| format!("failed to write config at {}", self.path.display()))?;
        }
        Ok(())
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn permissions(&self) -> &PermissionsToml {
        &self.permissions
    }

    #[must_use]
    pub fn permissions_path(&self) -> PathBuf {
        permissions_path_for_config_path(&self.path)
    }

    #[must_use]
    pub fn exec_policy_engine(&self) -> ExecPolicyEngine {
        if self.permissions.is_empty() {
            ExecPolicyEngine::new(Vec::new(), Vec::new())
        } else {
            ExecPolicyEngine::with_rulesets(vec![self.permissions.ruleset()])
        }
    }

    /// Atomically append ask-only permission rules to the sibling
    /// `permissions.toml` file.
    ///
    /// Existing comments and formatting are preserved. Exact duplicate rules
    /// are ignored, and the in-memory permissions snapshot is refreshed after
    /// a successful write.
    pub fn append_ask_rules(&mut self, rules: &[ToolAskRule]) -> Result<usize> {
        if rules.is_empty() {
            return Ok(0);
        }

        let path = self.permissions_path();
        let raw = if path.exists() {
            fs::read_to_string(&path)
                .with_context(|| format!("failed to read permissions at {}", path.display()))?
        } else {
            String::new()
        };
        let mut permissions = if raw.trim().is_empty() {
            PermissionsToml::default()
        } else {
            toml::from_str(&raw)
                .with_context(|| format!("failed to parse permissions at {}", path.display()))?
        };
        let mut document = if raw.trim().is_empty() {
            toml_edit::DocumentMut::new()
        } else {
            raw.parse::<toml_edit::DocumentMut>()
                .with_context(|| format!("failed to edit permissions at {}", path.display()))?
        };

        if !document.contains_key("rules") {
            document["rules"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
        }
        let rules_item = document
            .get_mut("rules")
            .expect("rules entry was inserted above");

        let mut added = 0;
        for rule in rules {
            if permissions.rules.contains(rule) {
                continue;
            }
            append_ask_rule(rules_item, rule)?;
            permissions.rules.push(rule.clone());
            added += 1;
        }
        if added == 0 {
            self.permissions = permissions;
            return Ok(0);
        }

        let body = document.to_string();
        let persisted: PermissionsToml = toml::from_str(&body).with_context(|| {
            format!(
                "generated invalid permissions document for {}",
                path.display()
            )
        })?;
        write_permissions_atomic(&path, body.as_bytes())?;
        self.permissions = persisted;
        Ok(added)
    }
}

fn config_backup_path(path: &Path) -> PathBuf {
    let mut file_name = path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| std::ffi::OsString::from(CONFIG_FILE_NAME));
    file_name.push(".bak");
    path.with_file_name(file_name)
}

fn write_one_time_config_backup(path: &Path) -> Result<()> {
    let backup = config_backup_path(path);
    if backup.exists() {
        return Ok(());
    }
    fs::copy(path, &backup).with_context(|| {
        format!(
            "failed to create config backup {} from {}",
            backup.display(),
            path.display()
        )
    })?;
    #[cfg(unix)]
    {
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o600)).with_context(|| {
            format!(
                "failed to set config backup permissions at {}",
                backup.display()
            )
        })?;
    }
    Ok(())
}

/// Merge comments and formatting from an original TOML file into a
/// freshly serialized document so user annotations (comments, whitespace,
/// disabled keys) survive config rewrites.
///
/// `original_raw` is the raw text of the file before the change; the
/// function parses it internally with [`toml_edit`] so callers stay free
/// of that dependency.
pub fn merge_and_preserve_comments(serialized: &str, original_raw: &str) -> Result<String> {
    let original = original_raw
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse original config for comment merge")?;

    let mut new_doc = serialized
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse serialized config for comment merge")?;

    // Reuse the original document’s trailing text (file-footer comments /
    // disabled keys) so they survive the rewrite.
    new_doc.set_trailing(original.trailing().clone());

    // Copy the top-level table's decor (document-header comments, whitespace
    // before the first key) which `toml_edit` stores on the root `Table` itself.
    *new_doc.as_table_mut().decor_mut() = original.as_table().decor().clone();

    merge_decor_table(new_doc.as_table_mut(), original.as_table());

    Ok(new_doc.to_string())
}

/// Recursively copy `decor` (prefix/suffix comments and whitespace) from
/// every key in `source` that also exists in `target`.
fn merge_decor_table(target: &mut toml_edit::Table, source: &toml_edit::Table) {
    // Collect keys first — the borrow checker won't let us hold
    // `get_key_value_mut` while iterating.
    let keys: Vec<String> = source.iter().map(|(k, _)| k.to_owned()).collect();
    for key in &keys {
        let Some((source_key, source_item)) = source.get_key_value(key) else {
            continue;
        };
        let Some((mut target_key_mut, target_item)) = target.get_key_value_mut(key) else {
            continue;
        };

        // Copy the key-level decor (comments before the key itself)
        *target_key_mut.leaf_decor_mut() = source_key.leaf_decor().clone();

        copy_item_decor(target_item, source_item);

        if let (Some(tt), Some(st)) = (target_item.as_table_mut(), source_item.as_table()) {
            merge_decor_table(tt, st);
        }

        if let (Some(ta), Some(sa)) = (
            target_item.as_array_of_tables_mut(),
            source_item.as_array_of_tables(),
        ) {
            for (i, source_table) in sa.iter().enumerate() {
                if let Some(target_table) = ta.get_mut(i) {
                    copy_item_decor_table(target_table, source_table);
                    merge_decor_table(target_table, source_table);
                }
            }
        }
    }
}

/// Copy the decor (comments and surrounding whitespace) from `source` to `target`,
/// respecting the concrete item type since [`toml_edit::Item`] has no uniform
/// `decor` accessor.
fn copy_item_decor(target: &mut toml_edit::Item, source: &toml_edit::Item) {
    match (target, source) {
        (toml_edit::Item::Table(tt), toml_edit::Item::Table(st)) => {
            *tt.decor_mut() = st.decor().clone();
        }
        (toml_edit::Item::Value(tv), toml_edit::Item::Value(sv)) => {
            *tv.decor_mut() = sv.decor().clone();
        }
        _ => {}
    }
}

fn copy_item_decor_table(target: &mut toml_edit::Table, source: &toml_edit::Table) {
    *target.decor_mut() = source.decor().clone();
}

/// Process-wide default [`Secrets`] façade. The first caller wins; the
/// lock is exposed so test or CLI code can install an explicit
/// backend (e.g. an [`codewhale_secrets::InMemoryKeyringStore`]) before
/// any resolver runs.
pub fn default_secrets() -> &'static Secrets {
    static SECRETS: OnceLock<Secrets> = OnceLock::new();
    SECRETS.get_or_init(|| {
        // Tests should never poke real platform credential stores. Cargo sets the
        // `RUST_TEST_*` family of env vars (and `CARGO_PKG_NAME` is
        // always populated), but the `cfg(test)` flag is the canonical
        // signal here. See `install_test_secrets` for explicit installs.
        #[cfg(test)]
        {
            Secrets::new(std::sync::Arc::new(
                codewhale_secrets::InMemoryKeyringStore::new(),
            ))
        }
        #[cfg(not(test))]
        {
            Secrets::auto_detect()
        }
    })
}

// ── CodeWhale state root (v0.8.44) ──────────────────────────────────
//
// v0.8.44 migrates product-owned app state from ~/.deepseek/ to
// ~/.codewhale/ while keeping ~/.deepseek/ as a compatibility fallback.
// New installs write to ~/.codewhale/. Existing installs with only
// ~/.deepseek/ continue working without data loss.

/// Canonical CodeWhale app directory name under $HOME.
pub const CODEWHALE_APP_DIR: &str = ".codewhale";

/// Legacy DeepSeek-branded app directory name (compatibility fallback).
pub const LEGACY_APP_DIR: &str = ".deepseek";

/// Resolve the primary CodeWhale home directory.
///
/// `$CODEWHALE_HOME` takes precedence when set. Otherwise defaults to
/// `$HOME/.codewhale`. This is the write target for new product state.
pub fn codewhale_home() -> Result<PathBuf> {
    if let Ok(val) = std::env::var("CODEWHALE_HOME") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home = effective_home_dir().context("failed to resolve home directory")?;
    Ok(home.join(CODEWHALE_APP_DIR))
}

/// Resolve the legacy DeepSeek home directory (`$HOME/.deepseek`).
///
/// Always returns the legacy path regardless of whether it exists.
pub fn legacy_deepseek_home() -> Result<PathBuf> {
    let home = effective_home_dir().context("failed to resolve home directory")?;
    Ok(home.join(LEGACY_APP_DIR))
}

fn effective_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// Reject state subdirs that could escape the state root via path injection.
///
/// `ensure_state_dir` / `resolve_state_dir` are public APIs taking an arbitrary
/// subdir string; every in-tree caller passes a hardcoded single component
/// (e.g. `"sessions"`, `"."`). This validates defensively so a future caller
/// can never traverse out of the state root via `..` components or an absolute
/// path. Nested relative paths such as `"a/b"` are permitted.
fn ensure_safe_state_subdir(subdir: &str) -> Result<()> {
    if subdir.is_empty() {
        bail!("state subdir must not be empty");
    }
    let path = std::path::Path::new(subdir);
    if path.is_absolute() {
        bail!("state subdir must not be an absolute path: {subdir}");
    }
    if path.components().any(|c| {
        matches!(
            c,
            std::path::Component::RootDir | std::path::Component::Prefix(_)
        )
    }) {
        bail!("state subdir must not contain a root or prefix: {subdir}");
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("state subdir must not contain parent-dir (..) components: {subdir}");
    }
    Ok(())
}

/// Resolve a state subdirectory, preferring the CodeWhale root if
/// it already exists, otherwise falling back to the legacy root.
///
/// This is the read-path resolver: it returns the primary path when
/// migration has occurred or on a fresh install, but keeps reading
/// from the legacy path for users who haven't migrated yet.
pub fn resolve_state_dir(subdir: &str) -> Result<PathBuf> {
    ensure_safe_state_subdir(subdir)?;
    let primary = codewhale_home()?.join(subdir);
    if primary.exists() {
        return Ok(primary);
    }
    let legacy = legacy_deepseek_home()?.join(subdir);
    if legacy.exists() {
        return Ok(legacy);
    }
    // Neither exists — return primary for first-write creation.
    Ok(primary)
}

/// Ensure a state subdirectory exists under the primary CodeWhale root,
/// creating it if necessary. This is the write-path resolver.
///
/// On the first creation of a real subdirectory (not the root sentinel `"."`),
/// if a legacy `~/.deepseek/<subdir>` exists but the primary
/// `~/.codewhale/<subdir>` does not, the legacy directory is relocated into
/// the primary location so the user keeps their data and the legacy tree
/// stops growing (#3240). After migration, [`resolve_state_dir`] finds the
/// data in the primary location; the read resolver itself is unchanged.
pub fn ensure_state_dir(subdir: &str) -> Result<PathBuf> {
    ensure_safe_state_subdir(subdir)?;
    let dir = codewhale_home()?.join(subdir);
    migrate_legacy_state_dir(&dir, subdir)?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}/", dir.display()))?;
    Ok(dir)
}

/// One-time relocation of a legacy `~/.deepseek/<subdir>` state directory into
/// the primary `~/.codewhale/<subdir>` location (#3240). No-op once the primary
/// exists, for the root sentinel `"."` (a whole-tree move is owned by the
/// config-file migration), or when no legacy directory is present.
fn migrate_legacy_state_dir(primary: &Path, subdir: &str) -> Result<()> {
    if primary.exists() || subdir == "." || subdir.is_empty() {
        return Ok(());
    }
    let legacy = match legacy_deepseek_home() {
        Ok(home) => home.join(subdir),
        Err(_) => return Ok(()),
    };
    if !legacy.exists() {
        return Ok(());
    }
    // The primary's parent (the ~/.codewhale root) must exist for the rename.
    if let Some(parent) = primary.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                target: "config::migration",
                "Could not create {} for state migration ({}); writing to primary anyway",
                parent.display(),
                err
            );
        }
    }
    match std::fs::rename(&legacy, primary) {
        Ok(()) => {
            tracing::info!(
                target: "config::migration",
                "Migrated legacy state directory {} -> {} (relocated). The .deepseek copy was removed.",
                legacy.display(),
                primary.display()
            );
        }
        Err(err) => {
            // Cross-device rename or permission issue: fall back to a
            // recursive copy so the user keeps their data. The legacy tree is
            // left in place; it stops growing because writes now target the
            // primary path.
            match copy_dir_recursive(&legacy, primary) {
                Ok(()) => {
                    tracing::info!(
                        target: "config::migration",
                        "Migrated legacy state directory {} -> {} (copied; rename failed: {err}). \
                         The legacy .deepseek copy was left in place.",
                        legacy.display(),
                        primary.display()
                    );
                }
                Err(copy_err) => {
                    tracing::warn!(
                        target: "config::migration",
                        "Could not migrate legacy state {} -> {} (rename: {err}; copy: {copy_err}). \
                         New data is written to the primary path; the legacy tree remains untouched.",
                        legacy.display(),
                        primary.display()
                    );
                }
            }
        }
    }
    Ok(())
}

/// Recursively copy a directory tree from `src` to `dst`, creating `dst`.
/// Symlinks and other non-file/non-dir entries are skipped (rare in state dirs).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in
        std::fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry in {}", src.display()))?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type for {}", path.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else if file_type.is_file() {
            std::fs::copy(&path, &target).with_context(|| {
                format!("failed to copy {} -> {}", path.display(), target.display())
            })?;
        }
    }
    Ok(())
}

/// Resolve a project-local state subdirectory, preferring `.codewhale/`
/// when it exists, falling back to `.deepseek/` for legacy projects.
///
/// Returns `(true, path)` when the primary `.codewhale/` path is used,
/// `(false, path)` for the legacy fallback. The boolean helps callers
/// emit a deprecation notice on legacy paths.
pub fn resolve_project_state_dir(workspace: &Path, subdir: &str) -> (bool, PathBuf) {
    let primary = workspace.join(CODEWHALE_APP_DIR).join(subdir);
    if primary.exists() {
        return (true, primary);
    }
    let legacy = workspace.join(LEGACY_APP_DIR).join(subdir);
    (false, legacy)
}

/// Ensure a project-local state subdirectory exists under `.codewhale/`,
/// creating it if necessary. Returns the directory path.
pub fn ensure_project_state_dir(workspace: &Path, subdir: &str) -> Result<PathBuf> {
    let dir = workspace.join(CODEWHALE_APP_DIR).join(subdir);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}/", dir.display()))?;
    Ok(dir)
}

pub fn resolve_config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let path = if let Some(path) = explicit {
        path
    } else if let Ok(path) = std::env::var("CODEWHALE_CONFIG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            PathBuf::from(trimmed)
        } else {
            return default_config_path();
        }
    } else if let Ok(path) = std::env::var("DEEPSEEK_CONFIG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            PathBuf::from(trimmed)
        } else {
            return default_config_path();
        }
    } else {
        return default_config_path();
    };
    normalize_config_file_path(path)
}

#[must_use]
pub fn permissions_path_for_config_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name(PERMISSIONS_FILE_NAME)
}

pub fn resolve_permissions_path(config_path: Option<PathBuf>) -> Result<PathBuf> {
    Ok(permissions_path_for_config_path(&resolve_config_path(
        config_path,
    )?))
}

fn load_sibling_permissions(config_path: &Path) -> Result<PermissionsToml> {
    let permissions_path = permissions_path_for_config_path(config_path);
    if !permissions_path.exists() {
        return Ok(PermissionsToml::default());
    }

    let raw = fs::read_to_string(&permissions_path).with_context(|| {
        format!(
            "failed to read permissions at {}",
            permissions_path.display()
        )
    })?;
    toml::from_str(&raw).with_context(|| {
        format!(
            "failed to parse permissions at {}",
            permissions_path.display()
        )
    })
}

fn append_ask_rule(item: &mut toml_edit::Item, rule: &ToolAskRule) -> Result<()> {
    match item {
        toml_edit::Item::ArrayOfTables(rules) => {
            rules.push(ask_rule_table(rule));
            Ok(())
        }
        toml_edit::Item::Value(value) => {
            let Some(rules) = value.as_array_mut() else {
                bail!("`rules` in permissions.toml must be an array");
            };
            rules.push(toml_edit::Value::InlineTable(ask_rule_inline_table(rule)));
            Ok(())
        }
        _ => bail!("`rules` in permissions.toml must be an array"),
    }
}

fn ask_rule_table(rule: &ToolAskRule) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    table["tool"] = toml_edit::value(rule.tool.clone());
    if let Some(command) = rule.command.as_deref() {
        table["command"] = toml_edit::value(command);
    }
    if let Some(path) = rule.path.as_deref() {
        table["path"] = toml_edit::value(path);
    }
    table
}

fn ask_rule_inline_table(rule: &ToolAskRule) -> toml_edit::InlineTable {
    let mut table = toml_edit::InlineTable::new();
    table.insert("tool", toml_edit::Value::from(rule.tool.clone()));
    if let Some(command) = rule.command.as_deref() {
        table.insert("command", toml_edit::Value::from(command));
    }
    if let Some(path) = rule.path.as_deref() {
        table.insert("path", toml_edit::Value::from(path));
    }
    table
}

fn write_permissions_atomic(path: &Path, body: &[u8]) -> Result<()> {
    let parent = path.parent().with_context(|| {
        format!(
            "permissions path has no parent directory: {}",
            path.display()
        )
    })?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create permissions directory {}",
            parent.display()
        )
    })?;

    let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create temporary permissions file in {}",
            parent.display()
        )
    })?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| {
            format!(
                "failed to secure temporary permissions file for {}",
                path.display()
            )
        })?;
    temporary
        .write_all(body)
        .with_context(|| format!("failed to write permissions at {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("failed to sync permissions at {}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace permissions at {}", path.display()))?;
    Ok(())
}

pub fn default_config_path() -> Result<PathBuf> {
    // Prefer ~/.codewhale/config.toml when it exists (fresh install or
    // migrated), otherwise fall back to ~/.deepseek/config.toml.
    let primary = codewhale_home()?.join(CONFIG_FILE_NAME);
    if primary.exists() {
        return Ok(primary);
    }
    let legacy = legacy_deepseek_home()?.join(CONFIG_FILE_NAME);
    if legacy.exists() {
        return Ok(legacy);
    }
    // Neither exists — return primary so first write creates it there.
    Ok(primary)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigMigration {
    pub legacy_path: PathBuf,
    pub primary_path: PathBuf,
}

impl ConfigMigration {
    pub fn user_notice(&self) -> String {
        format!(
            "Migrated legacy config from {} to {}. Use the .codewhale path for future edits; the .deepseek file remains only as a compatibility fallback.",
            self.legacy_path.display(),
            self.primary_path.display()
        )
    }
}

/// v0.8.44: one-time migration from `~/.deepseek/config.toml` to
/// `~/.codewhale/config.toml`. Called on first launch after the config
/// is loaded; copies the legacy file if the primary doesn't exist yet.
/// Never overwrites an existing primary config.
pub fn migrate_config_if_needed() -> Result<Option<ConfigMigration>> {
    let primary = codewhale_home()?.join(CONFIG_FILE_NAME);
    if primary.exists() {
        return Ok(None);
    }
    let legacy = legacy_deepseek_home()?.join(CONFIG_FILE_NAME);
    if !legacy.exists() {
        return Ok(None);
    }
    // Copy the config to the new home.
    if let Some(parent) = primary.parent() {
        std::fs::create_dir_all(parent).context("failed to create codewhale config directory")?;
    }
    std::fs::copy(&legacy, &primary)
        .context("failed to migrate config from deepseek to codewhale home")?;
    tracing::info!(
        "Migrated config from {} to {}",
        legacy.display(),
        primary.display()
    );
    Ok(Some(ConfigMigration {
        legacy_path: legacy,
        primary_path: primary,
    }))
}

fn parse_bool(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Ok(true),
        "0" | "false" | "no" | "off" | "disabled" => Ok(false),
        _ => bail!("invalid boolean '{raw}'"),
    }
}

fn parse_http_headers(raw: &str) -> Result<BTreeMap<String, String>> {
    let mut headers = BTreeMap::new();
    for pair in raw.trim().split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((name, value)) = pair.split_once('=') else {
            bail!("invalid header pair '{pair}', expected name=value");
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            bail!("header name cannot be empty");
        }
        if value.is_empty() {
            continue;
        }
        headers.insert(name.to_string(), value.to_string());
    }
    Ok(headers)
}

fn serialize_http_headers(headers: &BTreeMap<String, String>) -> Option<String> {
    if headers.is_empty() {
        return None;
    }
    Some(
        headers
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(","),
    )
}

fn serialize_http_headers_for_display(headers: &BTreeMap<String, String>) -> Option<String> {
    if headers.is_empty() {
        return None;
    }
    Some(
        headers
            .iter()
            .map(|(name, value)| {
                let display_value = if is_sensitive_config_key(name) {
                    redact_secret(value)
                } else {
                    value.clone()
                };
                format!("{name}={display_value}")
            })
            .collect::<Vec<_>>()
            .join(","),
    )
}

fn redact_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 16 {
        return "********".to_string();
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}***{suffix}")
}

#[must_use]
pub fn is_sensitive_config_key(key: &str) -> bool {
    let Some(segment) = key.rsplit('.').next() else {
        return false;
    };
    let normalized = segment
        .trim()
        .trim_matches('"')
        .replace('-', "_")
        .to_ascii_lowercase();

    matches!(
        normalized.as_str(),
        "api_key"
            | "apikey"
            | "api_keys"
            | "authorization"
            | "bearer"
            | "client_secret"
            | "credential"
            | "credentials"
            | "id_token"
            | "password"
            | "passwords"
            | "passwd"
            | "proxy_authorization"
            | "refresh_token"
            | "secret"
            | "secrets"
            | "token"
            | "tokens"
    ) || normalized.ends_with("_api_key")
        || normalized.ends_with("_authorization")
        || normalized.ends_with("_password")
        || normalized.ends_with("_secret")
        || normalized.ends_with("_token")
}

fn redact_toml_value_for_display(key: &str, value: &toml::Value) -> String {
    redact_toml_value_for_display_inner(key, false, value).to_string()
}

fn redact_toml_value_for_display_inner(
    key: &str,
    sensitive_ancestor: bool,
    value: &toml::Value,
) -> toml::Value {
    let sensitive = sensitive_ancestor || is_sensitive_config_key(key);
    match value {
        toml::Value::String(value) if sensitive => toml::Value::String(redact_secret(value)),
        toml::Value::Array(values) => toml::Value::Array(
            values
                .iter()
                .map(|value| redact_toml_value_for_display_inner(key, sensitive, value))
                .collect(),
        ),
        toml::Value::Table(table) => {
            let mut redacted = toml::map::Map::new();
            for (child_key, child_value) in table {
                let path = if key.is_empty() {
                    child_key.clone()
                } else {
                    format!("{key}.{child_key}")
                };
                redacted.insert(
                    child_key.clone(),
                    redact_toml_value_for_display_inner(&path, sensitive, child_value),
                );
            }
            toml::Value::Table(redacted)
        }
        _ if sensitive => toml::Value::String("********".to_string()),
        _ => value.clone(),
    }
}

fn normalize_config_file_path(path: PathBuf) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("config path cannot be empty");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("config path cannot contain '..' components");
    }
    if path.file_name().is_none() {
        bail!("config path must include a file name");
    }
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(std::env::current_dir()
        .context("failed to resolve current directory for config path")?
        .join(path))
}

#[derive(Debug, Clone, Default)]
struct EnvRuntimeOverrides {
    provider: Option<ProviderKind>,
    provider_source: Option<&'static str>,
    model: Option<String>,
    volcengine_model: Option<String>,
    wanjie_ark_model: Option<String>,
    openrouter_model: Option<String>,
    moonshot_model: Option<String>,
    xiaomi_mimo_model: Option<String>,
    xiaomi_mimo_mode: Option<String>,
    novita_model: Option<String>,
    fireworks_model: Option<String>,
    arcee_model: Option<String>,
    output_mode: Option<String>,
    auth_mode: Option<String>,
    log_level: Option<String>,
    telemetry: Option<bool>,
    approval_policy: Option<String>,
    sandbox_mode: Option<String>,
    yolo: Option<bool>,
    verbosity: Option<String>,
    http_headers: Option<BTreeMap<String, String>>,
    deepseek_base_url: Option<String>,
    nvidia_base_url: Option<String>,
    openai_base_url: Option<String>,
    atlascloud_base_url: Option<String>,
    volcengine_base_url: Option<String>,
    wanjie_ark_base_url: Option<String>,
    openrouter_base_url: Option<String>,
    xiaomi_mimo_base_url: Option<String>,
    novita_base_url: Option<String>,
    fireworks_base_url: Option<String>,
    siliconflow_base_url: Option<String>,
    siliconflow_model: Option<String>,
    arcee_base_url: Option<String>,
    moonshot_base_url: Option<String>,
    sglang_base_url: Option<String>,
    vllm_base_url: Option<String>,
    ollama_base_url: Option<String>,
    huggingface_base_url: Option<String>,
    huggingface_model: Option<String>,
    together_base_url: Option<String>,
    together_model: Option<String>,
    openai_codex_base_url: Option<String>,
    openai_codex_model: Option<String>,
    anthropic_base_url: Option<String>,
    anthropic_model: Option<String>,
    zai_base_url: Option<String>,
    zai_model: Option<String>,
    stepfun_base_url: Option<String>,
    stepfun_model: Option<String>,
    minimax_base_url: Option<String>,
    minimax_model: Option<String>,
    deepinfra_base_url: Option<String>,
    deepinfra_model: Option<String>,
}

impl EnvRuntimeOverrides {
    fn load() -> Self {
        let (provider, provider_source) = Self::load_provider();
        Self {
            provider,
            provider_source,
            model: std::env::var("CODEWHALE_MODEL")
                .or_else(|_| std::env::var("DEEPSEEK_MODEL"))
                .or_else(|_| std::env::var("DEEPSEEK_DEFAULT_TEXT_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            volcengine_model: std::env::var("VOLCENGINE_MODEL")
                .or_else(|_| std::env::var("VOLCENGINE_ARK_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            wanjie_ark_model: std::env::var("WANJIE_ARK_MODEL")
                .or_else(|_| std::env::var("WANJIE_MODEL"))
                .or_else(|_| std::env::var("WANJIE_MAAS_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            openrouter_model: std::env::var("OPENROUTER_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            moonshot_model: std::env::var("MOONSHOT_MODEL")
                .or_else(|_| std::env::var("KIMI_MODEL_NAME"))
                .or_else(|_| std::env::var("KIMI_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            xiaomi_mimo_model: std::env::var("XIAOMI_MIMO_MODEL")
                .or_else(|_| std::env::var("MIMO_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            xiaomi_mimo_mode: std::env::var("XIAOMI_MIMO_MODE")
                .or_else(|_| std::env::var("MIMO_MODE"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            novita_model: std::env::var("NOVITA_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            fireworks_model: std::env::var("FIREWORKS_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            arcee_model: std::env::var("ARCEE_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            verbosity: std::env::var("CODEWHALE_VERBOSITY")
                .or_else(|_| std::env::var("DEEPSEEK_VERBOSITY"))
                .ok(),
            output_mode: std::env::var("DEEPSEEK_OUTPUT_MODE").ok(),
            auth_mode: std::env::var("DEEPSEEK_AUTH_MODE").ok(),
            log_level: std::env::var("DEEPSEEK_LOG_LEVEL").ok(),
            telemetry: std::env::var("DEEPSEEK_TELEMETRY")
                .ok()
                .and_then(|v| match parse_bool(&v) {
                    Ok(b) => Some(b),
                    Err(_) => {
                        tracing::warn!("Invalid DEEPSEEK_TELEMETRY value '{v}', expected true/false");
                        None
                    }
                }),
            approval_policy: std::env::var("DEEPSEEK_APPROVAL_POLICY").ok(),
            sandbox_mode: std::env::var("DEEPSEEK_SANDBOX_MODE").ok(),
            yolo: std::env::var("DEEPSEEK_YOLO")
                .ok()
                .and_then(|v| match parse_bool(&v) {
                    Ok(b) => Some(b),
                    Err(_) => {
                        tracing::warn!("Invalid DEEPSEEK_YOLO value '{v}', expected true/false");
                        None
                    }
                }),
            http_headers: std::env::var("DEEPSEEK_HTTP_HEADERS")
                .ok()
                .and_then(|value| match parse_http_headers(&value) {
                    Ok(h) => Some(h),
                    Err(_) => {
                        tracing::warn!("Invalid DEEPSEEK_HTTP_HEADERS value, expected format: header1=val1,header2=val2");
                        None
                    }
                })
                .filter(|headers| !headers.is_empty()),
            deepseek_base_url: std::env::var("CODEWHALE_BASE_URL")
                .or_else(|_| std::env::var("DEEPSEEK_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            nvidia_base_url: std::env::var("NVIDIA_NIM_BASE_URL")
                .or_else(|_| std::env::var("NIM_BASE_URL"))
                .or_else(|_| std::env::var("NVIDIA_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            openai_base_url: std::env::var("OPENAI_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            atlascloud_base_url: std::env::var("ATLASCLOUD_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            volcengine_base_url: std::env::var("VOLCENGINE_BASE_URL")
                .or_else(|_| std::env::var("VOLCENGINE_ARK_BASE_URL"))
                .or_else(|_| std::env::var("ARK_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            wanjie_ark_base_url: std::env::var("WANJIE_ARK_BASE_URL")
                .or_else(|_| std::env::var("WANJIE_BASE_URL"))
                .or_else(|_| std::env::var("WANJIE_MAAS_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            openrouter_base_url: std::env::var("OPENROUTER_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            xiaomi_mimo_base_url: std::env::var("XIAOMI_MIMO_BASE_URL")
                .or_else(|_| std::env::var("MIMO_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            novita_base_url: std::env::var("NOVITA_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            fireworks_base_url: std::env::var("FIREWORKS_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            siliconflow_base_url: std::env::var("SILICONFLOW_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            siliconflow_model: std::env::var("SILICONFLOW_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            arcee_base_url: std::env::var("ARCEE_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            moonshot_base_url: std::env::var("MOONSHOT_BASE_URL")
                .or_else(|_| std::env::var("KIMI_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            sglang_base_url: std::env::var("SGLANG_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            vllm_base_url: std::env::var("VLLM_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            ollama_base_url: std::env::var("OLLAMA_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            huggingface_base_url: std::env::var("HUGGINGFACE_BASE_URL")
                .or_else(|_| std::env::var("HF_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            huggingface_model: std::env::var("HUGGINGFACE_MODEL")
                .or_else(|_| std::env::var("HF_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            together_base_url: std::env::var("TOGETHER_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            together_model: std::env::var("TOGETHER_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            openai_codex_base_url: std::env::var("OPENAI_CODEX_BASE_URL")
                .or_else(|_| std::env::var("CODEX_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            openai_codex_model: std::env::var("OPENAI_CODEX_MODEL")
                .or_else(|_| std::env::var("CODEX_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            anthropic_base_url: std::env::var("ANTHROPIC_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            anthropic_model: std::env::var("ANTHROPIC_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            zai_base_url: std::env::var("ZAI_BASE_URL")
                .or_else(|_| std::env::var("Z_AI_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            zai_model: std::env::var("ZAI_MODEL")
                .or_else(|_| std::env::var("Z_AI_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            stepfun_base_url: std::env::var("STEPFUN_BASE_URL")
                .or_else(|_| std::env::var("STEP_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            stepfun_model: std::env::var("STEPFUN_MODEL")
                .or_else(|_| std::env::var("STEP_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            minimax_base_url: std::env::var("MINIMAX_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            minimax_model: std::env::var("MINIMAX_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            deepinfra_base_url: std::env::var("DEEPINFRA_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            deepinfra_model: std::env::var("DEEPINFRA_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
        }
    }

    fn load_provider() -> (Option<ProviderKind>, Option<&'static str>) {
        if let Ok(value) = std::env::var("CODEWHALE_PROVIDER") {
            let parsed = ProviderKind::parse(&value);
            return (parsed, parsed.map(|_| "CODEWHALE_PROVIDER"));
        }

        if let Ok(value) = std::env::var("DEEPSEEK_PROVIDER") {
            let parsed = ProviderKind::parse(&value);
            return (parsed, parsed.map(|_| "DEEPSEEK_PROVIDER"));
        }

        (None, None)
    }

    fn base_url_for(&self, provider: ProviderKind) -> Option<String> {
        // Defaults belong in the resolver's final fallback so config-file
        // values (`providers.<name>.base_url`) still win when env is unset.
        match provider {
            ProviderKind::Deepseek => self.deepseek_base_url.clone(),
            ProviderKind::NvidiaNim => self.nvidia_base_url.clone(),
            ProviderKind::Openai => self.openai_base_url.clone(),
            ProviderKind::Atlascloud => self.atlascloud_base_url.clone(),
            ProviderKind::WanjieArk => self.wanjie_ark_base_url.clone(),
            ProviderKind::Volcengine => self.volcengine_base_url.clone(),
            ProviderKind::Openrouter => self.openrouter_base_url.clone(),
            ProviderKind::XiaomiMimo => self.xiaomi_mimo_base_url.clone(),
            ProviderKind::Novita => self.novita_base_url.clone(),
            ProviderKind::Fireworks => self.fireworks_base_url.clone(),
            ProviderKind::Siliconflow | ProviderKind::SiliconflowCN => {
                self.siliconflow_base_url.clone()
            }
            ProviderKind::Arcee => self.arcee_base_url.clone(),
            ProviderKind::Moonshot => self.moonshot_base_url.clone(),
            ProviderKind::Sglang => self.sglang_base_url.clone(),
            ProviderKind::Vllm => self.vllm_base_url.clone(),
            ProviderKind::Ollama => self.ollama_base_url.clone(),
            ProviderKind::Huggingface => self.huggingface_base_url.clone(),
            ProviderKind::Together => self.together_base_url.clone(),
            ProviderKind::OpenaiCodex => self.openai_codex_base_url.clone(),
            ProviderKind::Anthropic => self.anthropic_base_url.clone(),
            ProviderKind::Zai => self.zai_base_url.clone(),
            ProviderKind::Stepfun => self.stepfun_base_url.clone(),
            ProviderKind::Minimax => self.minimax_base_url.clone(),
            ProviderKind::Deepinfra => self.deepinfra_base_url.clone(),
        }
    }

    fn model_for(&self, provider: ProviderKind, base_url: &str) -> Option<String> {
        let model = match provider {
            ProviderKind::WanjieArk => self.wanjie_ark_model.clone(),
            ProviderKind::Volcengine => self.volcengine_model.clone(),
            ProviderKind::Openrouter => self.openrouter_model.clone(),
            ProviderKind::Siliconflow | ProviderKind::SiliconflowCN => {
                self.siliconflow_model.clone()
            }
            ProviderKind::Arcee => self.arcee_model.clone(),
            ProviderKind::Moonshot => self.moonshot_model.clone(),
            ProviderKind::XiaomiMimo => self.xiaomi_mimo_model.clone(),
            ProviderKind::Novita => self.novita_model.clone(),
            ProviderKind::Fireworks => self.fireworks_model.clone(),
            ProviderKind::Huggingface => self.huggingface_model.clone(),
            ProviderKind::Together => self.together_model.clone(),
            ProviderKind::OpenaiCodex => self.openai_codex_model.clone(),
            ProviderKind::Anthropic => self.anthropic_model.clone(),
            ProviderKind::Zai => self.zai_model.clone(),
            ProviderKind::Stepfun => self.stepfun_model.clone(),
            ProviderKind::Minimax => self.minimax_model.clone(),
            ProviderKind::Deepinfra => self.deepinfra_model.clone(),
            _ => None,
        }?;

        if provider_preserves_custom_base_url_model(provider, base_url) {
            Some(model.trim().to_string())
        } else {
            Some(normalize_model_for_provider(provider, &model))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::ffi::OsString;
    use std::sync::Arc;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn network_policy_toml_deserializes_proxy_hosts() {
        let policy: NetworkPolicyToml = toml::from_str(
            r#"
            default = "allow"
            proxy = ["github.com", ".githubusercontent.com"]
            "#,
        )
        .expect("network policy toml");

        assert_eq!(policy.default, "allow");
        assert_eq!(policy.proxy, ["github.com", ".githubusercontent.com"]);
        assert!(policy.audit);
    }

    #[test]
    fn permissions_toml_deserializes_typed_ask_rules() {
        let permissions: PermissionsToml = toml::from_str(
            r#"
            [[rules]]
            tool = "exec_shell"
            command = "cargo test"

            [[rules]]
            tool = "read_file"
            path = "secrets/api_key.txt"
            "#,
        )
        .expect("permissions toml");

        assert_eq!(
            permissions.rules,
            vec![
                ToolAskRule::exec_shell("cargo test"),
                ToolAskRule::file_path("read_file", "secrets/api_key.txt"),
            ]
        );
    }

    #[test]
    fn permissions_toml_rejects_typed_allow_deny_shape() {
        let err = toml::from_str::<PermissionsToml>(
            r#"
            [[rules]]
            tool = "exec_shell"
            decision = "allow"
            command = "cargo test"
            "#,
        )
        .expect_err("permissions.toml should be ask-only in this slice");

        assert!(err.message().contains("unknown field"));
    }

    #[test]
    fn hotbar_defaults_when_config_is_absent() {
        let config = ConfigToml::default();

        let resolved = config.resolve_hotbar_bindings(&DEFAULT_HOTBAR_ACTIONS);

        assert_eq!(resolved.warnings, Vec::new());
        assert_eq!(resolved.bindings, default_hotbar_bindings());
        assert_eq!(
            resolved
                .bindings
                .iter()
                .map(|binding| (binding.slot, binding.action.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (1, "voice.toggle"),
                (2, "session.compact"),
                (3, "mode.plan"),
                (4, "mode.agent"),
                (5, "mode.yolo"),
                (6, "palette.open"),
                (7, "sidebar.toggle"),
                (8, "trust.toggle"),
            ]
        );
    }

    #[test]
    fn hotbar_tables_parse_and_round_trip() {
        let config: ConfigToml = toml::from_str(
            r#"
[[hotbar]]
slot = 1
label = "Plan"
action = "mode.plan"

[[hotbar]]
slot = 2
action = "session.compact"
"#,
        )
        .expect("parse hotbar tables");

        let resolved = config.resolve_hotbar_bindings(&["mode.plan", "session.compact"]);

        assert_eq!(
            resolved.bindings,
            vec![
                HotbarBinding {
                    slot: 1,
                    action: "mode.plan".to_string(),
                    label: Some("Plan".to_string()),
                },
                HotbarBinding {
                    slot: 2,
                    action: "session.compact".to_string(),
                    label: None,
                },
            ]
        );
        assert_eq!(resolved.warnings, Vec::new());

        let serialized = toml::to_string_pretty(&config).expect("serialize config");
        let round_tripped: ConfigToml =
            toml::from_str(&serialized).expect("deserialize serialized config");
        assert_eq!(round_tripped.hotbar, config.hotbar);
    }

    #[test]
    fn hotbar_validation_warns_without_dropping_unknown_actions() {
        let config: ConfigToml = toml::from_str(
            r#"
[[hotbar]]
slot = 0
action = "mode.plan"

[[hotbar]]
slot = 2
action = "mode.plan"

[[hotbar]]
slot = 2
action = "custom.action"

[[hotbar]]
slot = 9
action = "mode.agent"
"#,
        )
        .expect("parse hotbar tables");

        let resolved = config.resolve_hotbar_bindings(&["mode.plan", "mode.agent"]);

        assert_eq!(
            resolved.bindings,
            vec![HotbarBinding {
                slot: 2,
                action: "custom.action".to_string(),
                label: None,
            }]
        );
        assert_eq!(
            resolved.warnings,
            vec![
                HotbarConfigWarning::SlotOutOfRange {
                    slot: 0,
                    action: "mode.plan".to_string(),
                },
                HotbarConfigWarning::UnknownAction {
                    slot: 2,
                    action: "custom.action".to_string(),
                },
                HotbarConfigWarning::DuplicateSlot {
                    slot: 2,
                    previous_action: "mode.plan".to_string(),
                    replacement_action: "custom.action".to_string(),
                },
                HotbarConfigWarning::SlotOutOfRange {
                    slot: 9,
                    action: "mode.agent".to_string(),
                },
            ]
        );
        assert!(resolved.warnings[1].to_string().contains("keeping binding"));
    }

    #[test]
    fn config_store_loads_sibling_permissions_toml() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codewhale-permissions-schema-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let config_path = dir.join(CONFIG_FILE_NAME);
        fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");
        fs::write(
            dir.join(PERMISSIONS_FILE_NAME),
            r#"
            [[rules]]
            tool = "exec_shell"
            command = "cargo test"

            [[rules]]
            tool = "read_file"
            path = "secrets/api_key.txt"
            "#,
        )
        .expect("write permissions");

        let store = ConfigStore::load(Some(config_path.clone())).expect("load config store");

        assert_eq!(store.config.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(
            store.permissions().rules.as_slice(),
            &[
                ToolAskRule::exec_shell("cargo test"),
                ToolAskRule::file_path("read_file", "secrets/api_key.txt"),
            ]
        );
        assert_eq!(
            store.permissions_path(),
            config_path.with_file_name(PERMISSIONS_FILE_NAME)
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn config_store_loads_permissions_even_when_config_is_absent() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codewhale-permissions-only-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let config_path = dir.join(CONFIG_FILE_NAME);
        fs::write(
            dir.join(PERMISSIONS_FILE_NAME),
            r#"
            [[rules]]
            tool = "exec_shell"
            command = "cargo check"
            "#,
        )
        .expect("write permissions");

        let store = ConfigStore::load(Some(config_path)).expect("load config store");

        assert!(store.config.model.is_none());
        assert_eq!(
            store.permissions().rules.as_slice(),
            &[ToolAskRule::exec_shell("cargo check")]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn config_store_exec_policy_engine_uses_sibling_permissions() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codewhale-permissions-engine-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let config_path = dir.join(CONFIG_FILE_NAME);
        fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");
        fs::write(
            dir.join(PERMISSIONS_FILE_NAME),
            r#"
            [[rules]]
            tool = "exec_shell"
            command = "cargo test"
            "#,
        )
        .expect("write permissions");

        let store = ConfigStore::load(Some(config_path)).expect("load config store");
        let decision = store
            .exec_policy_engine()
            .check(codewhale_execpolicy::ExecPolicyContext {
                command: "cargo test --workspace",
                cwd: "/workspace",
                tool: Some("exec_shell"),
                path: None,
                ask_for_approval: codewhale_execpolicy::AskForApproval::UnlessTrusted,
                sandbox_mode: Some("workspace-write"),
            })
            .expect("policy check");

        assert!(decision.allow);
        assert!(decision.requires_approval);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool=exec_shell command=cargo test")
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn config_store_appends_ask_rules_without_losing_comments_or_duplicates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join(CONFIG_FILE_NAME);
        let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
        fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");
        fs::write(
            &permissions_path,
            r#"# keep this permission note
[[rules]]
tool = "exec_shell"
command = "cargo check"
"#,
        )
        .expect("write permissions");

        let mut store = ConfigStore::load(Some(config_path)).expect("load config store");
        let existing = ToolAskRule::exec_shell("cargo check");
        let added_rule = ToolAskRule::file_path("read_file", "docs/README.md");
        let added = store
            .append_ask_rules(&[existing, added_rule.clone(), added_rule.clone()])
            .expect("append ask rules");

        assert_eq!(added, 1);
        assert_eq!(
            store.permissions().rules,
            vec![ToolAskRule::exec_shell("cargo check"), added_rule.clone(),]
        );
        let body = fs::read_to_string(&permissions_path).expect("read permissions");
        assert!(body.contains("# keep this permission note"));
        assert_eq!(body.matches("docs/README.md").count(), 1);
        assert!(!body.contains("decision"));

        let before_duplicate_append = body;
        assert_eq!(
            store
                .append_ask_rules(&[added_rule])
                .expect("dedupe ask rule"),
            0
        );
        assert_eq!(
            fs::read_to_string(&permissions_path).expect("read unchanged permissions"),
            before_duplicate_append
        );

        let reloaded = ConfigStore::load(Some(dir.path().join(CONFIG_FILE_NAME)))
            .expect("reload config store");
        assert_eq!(reloaded.permissions(), store.permissions());
    }

    #[test]
    fn config_store_appends_ask_rule_to_inline_rules_array() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join(CONFIG_FILE_NAME);
        let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
        fs::write(
            &permissions_path,
            "# inline rules stay valid\nrules = [{ tool = \"exec_shell\", command = \"cargo check\" }]\n",
        )
        .expect("write permissions");

        let mut store = ConfigStore::load(Some(config_path)).expect("load config store");
        assert_eq!(
            store
                .append_ask_rules(&[ToolAskRule::file_path("read_file", "README.md")])
                .expect("append inline ask rule"),
            1
        );

        let body = fs::read_to_string(&permissions_path).expect("read permissions");
        assert!(body.contains("# inline rules stay valid"));
        let parsed: PermissionsToml = toml::from_str(&body).expect("parse persisted permissions");
        assert_eq!(
            parsed.rules,
            vec![
                ToolAskRule::exec_shell("cargo check"),
                ToolAskRule::file_path("read_file", "README.md"),
            ]
        );
    }

    #[test]
    fn config_store_does_not_overwrite_invalid_permissions_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join(CONFIG_FILE_NAME);
        let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
        let mut store = ConfigStore::load(Some(config_path)).expect("load config store");
        let invalid = "rules = \"not-an-array\"\n";
        fs::write(&permissions_path, invalid).expect("write invalid permissions");

        let error = store
            .append_ask_rules(&[ToolAskRule::exec_shell("cargo test")])
            .expect_err("invalid permissions should fail");

        assert!(error.to_string().contains("failed to parse permissions"));
        assert_eq!(
            fs::read_to_string(&permissions_path).expect("read invalid permissions"),
            invalid
        );
        assert!(store.permissions().is_empty());
    }

    #[test]
    fn duplicate_append_refreshes_permissions_changed_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join(CONFIG_FILE_NAME);
        let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
        let mut store = ConfigStore::load(Some(config_path)).expect("load config store");
        fs::write(
            permissions_path,
            "[[rules]]\ntool = \"exec_shell\"\ncommand = \"cargo check\"\n",
        )
        .expect("write external permissions update");

        assert_eq!(
            store
                .append_ask_rules(&[ToolAskRule::exec_shell("cargo check")])
                .expect("dedupe external ask rule"),
            0
        );
        assert_eq!(
            store.permissions().rules,
            vec![ToolAskRule::exec_shell("cargo check")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_store_secures_persisted_permissions_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join(CONFIG_FILE_NAME);
        let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
        let mut store = ConfigStore::load(Some(config_path)).expect("load config store");

        store
            .append_ask_rules(&[ToolAskRule::exec_shell("cargo test")])
            .expect("append ask rule");

        let mode = fs::metadata(permissions_path)
            .expect("permissions metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    struct EnvGuard {
        deepseek_api_key: Option<OsString>,
        deepseek_base_url: Option<OsString>,
        deepseek_http_headers: Option<OsString>,
        deepseek_model: Option<OsString>,
        deepseek_default_text_model: Option<OsString>,
        deepseek_provider: Option<OsString>,
        deepseek_auth_mode: Option<OsString>,
        nvidia_api_key: Option<OsString>,
        nvidia_nim_api_key: Option<OsString>,
        nim_base_url: Option<OsString>,
        nvidia_base_url: Option<OsString>,
        nvidia_nim_base_url: Option<OsString>,
        openrouter_api_key: Option<OsString>,
        openrouter_base_url: Option<OsString>,
        openrouter_model: Option<OsString>,
        xiaomi_mimo_token_plan_api_key: Option<OsString>,
        mimo_token_plan_api_key: Option<OsString>,
        xiaomi_mimo_api_key: Option<OsString>,
        xiaomi_api_key: Option<OsString>,
        mimo_api_key: Option<OsString>,
        xiaomi_mimo_base_url: Option<OsString>,
        mimo_base_url: Option<OsString>,
        xiaomi_mimo_model: Option<OsString>,
        mimo_model: Option<OsString>,
        xiaomi_mimo_mode: Option<OsString>,
        mimo_mode: Option<OsString>,
        wanjie_ark_api_key: Option<OsString>,
        volcengine_api_key: Option<OsString>,
        volcengine_ark_api_key: Option<OsString>,
        ark_api_key: Option<OsString>,
        volcengine_base_url: Option<OsString>,
        volcengine_ark_base_url: Option<OsString>,
        ark_base_url: Option<OsString>,
        wanjie_ark_base_url: Option<OsString>,
        wanjie_base_url: Option<OsString>,
        wanjie_maas_base_url: Option<OsString>,
        volcengine_model: Option<OsString>,
        volcengine_ark_model: Option<OsString>,
        wanjie_ark_model: Option<OsString>,
        wanjie_model: Option<OsString>,
        wanjie_maas_model: Option<OsString>,
        novita_api_key: Option<OsString>,
        novita_base_url: Option<OsString>,
        novita_model: Option<OsString>,
        fireworks_api_key: Option<OsString>,
        fireworks_base_url: Option<OsString>,
        fireworks_model: Option<OsString>,
        siliconflow_api_key: Option<OsString>,
        siliconflow_base_url: Option<OsString>,
        siliconflow_model: Option<OsString>,
        arcee_api_key: Option<OsString>,
        arcee_base_url: Option<OsString>,
        arcee_model: Option<OsString>,
        moonshot_api_key: Option<OsString>,
        moonshot_base_url: Option<OsString>,
        moonshot_model: Option<OsString>,
        kimi_api_key: Option<OsString>,
        kimi_base_url: Option<OsString>,
        kimi_model: Option<OsString>,
        kimi_model_name: Option<OsString>,
        zai_api_key: Option<OsString>,
        z_ai_api_key: Option<OsString>,
        zai_base_url: Option<OsString>,
        zai_model: Option<OsString>,
        stepfun_api_key: Option<OsString>,
        step_api_key: Option<OsString>,
        stepfun_base_url: Option<OsString>,
        stepfun_model: Option<OsString>,
        minimax_api_key: Option<OsString>,
        minimax_base_url: Option<OsString>,
        minimax_model: Option<OsString>,
        sglang_api_key: Option<OsString>,
        sglang_base_url: Option<OsString>,
        vllm_api_key: Option<OsString>,
        vllm_base_url: Option<OsString>,
        ollama_api_key: Option<OsString>,
        ollama_base_url: Option<OsString>,
        huggingface_api_key: Option<OsString>,
        huggingface_token: Option<OsString>,
        huggingface_base_url: Option<OsString>,
        hf_base_url: Option<OsString>,
        huggingface_model: Option<OsString>,
        hf_model: Option<OsString>,
        codewhale_provider: Option<OsString>,
        codewhale_model: Option<OsString>,
        codewhale_base_url: Option<OsString>,
    }

    impl EnvGuard {
        fn without_deepseek_runtime_overrides() -> Self {
            let guard = Self {
                deepseek_api_key: env::var_os("DEEPSEEK_API_KEY"),
                deepseek_base_url: env::var_os("DEEPSEEK_BASE_URL"),
                deepseek_http_headers: env::var_os("DEEPSEEK_HTTP_HEADERS"),
                deepseek_model: env::var_os("DEEPSEEK_MODEL"),
                deepseek_default_text_model: env::var_os("DEEPSEEK_DEFAULT_TEXT_MODEL"),
                deepseek_provider: env::var_os("DEEPSEEK_PROVIDER"),
                deepseek_auth_mode: env::var_os("DEEPSEEK_AUTH_MODE"),
                codewhale_provider: env::var_os("CODEWHALE_PROVIDER"),
                codewhale_model: env::var_os("CODEWHALE_MODEL"),
                codewhale_base_url: env::var_os("CODEWHALE_BASE_URL"),
                nvidia_api_key: env::var_os("NVIDIA_API_KEY"),
                nvidia_nim_api_key: env::var_os("NVIDIA_NIM_API_KEY"),
                nim_base_url: env::var_os("NIM_BASE_URL"),
                nvidia_base_url: env::var_os("NVIDIA_BASE_URL"),
                nvidia_nim_base_url: env::var_os("NVIDIA_NIM_BASE_URL"),
                openrouter_api_key: env::var_os("OPENROUTER_API_KEY"),
                openrouter_base_url: env::var_os("OPENROUTER_BASE_URL"),
                openrouter_model: env::var_os("OPENROUTER_MODEL"),
                xiaomi_mimo_token_plan_api_key: env::var_os("XIAOMI_MIMO_TOKEN_PLAN_API_KEY"),
                mimo_token_plan_api_key: env::var_os("MIMO_TOKEN_PLAN_API_KEY"),
                xiaomi_mimo_api_key: env::var_os("XIAOMI_MIMO_API_KEY"),
                xiaomi_api_key: env::var_os("XIAOMI_API_KEY"),
                mimo_api_key: env::var_os("MIMO_API_KEY"),
                xiaomi_mimo_base_url: env::var_os("XIAOMI_MIMO_BASE_URL"),
                mimo_base_url: env::var_os("MIMO_BASE_URL"),
                xiaomi_mimo_model: env::var_os("XIAOMI_MIMO_MODEL"),
                mimo_model: env::var_os("MIMO_MODEL"),
                xiaomi_mimo_mode: env::var_os("XIAOMI_MIMO_MODE"),
                mimo_mode: env::var_os("MIMO_MODE"),
                wanjie_ark_api_key: env::var_os("WANJIE_ARK_API_KEY"),
                volcengine_api_key: env::var_os("VOLCENGINE_API_KEY"),
                volcengine_ark_api_key: env::var_os("VOLCENGINE_ARK_API_KEY"),
                ark_api_key: env::var_os("ARK_API_KEY"),
                volcengine_base_url: env::var_os("VOLCENGINE_BASE_URL"),
                volcengine_ark_base_url: env::var_os("VOLCENGINE_ARK_BASE_URL"),
                ark_base_url: env::var_os("ARK_BASE_URL"),
                wanjie_ark_base_url: env::var_os("WANJIE_ARK_BASE_URL"),
                wanjie_base_url: env::var_os("WANJIE_BASE_URL"),
                wanjie_maas_base_url: env::var_os("WANJIE_MAAS_BASE_URL"),
                volcengine_model: env::var_os("VOLCENGINE_MODEL"),
                volcengine_ark_model: env::var_os("VOLCENGINE_ARK_MODEL"),
                wanjie_ark_model: env::var_os("WANJIE_ARK_MODEL"),
                wanjie_model: env::var_os("WANJIE_MODEL"),
                wanjie_maas_model: env::var_os("WANJIE_MAAS_MODEL"),
                novita_api_key: env::var_os("NOVITA_API_KEY"),
                novita_base_url: env::var_os("NOVITA_BASE_URL"),
                novita_model: env::var_os("NOVITA_MODEL"),
                fireworks_api_key: env::var_os("FIREWORKS_API_KEY"),
                fireworks_base_url: env::var_os("FIREWORKS_BASE_URL"),
                fireworks_model: env::var_os("FIREWORKS_MODEL"),
                siliconflow_api_key: env::var_os("SILICONFLOW_API_KEY"),
                siliconflow_base_url: env::var_os("SILICONFLOW_BASE_URL"),
                siliconflow_model: env::var_os("SILICONFLOW_MODEL"),
                arcee_api_key: env::var_os("ARCEE_API_KEY"),
                arcee_base_url: env::var_os("ARCEE_BASE_URL"),
                arcee_model: env::var_os("ARCEE_MODEL"),
                moonshot_api_key: env::var_os("MOONSHOT_API_KEY"),
                moonshot_base_url: env::var_os("MOONSHOT_BASE_URL"),
                moonshot_model: env::var_os("MOONSHOT_MODEL"),
                kimi_api_key: env::var_os("KIMI_API_KEY"),
                kimi_base_url: env::var_os("KIMI_BASE_URL"),
                kimi_model: env::var_os("KIMI_MODEL"),
                kimi_model_name: env::var_os("KIMI_MODEL_NAME"),
                zai_api_key: env::var_os("ZAI_API_KEY"),
                z_ai_api_key: env::var_os("Z_AI_API_KEY"),
                zai_base_url: env::var_os("ZAI_BASE_URL"),
                zai_model: env::var_os("ZAI_MODEL"),
                stepfun_api_key: env::var_os("STEPFUN_API_KEY"),
                step_api_key: env::var_os("STEP_API_KEY"),
                stepfun_base_url: env::var_os("STEPFUN_BASE_URL"),
                stepfun_model: env::var_os("STEPFUN_MODEL"),
                minimax_api_key: env::var_os("MINIMAX_API_KEY"),
                minimax_base_url: env::var_os("MINIMAX_BASE_URL"),
                minimax_model: env::var_os("MINIMAX_MODEL"),
                sglang_api_key: env::var_os("SGLANG_API_KEY"),
                sglang_base_url: env::var_os("SGLANG_BASE_URL"),
                vllm_api_key: env::var_os("VLLM_API_KEY"),
                vllm_base_url: env::var_os("VLLM_BASE_URL"),
                ollama_api_key: env::var_os("OLLAMA_API_KEY"),
                ollama_base_url: env::var_os("OLLAMA_BASE_URL"),
                huggingface_api_key: env::var_os("HUGGINGFACE_API_KEY"),
                huggingface_token: env::var_os("HF_TOKEN"),
                huggingface_base_url: env::var_os("HUGGINGFACE_BASE_URL"),
                hf_base_url: env::var_os("HF_BASE_URL"),
                huggingface_model: env::var_os("HUGGINGFACE_MODEL"),
                hf_model: env::var_os("HF_MODEL"),
            };
            // Safety: test-only environment mutation guarded by a module mutex.
            unsafe {
                env::remove_var("DEEPSEEK_API_KEY");
                env::remove_var("DEEPSEEK_BASE_URL");
                env::remove_var("DEEPSEEK_HTTP_HEADERS");
                env::remove_var("DEEPSEEK_MODEL");
                env::remove_var("DEEPSEEK_DEFAULT_TEXT_MODEL");
                env::remove_var("DEEPSEEK_PROVIDER");
                env::remove_var("DEEPSEEK_AUTH_MODE");
                env::remove_var("CODEWHALE_PROVIDER");
                env::remove_var("CODEWHALE_MODEL");
                env::remove_var("CODEWHALE_BASE_URL");
                env::remove_var("NVIDIA_API_KEY");
                env::remove_var("NVIDIA_NIM_API_KEY");
                env::remove_var("NIM_BASE_URL");
                env::remove_var("NVIDIA_BASE_URL");
                env::remove_var("NVIDIA_NIM_BASE_URL");
                env::remove_var("OPENROUTER_API_KEY");
                env::remove_var("OPENROUTER_BASE_URL");
                env::remove_var("OPENROUTER_MODEL");
                env::remove_var("XIAOMI_MIMO_TOKEN_PLAN_API_KEY");
                env::remove_var("MIMO_TOKEN_PLAN_API_KEY");
                env::remove_var("XIAOMI_MIMO_API_KEY");
                env::remove_var("XIAOMI_API_KEY");
                env::remove_var("MIMO_API_KEY");
                env::remove_var("XIAOMI_MIMO_BASE_URL");
                env::remove_var("MIMO_BASE_URL");
                env::remove_var("XIAOMI_MIMO_MODEL");
                env::remove_var("MIMO_MODEL");
                env::remove_var("XIAOMI_MIMO_MODE");
                env::remove_var("MIMO_MODE");
                env::remove_var("WANJIE_ARK_API_KEY");
                env::remove_var("VOLCENGINE_API_KEY");
                env::remove_var("VOLCENGINE_ARK_API_KEY");
                env::remove_var("ARK_API_KEY");
                env::remove_var("VOLCENGINE_BASE_URL");
                env::remove_var("VOLCENGINE_ARK_BASE_URL");
                env::remove_var("ARK_BASE_URL");
                env::remove_var("WANJIE_ARK_BASE_URL");
                env::remove_var("WANJIE_BASE_URL");
                env::remove_var("WANJIE_MAAS_BASE_URL");
                env::remove_var("VOLCENGINE_MODEL");
                env::remove_var("VOLCENGINE_ARK_MODEL");
                env::remove_var("WANJIE_ARK_MODEL");
                env::remove_var("WANJIE_MODEL");
                env::remove_var("WANJIE_MAAS_MODEL");
                env::remove_var("NOVITA_API_KEY");
                env::remove_var("NOVITA_BASE_URL");
                env::remove_var("NOVITA_MODEL");
                env::remove_var("FIREWORKS_API_KEY");
                env::remove_var("FIREWORKS_BASE_URL");
                env::remove_var("FIREWORKS_MODEL");
                env::remove_var("SILICONFLOW_API_KEY");
                env::remove_var("SILICONFLOW_BASE_URL");
                env::remove_var("SILICONFLOW_MODEL");
                env::remove_var("ARCEE_API_KEY");
                env::remove_var("ARCEE_BASE_URL");
                env::remove_var("ARCEE_MODEL");
                env::remove_var("MOONSHOT_API_KEY");
                env::remove_var("MOONSHOT_BASE_URL");
                env::remove_var("MOONSHOT_MODEL");
                env::remove_var("KIMI_API_KEY");
                env::remove_var("KIMI_BASE_URL");
                env::remove_var("KIMI_MODEL");
                env::remove_var("KIMI_MODEL_NAME");
                env::remove_var("ZAI_API_KEY");
                env::remove_var("Z_AI_API_KEY");
                env::remove_var("ZAI_BASE_URL");
                env::remove_var("ZAI_MODEL");
                env::remove_var("STEPFUN_API_KEY");
                env::remove_var("STEP_API_KEY");
                env::remove_var("STEPFUN_BASE_URL");
                env::remove_var("STEPFUN_MODEL");
                env::remove_var("MINIMAX_API_KEY");
                env::remove_var("MINIMAX_BASE_URL");
                env::remove_var("MINIMAX_MODEL");
                env::remove_var("SGLANG_API_KEY");
                env::remove_var("SGLANG_BASE_URL");
                env::remove_var("VLLM_API_KEY");
                env::remove_var("VLLM_BASE_URL");
                env::remove_var("OLLAMA_API_KEY");
                env::remove_var("OLLAMA_BASE_URL");
                env::remove_var("HUGGINGFACE_API_KEY");
                env::remove_var("HF_TOKEN");
                env::remove_var("HUGGINGFACE_BASE_URL");
                env::remove_var("HF_BASE_URL");
                env::remove_var("HUGGINGFACE_MODEL");
                env::remove_var("HF_MODEL");
            }
            guard
        }

        unsafe fn restore_var(key: &str, value: Option<OsString>) {
            if let Some(value) = value {
                unsafe { env::set_var(key, value) };
            } else {
                unsafe { env::remove_var(key) };
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // Safety: test-only environment mutation guarded by a module mutex.
            unsafe {
                Self::restore_var("DEEPSEEK_API_KEY", self.deepseek_api_key.take());
                Self::restore_var("DEEPSEEK_BASE_URL", self.deepseek_base_url.take());
                Self::restore_var("DEEPSEEK_HTTP_HEADERS", self.deepseek_http_headers.take());
                Self::restore_var("DEEPSEEK_MODEL", self.deepseek_model.take());
                Self::restore_var(
                    "DEEPSEEK_DEFAULT_TEXT_MODEL",
                    self.deepseek_default_text_model.take(),
                );
                Self::restore_var("DEEPSEEK_PROVIDER", self.deepseek_provider.take());
                Self::restore_var("DEEPSEEK_AUTH_MODE", self.deepseek_auth_mode.take());
                Self::restore_var("CODEWHALE_PROVIDER", self.codewhale_provider.take());
                Self::restore_var("CODEWHALE_MODEL", self.codewhale_model.take());
                Self::restore_var("CODEWHALE_BASE_URL", self.codewhale_base_url.take());
                Self::restore_var("NVIDIA_API_KEY", self.nvidia_api_key.take());
                Self::restore_var("NVIDIA_NIM_API_KEY", self.nvidia_nim_api_key.take());
                Self::restore_var("NIM_BASE_URL", self.nim_base_url.take());
                Self::restore_var("NVIDIA_BASE_URL", self.nvidia_base_url.take());
                Self::restore_var("NVIDIA_NIM_BASE_URL", self.nvidia_nim_base_url.take());
                Self::restore_var("OPENROUTER_API_KEY", self.openrouter_api_key.take());
                Self::restore_var("OPENROUTER_BASE_URL", self.openrouter_base_url.take());
                Self::restore_var("OPENROUTER_MODEL", self.openrouter_model.take());
                Self::restore_var(
                    "XIAOMI_MIMO_TOKEN_PLAN_API_KEY",
                    self.xiaomi_mimo_token_plan_api_key.take(),
                );
                Self::restore_var(
                    "MIMO_TOKEN_PLAN_API_KEY",
                    self.mimo_token_plan_api_key.take(),
                );
                Self::restore_var("XIAOMI_MIMO_API_KEY", self.xiaomi_mimo_api_key.take());
                Self::restore_var("XIAOMI_API_KEY", self.xiaomi_api_key.take());
                Self::restore_var("MIMO_API_KEY", self.mimo_api_key.take());
                Self::restore_var("XIAOMI_MIMO_BASE_URL", self.xiaomi_mimo_base_url.take());
                Self::restore_var("MIMO_BASE_URL", self.mimo_base_url.take());
                Self::restore_var("XIAOMI_MIMO_MODEL", self.xiaomi_mimo_model.take());
                Self::restore_var("MIMO_MODEL", self.mimo_model.take());
                Self::restore_var("XIAOMI_MIMO_MODE", self.xiaomi_mimo_mode.take());
                Self::restore_var("MIMO_MODE", self.mimo_mode.take());
                Self::restore_var("WANJIE_ARK_API_KEY", self.wanjie_ark_api_key.take());
                Self::restore_var("VOLCENGINE_API_KEY", self.volcengine_api_key.take());
                Self::restore_var("VOLCENGINE_ARK_API_KEY", self.volcengine_ark_api_key.take());
                Self::restore_var("ARK_API_KEY", self.ark_api_key.take());
                Self::restore_var("VOLCENGINE_BASE_URL", self.volcengine_base_url.take());
                Self::restore_var(
                    "VOLCENGINE_ARK_BASE_URL",
                    self.volcengine_ark_base_url.take(),
                );
                Self::restore_var("ARK_BASE_URL", self.ark_base_url.take());
                Self::restore_var("WANJIE_ARK_BASE_URL", self.wanjie_ark_base_url.take());
                Self::restore_var("WANJIE_BASE_URL", self.wanjie_base_url.take());
                Self::restore_var("WANJIE_MAAS_BASE_URL", self.wanjie_maas_base_url.take());
                Self::restore_var("VOLCENGINE_MODEL", self.volcengine_model.take());
                Self::restore_var("VOLCENGINE_ARK_MODEL", self.volcengine_ark_model.take());
                Self::restore_var("WANJIE_ARK_MODEL", self.wanjie_ark_model.take());
                Self::restore_var("WANJIE_MODEL", self.wanjie_model.take());
                Self::restore_var("WANJIE_MAAS_MODEL", self.wanjie_maas_model.take());
                Self::restore_var("NOVITA_API_KEY", self.novita_api_key.take());
                Self::restore_var("NOVITA_BASE_URL", self.novita_base_url.take());
                Self::restore_var("NOVITA_MODEL", self.novita_model.take());
                Self::restore_var("FIREWORKS_API_KEY", self.fireworks_api_key.take());
                Self::restore_var("FIREWORKS_BASE_URL", self.fireworks_base_url.take());
                Self::restore_var("FIREWORKS_MODEL", self.fireworks_model.take());
                Self::restore_var("SILICONFLOW_API_KEY", self.siliconflow_api_key.take());
                Self::restore_var("SILICONFLOW_BASE_URL", self.siliconflow_base_url.take());
                Self::restore_var("SILICONFLOW_MODEL", self.siliconflow_model.take());
                Self::restore_var("ARCEE_API_KEY", self.arcee_api_key.take());
                Self::restore_var("ARCEE_BASE_URL", self.arcee_base_url.take());
                Self::restore_var("ARCEE_MODEL", self.arcee_model.take());
                Self::restore_var("MOONSHOT_API_KEY", self.moonshot_api_key.take());
                Self::restore_var("MOONSHOT_BASE_URL", self.moonshot_base_url.take());
                Self::restore_var("MOONSHOT_MODEL", self.moonshot_model.take());
                Self::restore_var("KIMI_API_KEY", self.kimi_api_key.take());
                Self::restore_var("KIMI_BASE_URL", self.kimi_base_url.take());
                Self::restore_var("KIMI_MODEL", self.kimi_model.take());
                Self::restore_var("KIMI_MODEL_NAME", self.kimi_model_name.take());
                Self::restore_var("ZAI_API_KEY", self.zai_api_key.take());
                Self::restore_var("Z_AI_API_KEY", self.z_ai_api_key.take());
                Self::restore_var("ZAI_BASE_URL", self.zai_base_url.take());
                Self::restore_var("ZAI_MODEL", self.zai_model.take());
                Self::restore_var("STEPFUN_API_KEY", self.stepfun_api_key.take());
                Self::restore_var("STEP_API_KEY", self.step_api_key.take());
                Self::restore_var("STEPFUN_BASE_URL", self.stepfun_base_url.take());
                Self::restore_var("STEPFUN_MODEL", self.stepfun_model.take());
                Self::restore_var("MINIMAX_API_KEY", self.minimax_api_key.take());
                Self::restore_var("MINIMAX_BASE_URL", self.minimax_base_url.take());
                Self::restore_var("MINIMAX_MODEL", self.minimax_model.take());
                Self::restore_var("SGLANG_API_KEY", self.sglang_api_key.take());
                Self::restore_var("SGLANG_BASE_URL", self.sglang_base_url.take());
                Self::restore_var("VLLM_API_KEY", self.vllm_api_key.take());
                Self::restore_var("VLLM_BASE_URL", self.vllm_base_url.take());
                Self::restore_var("OLLAMA_API_KEY", self.ollama_api_key.take());
                Self::restore_var("OLLAMA_BASE_URL", self.ollama_base_url.take());
                Self::restore_var("HUGGINGFACE_API_KEY", self.huggingface_api_key.take());
                Self::restore_var("HF_TOKEN", self.huggingface_token.take());
                Self::restore_var("HUGGINGFACE_BASE_URL", self.huggingface_base_url.take());
                Self::restore_var("HF_BASE_URL", self.hf_base_url.take());
                Self::restore_var("HUGGINGFACE_MODEL", self.huggingface_model.take());
                Self::restore_var("HF_MODEL", self.hf_model.take());
            }
        }
    }

    struct RecordingSecretsStore {
        gets: Mutex<Vec<String>>,
        value: Option<String>,
    }

    impl RecordingSecretsStore {
        fn with_value(value: &str) -> Self {
            Self {
                gets: Mutex::new(Vec::new()),
                value: Some(value.to_string()),
            }
        }
    }

    impl codewhale_secrets::KeyringStore for RecordingSecretsStore {
        fn get(&self, key: &str) -> Result<Option<String>, codewhale_secrets::SecretsError> {
            self.gets.lock().unwrap().push(key.to_string());
            Ok(self.value.clone())
        }

        fn set(&self, _key: &str, _value: &str) -> Result<(), codewhale_secrets::SecretsError> {
            Ok(())
        }

        fn delete(&self, _key: &str) -> Result<(), codewhale_secrets::SecretsError> {
            Ok(())
        }

        fn backend_name(&self) -> &'static str {
            "recording"
        }
    }

    #[test]
    fn root_deepseek_fields_are_runtime_fallbacks() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            api_key: Some("root-key".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.api_key.as_deref(), Some("root-key"));
        assert_eq!(resolved.base_url, "https://api.deepseek.com");
        assert_eq!(resolved.model, "deepseek-v4-pro");
    }

    #[test]
    fn deepseek_runtime_defaults_to_beta_endpoint() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml::default();

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.base_url, DEFAULT_DEEPSEEK_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_DEEPSEEK_MODEL);
    }

    #[test]
    fn provider_specific_deepseek_fields_override_tui_compat_fields() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            api_key: Some("root-key".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..ConfigToml::default()
        };
        config.providers.deepseek.api_key = Some("provider-key".to_string());
        config.providers.deepseek.base_url = Some("https://gateway.example/v1".to_string());
        config.providers.deepseek.model = Some("deepseek-v4-flash".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.api_key.as_deref(), Some("provider-key"));
        assert_eq!(resolved.base_url, "https://gateway.example/v1");
        assert_eq!(resolved.model, "deepseek-v4-flash");
    }

    #[test]
    fn provider_http_headers_override_root_headers() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            api_key: Some("root-key".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..ConfigToml::default()
        };
        config.providers.deepseek.api_key = Some("provider-key".to_string());
        config.providers.deepseek.base_url = Some("https://gateway.example/v1".to_string());
        config.providers.deepseek.model = Some("deepseek-v4-flash".to_string());
        config
            .http_headers
            .insert("X-Shared".to_string(), "root".to_string());
        config
            .providers
            .deepseek
            .http_headers
            .insert("X-Model-Provider-Id".to_string(), "tongyi".to_string());
        config
            .providers
            .deepseek
            .http_headers
            .insert("X-Shared".to_string(), "provider".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.api_key.as_deref(), Some("provider-key"));
        assert_eq!(resolved.base_url, "https://gateway.example/v1");
        assert_eq!(resolved.model, "deepseek-v4-flash");
        assert_eq!(
            resolved
                .http_headers
                .get("X-Model-Provider-Id")
                .map(String::as_str),
            Some("tongyi")
        );
        assert_eq!(
            resolved.http_headers.get("X-Shared").map(String::as_str),
            Some("provider")
        );
    }

    #[test]
    fn insecure_skip_tls_verify_resolves_only_for_active_provider() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::Openai,
            ..ConfigToml::default()
        };
        config.providers.deepseek.insecure_skip_tls_verify = Some(true);

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Openai);
        assert!(!resolved.insecure_skip_tls_verify);

        config.providers.openai.insecure_skip_tls_verify = Some(true);
        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Openai);
        assert!(resolved.insecure_skip_tls_verify);
    }

    #[test]
    fn http_headers_env_overrides_config() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml::default();
        config
            .http_headers
            .insert("X-Model-Provider-Id".to_string(), "from-file".to_string());
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_HTTP_HEADERS", "X-Model-Provider-Id=from-env");
        }

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(
            resolved
                .http_headers
                .get("X-Model-Provider-Id")
                .map(String::as_str),
            Some("from-env")
        );
    }

    #[test]
    fn nvidia_nim_provider_defaults_to_catalog_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::NvidiaNim,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.base_url, DEFAULT_NVIDIA_NIM_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_NVIDIA_NIM_MODEL);
    }

    #[test]
    fn nvidia_nim_provider_uses_provider_specific_credentials() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::NvidiaNim,
            ..ConfigToml::default()
        };
        config.providers.nvidia_nim.api_key = Some("nim-key".to_string());
        config.providers.nvidia_nim.base_url = Some("https://nim.example/v1".to_string());
        config.providers.nvidia_nim.model = Some("deepseek-ai/deepseek-v4-pro".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.api_key.as_deref(), Some("nim-key"));
        assert_eq!(resolved.base_url, "https://nim.example/v1");
        assert_eq!(resolved.model, "deepseek-ai/deepseek-v4-pro");
    }

    #[test]
    fn nvidia_nim_provider_normalizes_flash_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::NvidiaNim),
            model: Some("deepseek-v4-flash".to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.model, DEFAULT_NVIDIA_NIM_FLASH_MODEL);
    }

    #[test]
    fn nvidia_nim_provider_uses_nvidia_env_credentials() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
            env::set_var("NVIDIA_API_KEY", "nim-env-key");
            env::set_var("NVIDIA_NIM_BASE_URL", "https://nim-env.example/v1");
        }

        let config = ConfigToml::default();
        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.api_key.as_deref(), Some("nim-env-key"));
        assert_eq!(resolved.base_url, "https://nim-env.example/v1");
        assert_eq!(resolved.model, DEFAULT_NVIDIA_NIM_MODEL);
    }

    #[test]
    fn nvidia_nim_provider_accepts_short_nim_base_url_alias() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
            env::set_var("NVIDIA_API_KEY", "nim-env-key");
            env::set_var("NIM_BASE_URL", "https://short-nim.example/v1");
        }

        let config = ConfigToml::default();
        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.base_url, "https://short-nim.example/v1");
    }

    #[test]
    fn nvidia_nim_provider_can_fallback_to_deepseek_api_key_env() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
            env::set_var("DEEPSEEK_API_KEY", "deepseek-compat-key");
        }

        let config = ConfigToml::default();
        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.api_key.as_deref(), Some("deepseek-compat-key"));
    }

    #[test]
    fn list_values_redacts_root_api_key() {
        let config = ConfigToml {
            api_key: Some("sk-deepseek-secret".to_string()),
            ..ConfigToml::default()
        };

        let values = config.list_values();

        assert_eq!(
            values.get("api_key").map(String::as_str),
            Some("sk-d***cret")
        );
    }

    #[test]
    fn list_values_fully_redacts_short_api_key() {
        let config = ConfigToml {
            api_key: Some("short-key".to_string()),
            ..ConfigToml::default()
        };

        let values = config.list_values();

        assert_eq!(values.get("api_key").map(String::as_str), Some("********"));
    }

    #[test]
    fn get_display_value_redacts_sensitive_keys() {
        let mut config = ConfigToml {
            api_key: Some("sk-deepseek-secret".to_string()),
            ..ConfigToml::default()
        };
        config.providers.openrouter.api_key = Some("openrouter-secret-value".to_string());
        config.model = Some("deepseek-v4-pro".to_string());

        assert_eq!(
            config.get_display_value("api_key").as_deref(),
            Some("sk-d***cret")
        );
        assert_eq!(
            config
                .get_display_value("providers.openrouter.api_key")
                .as_deref(),
            Some("open***alue")
        );
        assert_eq!(
            config.get_display_value("model").as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    #[test]
    fn config_display_redacts_nested_extra_secrets() {
        let mut config = ConfigToml::default();
        let mut profile = toml::map::Map::new();
        profile.insert(
            "chatgpt_access_token".to_string(),
            toml::Value::String("raw-chatgpt-access-token-value".to_string()),
        );
        profile.insert(
            "safe_label".to_string(),
            toml::Value::String("visible".to_string()),
        );

        let mut nested = toml::map::Map::new();
        nested.insert(
            "refresh_token".to_string(),
            toml::Value::String("raw-refresh-token-value".to_string()),
        );
        nested.insert("expires_at".to_string(), toml::Value::Integer(1234));
        profile.insert("session".to_string(), toml::Value::Table(nested));

        config
            .extras
            .insert("extras".to_string(), toml::Value::Table(profile));

        let listed = config.list_values();
        let rendered = listed.get("extras").expect("extras are listed");

        assert!(rendered.contains("chatgpt_access_token"));
        assert!(rendered.contains("refresh_token"));
        assert!(rendered.contains("safe_label = \"visible\""));
        assert!(!rendered.contains("raw-chatgpt-access-token-value"));
        assert!(!rendered.contains("raw-refresh-token-value"));

        let display = config
            .get_display_value("extras")
            .expect("extras display value");
        assert!(!display.contains("raw-chatgpt-access-token-value"));
        assert!(!display.contains("raw-refresh-token-value"));
    }

    #[test]
    fn config_display_redacts_sensitive_extra_leaf_keys_and_headers() {
        let mut config = ConfigToml::default();
        config.extras.insert(
            "chatgpt_access_token".to_string(),
            toml::Value::String("raw-chatgpt-token-value".to_string()),
        );
        config.http_headers.insert(
            "Authorization".to_string(),
            "Bearer raw-header-token".to_string(),
        );
        config
            .http_headers
            .insert("X-Test".to_string(), "ok".to_string());

        assert_eq!(
            config.get_display_value("chatgpt_access_token").as_deref(),
            Some("\"raw-***alue\"")
        );

        let headers = config
            .list_values()
            .get("http_headers")
            .expect("headers are listed")
            .clone();
        assert!(headers.contains("Authorization=Bear***oken"));
        assert!(headers.contains("X-Test=ok"));
        assert!(!headers.contains("raw-header-token"));
    }

    #[test]
    fn hook_sinks_config_uses_separate_table_from_lifecycle_hooks() -> Result<()> {
        let raw = r#"
[hooks]
enabled = true
default_timeout_secs = 20

[[hooks.hooks]]
event = "message_submit"
command = "echo ok"

[hook_sinks]
unix_socket_path = "/tmp/cw-hooks.sock"
"#;

        let config: ConfigToml = toml::from_str(raw)?;

        assert_eq!(
            config.get_value("hook_sinks.unix_socket_path").as_deref(),
            Some("/tmp/cw-hooks.sock")
        );
        assert!(
            config.extras.contains_key("hooks"),
            "legacy lifecycle hooks table must remain an opaque extra"
        );

        let serialized = toml::to_string_pretty(&config)?;
        let round_tripped: ConfigToml = toml::from_str(&serialized)?;
        let hooks = round_tripped
            .extras
            .get("hooks")
            .and_then(toml::Value::as_table)
            .expect("hooks table preserved");

        assert_eq!(
            hooks.get("enabled").and_then(toml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            hooks
                .get("default_timeout_secs")
                .and_then(toml::Value::as_integer),
            Some(20)
        );
        assert!(
            hooks.get("hooks").and_then(toml::Value::as_array).is_some(),
            "nested lifecycle hooks array must survive config rewrites"
        );
        assert_eq!(
            round_tripped
                .get_value("hook_sinks.unix_socket_path")
                .as_deref(),
            Some("/tmp/cw-hooks.sock")
        );

        Ok(())
    }

    #[test]
    fn hook_sinks_unix_socket_path_round_trips_through_key_value_api() -> Result<()> {
        let mut config = ConfigToml::default();

        config.set_value("hook_sinks.unix_socket_path", "/tmp/cw-events.sock")?;

        assert_eq!(
            config.get_value("hook_sinks.unix_socket_path").as_deref(),
            Some("/tmp/cw-events.sock")
        );
        assert_eq!(
            config
                .list_values()
                .get("hook_sinks.unix_socket_path")
                .map(String::as_str),
            Some("/tmp/cw-events.sock")
        );

        config.unset_value("hook_sinks.unix_socket_path")?;
        assert_eq!(config.get_value("hook_sinks.unix_socket_path"), None);

        Ok(())
    }

    /// End-to-end smoke for the preferred Kimi Code setup path:
    ///   1. Start from a fresh root config that uses DeepSeek defaults.
    ///   2. Mutate it through the same key-value setters the
    ///      `codewhale config set providers.moonshot.*` CLI invokes.
    ///   3. Switch the active provider through `CODEWHALE_PROVIDER` —
    ///      the public env alias — without ever touching the legacy
    ///      `DEEPSEEK_PROVIDER` name.
    ///   4. Resolve the runtime and confirm the doctor/runtime values.
    ///
    /// No real API key is required; the `api_key` here is just a
    /// non-empty placeholder.
    #[test]
    fn moonshot_kimi_code_smoke_config_set_then_resolve() -> Result<()> {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();

        let mut config = ConfigToml {
            provider: ProviderKind::Deepseek,
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..ConfigToml::default()
        };

        // Same key paths a user would run via `codewhale config set`.
        config.set_value("providers.moonshot.api_key", "kimi-code-key-placeholder")?;
        config.set_value("providers.moonshot.auth_mode", "api_key")?;
        config.set_value("providers.moonshot.base_url", DEFAULT_KIMI_CODE_BASE_URL)?;
        config.set_value("providers.moonshot.model", DEFAULT_KIMI_CODE_MODEL)?;

        // Public env alias for the active-provider switch.
        // Safety: test-only env mutation guarded by env_lock().
        unsafe { env::set_var("CODEWHALE_PROVIDER", "moonshot") };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.base_url, DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_KIMI_CODE_MODEL);
        assert_eq!(resolved.auth_mode.as_deref(), Some("api_key"));
        assert_eq!(
            resolved.api_key.as_deref(),
            Some("kimi-code-key-placeholder")
        );
        assert_eq!(
            resolved.api_key_source,
            Some(RuntimeApiKeySource::ConfigFile)
        );
        Ok(())
    }

    #[test]
    fn moonshot_provider_config_values_round_trip() -> Result<()> {
        let mut config = ConfigToml::default();

        config.set_value("providers.moonshot.api_key", "moonshot-secret-value")?;
        config.set_value("providers.moonshot.base_url", DEFAULT_KIMI_CODE_BASE_URL)?;
        config.set_value("providers.moonshot.model", DEFAULT_KIMI_CODE_MODEL)?;
        config.set_value("providers.moonshot.auth_mode", "api_key")?;
        config.set_value("providers.moonshot.http_headers", "X-Test=ok")?;

        assert_eq!(
            config
                .get_display_value("providers.moonshot.api_key")
                .as_deref(),
            Some("moon***alue")
        );
        assert_eq!(
            config.get_value("providers.moonshot.base_url").as_deref(),
            Some(DEFAULT_KIMI_CODE_BASE_URL)
        );
        assert_eq!(
            config.get_value("providers.moonshot.model").as_deref(),
            Some(DEFAULT_KIMI_CODE_MODEL)
        );
        assert_eq!(
            config.get_value("providers.moonshot.auth_mode").as_deref(),
            Some("api_key")
        );
        assert_eq!(
            config
                .list_values()
                .get("providers.moonshot.api_key")
                .map(String::as_str),
            Some("moon***alue")
        );

        config.unset_value("providers.moonshot.auth_mode")?;
        config.unset_value("providers.moonshot.base_url")?;
        config.unset_value("providers.moonshot.model")?;

        assert_eq!(config.get_value("providers.moonshot.auth_mode"), None);
        assert_eq!(config.get_value("providers.moonshot.base_url"), None);
        assert_eq!(config.get_value("providers.moonshot.model"), None);
        Ok(())
    }

    #[test]
    fn siliconflow_cn_provider_config_values_round_trip() -> Result<()> {
        let mut config = ConfigToml::default();

        config.set_value("providers.siliconflow_cn.api_key", "sf-cn-secret-value")?;
        config.set_value(
            "providers.siliconflow_cn.base_url",
            DEFAULT_SILICONFLOW_CN_BASE_URL,
        )?;
        config.set_value("providers.siliconflow_cn.model", DEFAULT_SILICONFLOW_MODEL)?;
        config.set_value("providers.siliconflow_cn.http_headers", "X-Test=ok")?;

        assert_eq!(
            config
                .get_display_value("providers.siliconflow_cn.api_key")
                .as_deref(),
            Some("sf-c***alue")
        );
        assert_eq!(
            config
                .get_value("providers.siliconflow_cn.base_url")
                .as_deref(),
            Some(DEFAULT_SILICONFLOW_CN_BASE_URL)
        );
        assert_eq!(
            config
                .get_value("providers.siliconflow_cn.model")
                .as_deref(),
            Some(DEFAULT_SILICONFLOW_MODEL)
        );
        assert_eq!(
            config
                .list_values()
                .get("providers.siliconflow_cn.api_key")
                .map(String::as_str),
            Some("sf-c***alue")
        );

        config.unset_value("providers.siliconflow_cn.api_key")?;
        config.unset_value("providers.siliconflow_cn.base_url")?;
        config.unset_value("providers.siliconflow_cn.model")?;
        config.unset_value("providers.siliconflow_cn.http_headers")?;

        assert_eq!(config.get_value("providers.siliconflow_cn.api_key"), None);
        assert_eq!(config.get_value("providers.siliconflow_cn.base_url"), None);
        assert_eq!(config.get_value("providers.siliconflow_cn.model"), None);
        assert_eq!(
            config.get_value("providers.siliconflow_cn.http_headers"),
            None
        );
        Ok(())
    }

    #[test]
    fn volcengine_provider_config_values_round_trip() -> Result<()> {
        let mut config = ConfigToml::default();

        config.set_value("providers.volcengine.api_key", "volcengine-secret-value")?;
        config.set_value("providers.volcengine.base_url", DEFAULT_VOLCENGINE_BASE_URL)?;
        config.set_value("providers.volcengine.model", DEFAULT_VOLCENGINE_MODEL)?;
        config.set_value("providers.volcengine.http_headers", "X-Test=ok")?;

        assert_eq!(
            config
                .get_display_value("providers.volcengine.api_key")
                .as_deref(),
            Some("volc***alue")
        );
        assert_eq!(
            config.get_value("providers.volcengine.base_url").as_deref(),
            Some(DEFAULT_VOLCENGINE_BASE_URL)
        );
        assert_eq!(
            config.get_value("providers.volcengine.model").as_deref(),
            Some(DEFAULT_VOLCENGINE_MODEL)
        );
        assert_eq!(
            config
                .get_value("providers.volcengine.http_headers")
                .as_deref(),
            Some("X-Test=ok")
        );
        assert_eq!(
            config
                .list_values()
                .get("providers.volcengine.http_headers")
                .map(String::as_str),
            Some("X-Test=ok")
        );

        config.unset_value("providers.volcengine.http_headers")?;
        assert_eq!(config.get_value("providers.volcengine.http_headers"), None);
        Ok(())
    }

    #[test]
    fn provider_key_value_api_covers_all_provider_metadata_entries() -> Result<()> {
        for provider in ProviderKind::ALL {
            let table = provider.provider().provider_config_key();
            let mut config = ConfigToml::default();
            let api_key = format!("secret-value-for-{table}-123456");
            let api_key_path = format!("providers.{table}.api_key");
            let base_url_path = format!("providers.{table}.base_url");
            let model_path = format!("providers.{table}.model");
            let headers_path = format!("providers.{table}.http_headers");
            let mode_path = format!("providers.{table}.mode");
            let auth_mode_path = format!("providers.{table}.auth_mode");
            let insecure_path = format!("providers.{table}.insecure_skip_tls_verify");
            let path_suffix_path = format!("providers.{table}.path_suffix");

            config.set_value(&api_key_path, &api_key)?;
            config.set_value(&base_url_path, "https://gateway.example/v1")?;
            config.set_value(&model_path, "provider-test-model")?;
            config.set_value(&headers_path, "X-Test=ok")?;
            config.set_value(&mode_path, "concise")?;
            config.set_value(&auth_mode_path, "api_key")?;
            config.set_value(&insecure_path, "true")?;
            config.set_value(&path_suffix_path, "/chat/completions")?;

            assert_eq!(
                config.get_value(&api_key_path).as_deref(),
                Some(api_key.as_str())
            );
            assert_eq!(
                config.get_value(&base_url_path).as_deref(),
                Some("https://gateway.example/v1")
            );
            assert_eq!(
                config.get_value(&model_path).as_deref(),
                Some("provider-test-model")
            );
            assert_eq!(
                config.get_value(&headers_path).as_deref(),
                Some("X-Test=ok")
            );
            assert_eq!(config.get_value(&mode_path).as_deref(), Some("concise"));
            assert_eq!(
                config.get_value(&auth_mode_path).as_deref(),
                Some("api_key")
            );
            assert_eq!(config.get_value(&insecure_path).as_deref(), Some("true"));
            assert_eq!(
                config.get_value(&path_suffix_path).as_deref(),
                Some("/chat/completions")
            );

            let listed = config.list_values();
            let listed_api_key = listed
                .get(&api_key_path)
                .expect("provider API key is listed");
            assert!(listed_api_key.contains("***"));
            assert_ne!(listed_api_key, &api_key);
            assert_eq!(
                listed.get(&headers_path).map(String::as_str),
                Some("X-Test=ok")
            );
            assert_eq!(listed.get(&insecure_path).map(String::as_str), Some("true"));

            config.unset_value(&api_key_path)?;
            config.unset_value(&base_url_path)?;
            config.unset_value(&model_path)?;
            config.unset_value(&headers_path)?;
            config.unset_value(&mode_path)?;
            config.unset_value(&auth_mode_path)?;
            config.unset_value(&insecure_path)?;
            config.unset_value(&path_suffix_path)?;

            assert_eq!(config.get_value(&api_key_path), None);
            assert_eq!(config.get_value(&base_url_path), None);
            assert_eq!(config.get_value(&model_path), None);
            assert_eq!(config.get_value(&headers_path), None);
            assert_eq!(config.get_value(&mode_path), None);
            assert_eq!(config.get_value(&auth_mode_path), None);
            assert_eq!(config.get_value(&insecure_path), None);
            assert_eq!(config.get_value(&path_suffix_path), None);

            if provider == ProviderKind::Deepseek {
                assert_eq!(config.api_key, None);
                assert_eq!(config.base_url, None);
                assert_eq!(config.default_text_model, None);
                assert!(config.http_headers.is_empty());
            }
        }

        Ok(())
    }

    #[test]
    fn project_merge_denies_credentials_endpoints_and_provider_selection() {
        let mut base = ConfigToml {
            provider: ProviderKind::Deepseek,
            api_key: Some("user-key".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
            default_text_model: Some("deepseek-v4-flash".to_string()),
            ..ConfigToml::default()
        };
        base.providers.openrouter.api_key = Some("user-openrouter-key".to_string());
        base.providers.openrouter.path_suffix = Some("/chat/completions".to_string());

        let mut project = ConfigToml {
            provider: ProviderKind::Openrouter,
            api_key: Some("attacker-key".to_string()),
            base_url: Some("https://evil.example/v1".to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            auth_mode: Some("oauth".to_string()),
            telemetry: Some(true),
            ..ConfigToml::default()
        };
        project.providers.openrouter.api_key = Some("attacker-openrouter-key".to_string());
        project.providers.openrouter.base_url = Some("https://evil.example/openrouter".to_string());
        project.providers.openrouter.insecure_skip_tls_verify = Some(true);
        project.providers.openrouter.path_suffix = Some("/attacker/chat".to_string());
        project.providers.openrouter.model = Some("deepseek/deepseek-v4-pro".to_string());
        project.providers.volcengine.model = Some("DeepSeek-V4-Pro".to_string());
        project.providers.moonshot.model = Some("kimi-k2.6".to_string());

        base.merge_project_overrides(project);

        assert_eq!(base.provider, ProviderKind::Deepseek);
        assert_eq!(base.api_key.as_deref(), Some("user-key"));
        assert_eq!(base.base_url.as_deref(), Some("https://api.deepseek.com"));
        assert_eq!(base.auth_mode, None);
        assert_eq!(base.telemetry, None);
        assert_eq!(
            base.providers.openrouter.api_key.as_deref(),
            Some("user-openrouter-key")
        );
        assert_eq!(base.providers.openrouter.base_url, None);
        assert_eq!(base.providers.openrouter.insecure_skip_tls_verify, None);
        assert_eq!(
            base.providers.openrouter.path_suffix.as_deref(),
            Some("/chat/completions")
        );
        assert_eq!(base.default_text_model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(
            base.providers.openrouter.model.as_deref(),
            Some("deepseek/deepseek-v4-pro")
        );
        assert_eq!(
            base.providers.volcengine.model.as_deref(),
            Some("DeepSeek-V4-Pro")
        );
        assert_eq!(base.providers.moonshot.model.as_deref(), Some("kimi-k2.6"));
    }

    #[test]
    fn project_merge_forwards_all_provider_model_overrides() {
        let mut project_toml = String::new();
        for provider in ProviderKind::ALL {
            let key = provider.provider().provider_config_key();
            project_toml.push_str(&format!(
                "[providers.{key}]\nmodel = \"project-{key}-model\"\n\n"
            ));
        }

        let project: ConfigToml =
            toml::from_str(&project_toml).expect("project provider overrides parse");
        let mut base = ConfigToml::default();

        base.merge_project_overrides(project);

        for provider in ProviderKind::ALL {
            let key = provider.provider().provider_config_key();
            let expected = format!("project-{key}-model");
            assert_eq!(
                base.providers.for_provider(provider).model.as_deref(),
                Some(expected.as_str()),
                "provider {key} should merge repo-local model override"
            );
        }
    }

    #[test]
    fn project_merge_only_tightens_approval_and_sandbox_policy() {
        let mut strict = ConfigToml {
            approval_policy: Some("never".to_string()),
            sandbox_mode: Some("read-only".to_string()),
            ..ConfigToml::default()
        };
        strict.merge_project_overrides(ConfigToml {
            approval_policy: Some("on-request".to_string()),
            sandbox_mode: Some("workspace-write".to_string()),
            ..ConfigToml::default()
        });
        assert_eq!(strict.approval_policy.as_deref(), Some("never"));
        assert_eq!(strict.sandbox_mode.as_deref(), Some("read-only"));

        let mut permissive = ConfigToml {
            approval_policy: Some("auto".to_string()),
            sandbox_mode: Some("workspace-write".to_string()),
            ..ConfigToml::default()
        };
        permissive.merge_project_overrides(ConfigToml {
            approval_policy: Some("never".to_string()),
            sandbox_mode: Some("read-only".to_string()),
            ..ConfigToml::default()
        });
        assert_eq!(permissive.approval_policy.as_deref(), Some("never"));
        assert_eq!(permissive.sandbox_mode.as_deref(), Some("read-only"));

        let mut unset = ConfigToml::default();
        unset.merge_project_overrides(ConfigToml {
            approval_policy: Some("on-request".to_string()),
            sandbox_mode: Some("workspace-write".to_string()),
            ..ConfigToml::default()
        });
        assert_eq!(unset.approval_policy, None);
        assert_eq!(unset.sandbox_mode, None);
    }

    #[test]
    fn list_values_redacts_unicode_api_key_without_byte_slicing() {
        let config = ConfigToml {
            api_key: Some("密钥密钥密钥密钥123456789".to_string()),
            ..ConfigToml::default()
        };

        let values = config.list_values();

        assert_eq!(
            values.get("api_key").map(String::as_str),
            Some("密钥密钥***6789")
        );
    }

    #[test]
    fn app_homes_prefer_home_env_before_platform_home_fallback() {
        let _lock = env_lock();
        struct HomeEnvGuard {
            home: Option<OsString>,
            userprofile: Option<OsString>,
            codewhale_home: Option<OsString>,
        }

        impl Drop for HomeEnvGuard {
            fn drop(&mut self) {
                // Safety: test-only environment mutation is serialized by env_lock().
                unsafe {
                    match self.home.take() {
                        Some(value) => env::set_var("HOME", value),
                        None => env::remove_var("HOME"),
                    }
                    match self.userprofile.take() {
                        Some(value) => env::set_var("USERPROFILE", value),
                        None => env::remove_var("USERPROFILE"),
                    }
                    match self.codewhale_home.take() {
                        Some(value) => env::set_var("CODEWHALE_HOME", value),
                        None => env::remove_var("CODEWHALE_HOME"),
                    }
                }
            }
        }

        let home =
            std::env::temp_dir().join(format!("codewhale-config-home-env-{}", std::process::id()));
        let userprofile = std::env::temp_dir().join(format!(
            "codewhale-config-userprofile-{}",
            std::process::id()
        ));
        let _env = HomeEnvGuard {
            home: env::var_os("HOME"),
            userprofile: env::var_os("USERPROFILE"),
            codewhale_home: env::var_os("CODEWHALE_HOME"),
        };
        // Safety: test-only environment mutation is serialized by env_lock().
        unsafe {
            env::set_var("HOME", &home);
            env::set_var("USERPROFILE", &userprofile);
            env::remove_var("CODEWHALE_HOME");
        }

        assert_eq!(
            codewhale_home().expect("codewhale home"),
            home.join(CODEWHALE_APP_DIR)
        );
        assert_eq!(
            legacy_deepseek_home().expect("legacy home"),
            home.join(LEGACY_APP_DIR)
        );

        let explicit = std::env::temp_dir().join(format!(
            "codewhale-config-explicit-home-{}",
            std::process::id()
        ));
        // Safety: test-only environment mutation is serialized by env_lock().
        unsafe {
            env::set_var("CODEWHALE_HOME", &explicit);
        }
        assert_eq!(codewhale_home().expect("explicit home"), explicit);
    }

    #[test]
    fn migrate_config_reports_copied_legacy_path() {
        let _lock = env_lock();
        struct HomeEnvGuard {
            home: Option<OsString>,
            userprofile: Option<OsString>,
            codewhale_home: Option<OsString>,
        }

        impl Drop for HomeEnvGuard {
            fn drop(&mut self) {
                // Safety: test-only environment mutation is serialized by env_lock().
                unsafe {
                    match self.home.take() {
                        Some(value) => env::set_var("HOME", value),
                        None => env::remove_var("HOME"),
                    }
                    match self.userprofile.take() {
                        Some(value) => env::set_var("USERPROFILE", value),
                        None => env::remove_var("USERPROFILE"),
                    }
                    match self.codewhale_home.take() {
                        Some(value) => env::set_var("CODEWHALE_HOME", value),
                        None => env::remove_var("CODEWHALE_HOME"),
                    }
                }
            }
        }

        struct LegacyConfigGuard {
            path: PathBuf,
            original: Option<Vec<u8>>,
        }

        impl LegacyConfigGuard {
            fn install(path: PathBuf, contents: &[u8]) -> Self {
                let original = fs::read(&path).ok();
                fs::create_dir_all(path.parent().expect("legacy config parent"))
                    .expect("legacy dir");
                fs::write(&path, contents).expect("legacy config");
                Self { path, original }
            }
        }

        impl Drop for LegacyConfigGuard {
            fn drop(&mut self) {
                if let Some(original) = self.original.take() {
                    let _ = fs::write(&self.path, original);
                } else {
                    let _ = fs::remove_file(&self.path);
                    if let Some(parent) = self.path.parent() {
                        let _ = fs::remove_dir(parent);
                    }
                }
            }
        }

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let home = std::env::temp_dir().join(format!(
            "codewhale-config-migration-{}-{unique}",
            std::process::id()
        ));
        let legacy_dir = home.join(LEGACY_APP_DIR);
        let primary_dir = home.join(CODEWHALE_APP_DIR);
        let legacy_config = legacy_dir.join(CONFIG_FILE_NAME);
        let _legacy =
            LegacyConfigGuard::install(legacy_config.clone(), b"provider = \"deepseek\"\n");

        let _env = HomeEnvGuard {
            home: env::var_os("HOME"),
            userprofile: env::var_os("USERPROFILE"),
            codewhale_home: env::var_os("CODEWHALE_HOME"),
        };
        // Safety: test-only environment mutation is serialized by env_lock().
        unsafe {
            env::set_var("HOME", &home);
            env::set_var("USERPROFILE", &home);
            env::set_var("CODEWHALE_HOME", &primary_dir);
        }

        let migration = migrate_config_if_needed()
            .expect("migration")
            .expect("legacy config should be copied");

        assert_eq!(migration.legacy_path, legacy_config);
        assert_eq!(migration.primary_path, primary_dir.join(CONFIG_FILE_NAME));
        let notice = migration.user_notice();
        assert!(notice.contains(&legacy_dir.join(CONFIG_FILE_NAME).display().to_string()));
        assert!(notice.contains(&primary_dir.join(CONFIG_FILE_NAME).display().to_string()));
        assert!(notice.contains(".codewhale path for future edits"));
        assert!(notice.contains(".deepseek file remains only as a compatibility fallback"));
        assert_eq!(
            fs::read_to_string(primary_dir.join(CONFIG_FILE_NAME)).expect("primary config"),
            "provider = \"deepseek\"\n"
        );

        let _ = fs::remove_dir_all(home);
    }

    // ── ensure_state_dir legacy migration (#3240) ───────────────────────

    /// Saves and restores the env vars that the state-resolvers read.
    struct StateEnvRestore {
        home: Option<OsString>,
        userprofile: Option<OsString>,
        codewhale_home: Option<OsString>,
    }

    impl Drop for StateEnvRestore {
        fn drop(&mut self) {
            // Safety: test-only environment mutation is serialized by env_lock().
            unsafe {
                match self.home.take() {
                    Some(value) => env::set_var("HOME", value),
                    None => env::remove_var("HOME"),
                }
                match self.userprofile.take() {
                    Some(value) => env::set_var("USERPROFILE", value),
                    None => env::remove_var("USERPROFILE"),
                }
                match self.codewhale_home.take() {
                    Some(value) => env::set_var("CODEWHALE_HOME", value),
                    None => env::remove_var("CODEWHALE_HOME"),
                }
            }
        }
    }

    /// Points `HOME`/`USERPROFILE`/`CODEWHALE_HOME` at a fresh temp tree so
    /// `codewhale_home()` -> `<home>/.codewhale` and `legacy_deepseek_home()`
    /// -> `<home>/.deepseek`. Env is restored on drop.
    struct StateDirEnv {
        home: PathBuf,
        _restore: StateEnvRestore,
    }

    impl StateDirEnv {
        fn install(unique: u128) -> Self {
            let home = std::env::temp_dir().join(format!(
                "codewhale-state-migration-{}-{unique}",
                std::process::id()
            ));
            let restore = StateEnvRestore {
                home: env::var_os("HOME"),
                userprofile: env::var_os("USERPROFILE"),
                codewhale_home: env::var_os("CODEWHALE_HOME"),
            };
            // Safety: test-only environment mutation is serialized by env_lock().
            unsafe {
                env::set_var("HOME", &home);
                env::set_var("USERPROFILE", &home);
                env::set_var("CODEWHALE_HOME", home.join(CODEWHALE_APP_DIR));
            }
            Self {
                home,
                _restore: restore,
            }
        }
        fn legacy(&self, sub: &str) -> PathBuf {
            self.home.join(LEGACY_APP_DIR).join(sub)
        }
        fn primary(&self, sub: &str) -> PathBuf {
            self.home.join(CODEWHALE_APP_DIR).join(sub)
        }
    }

    #[test]
    fn ensure_state_dir_relocates_legacy_subdir_on_first_write() {
        let _lock = env_lock();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let state_env = StateDirEnv::install(unique);
        // Seed a legacy subdir; primary must not exist yet.
        fs::create_dir_all(state_env.legacy("slop_ledger")).expect("legacy dir");
        fs::write(
            state_env.legacy("slop_ledger").join("slop_ledger.json"),
            b"legacy",
        )
        .expect("legacy file");
        assert!(!state_env.primary("slop_ledger").exists());

        let dir = ensure_state_dir("slop_ledger").expect("ensure_state_dir");
        assert_eq!(dir, state_env.primary("slop_ledger"));
        // Legacy contents relocated into primary.
        assert_eq!(
            fs::read_to_string(state_env.primary("slop_ledger").join("slop_ledger.json"))
                .expect("migrated file"),
            "legacy"
        );
        // The legacy subdir was relocated (moved), so .deepseek stops growing.
        assert!(
            !state_env.legacy("slop_ledger").exists(),
            "legacy subdir should be removed after relocation"
        );
        // Idempotent: a second call is a no-op now that primary exists.
        ensure_state_dir("slop_ledger").expect("idempotent ensure");
        let _ = fs::remove_dir_all(&state_env.home);
    }

    #[test]
    fn ensure_state_dir_writes_to_primary_when_both_exist() {
        let _lock = env_lock();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let state_env = StateDirEnv::install(unique);
        // Migrated user: primary already exists; a legacy orphan also remains.
        fs::create_dir_all(state_env.primary("sessions")).expect("primary dir");
        fs::write(state_env.primary("sessions").join("a.json"), b"primary").expect("primary file");
        fs::create_dir_all(state_env.legacy("sessions")).expect("legacy dir");
        fs::write(state_env.legacy("sessions").join("old.json"), b"legacy").expect("legacy file");

        let dir = ensure_state_dir("sessions").expect("ensure_state_dir");
        assert_eq!(dir, state_env.primary("sessions"));
        // Primary untouched; legacy orphan left as-is (not migrated, not deleted).
        assert_eq!(
            fs::read_to_string(state_env.primary("sessions").join("a.json")).expect("primary"),
            "primary"
        );
        assert!(
            state_env.legacy("sessions").exists(),
            "existing legacy orphan must not be deleted when primary exists"
        );
        let _ = fs::remove_dir_all(&state_env.home);
    }

    #[test]
    fn resolve_state_dir_still_finds_legacy_for_backfill() {
        let _lock = env_lock();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let state_env = StateDirEnv::install(unique);
        // Only legacy exists -> read resolver returns legacy (backfill).
        fs::create_dir_all(state_env.legacy("catalog")).expect("legacy dir");
        assert_eq!(
            resolve_state_dir("catalog").expect("resolve"),
            state_env.legacy("catalog")
        );
        // After the primary is created (e.g. via a write), the read resolver
        // returns primary — legacy is reachable only while primary is absent.
        ensure_state_dir("catalog").expect("ensure");
        assert_eq!(
            resolve_state_dir("catalog").expect("resolve after migrate"),
            state_env.primary("catalog")
        );
        let _ = fs::remove_dir_all(&state_env.home);
    }

    #[test]
    fn state_resolvers_reject_path_traversal_subdirs() {
        // Defense against path injection (#3240 hardening): the public state
        // resolvers must refuse subdirs that could escape the state root.
        for bad in ["..", "../secret", "/etc", "a/../../b"] {
            let err = ensure_state_dir(bad)
                .err()
                .unwrap_or_else(|| panic!("expected {bad:?} to be rejected"));
            assert!(
                format!("{err:#}").contains("state subdir"),
                "expected rejection of {bad:?}, got {err:#}"
            );
            assert!(
                resolve_state_dir(bad).is_err(),
                "read resolver must also reject {bad:?}"
            );
        }
        // Safe values are accepted (including the root sentinel ".").
        assert!(ensure_safe_state_subdir(".").is_ok());
        assert!(ensure_safe_state_subdir("sessions").is_ok());
        assert!(ensure_safe_state_subdir("a/b").is_ok());
        assert!(ensure_safe_state_subdir("").is_err());
    }

    #[test]
    fn normalize_config_file_path_rejects_traversal() {
        let err = normalize_config_file_path(PathBuf::from("../config.toml"))
            .expect_err("traversal path should fail");
        assert!(format!("{err:#}").contains("cannot contain '..'"));
    }

    #[cfg(unix)]
    #[test]
    fn save_clamps_existing_config_permissions() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "deepseek-config-perms-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, "api_key = \"old\"\n").expect("seed config");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod seed");

        let store = ConfigStore {
            path: path.clone(),
            config: ConfigToml {
                api_key: Some("new-secret".to_string()),
                ..ConfigToml::default()
            },
            permissions: PermissionsToml::default(),
            original_raw: None,
        };
        store.save().expect("save");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn config_store_save_skips_identical_serialized_body() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codewhale-config-noop-save-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(CONFIG_FILE_NAME);
        let config = ConfigToml {
            model: Some("deepseek-v4-flash".to_string()),
            ..ConfigToml::default()
        };
        let body = toml::to_string_pretty(&config).expect("serialize");
        fs::write(&path, &body).expect("seed config");
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).expect("chmod seed");

        let store = ConfigStore {
            path: path.clone(),
            config,
            permissions: PermissionsToml::default(),
            original_raw: None,
        };
        store.save().expect("identical save should not rewrite");

        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod restore");
        assert_eq!(fs::read_to_string(&path).expect("read config"), body);
        assert!(
            !config_backup_path(&path).exists(),
            "no-op save must not create a migration backup"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn config_store_save_creates_one_time_backup_before_changed_write() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codewhale-config-backup-save-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(CONFIG_FILE_NAME);
        let original = "model = \"deepseek-v4-flash\"\n";
        fs::write(&path, original).expect("seed config");

        let store = ConfigStore {
            path: path.clone(),
            config: ConfigToml {
                model: Some("deepseek-v4-pro".to_string()),
                ..ConfigToml::default()
            },
            permissions: PermissionsToml::default(),
            original_raw: None,
        };
        store.save().expect("changed save");

        let backup_path = config_backup_path(&path);
        assert_eq!(
            fs::read_to_string(&backup_path).expect("read backup"),
            original
        );
        let updated = fs::read_to_string(&path).expect("read updated config");
        assert!(updated.contains("model = \"deepseek-v4-pro\""));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn config_store_save_preserves_comments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join(CONFIG_FILE_NAME);
        let original = "# my model\nmodel = \"deepseek-v4-flash\"\n# end comment\n";
        fs::write(&config_path, original).expect("write config");

        let mut store = ConfigStore::load(Some(config_path.clone())).expect("load config store");
        store.config.model = Some("deepseek-v4-pro".to_string());
        store.save().expect("save");

        let body = fs::read_to_string(&config_path).expect("read config");
        assert!(body.contains("# my model"), "prefix comment preserved");
        assert!(body.contains("# end comment"), "suffix comment preserved");
        assert!(body.contains("model = \"deepseek-v4-pro\""));
    }

    #[test]
    fn config_store_save_preserves_disabled_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join(CONFIG_FILE_NAME);
        fs::write(
            &config_path,
            "# my note\nmodel = \"deepseek-v4-flash\"\n# base_url = \"http://localhost:11434/v1\"\n",
        )
        .expect("write config");

        let mut store = ConfigStore::load(Some(config_path.clone())).expect("load config store");
        store.config.model = Some("deepseek-v4-pro".to_string());
        store.save().expect("save");

        let body = fs::read_to_string(&config_path).expect("read config");
        assert!(
            body.contains("# base_url = \"http://localhost:11434/v1\""),
            "disabled key preserved as comment"
        );
        assert!(body.contains("model = \"deepseek-v4-pro\""));
    }

    #[test]
    fn config_store_save_preserves_comments_with_other_keys() {
        // Realistic scenario: user already has api_key + model, adds a comment,
        // then changes model via `codewhale config set model`.
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join(CONFIG_FILE_NAME);
        fs::write(
            &config_path,
            "# my deepseek key\napi_key = \"sk-1234\"\n\n# my current model\nmodel = \"deepseek-v4-flash\"\n",
        )
        .expect("write config");

        let mut store = ConfigStore::load(Some(config_path.clone())).expect("load config store");
        store.config.model = Some("deepseek-v4-pro".to_string());
        store.save().expect("save");

        let body = fs::read_to_string(&config_path).expect("read config");
        assert!(body.contains("# my deepseek key"), "api_key comment lost");
        assert!(body.contains("# my current model"), "model comment lost");
        assert!(
            body.contains("model = \"deepseek-v4-pro\""),
            "new model not written"
        );
        assert!(body.contains("api_key = \"sk-1234\""), "api_key lost");
    }

    #[test]
    fn merge_and_preserve_comments_returns_err_on_invalid_serialized() {
        let err = merge_and_preserve_comments("{{{ not toml", "model = 1\n")
            .expect_err("invalid serialized should fail");
        assert!(
            format!("{err:#}").contains("failed to parse serialized"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn merge_and_preserve_comments_returns_err_on_invalid_original() {
        let err = merge_and_preserve_comments("model = 1\n", "{{{ not toml")
            .expect_err("invalid original should fail");
        assert!(
            format!("{err:#}").contains("failed to parse original"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn config_store_save_falls_back_when_comment_merge_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join(CONFIG_FILE_NAME);
        // Valid TOML so load succeeds, but the raw is corrupt so the merge
        // will fail inside save() — save must still succeed and write the
        // plain serialized config.
        fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");

        // Bypass ConfigStore::load to inject a deliberately broken original_raw.
        let store = ConfigStore {
            path: config_path.clone(),
            config: ConfigToml {
                model: Some("deepseek-v4-pro".to_string()),
                ..ConfigToml::default()
            },
            permissions: PermissionsToml::default(),
            original_raw: Some("{ broken".to_string()),
        };
        store
            .save()
            .expect("save should succeed even when merge fails");

        let body = fs::read_to_string(&config_path).expect("read config");
        assert!(
            body.contains("deepseek-v4-pro"),
            "config should be written: {body}"
        );
    }

    #[test]
    fn provider_kind_parses_openrouter_and_novita_aliases() {
        assert_eq!(
            ProviderKind::parse("openrouter"),
            Some(ProviderKind::Openrouter)
        );
        assert_eq!(
            ProviderKind::parse("OPEN_ROUTER"),
            Some(ProviderKind::Openrouter)
        );
        assert_eq!(
            ProviderKind::parse("xiaomi-mimo"),
            Some(ProviderKind::XiaomiMimo)
        );
        assert_eq!(
            ProviderKind::parse("xiaomi"),
            Some(ProviderKind::XiaomiMimo)
        );
        assert_eq!(ProviderKind::parse("novita"), Some(ProviderKind::Novita));
        assert_eq!(ProviderKind::parse("Novita"), Some(ProviderKind::Novita));
        assert_eq!(
            ProviderKind::parse("fireworks-ai"),
            Some(ProviderKind::Fireworks)
        );
        assert_eq!(
            ProviderKind::parse("silicon-flow"),
            Some(ProviderKind::Siliconflow)
        );
        assert_eq!(
            ProviderKind::parse("silicon_flow"),
            Some(ProviderKind::Siliconflow)
        );
        assert_eq!(ProviderKind::parse("kimi"), Some(ProviderKind::Moonshot));
        assert_eq!(
            ProviderKind::parse("moonshot-ai"),
            Some(ProviderKind::Moonshot)
        );
        assert_eq!(ProviderKind::parse("sg-lang"), Some(ProviderKind::Sglang));
        assert_eq!(ProviderKind::parse("v-llm"), Some(ProviderKind::Vllm));
        assert_eq!(ProviderKind::parse("vllm"), Some(ProviderKind::Vllm));
        assert_eq!(ProviderKind::parse("ollama"), Some(ProviderKind::Ollama));
        assert_eq!(
            ProviderKind::parse("ollama-local"),
            Some(ProviderKind::Ollama)
        );
        assert_eq!(
            ProviderKind::parse("wanjie-ark"),
            Some(ProviderKind::WanjieArk)
        );
        assert_eq!(
            ProviderKind::parse("ark_wanjie"),
            Some(ProviderKind::WanjieArk)
        );
        for alias in ["huggingface", "hugging-face", "hugging_face", "hf"] {
            assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::Huggingface));

            let parsed: ConfigToml =
                toml::from_str(&format!("provider = \"{alias}\"")).expect("huggingface alias");
            assert_eq!(parsed.provider, ProviderKind::Huggingface);
        }

        for alias in ["deepinfra", "deep-infra", "deep_infra"] {
            assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::Deepinfra));

            let parsed: ConfigToml =
                toml::from_str(&format!("provider = \"{alias}\"")).expect("deepinfra alias");
            assert_eq!(parsed.provider, ProviderKind::Deepinfra);
        }

        let parsed: ConfigToml =
            toml::from_str("provider = \"ark-wanjie\"").expect("wanjie provider alias");
        assert_eq!(parsed.provider, ProviderKind::WanjieArk);

        let parsed: ConfigToml =
            toml::from_str("provider = \"silicon-flow\"").expect("siliconflow provider alias");
        assert_eq!(parsed.provider, ProviderKind::Siliconflow);
    }

    #[test]
    fn unknown_provider_error_lists_huggingface() {
        let mut config = ConfigToml::default();
        let err = config
            .set_value("provider", "not-a-provider")
            .expect_err("unknown provider should fail");
        let message = err.to_string();
        assert!(message.contains("unknown provider 'not-a-provider'"));
        assert!(message.contains("huggingface"));
    }

    #[test]
    fn provider_kind_accepts_legacy_deepseek_cn_aliases() {
        for alias in [
            "deepseek-cn",
            "deepseek_china",
            "deepseekcn",
            "deepseek-china",
        ] {
            assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::Deepseek));

            let parsed: ConfigToml =
                toml::from_str(&format!("provider = \"{alias}\"")).expect("legacy provider alias");
            assert_eq!(parsed.provider, ProviderKind::Deepseek);
        }
    }

    #[test]
    fn provider_metadata_registry_covers_every_provider_kind_once() {
        let providers = provider::all_providers();
        assert_eq!(providers.len(), ProviderKind::ALL.len());

        for (kind, provider) in ProviderKind::ALL.iter().zip(providers.iter()) {
            assert_eq!(provider.kind(), *kind);
            assert_eq!(provider.id(), kind.as_str());
            assert_eq!(kind.provider().id(), kind.as_str());
        }

        let mut ids = std::collections::BTreeSet::new();
        for provider in providers {
            assert!(ids.insert(provider.id()), "duplicate provider id");
        }
    }

    #[test]
    fn provider_metadata_lookup_does_not_fall_back_to_deepseek() {
        assert!(provider::lookup_provider("not-a-provider").is_none());
        assert!(provider::resolve_provider("not-a-provider").is_none());
        assert!(provider::lookup_provider("deepseek-cn").is_none());
        assert_eq!(
            provider::resolve_provider("deepseek-cn")
                .expect("legacy alias resolves")
                .kind(),
            ProviderKind::Deepseek
        );
    }

    #[test]
    fn provider_metadata_preserves_alias_and_config_key_semantics() {
        assert_eq!(
            provider::resolve_provider("open_router")
                .expect("openrouter alias")
                .kind(),
            ProviderKind::Openrouter
        );
        assert_eq!(
            provider::resolve_provider("xiaomi")
                .expect("xiaomi alias")
                .kind(),
            ProviderKind::XiaomiMimo
        );
        assert_eq!(
            provider::resolve_provider("kimi")
                .expect("kimi alias")
                .kind(),
            ProviderKind::Moonshot
        );
        assert_eq!(
            provider::resolve_provider("hf")
                .expect("huggingface alias")
                .kind(),
            ProviderKind::Huggingface
        );

        let siliconflow_cn =
            provider::resolve_provider("siliconflow-cn").expect("siliconflow-cn alias resolves");
        assert_eq!(siliconflow_cn.kind(), ProviderKind::SiliconflowCN);
        assert_eq!(siliconflow_cn.id(), "siliconflow-CN");
        assert_eq!(siliconflow_cn.provider_config_key(), "siliconflow_cn");

        let config = ProvidersToml::default();
        let shared_table = config.for_provider(ProviderKind::SiliconflowCN);
        assert!(!std::ptr::eq(
            shared_table,
            config.for_provider(ProviderKind::Siliconflow)
        ));
    }

    #[test]
    fn provider_metadata_defaults_match_runtime_helpers() {
        for kind in ProviderKind::ALL {
            let provider = kind.provider();
            assert_eq!(provider.default_model(), default_model_for_provider(kind));
            assert_eq!(
                provider.default_base_url(),
                default_base_url_for_provider(kind)
            );
            assert!(!provider.display_name().trim().is_empty());
            assert!(!provider.env_vars().is_empty());
            // OpenAI Codex (ChatGPT) speaks the Responses API and Anthropic
            // speaks the native Messages API; every other built-in provider
            // is OpenAI-compatible Chat Completions.
            let expected_wire = match kind {
                ProviderKind::OpenaiCodex => provider::WireFormat::Responses,
                ProviderKind::Anthropic => provider::WireFormat::AnthropicMessages,
                _ => provider::WireFormat::ChatCompletions,
            };
            assert_eq!(provider.wire(), expected_wire);
        }
    }

    #[test]
    fn openrouter_provider_defaults_to_canonical_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Openrouter,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.base_url, DEFAULT_OPENROUTER_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_OPENROUTER_MODEL);
    }

    #[test]
    fn xiaomi_mimo_provider_defaults_to_canonical_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::XiaomiMimo,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.base_url, DEFAULT_XIAOMI_MIMO_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_XIAOMI_MIMO_MODEL);
    }

    #[test]
    fn xiaomi_provider_alias_table_maps_to_mimo_runtime_config() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config: ConfigToml = toml::from_str(
            r#"
provider = "xiaomi-mimo"
default_text_model = "deepseek/deepseek-v4-pro"

[providers.xiaomi]
api_key = "mimo-table-key"
base_url = "https://token-plan-sgp.xiaomimimo.com/v1"
model = "mimo-v2.5-pro"
"#,
        )
        .expect("xiaomi provider alias config");

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.api_key.as_deref(), Some("mimo-table-key"));
        assert_eq!(
            resolved.base_url,
            "https://token-plan-sgp.xiaomimimo.com/v1"
        );
        assert_eq!(resolved.model, DEFAULT_XIAOMI_MIMO_MODEL);
    }

    #[test]
    fn xiaomi_token_plan_key_rewrites_saved_pay_as_you_go_base_url() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config: ConfigToml = toml::from_str(
            r#"
provider = "xiaomi-mimo"

[providers.xiaomi_mimo]
api_key = "tp-test-token-plan-key"
base_url = "https://api.xiaomimimo.com/v1"
model = "mimo-v2.5-pro"
"#,
        )
        .expect("xiaomi token-plan config");

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.base_url, DEFAULT_XIAOMI_MIMO_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_XIAOMI_MIMO_MODEL);
    }

    #[test]
    fn xiaomi_mimo_token_plan_mode_accepts_region_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config: ConfigToml = toml::from_str(
            r#"
provider = "mimo"

[providers.mimo]
mode = "token-plan-ams"
"#,
        )
        .expect("xiaomi token-plan region config");

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.base_url, XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL);
    }

    #[test]
    fn xiaomi_mimo_unknown_mode_stays_on_token_plan_endpoint() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config: ConfigToml = toml::from_str(
            r#"
provider = "mimo"

[providers.mimo]
mode = "token-plan-usa"
"#,
        )
        .expect("xiaomi token-plan unknown mode config");

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.base_url, DEFAULT_XIAOMI_MIMO_BASE_URL);
    }

    #[test]
    fn xiaomi_mimo_aliases_resolve_to_canonical_models() {
        assert_eq!(
            normalize_model_for_provider(ProviderKind::XiaomiMimo, "omni"),
            "mimo-v2.5"
        );
        assert_eq!(
            normalize_model_for_provider(ProviderKind::XiaomiMimo, "tts"),
            "mimo-v2.5-tts"
        );
        assert_eq!(
            normalize_model_for_provider(ProviderKind::XiaomiMimo, "voice-design"),
            "mimo-v2.5-tts-voicedesign"
        );
        assert_eq!(
            normalize_model_for_provider(ProviderKind::XiaomiMimo, "voiceclone"),
            "mimo-v2.5-tts-voiceclone"
        );
        assert_eq!(
            normalize_model_for_provider(ProviderKind::XiaomiMimo, "custom-mimo-model"),
            "custom-mimo-model"
        );
    }

    #[test]
    fn zai_aliases_resolve_to_canonical_models() {
        // GLM-5.2 is the default; the glm-5.1 alias must still resolve to 5.1
        // (not to the default), and GLM-5-Turbo resolves to its own id.
        assert_eq!(
            normalize_model_for_provider(ProviderKind::Zai, "glm-5.1"),
            ZAI_GLM_5_1_MODEL
        );
        assert_eq!(
            normalize_model_for_provider(ProviderKind::Zai, "glm-5-2"),
            DEFAULT_ZAI_MODEL
        );
        assert_eq!(DEFAULT_ZAI_MODEL, ZAI_GLM_5_2_MODEL);
        assert_eq!(
            normalize_model_for_provider(ProviderKind::Zai, "glm-5-turbo"),
            ZAI_GLM_5_TURBO_MODEL
        );
        assert_eq!(
            normalize_model_for_provider(ProviderKind::Zai, "custom-glm-preview"),
            "custom-glm-preview"
        );
    }

    #[test]
    fn novita_provider_defaults_to_canonical_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Novita,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Novita);
        assert_eq!(resolved.base_url, DEFAULT_NOVITA_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_NOVITA_MODEL);
    }

    #[test]
    fn fireworks_provider_defaults_to_canonical_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Fireworks,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Fireworks);
        assert_eq!(resolved.base_url, DEFAULT_FIREWORKS_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_FIREWORKS_MODEL);
    }

    #[test]
    fn siliconflow_provider_defaults_to_canonical_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Siliconflow,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.base_url, DEFAULT_SILICONFLOW_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_SILICONFLOW_MODEL);
    }

    #[test]
    fn siliconflow_cn_config_falls_back_to_shared_table_when_unset() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::SiliconflowCN,
            ..ConfigToml::default()
        };
        config.providers.siliconflow.api_key = Some("sf-shared-key".to_string());
        config.providers.siliconflow.base_url = Some(DEFAULT_SILICONFLOW_BASE_URL.to_string());
        config.providers.siliconflow.model = Some("deepseek-chat".to_string());
        config.providers.siliconflow_cn.base_url =
            Some(DEFAULT_SILICONFLOW_CN_BASE_URL.to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::SiliconflowCN);
        assert_eq!(resolved.api_key.as_deref(), Some("sf-shared-key"));
        assert_eq!(resolved.base_url, DEFAULT_SILICONFLOW_CN_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_SILICONFLOW_FLASH_MODEL);
    }

    #[test]
    fn moonshot_provider_defaults_to_kimi_k27_code() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Moonshot,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.base_url, DEFAULT_MOONSHOT_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_MOONSHOT_MODEL);
    }

    #[test]
    fn zai_stepfun_and_minimax_default_to_first_party_routes() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();

        for (provider, expected_base_url, expected_model) in [
            (ProviderKind::Zai, DEFAULT_ZAI_BASE_URL, DEFAULT_ZAI_MODEL),
            (
                ProviderKind::Stepfun,
                DEFAULT_STEPFUN_BASE_URL,
                DEFAULT_STEPFUN_MODEL,
            ),
            (
                ProviderKind::Minimax,
                DEFAULT_MINIMAX_BASE_URL,
                DEFAULT_MINIMAX_MODEL,
            ),
        ] {
            let config = ConfigToml {
                provider,
                ..ConfigToml::default()
            };
            let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

            assert_eq!(resolved.provider, provider);
            assert_eq!(resolved.base_url, expected_base_url);
            assert_eq!(resolved.model, expected_model);
        }
    }

    #[test]
    fn first_party_provider_env_model_overrides_pass_through() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "minimax");
            env::set_var("MINIMAX_MODEL", "MiniMax-M2.7-highspeed");
            env::set_var("MINIMAX_BASE_URL", "https://minimax.example/v1");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Minimax);
        assert_eq!(resolved.base_url, "https://minimax.example/v1");
        assert_eq!(resolved.model, "MiniMax-M2.7-highspeed");
    }

    #[test]
    fn minimax_env_model_override_canonicalizes_known_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "minimax");
            env::set_var("MINIMAX_MODEL", "minimax-m2-5-highspeed");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Minimax);
        assert_eq!(resolved.model, "MiniMax-M2.5-highspeed");
    }

    #[test]
    fn moonshot_provider_preserves_explicit_kimi_k26() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::Moonshot,
            ..ConfigToml::default()
        };
        config.providers.moonshot.model = Some("kimi-k2.6".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.model, MOONSHOT_KIMI_K2_6_MODEL);
    }

    #[test]
    fn moonshot_kimi_oauth_uses_kimi_code_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::Moonshot,
            ..ConfigToml::default()
        };
        config.providers.moonshot.auth_mode = Some("kimi_oauth".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.auth_mode.as_deref(), Some("kimi_oauth"));
        assert_eq!(resolved.base_url, DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_KIMI_CODE_MODEL);
        assert_eq!(resolved.api_key, None);
        assert_eq!(resolved.api_key_source, None);
    }

    #[test]
    fn moonshot_kimi_code_api_key_endpoint_defaults_to_kimi_for_coding() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::Moonshot,
            ..ConfigToml::default()
        };
        config.providers.moonshot.api_key = Some("kimi-code-key".to_string());
        config.providers.moonshot.base_url = Some(DEFAULT_KIMI_CODE_BASE_URL.to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.auth_mode, None);
        assert_eq!(resolved.base_url, DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_KIMI_CODE_MODEL);
        assert_eq!(resolved.api_key.as_deref(), Some("kimi-code-key"));
        assert_eq!(
            resolved.api_key_source,
            Some(RuntimeApiKeySource::ConfigFile)
        );
    }

    /// `CODEWHALE_PROVIDER` is the user-facing env alias for switching the
    /// active provider. It must be honored by the runtime resolver and win
    /// over a root `provider = "deepseek"` config entry.
    #[test]
    fn codewhale_provider_env_switches_active_provider() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only env mutation guarded by env_lock().
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "moonshot");
        }
        let mut config = ConfigToml {
            provider: ProviderKind::Deepseek,
            ..ConfigToml::default()
        };
        config.providers.moonshot.api_key = Some("kimi-code-key".to_string());
        config.providers.moonshot.base_url = Some(DEFAULT_KIMI_CODE_BASE_URL.to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(
            resolved.provider_source,
            ProviderSource::Env("CODEWHALE_PROVIDER")
        );
        assert_eq!(resolved.base_url, DEFAULT_KIMI_CODE_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_KIMI_CODE_MODEL);
        assert_eq!(resolved.api_key.as_deref(), Some("kimi-code-key"));
    }

    /// When both `CODEWHALE_PROVIDER` and the legacy `DEEPSEEK_PROVIDER`
    /// are set, the public alias wins — a user adopting `CODEWHALE_*` in a
    /// fresh shell config is not tripped up by a stale legacy export still
    /// living in their dotfiles.
    #[test]
    fn codewhale_provider_env_wins_over_deepseek_provider_env() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only env mutation guarded by env_lock().
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "moonshot");
            env::set_var("DEEPSEEK_PROVIDER", "openrouter");
        }
        let config = ConfigToml {
            provider: ProviderKind::Deepseek,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(
            resolved.provider_source,
            ProviderSource::Env("CODEWHALE_PROVIDER")
        );
    }

    #[test]
    fn legacy_deepseek_provider_env_records_provider_source() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only env mutation guarded by env_lock().
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "openrouter");
        }
        let config = ConfigToml {
            provider: ProviderKind::Deepseek,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Openrouter);
        assert_eq!(
            resolved.provider_source,
            ProviderSource::Env("DEEPSEEK_PROVIDER")
        );
    }

    #[test]
    fn cli_provider_records_provider_source() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only env mutation guarded by env_lock().
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "moonshot");
        }
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Openai),
            ..CliRuntimeOverrides::default()
        };
        let config = ConfigToml {
            provider: ProviderKind::Deepseek,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Openai);
        assert_eq!(resolved.provider_source, ProviderSource::Cli);
    }

    #[test]
    fn config_provider_records_provider_source() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Moonshot,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.provider_source, ProviderSource::Config);
    }

    /// `CODEWHALE_MODEL` is the user-facing env alias for picking a model
    /// against the active provider. It must be honored by the runtime
    /// resolver in place of `DEEPSEEK_MODEL`.
    #[test]
    fn codewhale_model_env_alias_overrides_default_for_active_provider() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only env mutation guarded by env_lock().
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "moonshot");
            env::set_var("CODEWHALE_MODEL", "custom-kimi-test-model");
        }
        let config = ConfigToml::default();

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.model, "custom-kimi-test-model");
    }

    #[test]
    fn blank_codewhale_model_env_alias_does_not_override_default_for_active_provider() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only env mutation guarded by env_lock().
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "moonshot");
            env::set_var("CODEWHALE_MODEL", "   ");
        }
        let config = ConfigToml::default();

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.model, DEFAULT_MOONSHOT_MODEL);
    }

    #[test]
    fn deepseek_default_text_model_legacy_alias_still_overrides_active_provider_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only env mutation guarded by env_lock().
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "moonshot");
            env::set_var("DEEPSEEK_DEFAULT_TEXT_MODEL", "legacy-env-model");
        }
        let config = ConfigToml::default();

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.model, "legacy-env-model");
    }

    #[test]
    fn wanjie_ark_provider_defaults_to_openai_compatible_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::WanjieArk,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::WanjieArk);
        assert_eq!(resolved.base_url, DEFAULT_WANJIE_ARK_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_WANJIE_ARK_MODEL);
    }

    #[test]
    fn sglang_provider_defaults_to_local_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Sglang,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Sglang);
        assert_eq!(resolved.base_url, DEFAULT_SGLANG_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_SGLANG_MODEL);
    }

    #[test]
    fn vllm_provider_defaults_to_local_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Vllm,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Vllm);
        assert_eq!(resolved.base_url, DEFAULT_VLLM_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_VLLM_MODEL);
    }

    #[test]
    fn ollama_provider_defaults_to_local_endpoint_and_small_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Ollama,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Ollama);
        assert_eq!(resolved.base_url, DEFAULT_OLLAMA_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_OLLAMA_MODEL);
        assert_eq!(resolved.api_key, None);
    }

    #[test]
    fn self_hosted_providers_do_not_probe_secret_store_by_default() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let store = Arc::new(RecordingSecretsStore::with_value("secret-store-key"));
        let secrets = Secrets::new(store.clone());

        for provider in [
            ProviderKind::Sglang,
            ProviderKind::Vllm,
            ProviderKind::Ollama,
        ] {
            let config = ConfigToml {
                provider,
                ..ConfigToml::default()
            };

            let resolved = config
                .resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

            assert_eq!(resolved.provider, provider);
            assert_eq!(resolved.api_key, None);
        }

        assert!(
            store.gets.lock().unwrap().is_empty(),
            "self-hosted providers should not read the secret store by default"
        );
    }

    #[test]
    fn self_hosted_api_key_auth_can_use_secret_store_when_requested() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let store = Arc::new(RecordingSecretsStore::with_value("secret-store-key"));
        let secrets = Secrets::new(store.clone());
        let config = ConfigToml {
            provider: ProviderKind::Ollama,
            auth_mode: Some("api_key".to_string()),
            ..ConfigToml::default()
        };

        let resolved =
            config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

        assert_eq!(resolved.api_key.as_deref(), Some("secret-store-key"));
        assert_eq!(store.gets.lock().unwrap().as_slice(), ["ollama"]);
    }

    #[test]
    fn moonshot_api_key_mode_can_use_secret_store_by_default() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let store = Arc::new(RecordingSecretsStore::with_value("secret-store-key"));
        let secrets = Secrets::new(store.clone());
        let config = ConfigToml {
            provider: ProviderKind::Moonshot,
            ..ConfigToml::default()
        };

        let resolved =
            config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

        assert_eq!(resolved.api_key.as_deref(), Some("secret-store-key"));
        assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Keyring));
        assert_eq!(store.gets.lock().unwrap().as_slice(), ["moonshot"]);
    }

    #[test]
    fn loopback_custom_deepseek_base_url_does_not_probe_secret_store_by_default() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let store = Arc::new(RecordingSecretsStore::with_value("stale-deepseek-key"));
        let secrets = Secrets::new(store.clone());
        let config = ConfigToml {
            base_url: Some("http://127.0.0.1:8000/v1".to_string()),
            ..ConfigToml::default()
        };

        let resolved =
            config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

        assert_eq!(resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(resolved.api_key, None);
        assert!(
            store.gets.lock().unwrap().is_empty(),
            "loopback custom endpoints should not read macOS Keychain or any secret store"
        );
    }

    #[test]
    fn ollama_provider_preserves_model_tags() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Ollama),
            model: Some("deepseek-coder-v2:16b".to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Ollama);
        assert_eq!(resolved.model, "deepseek-coder-v2:16b");
    }

    #[test]
    fn ollama_env_overrides_provider_base_url_and_optional_key() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "ollama-local");
            env::set_var("OLLAMA_BASE_URL", "http://ollama.example/v1");
            env::set_var("OLLAMA_API_KEY", "ollama-env-key");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Ollama);
        assert_eq!(resolved.base_url, "http://ollama.example/v1");
        assert_eq!(resolved.api_key.as_deref(), Some("ollama-env-key"));
    }

    #[test]
    fn openrouter_env_overrides_key_and_model_when_config_missing() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "openrouter");
            env::set_var("OPENROUTER_API_KEY", "or-env-key");
            env::set_var("OPENROUTER_MODEL", "deepseek-v4-flash");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.api_key.as_deref(), Some("or-env-key"));
        assert_eq!(resolved.base_url, DEFAULT_OPENROUTER_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_OPENROUTER_FLASH_MODEL);
    }

    #[test]
    fn xiaomi_mimo_env_overrides_provider_key_base_url_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "xiaomi-mimo");
            env::set_var("MIMO_API_KEY", "mimo-env-key");
            env::set_var("MIMO_BASE_URL", "https://mimo-gateway.example/v1");
            env::set_var("MIMO_MODEL", "mimo-v2.5");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.api_key.as_deref(), Some("mimo-env-key"));
        assert_eq!(resolved.base_url, "https://mimo-gateway.example/v1");
        assert_eq!(resolved.model, "mimo-v2.5");
    }

    #[test]
    fn xiaomi_mimo_env_token_plan_mode_uses_token_plan_key_and_endpoint() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "xiaomi-mimo");
            env::set_var("XIAOMI_MIMO_MODE", "token-plan-cn");
            env::set_var("XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "tp-env-key");
            env::set_var("XIAOMI_MIMO_API_KEY", "sk-env-key");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.api_key.as_deref(), Some("tp-env-key"));
        assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Env));
        assert_eq!(resolved.base_url, XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL);
    }

    #[test]
    fn xiaomi_mimo_env_pay_as_you_go_mode_prefers_standard_key() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "xiaomi-mimo");
            env::set_var("XIAOMI_MIMO_MODE", "pay-as-you-go");
            env::set_var("XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "tp-env-key");
            env::set_var("XIAOMI_MIMO_API_KEY", "sk-env-key");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.api_key.as_deref(), Some("sk-env-key"));
        assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Env));
        assert_eq!(resolved.base_url, XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL);
    }

    #[test]
    fn novita_env_overrides_key_and_model_when_config_missing() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "novita");
            env::set_var("NOVITA_API_KEY", "novita-env-key");
            env::set_var("NOVITA_MODEL", "deepseek-v4-flash");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Novita);
        assert_eq!(resolved.api_key.as_deref(), Some("novita-env-key"));
        assert_eq!(resolved.base_url, DEFAULT_NOVITA_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_NOVITA_FLASH_MODEL);
    }

    #[test]
    fn fireworks_env_overrides_key_and_model_when_config_missing() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "fireworks");
            env::set_var("FIREWORKS_API_KEY", "fw-env-key");
            env::set_var(
                "FIREWORKS_MODEL",
                "accounts/fireworks/models/account-specific-model",
            );
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Fireworks);
        assert_eq!(resolved.api_key.as_deref(), Some("fw-env-key"));
        assert_eq!(resolved.base_url, DEFAULT_FIREWORKS_BASE_URL);
        assert_eq!(
            resolved.model,
            "accounts/fireworks/models/account-specific-model"
        );
    }

    #[test]
    fn siliconflow_env_overrides_key_base_url_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "siliconflow");
            env::set_var("SILICONFLOW_API_KEY", "sf-env-key");
            env::set_var("SILICONFLOW_BASE_URL", "https://sf-mirror.example/v1");
            env::set_var("SILICONFLOW_MODEL", "deepseek-v4-flash");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.api_key.as_deref(), Some("sf-env-key"));
        assert_eq!(resolved.base_url, "https://sf-mirror.example/v1");
        assert_eq!(resolved.model, "deepseek-v4-flash");
    }

    #[test]
    fn arcee_provider_defaults_to_direct_api_endpoint_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Arcee,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.base_url, DEFAULT_ARCEE_BASE_URL);
        assert_eq!(resolved.model, DEFAULT_ARCEE_MODEL);
    }

    #[test]
    fn arcee_env_overrides_key_base_url_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "arcee");
            env::set_var("ARCEE_API_KEY", "arcee-env-key");
            env::set_var("ARCEE_BASE_URL", "https://arcee-mirror.example/api/v1");
            env::set_var("ARCEE_MODEL", "trinity-large-preview");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.api_key.as_deref(), Some("arcee-env-key"));
        assert_eq!(resolved.base_url, "https://arcee-mirror.example/api/v1");
        assert_eq!(resolved.model, "trinity-large-preview");
    }

    #[test]
    fn arcee_provider_config_overrides_runtime_defaults() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::Arcee,
            ..ConfigToml::default()
        };
        config.providers.arcee.api_key = Some("arcee-file-key".to_string());
        config.providers.arcee.base_url = Some(DEFAULT_ARCEE_BASE_URL.to_string());
        config.providers.arcee.model = Some("arcee-trinity-large-preview".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.api_key.as_deref(), Some("arcee-file-key"));
        assert_eq!(resolved.base_url, DEFAULT_ARCEE_BASE_URL);
        assert_eq!(resolved.model, ARCEE_TRINITY_LARGE_PREVIEW_MODEL);
    }

    #[test]
    fn huggingface_env_precedence_prefers_documented_names() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "hf");
            env::set_var("HUGGINGFACE_API_KEY", "hf-full-key");
            env::set_var("HF_TOKEN", "hf-token-fallback");
            env::set_var("HUGGINGFACE_BASE_URL", "https://hf-full.example/v1");
            env::set_var("HF_BASE_URL", "https://hf-short.example/v1");
            env::set_var("HUGGINGFACE_MODEL", "org/full-model");
            env::set_var("HF_MODEL", "org/short-model");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Huggingface);
        assert_eq!(resolved.api_key.as_deref(), Some("hf-full-key"));
        assert_eq!(resolved.base_url, "https://hf-full.example/v1");
        assert_eq!(resolved.model, "org/full-model");
    }

    #[test]
    fn huggingface_short_env_fallbacks_resolve_when_primary_names_are_absent() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "huggingface");
            env::set_var("HF_TOKEN", "hf-token-fallback");
            env::set_var("HF_BASE_URL", "https://hf-short.example/v1");
            env::set_var("HF_MODEL", "org/short-model");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Huggingface);
        assert_eq!(resolved.api_key.as_deref(), Some("hf-token-fallback"));
        assert_eq!(resolved.base_url, "https://hf-short.example/v1");
        assert_eq!(resolved.model, "org/short-model");
    }

    #[test]
    fn huggingface_token_fallback_resolves_when_primary_api_key_is_blank() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "huggingface");
            env::set_var("HUGGINGFACE_API_KEY", " ");
            env::set_var("HF_TOKEN", "hf-token-fallback");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Huggingface);
        assert_eq!(resolved.api_key.as_deref(), Some("hf-token-fallback"));
    }

    #[test]
    fn siliconflow_cn_base_url_env_normalizes_model_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "siliconflow");
            env::set_var("SILICONFLOW_API_KEY", "sf-env-key");
            env::set_var("SILICONFLOW_BASE_URL", "https://api.siliconflow.cn/v1");
        }

        for (alias, expected) in [
            ("deepseek-v4-flash", DEFAULT_SILICONFLOW_FLASH_MODEL),
            ("deepseek-reasoner", DEFAULT_SILICONFLOW_MODEL),
        ] {
            // Safety: test-only environment mutation guarded by a module mutex.
            unsafe {
                env::set_var("SILICONFLOW_MODEL", alias);
            }

            let resolved =
                ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

            assert_eq!(resolved.provider, ProviderKind::Siliconflow);
            assert_eq!(resolved.base_url, "https://api.siliconflow.cn/v1");
            assert_eq!(resolved.model, expected);
        }
    }

    #[test]
    fn wanjie_ark_env_api_key_and_base_url_fall_back_when_config_missing() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "wanjie-ark");
            env::set_var("WANJIE_ARK_API_KEY", "wanjie-env-key");
            env::set_var("WANJIE_ARK_BASE_URL", "https://wanjie.example/api/v1");
            env::set_var("WANJIE_ARK_MODEL", "account-model-id");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::WanjieArk);
        assert_eq!(resolved.api_key.as_deref(), Some("wanjie-env-key"));
        assert_eq!(resolved.base_url, "https://wanjie.example/api/v1");
        assert_eq!(resolved.model, "account-model-id");
    }

    #[test]
    fn volcengine_env_aliases_override_key_base_url_and_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "volcengine");
            env::set_var("ARK_API_KEY", "volcengine-env-key");
            env::set_var("ARK_BASE_URL", "https://volcengine.example/api/coding/v3");
            env::set_var("VOLCENGINE_ARK_MODEL", "DeepSeek-V4-Flash");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Volcengine);
        assert_eq!(resolved.api_key.as_deref(), Some("volcengine-env-key"));
        assert_eq!(
            resolved.base_url,
            "https://volcengine.example/api/coding/v3"
        );
        assert_eq!(resolved.model, "DeepSeek-V4-Flash");
    }

    #[test]
    fn openrouter_provider_normalizes_flash_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Openrouter),
            model: Some("deepseek-v4-flash".to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.model, DEFAULT_OPENROUTER_FLASH_MODEL);
    }

    #[test]
    fn qwen3_6_plus_resolves_to_canonical_on_openrouter() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Openrouter,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides {
            model: Some("qwen3.6-plus".to_string()),
            ..CliRuntimeOverrides::default()
        });

        assert_eq!(resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.model, OPENROUTER_QWEN_3_6_PLUS_MODEL);
    }

    #[test]
    fn qwen3_6_plus_alias_qwen_dash_resolves() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Openrouter,
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides {
            model: Some("qwen-3.6-plus".to_string()),
            ..CliRuntimeOverrides::default()
        });

        assert_eq!(resolved.model, OPENROUTER_QWEN_3_6_PLUS_MODEL);
    }

    #[test]
    fn openrouter_provider_normalizes_recent_large_model_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();

        for (alias, expected) in [
            (
                "trinity-large-thinking",
                OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL,
            ),
            ("qwen3.6-flash", OPENROUTER_QWEN_3_6_FLASH_MODEL),
            ("qwen3.6-35b-a3b", OPENROUTER_QWEN_3_6_35B_A3B_MODEL),
            ("qwen3.6-max-preview", OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL),
            ("qwen3.6-plus", OPENROUTER_QWEN_3_6_PLUS_MODEL),
            ("mimo-v2.5-pro", OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL),
            ("kimi-k2.7-code", OPENROUTER_KIMI_K2_7_CODE_MODEL),
            ("kimi", OPENROUTER_KIMI_K2_7_CODE_MODEL),
            ("kimi-k2.6", OPENROUTER_KIMI_K2_6_MODEL),
            ("minimax-m3", OPENROUTER_MINIMAX_M3_MODEL),
            ("minimax-2.7", OPENROUTER_MINIMAX_2_7_MODEL),
            ("gemma-4-31b-it", OPENROUTER_GEMMA_4_31B_MODEL),
            ("glm-5.1", OPENROUTER_GLM_5_1_MODEL),
            ("glm-5.2", OPENROUTER_GLM_5_2_MODEL),
        ] {
            let cli = CliRuntimeOverrides {
                provider: Some(ProviderKind::Openrouter),
                model: Some(alias.to_string()),
                ..CliRuntimeOverrides::default()
            };

            let resolved = ConfigToml::default().resolve_runtime_options(&cli);

            assert_eq!(resolved.provider, ProviderKind::Openrouter);
            assert_eq!(resolved.model, expected);
        }
    }

    #[test]
    fn novita_provider_normalizes_flash_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Novita),
            model: Some("deepseek-v4-flash".to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Novita);
        assert_eq!(resolved.model, DEFAULT_NOVITA_FLASH_MODEL);
    }

    #[test]
    fn siliconflow_provider_normalizes_flash_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Siliconflow),
            model: Some("deepseek-v4-flash".to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.model, DEFAULT_SILICONFLOW_FLASH_MODEL);
    }

    #[test]
    fn siliconflow_provider_normalizes_reasoning_aliases_to_pro() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();

        for alias in ["deepseek-reasoner", "deepseek-r1"] {
            let cli = CliRuntimeOverrides {
                provider: Some(ProviderKind::Siliconflow),
                model: Some(alias.to_string()),
                ..CliRuntimeOverrides::default()
            };

            let resolved = ConfigToml::default().resolve_runtime_options(&cli);

            assert_eq!(resolved.provider, ProviderKind::Siliconflow);
            assert_eq!(resolved.model, DEFAULT_SILICONFLOW_MODEL);
        }
    }

    #[test]
    fn siliconflow_provider_preserves_deepseek_v3_2_alias() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Siliconflow),
            model: Some("deepseek-v3.2".to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.model, "deepseek-v3.2");
    }

    #[test]
    fn sglang_provider_normalizes_flash_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Sglang),
            model: Some("deepseek-v4-flash".to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Sglang);
        assert_eq!(resolved.model, DEFAULT_SGLANG_FLASH_MODEL);
    }

    #[test]
    fn vllm_provider_normalizes_flash_aliases() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Vllm),
            model: Some("deepseek-v4-flash".to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Vllm);
        assert_eq!(resolved.model, DEFAULT_VLLM_FLASH_MODEL);
    }

    #[test]
    fn openrouter_provider_specific_config_overrides_env() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::Openrouter,
            ..ConfigToml::default()
        };
        config.providers.openrouter.api_key = Some("file-key".to_string());
        config.providers.openrouter.base_url = Some("https://or-mirror.example/v1".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.api_key.as_deref(), Some("file-key"));
        assert_eq!(resolved.base_url, "https://or-mirror.example/v1");
    }

    #[test]
    fn openrouter_custom_base_url_preserves_provider_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::Openrouter,
            ..ConfigToml::default()
        };
        config.providers.openrouter.base_url = Some("https://gateway.example.com/v1".to_string());
        config.providers.openrouter.model = Some("DeepSeek-V4-Pro".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.base_url, "https://gateway.example.com/v1");
        assert_eq!(resolved.model, "DeepSeek-V4-Pro");
    }

    #[test]
    fn fireworks_custom_base_url_preserves_provider_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::Fireworks,
            ..ConfigToml::default()
        };
        config.providers.fireworks.base_url = Some("https://my-gateway.example/v1".to_string());
        config.providers.fireworks.model = Some("DeepSeek-V4-Pro".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Fireworks);
        assert_eq!(resolved.base_url, "https://my-gateway.example/v1");
        // Custom base URL skips provider-specific model prefixing.
        assert_eq!(resolved.model, "DeepSeek-V4-Pro");
    }

    #[test]
    fn siliconflow_custom_base_url_preserves_provider_model() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let mut config = ConfigToml {
            provider: ProviderKind::Siliconflow,
            ..ConfigToml::default()
        };
        config.providers.siliconflow.base_url = Some("https://my-gateway.example/v1".to_string());
        config.providers.siliconflow.model = Some("DeepSeek-V4-Pro".to_string());

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.base_url, "https://my-gateway.example/v1");
        assert_eq!(resolved.model, "DeepSeek-V4-Pro");
    }

    #[test]
    fn config_file_resolves_above_env_and_keyring() {
        use codewhale_secrets::KeyringStore;
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "env-key") };

        let store = std::sync::Arc::new(codewhale_secrets::InMemoryKeyringStore::new());
        store.set("deepseek", "ring-key").unwrap();
        let secrets = Secrets::new(store);

        let mut config = ConfigToml::default();
        config.providers.deepseek.api_key = Some("file-key".to_string());

        let resolved =
            config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);
        assert_eq!(resolved.api_key.as_deref(), Some("file-key"));
        assert_eq!(
            resolved.api_key_source,
            Some(RuntimeApiKeySource::ConfigFile)
        );

        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
    }

    #[test]
    fn env_resolves_when_config_file_and_keyring_empty() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "env-key") };

        let secrets = Secrets::new(std::sync::Arc::new(
            codewhale_secrets::InMemoryKeyringStore::new(),
        ));
        let config = ConfigToml::default();

        let resolved =
            config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);
        assert_eq!(resolved.api_key.as_deref(), Some("env-key"));
        assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Env));

        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
    }

    #[test]
    fn config_file_resolves_when_keyring_and_env_empty() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();

        let secrets = Secrets::new(std::sync::Arc::new(
            codewhale_secrets::InMemoryKeyringStore::new(),
        ));
        let mut config = ConfigToml::default();
        config.providers.deepseek.api_key = Some("file-key".to_string());

        let resolved =
            config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);
        assert_eq!(resolved.api_key.as_deref(), Some("file-key"));
        assert_eq!(
            resolved.api_key_source,
            Some(RuntimeApiKeySource::ConfigFile)
        );
    }

    #[test]
    fn keyring_resolves_when_config_file_empty_even_if_env_is_set() {
        use codewhale_secrets::KeyringStore;
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "stale-env-key") };

        let store = std::sync::Arc::new(codewhale_secrets::InMemoryKeyringStore::new());
        store.set("deepseek", "ring-key").unwrap();
        let secrets = Secrets::new(store);

        let resolved = ConfigToml::default()
            .resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);
        assert_eq!(resolved.api_key.as_deref(), Some("ring-key"));
        assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Keyring));

        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
    }

    #[test]
    fn cli_flag_still_overrides_keyring() {
        use codewhale_secrets::KeyringStore;
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();

        let store = std::sync::Arc::new(codewhale_secrets::InMemoryKeyringStore::new());
        store.set("deepseek", "ring-key").unwrap();
        let secrets = Secrets::new(store);

        let cli = CliRuntimeOverrides {
            api_key: Some("cli-key".to_string()),
            ..CliRuntimeOverrides::default()
        };
        let resolved = ConfigToml::default().resolve_runtime_options_with_secrets(&cli, &secrets);
        assert_eq!(resolved.api_key.as_deref(), Some("cli-key"));
        assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Cli));
    }

    #[test]
    fn provider_chain_initial_current_is_active() {
        let chain = ProviderChain::new(
            ProviderKind::NvidiaNim,
            &[ProviderKind::Deepseek, ProviderKind::Openrouter],
        );

        assert_eq!(chain.current(), ProviderKind::NvidiaNim);
        assert_eq!(chain.position(), 0);
        assert_eq!(
            chain.providers(),
            &[
                ProviderKind::NvidiaNim,
                ProviderKind::Deepseek,
                ProviderKind::Openrouter,
            ]
        );
        assert!(!chain.is_fallback_active());
    }

    #[test]
    fn provider_chain_advance_switches_to_fallback() {
        let mut chain = ProviderChain::new(
            ProviderKind::NvidiaNim,
            &[ProviderKind::Deepseek, ProviderKind::Openrouter],
        );

        assert!(chain.has_next());
        assert_eq!(chain.advance(), Some(ProviderKind::Deepseek));
        assert_eq!(chain.current(), ProviderKind::Deepseek);
        assert!(chain.is_fallback_active());
    }

    #[test]
    fn provider_chain_exhausts_returns_none() {
        let mut chain = ProviderChain::new(ProviderKind::Deepseek, &[ProviderKind::Openrouter]);

        assert_eq!(chain.advance(), Some(ProviderKind::Openrouter));
        assert!(!chain.has_next());
        assert_eq!(chain.advance(), None);
    }

    #[test]
    fn provider_chain_skips_duplicates() {
        let chain = ProviderChain::new(
            ProviderKind::Deepseek,
            &[
                ProviderKind::Deepseek,
                ProviderKind::NvidiaNim,
                ProviderKind::Deepseek,
            ],
        );

        assert_eq!(
            chain.providers(),
            &[ProviderKind::Deepseek, ProviderKind::NvidiaNim]
        );
    }

    #[test]
    fn provider_chain_remaining_counts_current_and_untried_entries() {
        let mut chain = ProviderChain::new(
            ProviderKind::Deepseek,
            &[ProviderKind::NvidiaNim, ProviderKind::Openrouter],
        );

        assert_eq!(chain.remaining(), 3);
        assert_eq!(chain.advance(), Some(ProviderKind::NvidiaNim));
        assert_eq!(chain.remaining(), 2);
    }

    #[test]
    fn config_toml_parses_fallback_providers() {
        let config: ConfigToml = toml::from_str(
            r#"
provider = "nvidia-nim"
fallback_providers = ["deepseek", "openrouter"]
"#,
        )
        .expect("fallback providers config");

        assert_eq!(config.provider, ProviderKind::NvidiaNim);
        assert_eq!(
            config.fallback_providers,
            [ProviderKind::Deepseek, ProviderKind::Openrouter]
        );
    }

    #[test]
    fn empty_fallback_providers_do_not_serialize() {
        let serialized = toml::to_string_pretty(&ConfigToml::default()).expect("config serializes");

        assert!(!serialized.contains("fallback_providers"));
    }

    #[test]
    fn fleet_exec_config_default_matches_subagent_depth() {
        // Fleet workers and standalone sub-agents share one recursion axis:
        // the fleet default equals DEFAULT_SPAWN_DEPTH (3) and affords >=3
        // nested delegation levels out of the box.
        assert_eq!(
            FleetExecConfig::default().max_spawn_depth,
            DEFAULT_SPAWN_DEPTH
        );
        assert_eq!(FleetExecConfig::default().max_spawn_depth, 3);
        const { assert!(DEFAULT_SPAWN_DEPTH <= MAX_SPAWN_DEPTH_CEILING) };
    }

    #[test]
    fn fleet_exec_config_parses_max_spawn_depth() {
        let config: ConfigToml = toml::from_str(
            r#"
[fleet.exec]
max_spawn_depth = 2
"#,
        )
        .expect("fleet exec config should parse");

        assert_eq!(config.fleet.expect("fleet config").exec.max_spawn_depth, 2);
    }

    #[test]
    fn fallback_providers_do_not_change_runtime_resolution() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::NvidiaNim,
            fallback_providers: vec![ProviderKind::Deepseek],
            ..ConfigToml::default()
        };

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
    }

    #[test]
    fn harness_posture_default_is_standard() {
        let posture = HarnessPosture::default();

        assert_eq!(
            posture,
            HarnessPosture {
                kind: HarnessPostureKind::Standard,
                max_subagents: 0,
                prefer_codebase_search: false,
                compaction_strategy: HarnessCompactionStrategy::Default,
                tool_surface: HarnessToolSurface::Full,
                safety_posture: HarnessSafetyPosture::Standard,
            }
        );
    }

    #[test]
    fn harness_posture_factories_are_typed() {
        assert_eq!(
            HarnessPosture::cache_heavy(),
            HarnessPosture {
                kind: HarnessPostureKind::CacheHeavy,
                max_subagents: 10,
                prefer_codebase_search: false,
                compaction_strategy: HarnessCompactionStrategy::PrefixCache,
                tool_surface: HarnessToolSurface::Full,
                safety_posture: HarnessSafetyPosture::Standard,
            }
        );
        assert_eq!(
            HarnessPosture::lean(),
            HarnessPosture {
                kind: HarnessPostureKind::Lean,
                max_subagents: 20,
                prefer_codebase_search: true,
                compaction_strategy: HarnessCompactionStrategy::Aggressive,
                tool_surface: HarnessToolSurface::Full,
                safety_posture: HarnessSafetyPosture::Standard,
            }
        );
    }

    #[test]
    fn harness_profile_serde_round_trips_as_a_whole_struct() {
        let profile = HarnessProfile {
            provider_route: "deepseek".to_string(),
            model_pattern: "deepseek-v4.*".to_string(),
            posture: HarnessPosture::cache_heavy(),
        };

        let json = serde_json::to_string(&profile).expect("serialize profile");
        let round_tripped: HarnessProfile =
            serde_json::from_str(&json).expect("deserialize profile");

        assert_eq!(round_tripped, profile);
    }

    #[test]
    fn config_toml_accepts_harness_profiles() {
        let config: ConfigToml = toml::from_str(
            r#"
provider = "deepseek"
model = "deepseek-v4-pro"

[[harness_profiles]]
provider_route = "deepseek"
model_pattern = "deepseek-v4.*"

[harness_profiles.posture]
kind = "cache-heavy"
max_subagents = 10
compaction_strategy = "prefix-cache"
tool_surface = "read-only"
safety_posture = "strict"
"#,
        )
        .expect("parse harness profiles");

        assert_eq!(
            config.harness_profiles,
            vec![HarnessProfile {
                provider_route: "deepseek".to_string(),
                model_pattern: "deepseek-v4.*".to_string(),
                posture: HarnessPosture {
                    kind: HarnessPostureKind::CacheHeavy,
                    max_subagents: 10,
                    prefer_codebase_search: false,
                    compaction_strategy: HarnessCompactionStrategy::PrefixCache,
                    tool_surface: HarnessToolSurface::ReadOnly,
                    safety_posture: HarnessSafetyPosture::Strict,
                },
            }]
        );
    }

    #[test]
    fn harness_profile_matches_provider_alias_and_model_wildcard() {
        let profile = HarnessProfile {
            provider_route: "xiaomi-mimo".to_string(),
            model_pattern: "mimo-v2.?-pro".to_string(),
            posture: HarnessPosture::cache_heavy(),
        };

        assert!(profile.matches_route("mimo", "mimo-v2.5-pro"));
        assert!(!profile.matches_route("mimo", "mimo-v2.50-pro"));
        assert!(!profile.matches_route("deepseek", "mimo-v2.5-pro"));
    }

    #[test]
    fn resolve_harness_profile_returns_first_matching_profile() {
        let config = ConfigToml {
            harness_profiles: vec![
                HarnessProfile {
                    provider_route: "deepseek".to_string(),
                    model_pattern: "deepseek-v4-flash".to_string(),
                    posture: HarnessPosture::lean(),
                },
                HarnessProfile {
                    provider_route: "deepseek".to_string(),
                    model_pattern: "deepseek-v4-*".to_string(),
                    posture: HarnessPosture::cache_heavy(),
                },
            ],
            ..ConfigToml::default()
        };

        let flash = config
            .resolve_harness_profile("deepseek-cn", "deepseek-v4-flash")
            .expect("exact profile should match first");
        assert_eq!(flash.posture.kind, HarnessPostureKind::Lean);

        let pro = config
            .resolve_harness_profile("deepseek", "deepseek-v4-pro")
            .expect("wildcard profile should match pro model");
        assert_eq!(pro.posture.kind, HarnessPostureKind::CacheHeavy);
    }

    #[test]
    fn resolve_harness_profile_uses_built_in_seed_when_config_has_no_match() {
        let config = ConfigToml::default();

        let xiaomi = config
            .resolve_harness_profile("xiaomi", "mimo-v2.5-pro")
            .expect("direct Xiaomi MiMo seed should resolve");
        assert_eq!(xiaomi.provider_route, "xiaomi-mimo");
        assert_eq!(xiaomi.posture.kind, HarnessPostureKind::CacheHeavy);

        let arcee = config
            .resolve_harness_profile("arcee", "trinity-large-thinking")
            .expect("direct Arcee seed should resolve");
        assert_eq!(arcee.posture.kind, HarnessPostureKind::CacheHeavy);

        let local = config
            .resolve_harness_profile("vllm", "Qwen/Qwen3.6-Coder")
            .expect("local seed should resolve");
        assert_eq!(local.posture.kind, HarnessPostureKind::Lean);
        assert!(local.posture.prefer_codebase_search);
    }

    #[test]
    fn configured_harness_profile_overrides_built_in_seed() {
        let config = ConfigToml {
            harness_profiles: vec![HarnessProfile {
                provider_route: "xiaomi-mimo".to_string(),
                model_pattern: "mimo-v2.5-pro".to_string(),
                posture: HarnessPosture {
                    kind: HarnessPostureKind::Custom,
                    max_subagents: 3,
                    prefer_codebase_search: true,
                    compaction_strategy: HarnessCompactionStrategy::Default,
                    tool_surface: HarnessToolSurface::Auto,
                    safety_posture: HarnessSafetyPosture::Strict,
                },
            }],
            ..ConfigToml::default()
        };

        let profile = config
            .resolve_harness_profile("xiaomi-mimo", "mimo-v2.5-pro")
            .expect("configured profile should match first");

        assert_eq!(profile.posture.kind, HarnessPostureKind::Custom);
        assert_eq!(profile.posture.max_subagents, 3);
        assert_eq!(profile.posture.tool_surface, HarnessToolSurface::Auto);
        assert_eq!(profile.posture.safety_posture, HarnessSafetyPosture::Strict);
    }

    #[test]
    fn resolve_harness_profile_returns_none_when_route_or_model_misses() {
        let config = ConfigToml {
            harness_profiles: vec![HarnessProfile {
                provider_route: "huggingface".to_string(),
                model_pattern: "deepseek-ai/*".to_string(),
                posture: HarnessPosture::lean(),
            }],
            ..ConfigToml::default()
        };

        assert!(
            config
                .resolve_harness_profile("openrouter", "deepseek-ai/DeepSeek-V4-Pro")
                .is_none()
        );
        assert!(
            config
                .resolve_harness_profile("deepseek", "Qwen/Qwen3.6-Coder")
                .is_none()
        );
        assert!(
            config
                .resolve_harness_profile("openai", "mimo-v2.5-pro")
                .is_none()
        );
    }

    #[test]
    fn resolving_harness_profile_does_not_change_runtime_options() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        let config = ConfigToml {
            provider: ProviderKind::Deepseek,
            model: Some("deepseek-v4-pro".to_string()),
            harness_profiles: vec![HarnessProfile {
                provider_route: "deepseek".to_string(),
                model_pattern: "deepseek-v4-*".to_string(),
                posture: HarnessPosture::lean(),
            }],
            ..ConfigToml::default()
        };

        let profile = config
            .resolve_harness_profile("deepseek", "deepseek-v4-pro")
            .expect("profile should resolve for display/future runtime");
        assert_eq!(profile.posture.kind, HarnessPostureKind::Lean);

        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
        assert_eq!(resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.model, "deepseek-v4-pro");
    }

    #[test]
    fn harness_posture_kind_rejects_unknown_values() {
        let err = toml::from_str::<ConfigToml>(
            r#"
[[harness_profiles]]
provider_route = "deepseek"
model_pattern = "deepseek-v4.*"

[harness_profiles.posture]
kind = "cahce-heavy"
"#,
        )
        .expect_err("misspelled kind should not deserialize as custom");

        assert!(err.to_string().contains("cahce-heavy"));
    }

    #[test]
    fn harness_posture_rejects_unknown_policy_keys() {
        let err = toml::from_str::<ConfigToml>(
            r#"
[[harness_profiles]]
provider_route = "deepseek"
model_pattern = "deepseek-v4.*"

[harness_profiles.posture]
kind = "custom"
unknown_policy = "surprise"
"#,
        )
        .expect_err("unknown posture keys should not be ignored");

        assert!(err.to_string().contains("unknown_policy"));
    }

    #[test]
    fn test_verbosity_resolution() {
        let _lock = env_lock();
        // Test TOML parsing
        let toml_str = r#"
            verbosity = "concise"
        "#;
        let config: ConfigToml = toml::from_str(toml_str).unwrap();
        assert_eq!(config.verbosity, Some("concise".to_string()));

        // Test Env overrides
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        unsafe {
            std::env::set_var("CODEWHALE_VERBOSITY", "normal");
        }
        let env_overrides = EnvRuntimeOverrides::load();
        assert_eq!(env_overrides.verbosity, Some("normal".to_string()));
        unsafe {
            std::env::remove_var("CODEWHALE_VERBOSITY");
        }

        // Test fallback to DEEPSEEK_VERBOSITY
        unsafe {
            std::env::set_var("DEEPSEEK_VERBOSITY", "concise");
        }
        let env_overrides = EnvRuntimeOverrides::load();
        assert_eq!(env_overrides.verbosity, Some("concise".to_string()));
        unsafe {
            std::env::remove_var("DEEPSEEK_VERBOSITY");
        }
    }
}
