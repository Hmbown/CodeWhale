use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use codewhale_secrets::SecretSource;
pub use codewhale_secrets::Secrets;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const CONFIG_FILE_NAME: &str = "config.toml";
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
const OPENROUTER_QWEN_3_6_35B_A3B_MODEL: &str = "qwen/qwen3.6-35b-a3b";
const OPENROUTER_QWEN_3_6_27B_MODEL: &str = "qwen/qwen3.6-27b";
const OPENROUTER_TENCENT_HY3_PREVIEW_MODEL: &str = "tencent/hy3-preview";
const OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL: &str = "xiaomi/mimo-v2.5-pro";
const OPENROUTER_XIAOMI_MIMO_V2_5_MODEL: &str = "xiaomi/mimo-v2.5";
const DEFAULT_XIAOMI_MIMO_MODEL: &str = "mimo-v2.5-pro";
const DEFAULT_NOVITA_MODEL: &str = "deepseek/deepseek-v4-pro";
const DEFAULT_NOVITA_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";
const DEFAULT_FIREWORKS_MODEL: &str = "accounts/fireworks/models/deepseek-v4-pro";
const DEFAULT_SILICONFLOW_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_SILICONFLOW_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_MOONSHOT_MODEL: &str = "kimi-k2.6";
const DEFAULT_MOONSHOT_BASE_URL: &str = "https://api.moonshot.ai/v1";
const DEFAULT_KIMI_CODE_MODEL: &str = "kimi-for-coding";
const DEFAULT_KIMI_CODE_BASE_URL: &str = "https://api.kimi.com/coding/v1";
const DEFAULT_SGLANG_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
const DEFAULT_SGLANG_FLASH_MODEL: &str = "deepseek-ai/DeepSeek-V4-Flash";
const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_XIAOMI_MIMO_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
const DEFAULT_NOVITA_BASE_URL: &str = "https://api.novita.ai/v1";
const DEFAULT_FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";
const DEFAULT_SILICONFLOW_BASE_URL: &str = "https://api.siliconflow.com/v1";
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
    Moonshot,
    Sglang,
    Vllm,
    Ollama,
}

impl ProviderKind {
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
            Self::Moonshot => "moonshot",
            Self::Sglang => "sglang",
            Self::Vllm => "vllm",
            Self::Ollama => "ollama",
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
            "moonshot" | "moonshot-ai" | "kimi" | "kimi-k2" => Some(Self::Moonshot),
            "sglang" | "sg-lang" => Some(Self::Sglang),
            "vllm" | "v-llm" => Some(Self::Vllm),
            "ollama" | "ollama-local" => Some(Self::Ollama),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfigToml {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub auth_mode: Option<String>,
    #[serde(default)]
    pub http_headers: BTreeMap<String, String>,
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
    #[serde(default)]
    pub xiaomi_mimo: ProviderConfigToml,
    #[serde(default)]
    pub novita: ProviderConfigToml,
    #[serde(default)]
    pub fireworks: ProviderConfigToml,
    #[serde(default)]
    pub siliconflow: ProviderConfigToml,
    #[serde(default)]
    pub moonshot: ProviderConfigToml,
    #[serde(default)]
    pub sglang: ProviderConfigToml,
    #[serde(default)]
    pub vllm: ProviderConfigToml,
    #[serde(default)]
    pub ollama: ProviderConfigToml,
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
            ProviderKind::Moonshot => &self.moonshot,
            ProviderKind::Sglang => &self.sglang,
            ProviderKind::Vllm => &self.vllm,
            ProviderKind::Ollama => &self.ollama,
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
            ProviderKind::Moonshot => &mut self.moonshot,
            ProviderKind::Sglang => &mut self.sglang,
            ProviderKind::Vllm => &mut self.vllm,
            ProviderKind::Ollama => &mut self.ollama,
        }
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
    /// App-server hook sink configuration. Kept separate from the TUI
    /// lifecycle `[hooks]` table so config rewrites preserve existing hooks.
    #[serde(default)]
    pub hook_sinks: Option<HookSinksToml>,
    #[serde(flatten)]
    pub extras: BTreeMap<String, toml::Value>,
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
        merge_project_provider_config(&mut self.providers.sglang, &project.providers.sglang);
        merge_project_provider_config(&mut self.providers.vllm, &project.providers.vllm);
        merge_project_provider_config(&mut self.providers.ollama, &project.providers.ollama);
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
        let base_url = cli
            .base_url
            .clone()
            .or_else(|| env.base_url_for(provider))
            .or_else(|| provider_cfg.base_url.clone())
            .or(root_deepseek_base_url)
            .unwrap_or_else(|| match provider {
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
            });
        // CLI flag wins outright. Otherwise: config-file → injected secrets/env.
        // This makes `deepseek auth set` a reliable fix even when the user's
        // shell still exports an old key. When the file is empty, the injected
        // secrets façade recovers configured secret-store credentials before
        // falling back to ambient env.
        let uses_kimi_oauth = provider == ProviderKind::Moonshot
            && auth_mode.as_deref().is_some_and(auth_mode_uses_kimi_oauth);
        let from_file = provider_cfg.api_key.clone().or(root_deepseek_api_key);
        let (api_key, api_key_source) = if let Some(value) = cli.api_key.clone() {
            (Some(value), Some(RuntimeApiKeySource::Cli))
        } else if uses_kimi_oauth {
            (None, None)
        } else if let Some(value) = from_file.clone().filter(|v| !v.trim().is_empty()) {
            (Some(value), Some(RuntimeApiKeySource::ConfigFile))
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
            return toml::from_str(&raw).ok();
        }
    }
    None
}

