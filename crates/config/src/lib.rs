pub mod provider;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
#[cfg(unix)]
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
const OPENROUTER_KIMI_K2_6_MODEL: &str = "moonshotai/kimi-k2.6";
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
const DEFAULT_MOONSHOT_MODEL: &str = "kimi-k2.6";
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
const DEFAULT_NOVITA_BASE_URL: &str = "https://api.novita.ai/v1";
const DEFAULT_FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";
const DEFAULT_SILICONFLOW_BASE_URL: &str = "https://api.siliconflow.com/v1";
const DEFAULT_SILICONFLOW_CN_BASE_URL: &str = "https://api.siliconflow.cn/v1";
const DEFAULT_ARCEE_BASE_URL: &str = "https://api.arcee.ai/api/v1";
const DEFAULT_HUGGINGFACE_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_HUGGINGFACE_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_HUGGINGFACE_BASE_URL: &str = "https://router.huggingface.co/v1";
const DEFAULT_SGLANG_BASE_URL: &str = "http://localhost:30000/v1";
const DEFAULT_VLLM_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_VLLM_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_VLLM_BASE_URL: &str = "http://localhost:8000/v1";
const DEFAULT_OLLAMA_MODEL: &str = "deepseek-coder:1.3b";
const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

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
}

impl ProviderKind {
    pub const ALL: [Self; 18] = [
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
        Self::SiliconflowCN,
        Self::Arcee,
        Self::Moonshot,
        Self::Sglang,
        Self::Vllm,
        Self::Ollama,
        Self::Huggingface,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::NvidiaNim => "nvidia-nim",
            Self::Openai => "openai",
            Self::Atlascloud => "atlascloud",
            Self::WanjieArk => "wanjie-ark",
            Self::Volcengine => "volcengine",
            Self::Openrouter => "openrouter",
            Self::XiaomiMimo => "xiaomi-mimo",
            Self::Novita => "novita",
            Self::Fireworks => "fireworks",
            Self::Siliconflow => "siliconflow",
            Self::SiliconflowCN => "siliconflow-CN",
            Self::Arcee => "arcee",
            Self::Moonshot => "moonshot",
            Self::Sglang => "sglang",
            Self::Vllm => "vllm",
            Self::Ollama => "ollama",
            Self::Huggingface => "huggingface",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "deepseek" | "deep-seek" | "deepseek-cn" | "deepseek_china" | "deepseekcn"
            | "deepseek-china" => Some(Self::Deepseek),
            "nvidia" | "nvidia-nim" | "nvidia_nim" | "nim" => Some(Self::NvidiaNim),
            "openai" | "open-ai" => Some(Self::Openai),
            "atlascloud" | "atlas-cloud" | "atlas_cloud" | "atlas" => Some(Self::Atlascloud),
            "wanjie" | "wanjie-ark" | "wanjie_ark" | "ark-wanjie" | "ark_wanjie" | "wanjieark"
            | "wanjie-maas" | "wanjie_maas" | "wanjiemaas" => Some(Self::WanjieArk),
            "volcengine" | "volcengine-ark" | "volcengine_ark" | "ark" | "volc-ark"
            | "volcengineark" => Some(Self::Volcengine),
            "openrouter" | "open_router" => Some(Self::Openrouter),
            "xiaomi-mimo" | "xiaomi_mimo" | "xiaomimimo" | "mimo" | "xiaomi" => {
                Some(Self::XiaomiMimo)
            }
            "novita" => Some(Self::Novita),
            "fireworks" | "fireworks-ai" => Some(Self::Fireworks),
            "siliconflow" | "silicon-flow" | "silicon_flow" => Some(Self::Siliconflow),
            "siliconflow-cn" | "siliconflow-CN" => Some(Self::SiliconflowCN),
            "arcee" | "arcee-ai" | "arcee_ai" => Some(Self::Arcee),
            "moonshot" | "moonshot-ai" | "kimi" | "kimi-k2" => Some(Self::Moonshot),
            "sglang" | "sg-lang" => Some(Self::Sglang),
            "vllm" | "v-llm" => Some(Self::Vllm),
            "ollama" | "ollama-local" => Some(Self::Ollama),
            "huggingface" | "hugging-face" | "hugging_face" | "hf" => Some(Self::Huggingface),
            _ => None,
        }
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
}