fn normalize_model_for_provider(provider: ProviderKind, model: &str) -> String {
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
        _ => model.to_string(),
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
        OPENROUTER_QWEN_3_6_27B_MODEL | "qwen3.6-27b" | "qwen-3.6-27b" | "qwen3-6-27b" => {
            Some(OPENROUTER_QWEN_3_6_27B_MODEL)
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
        ProviderKind::Siliconflow => DEFAULT_SILICONFLOW_MODEL,
        ProviderKind::Moonshot => DEFAULT_MOONSHOT_MODEL,
        ProviderKind::Sglang => DEFAULT_SGLANG_MODEL,
        ProviderKind::Vllm => DEFAULT_VLLM_MODEL,
        ProviderKind::Ollama => DEFAULT_OLLAMA_MODEL,
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
        ProviderKind::Moonshot => DEFAULT_MOONSHOT_BASE_URL,
        ProviderKind::Sglang => DEFAULT_SGLANG_BASE_URL,
        ProviderKind::Vllm => DEFAULT_VLLM_BASE_URL,
        ProviderKind::Ollama => DEFAULT_OLLAMA_BASE_URL,
    }
}

fn moonshot_base_url_uses_kimi_code(base_url: &str) -> bool {
    let normalized = base_url.trim_end_matches('/').to_ascii_lowercase();
    normalized == DEFAULT_KIMI_CODE_BASE_URL
        || normalized == "https://api.kimi.com/coding"
        || normalized.starts_with("https://api.kimi.com/coding/")
}

fn base_url_is_custom_for_provider(provider: ProviderKind, base_url: &str) -> bool {
    if provider == ProviderKind::Siliconflow && siliconflow_base_url_is_official(base_url) {
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
}

impl ConfigStore {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = resolve_config_path(path)?;
        if !path.exists() {
            return Ok(Self {
                path,
                config: ConfigToml::default(),
            });
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let parsed: ConfigToml = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))?;

        Ok(Self {
            path,
            config: parsed,
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
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    Ok(home.join(CODEWHALE_APP_DIR))
}

/// Resolve the legacy DeepSeek home directory (`$HOME/.deepseek`).
///
/// Always returns the legacy path regardless of whether it exists.
pub fn legacy_deepseek_home() -> Result<PathBuf> {
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    Ok(home.join(LEGACY_APP_DIR))
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
    moonshot_model: Option<String>,
    xiaomi_mimo_model: Option<String>,
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
    moonshot_base_url: Option<String>,
    sglang_base_url: Option<String>,
    vllm_base_url: Option<String>,
    ollama_base_url: Option<String>,
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
            moonshot_model: std::env::var("MOONSHOT_MODEL")
                .or_else(|_| std::env::var("KIMI_MODEL_NAME"))
                .or_else(|_| std::env::var("KIMI_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            xiaomi_mimo_model: std::env::var("XIAOMI_MIMO_MODEL")
                .or_else(|_| std::env::var("MIMO_MODEL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            output_mode: std::env::var("DEEPSEEK_OUTPUT_MODE").ok(),
            auth_mode: std::env::var("DEEPSEEK_AUTH_MODE").ok(),
            log_level: std::env::var("DEEPSEEK_LOG_LEVEL").ok(),
            telemetry: std::env::var("DEEPSEEK_TELEMETRY")
                .ok()
                .and_then(|v| parse_bool(&v).ok()),
            approval_policy: std::env::var("DEEPSEEK_APPROVAL_POLICY").ok(),
            sandbox_mode: std::env::var("DEEPSEEK_SANDBOX_MODE").ok(),
            yolo: std::env::var("DEEPSEEK_YOLO")
                .ok()
                .and_then(|v| parse_bool(&v).ok()),
            http_headers: std::env::var("DEEPSEEK_HTTP_HEADERS")
                .ok()
                .and_then(|value| parse_http_headers(&value).ok())
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
            ProviderKind::Siliconflow => self.siliconflow_base_url.clone(),
            ProviderKind::Moonshot => self.moonshot_base_url.clone(),
            ProviderKind::Sglang => self.sglang_base_url.clone(),
            ProviderKind::Vllm => self.vllm_base_url.clone(),
            ProviderKind::Ollama => self.ollama_base_url.clone(),
        }
    }

    fn model_for(&self, provider: ProviderKind, base_url: &str) -> Option<String> {
        let model = match provider {
            ProviderKind::WanjieArk => self.wanjie_ark_model.clone(),
            ProviderKind::Volcengine => self.volcengine_model.clone(),
            ProviderKind::Siliconflow => self.siliconflow_model.clone(),
            ProviderKind::Moonshot => self.moonshot_model.clone(),
            ProviderKind::XiaomiMimo => self.xiaomi_mimo_model.clone(),
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
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
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
        xiaomi_mimo_api_key: Option<OsString>,
        xiaomi_api_key: Option<OsString>,
        mimo_api_key: Option<OsString>,
        xiaomi_mimo_base_url: Option<OsString>,
        mimo_base_url: Option<OsString>,
        xiaomi_mimo_model: Option<OsString>,
        mimo_model: Option<OsString>,
        wanjie_ark_api_key: Option<OsString>,
        wanjie_ark_base_url: Option<OsString>,
        wanjie_base_url: Option<OsString>,
        wanjie_maas_base_url: Option<OsString>,
        volcengine_model: Option<OsString>,
        wanjie_ark_model: Option<OsString>,
        wanjie_model: Option<OsString>,
        wanjie_maas_model: Option<OsString>,
        novita_api_key: Option<OsString>,
        novita_base_url: Option<OsString>,
        fireworks_api_key: Option<OsString>,
        fireworks_base_url: Option<OsString>,
        siliconflow_api_key: Option<OsString>,
        siliconflow_base_url: Option<OsString>,
        siliconflow_model: Option<OsString>,
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
                xiaomi_mimo_api_key: env::var_os("XIAOMI_MIMO_API_KEY"),
                xiaomi_api_key: env::var_os("XIAOMI_API_KEY"),
                mimo_api_key: env::var_os("MIMO_API_KEY"),
                xiaomi_mimo_base_url: env::var_os("XIAOMI_MIMO_BASE_URL"),
                mimo_base_url: env::var_os("MIMO_BASE_URL"),
                xiaomi_mimo_model: env::var_os("XIAOMI_MIMO_MODEL"),
                mimo_model: env::var_os("MIMO_MODEL"),
                wanjie_ark_api_key: env::var_os("WANJIE_ARK_API_KEY"),
                wanjie_ark_base_url: env::var_os("WANJIE_ARK_BASE_URL"),
                wanjie_base_url: env::var_os("WANJIE_BASE_URL"),
                wanjie_maas_base_url: env::var_os("WANJIE_MAAS_BASE_URL"),
                volcengine_model: env::var_os("VOLCENGINE_MODEL"),
                wanjie_ark_model: env::var_os("WANJIE_ARK_MODEL"),
                wanjie_model: env::var_os("WANJIE_MODEL"),
                wanjie_maas_model: env::var_os("WANJIE_MAAS_MODEL"),
                novita_api_key: env::var_os("NOVITA_API_KEY"),
                novita_base_url: env::var_os("NOVITA_BASE_URL"),
                fireworks_api_key: env::var_os("FIREWORKS_API_KEY"),
                fireworks_base_url: env::var_os("FIREWORKS_BASE_URL"),
                siliconflow_api_key: env::var_os("SILICONFLOW_API_KEY"),
                siliconflow_base_url: env::var_os("SILICONFLOW_BASE_URL"),
                siliconflow_model: env::var_os("SILICONFLOW_MODEL"),
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
                env::remove_var("XIAOMI_MIMO_API_KEY");
                env::remove_var("XIAOMI_API_KEY");
                env::remove_var("MIMO_API_KEY");
                env::remove_var("XIAOMI_MIMO_BASE_URL");
                env::remove_var("MIMO_BASE_URL");
                env::remove_var("XIAOMI_MIMO_MODEL");
                env::remove_var("MIMO_MODEL");
                env::remove_var("WANJIE_ARK_API_KEY");
                env::remove_var("WANJIE_ARK_BASE_URL");
                env::remove_var("WANJIE_BASE_URL");
                env::remove_var("WANJIE_MAAS_BASE_URL");
                env::remove_var("WANJIE_ARK_MODEL");
                env::remove_var("WANJIE_MODEL");
                env::remove_var("WANJIE_MAAS_MODEL");
                env::remove_var("NOVITA_API_KEY");
                env::remove_var("NOVITA_BASE_URL");
                env::remove_var("FIREWORKS_API_KEY");
                env::remove_var("FIREWORKS_BASE_URL");
                env::remove_var("SILICONFLOW_API_KEY");
                env::remove_var("SILICONFLOW_BASE_URL");
                env::remove_var("SILICONFLOW_MODEL");
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
                Self::restore_var("XIAOMI_MIMO_API_KEY", self.xiaomi_mimo_api_key.take());
                Self::restore_var("XIAOMI_API_KEY", self.xiaomi_api_key.take());
                Self::restore_var("MIMO_API_KEY", self.mimo_api_key.take());
                Self::restore_var("XIAOMI_MIMO_BASE_URL", self.xiaomi_mimo_base_url.take());
                Self::restore_var("MIMO_BASE_URL", self.mimo_base_url.take());
                Self::restore_var("XIAOMI_MIMO_MODEL", self.xiaomi_mimo_model.take());
                Self::restore_var("MIMO_MODEL", self.mimo_model.take());
                Self::restore_var("WANJIE_ARK_API_KEY", self.wanjie_ark_api_key.take());
                Self::restore_var("WANJIE_ARK_BASE_URL", self.wanjie_ark_base_url.take());
                Self::restore_var("WANJIE_BASE_URL", self.wanjie_base_url.take());
                Self::restore_var("WANJIE_MAAS_BASE_URL", self.wanjie_maas_base_url.take());
                Self::restore_var("VOLCENGINE_MODEL", self.volcengine_model.take());
                Self::restore_var("WANJIE_ARK_MODEL", self.wanjie_ark_model.take());
                Self::restore_var("WANJIE_MODEL", self.wanjie_model.take());
                Self::restore_var("WANJIE_MAAS_MODEL", self.wanjie_maas_model.take());
                Self::restore_var("NOVITA_API_KEY", self.novita_api_key.take());
                Self::restore_var("NOVITA_BASE_URL", self.novita_base_url.take());
                Self::restore_var("FIREWORKS_API_KEY", self.fireworks_api_key.take());
                Self::restore_var("FIREWORKS_BASE_URL", self.fireworks_base_url.take());
                Self::restore_var("SILICONFLOW_API_KEY", self.siliconflow_api_key.take());
                Self::restore_var("SILICONFLOW_BASE_URL", self.siliconflow_base_url.take());
                Self::restore_var("SILICONFLOW_MODEL", self.siliconflow_model.take());
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
    fn project_merge_denies_credentials_endpoints_and_provider_selection() {
        let mut base = ConfigToml {
            provider: ProviderKind::Deepseek,
            api_key: Some("user-key".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
            default_text_model: Some("deepseek-v4-flash".to_string()),
            ..ConfigToml::default()
        };
        base.providers.openrouter.api_key = Some("user-openrouter-key".to_string());

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
        project.providers.openrouter.model = Some("deepseek/deepseek-v4-pro".to_string());

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
        assert_eq!(base.default_text_model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(
            base.providers.openrouter.model.as_deref(),
            Some("deepseek/deepseek-v4-pro")
        );
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
    fn migrate_config_reports_copied_legacy_path() {
        let _lock = env_lock();
        struct HomeEnvGuard {
            home: Option<OsString>,
            userprofile: Option<OsString>,
            #[cfg(windows)]
            homedrive: Option<OsString>,
            #[cfg(windows)]
            homepath: Option<OsString>,
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
                    #[cfg(windows)]
                    match self.homedrive.take() {
                        Some(value) => env::set_var("HOMEDRIVE", value),
                        None => env::remove_var("HOMEDRIVE"),
                    }
                    #[cfg(windows)]
                    match self.homepath.take() {
                        Some(value) => env::set_var("HOMEPATH", value),
                        None => env::remove_var("HOMEPATH"),
                    }
                    match self.codewhale_home.take() {
                        Some(value) => env::set_var("CODEWHALE_HOME", value),
                        None => env::remove_var("CODEWHALE_HOME"),
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
        fs::create_dir_all(&legacy_dir).expect("legacy dir");
        fs::write(
            legacy_dir.join(CONFIG_FILE_NAME),
            "provider = \"deepseek\"\n",
        )
        .expect("legacy config");

        let _env = HomeEnvGuard {
            home: env::var_os("HOME"),
            userprofile: env::var_os("USERPROFILE"),
            #[cfg(windows)]
            homedrive: env::var_os("HOMEDRIVE"),
            #[cfg(windows)]
            homepath: env::var_os("HOMEPATH"),
            codewhale_home: env::var_os("CODEWHALE_HOME"),
        };
        // Safety: test-only environment mutation is serialized by env_lock().
        unsafe {
            env::set_var("HOME", &home);
            env::set_var("USERPROFILE", &home);
            #[cfg(windows)]
            {
                let mut components = home.components();
                if let Some(std::path::Component::Prefix(prefix)) = components.next() {
                    env::set_var("HOMEDRIVE", prefix.as_os_str());
                    env::set_var("HOMEPATH", components.as_path());
                }
            }
            env::remove_var("CODEWHALE_HOME");
        }

        let migration = migrate_config_if_needed()
            .expect("migration")
            .expect("legacy config should be copied");

        assert_eq!(migration.legacy_path, legacy_dir.join(CONFIG_FILE_NAME));
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
    fn openrouter_env_api_key_falls_back_when_config_missing() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "openrouter");
            env::set_var("OPENROUTER_API_KEY", "or-env-key");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.api_key.as_deref(), Some("or-env-key"));
        assert_eq!(resolved.base_url, DEFAULT_OPENROUTER_BASE_URL);
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
    fn novita_env_api_key_falls_back_when_config_missing() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "novita");
            env::set_var("NOVITA_API_KEY", "novita-env-key");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Novita);
        assert_eq!(resolved.api_key.as_deref(), Some("novita-env-key"));
        assert_eq!(resolved.base_url, DEFAULT_NOVITA_BASE_URL);
    }

    #[test]
    fn fireworks_env_api_key_falls_back_when_config_missing() {
        let _lock = env_lock();
        let _env = EnvGuard::without_deepseek_runtime_overrides();
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "fireworks");
            env::set_var("FIREWORKS_API_KEY", "fw-env-key");
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Fireworks);
        assert_eq!(resolved.api_key.as_deref(), Some("fw-env-key"));
        assert_eq!(resolved.base_url, DEFAULT_FIREWORKS_BASE_URL);
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
            ("qwen3.6-35b-a3b", OPENROUTER_QWEN_3_6_35B_A3B_MODEL),
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
}