/// Sibling `permissions.toml` schema.
///
/// This slice is intentionally ask-only: each rule is a typed condition that
/// means "ask before this tool invocation." Typed allow/deny records and UI
/// persistence are expected to land in follow-up PRs.
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
            ProviderKind::Siliconflow | ProviderKind::SiliconflowCN => &self.siliconflow,
            ProviderKind::Arcee => &self.arcee,
            ProviderKind::Moonshot => &self.moonshot,
            ProviderKind::Sglang => &self.sglang,
            ProviderKind::Vllm => &self.vllm,
            ProviderKind::Ollama => &self.ollama,
            ProviderKind::Huggingface => &self.huggingface,
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
            ProviderKind::Siliconflow | ProviderKind::SiliconflowCN => &mut self.siliconflow,
            ProviderKind::Arcee => &mut self.arcee,
            ProviderKind::Moonshot => &mut self.moonshot,
            ProviderKind::Sglang => &mut self.sglang,
            ProviderKind::Vllm => &mut self.vllm,
            ProviderKind::Ollama => &mut self.ollama,
            ProviderKind::Huggingface => &mut self.huggingface,
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
    pub log_level: Option<String>,
    pub telemetry: Option<bool>,
    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
    /// Native tool catalog controls shared with `codewhale-tui`.
    #[serde(default)]
    pub tools: Option<ToolsToml>,
    #[serde(default)]
    pub providers: ProvidersToml,
    /// Dormant provider fallback chain (#2574). This is parsed and preserved
    /// for future provider-routing work; current runtime resolution still uses
    /// the selected primary provider and does not auto-switch routes.
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
    #[serde(flatten)]
    pub extras: BTreeMap<String, toml::Value>,
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
        self.providers[self.position]
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
        merge_project_provider_config(&mut self.providers.deepseek, &project.providers.deepseek);
        merge_project_provider_config(
            &mut self.providers.nvidia_nim,
            &project.providers.nvidia_nim,
        );
        merge_project_provider_config(&mut self.providers.openai, &project.providers.openai);
        merge_project_provider_config(
            &mut self.providers.atlascloud,
            &project.providers.atlascloud,
        );
        merge_project_provider_config(
            &mut self.providers.wanjie_ark,
            &project.providers.wanjie_ark,
        );
        merge_project_provider_config(
            &mut self.providers.volcengine,
            &project.providers.volcengine,
        );
        merge_project_provider_config(
            &mut self.providers.openrouter,
            &project.providers.openrouter,
        );
        merge_project_provider_config(
            &mut self.providers.xiaomi_mimo,
            &project.providers.xiaomi_mimo,
        );
        merge_project_provider_config(&mut self.providers.novita, &project.providers.novita);
        merge_project_provider_config(&mut self.providers.fireworks, &project.providers.fireworks);
        merge_project_provider_config(
            &mut self.providers.siliconflow,
            &project.providers.siliconflow,
        );
        merge_project_provider_config(&mut self.providers.arcee, &project.providers.arcee);
        merge_project_provider_config(&mut self.providers.moonshot, &project.providers.moonshot);
        merge_project_provider_config(&mut self.providers.sglang, &project.providers.sglang);
        merge_project_provider_config(&mut self.providers.vllm, &project.providers.vllm);
        merge_project_provider_config(&mut self.providers.ollama, &project.providers.ollama);
        merge_project_provider_config(
            &mut self.providers.huggingface,
            &project.providers.huggingface,
        );
    }

    #[must_use]
    pub fn get_value(&self, key: &str) -> Option<String> {
        match key {
            "provider" => Some(self.provider.as_str().to_string()),
            "api_key" => self.api_key.clone(),
            "base_url" => self.base_url.clone(),
            "http_headers" => serialize_http_headers(&self.http_headers),
            "default_text_model" => self.default_text_model.clone(),
            "model" => self.model.clone(),
            "auth.mode" => self.auth_mode.clone(),
            "output_mode" => self.output_mode.clone(),
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
            "providers.deepseek.api_key" => self.providers.deepseek.api_key.clone(),
            "providers.deepseek.base_url" => self.providers.deepseek.base_url.clone(),
            "providers.deepseek.model" => self.providers.deepseek.model.clone(),
            "providers.deepseek.http_headers" => {
                serialize_http_headers(&self.providers.deepseek.http_headers)
            }
            "providers.nvidia_nim.api_key" => self.providers.nvidia_nim.api_key.clone(),
            "providers.nvidia_nim.base_url" => self.providers.nvidia_nim.base_url.clone(),
            "providers.nvidia_nim.model" => self.providers.nvidia_nim.model.clone(),
            "providers.nvidia_nim.http_headers" => {
                serialize_http_headers(&self.providers.nvidia_nim.http_headers)
            }
            "providers.openai.api_key" => self.providers.openai.api_key.clone(),
            "providers.openai.base_url" => self.providers.openai.base_url.clone(),
            "providers.openai.model" => self.providers.openai.model.clone(),
            "providers.openai.http_headers" => {
                serialize_http_headers(&self.providers.openai.http_headers)
            }
            "providers.atlascloud.api_key" => self.providers.atlascloud.api_key.clone(),
            "providers.atlascloud.base_url" => self.providers.atlascloud.base_url.clone(),
            "providers.atlascloud.model" => self.providers.atlascloud.model.clone(),
            "providers.atlascloud.http_headers" => {
                serialize_http_headers(&self.providers.atlascloud.http_headers)
            }
            "providers.wanjie_ark.api_key" => self.providers.wanjie_ark.api_key.clone(),
            "providers.wanjie_ark.base_url" => self.providers.wanjie_ark.base_url.clone(),
            "providers.wanjie_ark.model" => self.providers.wanjie_ark.model.clone(),
            "providers.volcengine.api_key" => self.providers.volcengine.api_key.clone(),
            "providers.volcengine.base_url" => self.providers.volcengine.base_url.clone(),
            "providers.volcengine.model" => self.providers.volcengine.model.clone(),
            "providers.volcengine.http_headers" => {
                serialize_http_headers(&self.providers.volcengine.http_headers)
            }
            "providers.wanjie_ark.http_headers" => {
                serialize_http_headers(&self.providers.wanjie_ark.http_headers)
            }
            "providers.openrouter.api_key" => self.providers.openrouter.api_key.clone(),
            "providers.openrouter.base_url" => self.providers.openrouter.base_url.clone(),
            "providers.openrouter.model" => self.providers.openrouter.model.clone(),
            "providers.openrouter.http_headers" => {
                serialize_http_headers(&self.providers.openrouter.http_headers)
            }
            "providers.xiaomi_mimo.api_key" => self.providers.xiaomi_mimo.api_key.clone(),
            "providers.xiaomi_mimo.base_url" => self.providers.xiaomi_mimo.base_url.clone(),
            "providers.xiaomi_mimo.model" => self.providers.xiaomi_mimo.model.clone(),
            "providers.xiaomi_mimo.mode" => self.providers.xiaomi_mimo.mode.clone(),
            "providers.xiaomi_mimo.http_headers" => {
                serialize_http_headers(&self.providers.xiaomi_mimo.http_headers)
            }
            "providers.novita.api_key" => self.providers.novita.api_key.clone(),
            "providers.novita.base_url" => self.providers.novita.base_url.clone(),
            "providers.novita.model" => self.providers.novita.model.clone(),
            "providers.novita.http_headers" => {
                serialize_http_headers(&self.providers.novita.http_headers)
            }
            "providers.fireworks.api_key" => self.providers.fireworks.api_key.clone(),
            "providers.fireworks.base_url" => self.providers.fireworks.base_url.clone(),
            "providers.fireworks.model" => self.providers.fireworks.model.clone(),
            "providers.fireworks.http_headers" => {
                serialize_http_headers(&self.providers.fireworks.http_headers)
            }
            "providers.siliconflow.api_key" => self.providers.siliconflow.api_key.clone(),
            "providers.siliconflow.base_url" => self.providers.siliconflow.base_url.clone(),
            "providers.siliconflow.model" => self.providers.siliconflow.model.clone(),
            "providers.siliconflow.http_headers" => {
                serialize_http_headers(&self.providers.siliconflow.http_headers)
            }
            "providers.arcee.api_key" => self.providers.arcee.api_key.clone(),
            "providers.arcee.base_url" => self.providers.arcee.base_url.clone(),
            "providers.arcee.model" => self.providers.arcee.model.clone(),
            "providers.arcee.http_headers" => {
                serialize_http_headers(&self.providers.arcee.http_headers)
            }
            "providers.moonshot.api_key" => self.providers.moonshot.api_key.clone(),
            "providers.moonshot.base_url" => self.providers.moonshot.base_url.clone(),
            "providers.moonshot.model" => self.providers.moonshot.model.clone(),
            "providers.moonshot.auth_mode" => self.providers.moonshot.auth_mode.clone(),
            "providers.moonshot.http_headers" => {
                serialize_http_headers(&self.providers.moonshot.http_headers)
            }
            "providers.sglang.api_key" => self.providers.sglang.api_key.clone(),
            "providers.sglang.base_url" => self.providers.sglang.base_url.clone(),
            "providers.sglang.model" => self.providers.sglang.model.clone(),
            "providers.sglang.http_headers" => {
                serialize_http_headers(&self.providers.sglang.http_headers)
            }
            "providers.vllm.api_key" => self.providers.vllm.api_key.clone(),
            "providers.vllm.base_url" => self.providers.vllm.base_url.clone(),
            "providers.vllm.model" => self.providers.vllm.model.clone(),
            "providers.vllm.http_headers" => {
                serialize_http_headers(&self.providers.vllm.http_headers)
            }
            "providers.ollama.api_key" => self.providers.ollama.api_key.clone(),
            "providers.ollama.base_url" => self.providers.ollama.base_url.clone(),
            "providers.ollama.model" => self.providers.ollama.model.clone(),
            "providers.ollama.http_headers" => {
                serialize_http_headers(&self.providers.ollama.http_headers)
            }
            "providers.huggingface.api_key" => self.providers.huggingface.api_key.clone(),
            "providers.huggingface.base_url" => self.providers.huggingface.base_url.clone(),
            "providers.huggingface.model" => self.providers.huggingface.model.clone(),
            "providers.huggingface.http_headers" => {
                serialize_http_headers(&self.providers.huggingface.http_headers)
            }
            _ => self.extras.get(key).map(toml::Value::to_string),
        }
    }

    #[must_use]
    pub fn get_display_value(&self, key: &str) -> Option<String> {
        self.get_value(key).map(|value| {
            if is_sensitive_config_key(key) {
                redact_secret(&value)
            } else {
                value
            }
        })
    }

    pub fn set_value(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "provider" => {
                self.provider = ProviderKind::parse(value)
                    .with_context(|| format!("unknown provider '{value}'"))?;
            }
            "api_key" => self.api_key = Some(value.to_string()),
            "base_url" => self.base_url = Some(value.to_string()),
            "http_headers" => self.http_headers = parse_http_headers(value)?,
            "default_text_model" => self.default_text_model = Some(value.to_string()),
            "model" => self.model = Some(value.to_string()),
            "auth.mode" => self.auth_mode = Some(value.to_string()),
            "output_mode" => self.output_mode = Some(value.to_string()),
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
            "providers.deepseek.api_key" => {
                let value = value.to_string();
                self.providers.deepseek.api_key = Some(value.clone());
                self.api_key = Some(value);
            }
            "providers.deepseek.base_url" => {
                let value = value.to_string();
                self.providers.deepseek.base_url = Some(value.clone());
                self.base_url = Some(value);
            }
            "providers.deepseek.model" => {
                let value = value.to_string();
                self.providers.deepseek.model = Some(value.clone());
                self.default_text_model = Some(value);
            }
            "providers.deepseek.http_headers" => {
                let headers = parse_http_headers(value)?;
                self.providers.deepseek.http_headers = headers.clone();
                self.http_headers = headers;
            }
            "providers.openai.api_key" => self.providers.openai.api_key = Some(value.to_string()),
            "providers.openai.base_url" => self.providers.openai.base_url = Some(value.to_string()),
            "providers.openai.model" => self.providers.openai.model = Some(value.to_string()),
            "providers.openai.http_headers" => {
                self.providers.openai.http_headers = parse_http_headers(value)?;
            }
            "providers.atlascloud.api_key" => {
                self.providers.atlascloud.api_key = Some(value.to_string());
            }
            "providers.atlascloud.base_url" => {
                self.providers.atlascloud.base_url = Some(value.to_string());
            }
            "providers.atlascloud.model" => {
                self.providers.atlascloud.model = Some(value.to_string());
            }
            "providers.atlascloud.http_headers" => {
                self.providers.atlascloud.http_headers = parse_http_headers(value)?;
            }
            "providers.wanjie_ark.api_key" => {
                self.providers.wanjie_ark.api_key = Some(value.to_string());
            }
            "providers.wanjie_ark.base_url" => {
                self.providers.wanjie_ark.base_url = Some(value.to_string());
            }
            "providers.wanjie_ark.model" => {
                self.providers.wanjie_ark.model = Some(value.to_string());
            }
            "providers.volcengine.api_key" => {
                self.providers.volcengine.api_key = Some(value.to_string());
            }
            "providers.volcengine.base_url" => {
                self.providers.volcengine.base_url = Some(value.to_string());
            }
            "providers.volcengine.model" => {
                self.providers.volcengine.model = Some(value.to_string());
            }
            "providers.volcengine.http_headers" => {
                self.providers.volcengine.http_headers = parse_http_headers(value)?;
            }
            "providers.wanjie_ark.http_headers" => {
                self.providers.wanjie_ark.http_headers = parse_http_headers(value)?;
            }
            "providers.nvidia_nim.api_key" => {
                self.providers.nvidia_nim.api_key = Some(value.to_string());
            }
            "providers.nvidia_nim.base_url" => {
                self.providers.nvidia_nim.base_url = Some(value.to_string());
            }
            "providers.nvidia_nim.model" => {
                self.providers.nvidia_nim.model = Some(value.to_string());
            }
            "providers.nvidia_nim.http_headers" => {
                self.providers.nvidia_nim.http_headers = parse_http_headers(value)?;
            }
            "providers.openrouter.api_key" => {
                self.providers.openrouter.api_key = Some(value.to_string());
            }
            "providers.openrouter.base_url" => {
                self.providers.openrouter.base_url = Some(value.to_string());
            }
            "providers.openrouter.model" => {
                self.providers.openrouter.model = Some(value.to_string());
            }
            "providers.openrouter.http_headers" => {
                self.providers.openrouter.http_headers = parse_http_headers(value)?;
            }
            "providers.xiaomi_mimo.api_key" => {
                self.providers.xiaomi_mimo.api_key = Some(value.to_string());
            }
            "providers.xiaomi_mimo.base_url" => {
                self.providers.xiaomi_mimo.base_url = Some(value.to_string());
            }
            "providers.xiaomi_mimo.model" => {
                self.providers.xiaomi_mimo.model = Some(value.to_string());
            }
            "providers.xiaomi_mimo.mode" => {
                self.providers.xiaomi_mimo.mode = Some(value.to_string());
            }
            "providers.xiaomi_mimo.http_headers" => {
                self.providers.xiaomi_mimo.http_headers = parse_http_headers(value)?;
            }
            "providers.novita.api_key" => {
                self.providers.novita.api_key = Some(value.to_string());
            }
            "providers.novita.base_url" => {
                self.providers.novita.base_url = Some(value.to_string());
            }
            "providers.novita.model" => {
                self.providers.novita.model = Some(value.to_string());
            }
            "providers.novita.http_headers" => {
                self.providers.novita.http_headers = parse_http_headers(value)?;
            }
            "providers.fireworks.api_key" => {
                self.providers.fireworks.api_key = Some(value.to_string());
            }
            "providers.fireworks.base_url" => {
                self.providers.fireworks.base_url = Some(value.to_string());
            }
            "providers.fireworks.model" => {
                self.providers.fireworks.model = Some(value.to_string());
            }
            "providers.fireworks.http_headers" => {
                self.providers.fireworks.http_headers = parse_http_headers(value)?;
            }
            "providers.siliconflow.api_key" => {
                self.providers.siliconflow.api_key = Some(value.to_string());
            }
            "providers.siliconflow.base_url" => {
                self.providers.siliconflow.base_url = Some(value.to_string());
            }
            "providers.siliconflow.model" => {
                self.providers.siliconflow.model = Some(value.to_string());
            }
            "providers.siliconflow.http_headers" => {
                self.providers.siliconflow.http_headers = parse_http_headers(value)?;
            }
            "providers.arcee.api_key" => {
                self.providers.arcee.api_key = Some(value.to_string());
            }
            "providers.arcee.base_url" => {
                self.providers.arcee.base_url = Some(value.to_string());
            }
            "providers.arcee.model" => {
                self.providers.arcee.model = Some(value.to_string());
            }
            "providers.arcee.http_headers" => {
                self.providers.arcee.http_headers = parse_http_headers(value)?;
            }
            "providers.moonshot.api_key" => {
                self.providers.moonshot.api_key = Some(value.to_string());
            }
            "providers.moonshot.base_url" => {
                self.providers.moonshot.base_url = Some(value.to_string());
            }
            "providers.moonshot.model" => {
                self.providers.moonshot.model = Some(value.to_string());
            }
            "providers.moonshot.auth_mode" => {
                self.providers.moonshot.auth_mode = Some(value.to_string());
            }
            "providers.moonshot.http_headers" => {
                self.providers.moonshot.http_headers = parse_http_headers(value)?;
            }
            "providers.sglang.api_key" => {
                self.providers.sglang.api_key = Some(value.to_string());
            }
            "providers.sglang.base_url" => {
                self.providers.sglang.base_url = Some(value.to_string());
            }
            "providers.sglang.model" => {
                self.providers.sglang.model = Some(value.to_string());
            }
            "providers.sglang.http_headers" => {
                self.providers.sglang.http_headers = parse_http_headers(value)?;
            }
            "providers.vllm.api_key" => {
                self.providers.vllm.api_key = Some(value.to_string());
            }
            "providers.vllm.base_url" => {
                self.providers.vllm.base_url = Some(value.to_string());
            }
            "providers.vllm.model" => {
                self.providers.vllm.model = Some(value.to_string());
            }
            "providers.vllm.http_headers" => {
                self.providers.vllm.http_headers = parse_http_headers(value)?;
            }
            "providers.ollama.api_key" => {
                self.providers.ollama.api_key = Some(value.to_string());
            }
            "providers.ollama.base_url" => {
                self.providers.ollama.base_url = Some(value.to_string());
            }
            "providers.ollama.model" => {
                self.providers.ollama.model = Some(value.to_string());
            }
            "providers.ollama.http_headers" => {
                self.providers.ollama.http_headers = parse_http_headers(value)?;
            }
            "providers.huggingface.api_key" => {
                self.providers.huggingface.api_key = Some(value.to_string());
            }
            "providers.huggingface.base_url" => {
                self.providers.huggingface.base_url = Some(value.to_string());
            }
            "providers.huggingface.model" => {
                self.providers.huggingface.model = Some(value.to_string());
            }
            "providers.huggingface.http_headers" => {
                self.providers.huggingface.http_headers = parse_http_headers(value)?;
            }
            _ => {
                self.extras
                    .insert(key.to_string(), toml::Value::String(value.to_string()));
            }
        }
        Ok(())
    }

    pub fn unset_value(&mut self, key: &str) -> Result<()> {
        match key {
            "provider" => self.provider = ProviderKind::Deepseek,
            "api_key" => self.api_key = None,
            "base_url" => self.base_url = None,
            "http_headers" => self.http_headers.clear(),
            "default_text_model" => self.default_text_model = None,
            "model" => self.model = None,
            "auth.mode" => self.auth_mode = None,
            "output_mode" => self.output_mode = None,
            "log_level" => self.log_level = None,
            "telemetry" => self.telemetry = None,
            "approval_policy" => self.approval_policy = None,
            "sandbox_mode" => self.sandbox_mode = None,
            "hook_sinks.unix_socket_path" => {
                if let Some(sinks) = self.hook_sinks.as_mut() {
                    sinks.unix_socket_path = None;
                }
            }
            "providers.deepseek.api_key" => {
                self.providers.deepseek.api_key = None;
                self.api_key = None;
            }
            "providers.deepseek.base_url" => {
                self.providers.deepseek.base_url = None;
                self.base_url = None;
            }
            "providers.deepseek.model" => {
                self.providers.deepseek.model = None;
                self.default_text_model = None;
            }
            "providers.deepseek.http_headers" => {
                self.providers.deepseek.http_headers.clear();
                self.http_headers.clear();
            }
            "providers.openai.api_key" => self.providers.openai.api_key = None,
            "providers.openai.base_url" => self.providers.openai.base_url = None,
            "providers.openai.model" => self.providers.openai.model = None,
            "providers.openai.http_headers" => self.providers.openai.http_headers.clear(),
            "providers.atlascloud.api_key" => self.providers.atlascloud.api_key = None,
            "providers.atlascloud.base_url" => self.providers.atlascloud.base_url = None,
            "providers.atlascloud.model" => self.providers.atlascloud.model = None,
            "providers.atlascloud.http_headers" => self.providers.atlascloud.http_headers.clear(),
            "providers.wanjie_ark.api_key" => self.providers.wanjie_ark.api_key = None,
            "providers.wanjie_ark.base_url" => self.providers.wanjie_ark.base_url = None,
            "providers.wanjie_ark.model" => self.providers.wanjie_ark.model = None,
            "providers.volcengine.api_key" => self.providers.volcengine.api_key = None,
            "providers.volcengine.base_url" => self.providers.volcengine.base_url = None,
            "providers.volcengine.model" => self.providers.volcengine.model = None,
            "providers.volcengine.http_headers" => {
                self.providers.volcengine.http_headers.clear();
            }
            "providers.wanjie_ark.http_headers" => {
                self.providers.wanjie_ark.http_headers.clear();
            }
            "providers.nvidia_nim.api_key" => self.providers.nvidia_nim.api_key = None,
            "providers.nvidia_nim.base_url" => self.providers.nvidia_nim.base_url = None,
            "providers.nvidia_nim.model" => self.providers.nvidia_nim.model = None,
            "providers.nvidia_nim.http_headers" => self.providers.nvidia_nim.http_headers.clear(),
            "providers.openrouter.api_key" => self.providers.openrouter.api_key = None,
            "providers.openrouter.base_url" => self.providers.openrouter.base_url = None,
            "providers.openrouter.model" => self.providers.openrouter.model = None,
            "providers.openrouter.http_headers" => self.providers.openrouter.http_headers.clear(),
            "providers.xiaomi_mimo.api_key" => self.providers.xiaomi_mimo.api_key = None,
            "providers.xiaomi_mimo.base_url" => self.providers.xiaomi_mimo.base_url = None,
            "providers.xiaomi_mimo.model" => self.providers.xiaomi_mimo.model = None,
            "providers.xiaomi_mimo.mode" => self.providers.xiaomi_mimo.mode = None,
            "providers.xiaomi_mimo.http_headers" => {
                self.providers.xiaomi_mimo.http_headers.clear();
            }
            "providers.novita.api_key" => self.providers.novita.api_key = None,
            "providers.novita.base_url" => self.providers.novita.base_url = None,
            "providers.novita.model" => self.providers.novita.model = None,
            "providers.novita.http_headers" => self.providers.novita.http_headers.clear(),
            "providers.fireworks.api_key" => self.providers.fireworks.api_key = None,
            "providers.fireworks.base_url" => self.providers.fireworks.base_url = None,
            "providers.fireworks.model" => self.providers.fireworks.model = None,
            "providers.fireworks.http_headers" => self.providers.fireworks.http_headers.clear(),
            "providers.siliconflow.api_key" => self.providers.siliconflow.api_key = None,
            "providers.siliconflow.base_url" => self.providers.siliconflow.base_url = None,
            "providers.siliconflow.model" => self.providers.siliconflow.model = None,
            "providers.siliconflow.http_headers" => {
                self.providers.siliconflow.http_headers.clear();
            }
            "providers.arcee.api_key" => self.providers.arcee.api_key = None,
            "providers.arcee.base_url" => self.providers.arcee.base_url = None,
            "providers.arcee.model" => self.providers.arcee.model = None,
            "providers.arcee.http_headers" => {
                self.providers.arcee.http_headers.clear();
            }
            "providers.moonshot.api_key" => self.providers.moonshot.api_key = None,
            "providers.moonshot.base_url" => self.providers.moonshot.base_url = None,
            "providers.moonshot.model" => self.providers.moonshot.model = None,
            "providers.moonshot.auth_mode" => self.providers.moonshot.auth_mode = None,
            "providers.moonshot.http_headers" => self.providers.moonshot.http_headers.clear(),
            "providers.sglang.api_key" => self.providers.sglang.api_key = None,
            "providers.sglang.base_url" => self.providers.sglang.base_url = None,
            "providers.sglang.model" => self.providers.sglang.model = None,
            "providers.sglang.http_headers" => self.providers.sglang.http_headers.clear(),
            "providers.vllm.api_key" => self.providers.vllm.api_key = None,
            "providers.vllm.base_url" => self.providers.vllm.base_url = None,
            "providers.vllm.model" => self.providers.vllm.model = None,
            "providers.vllm.http_headers" => self.providers.vllm.http_headers.clear(),
            "providers.ollama.api_key" => self.providers.ollama.api_key = None,
            "providers.ollama.base_url" => self.providers.ollama.base_url = None,
            "providers.ollama.model" => self.providers.ollama.model = None,
            "providers.ollama.http_headers" => self.providers.ollama.http_headers.clear(),
            "providers.huggingface.api_key" => self.providers.huggingface.api_key = None,
            "providers.huggingface.base_url" => self.providers.huggingface.base_url = None,
            "providers.huggingface.model" => self.providers.huggingface.model = None,
            "providers.huggingface.http_headers" => self.providers.huggingface.http_headers.clear(),
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
        if let Some(v) = serialize_http_headers(&self.http_headers) {
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
        if let Some(v) = self.providers.deepseek.api_key.as_ref() {
            out.insert("providers.deepseek.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.deepseek.base_url.as_ref() {
            out.insert("providers.deepseek.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.deepseek.model.as_ref() {
            out.insert("providers.deepseek.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.deepseek.http_headers) {
            out.insert("providers.deepseek.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.openai.api_key.as_ref() {
            out.insert("providers.openai.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.openai.base_url.as_ref() {
            out.insert("providers.openai.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.openai.model.as_ref() {
            out.insert("providers.openai.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.openai.http_headers) {
            out.insert("providers.openai.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.atlascloud.api_key.as_ref() {
            out.insert("providers.atlascloud.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.atlascloud.base_url.as_ref() {
            out.insert("providers.atlascloud.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.atlascloud.model.as_ref() {
            out.insert("providers.atlascloud.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.atlascloud.http_headers) {
            out.insert("providers.atlascloud.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.volcengine.api_key.as_ref() {
            out.insert("providers.volcengine.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.volcengine.base_url.as_ref() {
            out.insert("providers.volcengine.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.volcengine.model.as_ref() {
            out.insert("providers.volcengine.model".to_string(), v.clone());
        }
        if let Some(v) = self.providers.wanjie_ark.api_key.as_ref() {
            out.insert("providers.wanjie_ark.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.wanjie_ark.base_url.as_ref() {
            out.insert("providers.wanjie_ark.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.wanjie_ark.model.as_ref() {
            out.insert("providers.wanjie_ark.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.volcengine.http_headers) {
            out.insert("providers.volcengine.http_headers".to_string(), v);
        }
        if let Some(v) = serialize_http_headers(&self.providers.wanjie_ark.http_headers) {
            out.insert("providers.wanjie_ark.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.nvidia_nim.api_key.as_ref() {
            out.insert("providers.nvidia_nim.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.nvidia_nim.base_url.as_ref() {
            out.insert("providers.nvidia_nim.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.nvidia_nim.model.as_ref() {
            out.insert("providers.nvidia_nim.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.nvidia_nim.http_headers) {
            out.insert("providers.nvidia_nim.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.openrouter.api_key.as_ref() {
            out.insert("providers.openrouter.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.openrouter.base_url.as_ref() {
            out.insert("providers.openrouter.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.openrouter.model.as_ref() {
            out.insert("providers.openrouter.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.openrouter.http_headers) {
            out.insert("providers.openrouter.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.xiaomi_mimo.api_key.as_ref() {
            out.insert(
                "providers.xiaomi_mimo.api_key".to_string(),
                redact_secret(v),
            );
        }
        if let Some(v) = self.providers.xiaomi_mimo.base_url.as_ref() {
            out.insert("providers.xiaomi_mimo.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.xiaomi_mimo.model.as_ref() {
            out.insert("providers.xiaomi_mimo.model".to_string(), v.clone());
        }
        if let Some(v) = self.providers.xiaomi_mimo.mode.as_ref() {
            out.insert("providers.xiaomi_mimo.mode".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.xiaomi_mimo.http_headers) {
            out.insert("providers.xiaomi_mimo.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.novita.api_key.as_ref() {
            out.insert("providers.novita.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.novita.base_url.as_ref() {
            out.insert("providers.novita.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.novita.model.as_ref() {
            out.insert("providers.novita.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.novita.http_headers) {
            out.insert("providers.novita.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.fireworks.api_key.as_ref() {
            out.insert("providers.fireworks.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.fireworks.base_url.as_ref() {
            out.insert("providers.fireworks.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.fireworks.model.as_ref() {
            out.insert("providers.fireworks.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.fireworks.http_headers) {
            out.insert("providers.fireworks.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.siliconflow.api_key.as_ref() {
            out.insert(
                "providers.siliconflow.api_key".to_string(),
                redact_secret(v),
            );
        }
        if let Some(v) = self.providers.siliconflow.base_url.as_ref() {
            out.insert("providers.siliconflow.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.siliconflow.model.as_ref() {
            out.insert("providers.siliconflow.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.siliconflow.http_headers) {
            out.insert("providers.siliconflow.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.arcee.api_key.as_ref() {
            out.insert("providers.arcee.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.arcee.base_url.as_ref() {
            out.insert("providers.arcee.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.arcee.model.as_ref() {
            out.insert("providers.arcee.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.arcee.http_headers) {
            out.insert("providers.arcee.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.moonshot.api_key.as_ref() {
            out.insert("providers.moonshot.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.moonshot.base_url.as_ref() {
            out.insert("providers.moonshot.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.moonshot.model.as_ref() {
            out.insert("providers.moonshot.model".to_string(), v.clone());
        }
        if let Some(v) = self.providers.moonshot.auth_mode.as_ref() {
            out.insert("providers.moonshot.auth_mode".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.moonshot.http_headers) {
            out.insert("providers.moonshot.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.sglang.api_key.as_ref() {
            out.insert("providers.sglang.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.sglang.base_url.as_ref() {
            out.insert("providers.sglang.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.sglang.model.as_ref() {
            out.insert("providers.sglang.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.sglang.http_headers) {
            out.insert("providers.sglang.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.vllm.api_key.as_ref() {
            out.insert("providers.vllm.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.vllm.base_url.as_ref() {
            out.insert("providers.vllm.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.vllm.model.as_ref() {
            out.insert("providers.vllm.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.vllm.http_headers) {
            out.insert("providers.vllm.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.ollama.api_key.as_ref() {
            out.insert("providers.ollama.api_key".to_string(), redact_secret(v));
        }
        if let Some(v) = self.providers.ollama.base_url.as_ref() {
            out.insert("providers.ollama.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.ollama.model.as_ref() {
            out.insert("providers.ollama.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.ollama.http_headers) {
            out.insert("providers.ollama.http_headers".to_string(), v);
        }
        if let Some(v) = self.providers.huggingface.api_key.as_ref() {
            out.insert(
                "providers.huggingface.api_key".to_string(),
                redact_secret(v),
            );
        }
        if let Some(v) = self.providers.huggingface.base_url.as_ref() {
            out.insert("providers.huggingface.base_url".to_string(), v.clone());
        }
        if let Some(v) = self.providers.huggingface.model.as_ref() {
            out.insert("providers.huggingface.model".to_string(), v.clone());
        }
        if let Some(v) = serialize_http_headers(&self.providers.huggingface.http_headers) {
            out.insert("providers.huggingface.http_headers".to_string(), v);
        }

        for (k, v) in &self.extras {
            out.insert(k.clone(), v.to_string());
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
        let provider = cli.provider.or(env.provider).unwrap_or(self.provider);

        let provider_cfg = self.providers.for_provider(provider);
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
            match codewhale_secrets::env_for(provider.as_str()) {
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
                None => (None, None),
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

        ResolvedRuntimeOptions {
            provider,
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

    if matches!(
        provider,
        ProviderKind::Atlascloud
            | ProviderKind::WanjieArk
            | ProviderKind::Volcengine
            | ProviderKind::XiaomiMimo
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
            ProviderKind::Siliconflow,
            "deepseek-v4-pro" | "deepseek-v4pro" | "deepseek-reasoner" | "deepseek-r1",
        ) => DEFAULT_SILICONFLOW_MODEL.to_string(),
        (
            ProviderKind::Siliconflow,
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
        (ProviderKind::Moonshot, "kimi-k2.6" | "kimi-k2") => DEFAULT_MOONSHOT_MODEL.to_string(),
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
        OPENROUTER_KIMI_K2_6_MODEL | "kimi-k2.6" | "kimi-k2-6" | "moonshot-kimi-k2.6" => {
            Some(OPENROUTER_KIMI_K2_6_MODEL)
        }
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
        OPENROUTER_QWEN_3_7_MAX_MODEL | "qwen3.7-max" | "qwen-3.7-max" | "qwen3-7-max" => {
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

#[derive(Debug, Clone)]
pub struct ResolvedRuntimeOptions {
    pub provider: ProviderKind,
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
    pub http_headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    pub config: ConfigToml,
    permissions: PermissionsToml,
}

impl ConfigStore {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = resolve_config_path(path)?;
        let config = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read config at {}", path.display()))?;
            toml::from_str(&raw)
                .with_context(|| format!("failed to parse config at {}", path.display()))?
        } else {
            ConfigToml::default()
        };
        let permissions = load_sibling_permissions(&path)?;

        Ok(Self {
            path,
            config,
            permissions,
        })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }
        let body = toml::to_string_pretty(&self.config).context("failed to serialize config")?;
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

/// Resolve a state subdirectory, preferring the CodeWhale root if
/// it already exists, otherwise falling back to the legacy root.
///
/// This is the read-path resolver: it returns the primary path when
/// migration has occurred or on a fresh install, but keeps reading
/// from the legacy path for users who haven't migrated yet.
pub fn resolve_state_dir(subdir: &str) -> Result<PathBuf> {
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
pub fn ensure_state_dir(subdir: &str) -> Result<PathBuf> {
    let dir = codewhale_home()?.join(subdir);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}/", dir.display()))?;
    Ok(dir)
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
    key == "api_key" || key.ends_with(".api_key")
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
}

impl EnvRuntimeOverrides {
    fn load() -> Self {
        Self {
            provider: std::env::var("CODEWHALE_PROVIDER")
                .or_else(|_| std::env::var("DEEPSEEK_PROVIDER"))
                .ok()
                .and_then(|v| ProviderKind::parse(&v)),
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
        }
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
        sglang_api_key: Option<OsString>,
        sglang_base_url: Option<OsString>,
        vllm_api_key: Option<OsString>,
        vllm_base_url: Option<OsString>,
        ollama_api_key: Option<OsString>,
        ollama_base_url: Option<OsString>,
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
                sglang_api_key: env::var_os("SGLANG_API_KEY"),
                sglang_base_url: env::var_os("SGLANG_BASE_URL"),
                vllm_api_key: env::var_os("VLLM_API_KEY"),
                vllm_base_url: env::var_os("VLLM_BASE_URL"),
                ollama_api_key: env::var_os("OLLAMA_API_KEY"),
                ollama_base_url: env::var_os("OLLAMA_BASE_URL"),
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
                env::remove_var("SGLANG_API_KEY");
                env::remove_var("SGLANG_BASE_URL");
                env::remove_var("VLLM_API_KEY");
                env::remove_var("VLLM_BASE_URL");
                env::remove_var("OLLAMA_API_KEY");
                env::remove_var("OLLAMA_BASE_URL");
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
                Self::restore_var("SGLANG_API_KEY", self.sglang_api_key.take());
                Self::restore_var("SGLANG_BASE_URL", self.sglang_base_url.take());
                Self::restore_var("VLLM_API_KEY", self.vllm_api_key.take());
                Self::restore_var("VLLM_BASE_URL", self.vllm_base_url.take());
                Self::restore_var("OLLAMA_API_KEY", self.ollama_api_key.take());
                Self::restore_var("OLLAMA_BASE_URL", self.ollama_base_url.take());
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
        };
        store.save().expect("save");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let _ = fs::remove_dir_all(dir);
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

        let parsed: ConfigToml =
            toml::from_str("provider = \"ark-wanjie\"").expect("wanjie provider alias");
        assert_eq!(parsed.provider, ProviderKind::WanjieArk);

        let parsed: ConfigToml =
            toml::from_str("provider = \"silicon-flow\"").expect("siliconflow provider alias");
        assert_eq!(parsed.provider, ProviderKind::Siliconflow);
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
        assert_eq!(siliconflow_cn.provider_config_key(), "siliconflow");

        let config = ProvidersToml::default();
        let shared_table = config.for_provider(ProviderKind::SiliconflowCN);
        assert!(std::ptr::eq(
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
            assert_eq!(provider.wire(), provider::WireFormat::ChatCompletions);
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
    fn moonshot_provider_defaults_to_kimi_k2() {
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
            ("qwen3.7-max", OPENROUTER_QWEN_3_7_MAX_MODEL),
            ("mimo-v2.5-pro", OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL),
            ("kimi-k2.6", OPENROUTER_KIMI_K2_6_MODEL),
            ("gemma-4-31b-it", OPENROUTER_GEMMA_4_31B_MODEL),
            ("glm-5.1", OPENROUTER_GLM_5_1_MODEL),
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
}
