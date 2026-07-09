pub mod auth_source;
pub mod catalog;
mod harness;
pub mod model_reference;
pub mod models_dev;
pub mod persistence;
pub mod pricing;
pub mod provider;
mod provider_defaults;
mod provider_kind;
pub mod route;
pub mod setup_state;
pub mod user_constitution;
pub use harness::{
    HarnessCompactionStrategy, HarnessPosture, HarnessPostureKind, HarnessProfile,
    HarnessSafetyPosture, HarnessToolSurface, built_in_harness_profiles,
};
pub use model_reference::{Modality, ModelReferenceCard, ModelReferenceDatabase};
pub(crate) use provider_defaults::*;
pub use provider_kind::ProviderKind;
pub use setup_state::{
    ConstitutionAuthoring, ConstitutionChoice, ConstitutionSource, ConstitutionValidity,
    InheritedConfigFacts, RuntimePostureSource, SetupState, SetupStep, StepEntry, StepStatus,
};
pub use user_constitution::{
    AutonomyPreference, UntrustedDraftParse, UserConstitution, UserConstitutionLoad,
};

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
#[cfg(unix)]
use std::io::Read;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
pub use auth_source::{AuthSourceKind, ProviderAuthSourceToml};
pub use codewhale_execpolicy::ToolAskRule;
use codewhale_execpolicy::{ExecPolicyEngine, Ruleset};
use codewhale_secrets::SecretSource;
pub use codewhale_secrets::Secrets;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const PERMISSIONS_FILE_NAME: &str = "permissions.toml";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfigToml {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    #[serde(
        default,
        alias = "contextWindow",
        alias = "context_window_tokens",
        alias = "contextWindowTokens",
        alias = "context_length",
        alias = "contextLength"
    )]
    pub context_window: Option<u32>,
    pub mode: Option<String>,
    pub auth_mode: Option<String>,
    pub insecure_skip_tls_verify: Option<bool>,
    #[serde(default)]
    pub http_headers: BTreeMap<String, String>,
    pub path_suffix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<ProviderAuthSourceToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersToml {
    #[serde(default)]
    pub deepseek: ProviderConfigToml,
    #[serde(
        default,
        alias = "deepseek-anthropic",
        alias = "deepseekAnthropic",
        alias = "deepseek-claude",
        alias = "deepseek_claude"
    )]
    pub deepseek_anthropic: ProviderConfigToml,
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
        alias = "baidu-qianfan",
        alias = "baidu_qianfan",
        alias = "baidu"
    )]
    pub qianfan: ProviderConfigToml,
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
    #[serde(default, alias = "open-model", alias = "open_model")]
    pub openmodel: ProviderConfigToml,
    #[serde(
        default,
        alias = "z-ai",
        alias = "z_ai",
        alias = "z.ai",
        alias = "zhipu",
        alias = "zhipuai",
        alias = "bigmodel",
        alias = "big-model"
    )]
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
    #[serde(default, alias = "sakana-ai", alias = "sakana_ai", alias = "fugu")]
    pub sakana: ProviderConfigToml,
    #[serde(
        default,
        alias = "long-cat",
        alias = "meituan-longcat",
        alias = "meituan"
    )]
    pub longcat: ProviderConfigToml,
    #[serde(default, alias = "x-ai", alias = "x_ai", alias = "grok")]
    pub xai: ProviderConfigToml,
    /// Catch-all table for the dynamic OpenAI-compatible custom provider
    /// identity (#1519). Arbitrary `[providers.<name>]` tables are handled by
    /// the tui-side flatten map; this named slot keeps the canonical
    /// `ProviderKind::Custom` lookups total without leaking into another
    /// provider's config.
    #[serde(default)]
    pub custom: ProviderConfigToml,
}

/// Sibling `permissions.toml` schema.
///
/// Each rule is a typed condition that can deny, allow, or ask before a tool
/// invocation. UI actions that persist deny/allow rules are future work; the
/// approval card still saves ask rules.
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
        use codewhale_execpolicy::PermissionAction;
        let mut denied = Vec::new();
        let mut trusted = Vec::new();
        let mut ask_rules = Vec::new();

        for rule in &self.rules {
            match rule.action {
                PermissionAction::Deny => {
                    // Command-based deny rules are promoted to denied_prefixes
                    // so they are caught by execpolicy's deny-always-wins check.
                    if let Some(cmd) = &rule.command {
                        denied.push(cmd.clone());
                    }
                    // Always keep in ask_rules for path-based and tool-only matching.
                    ask_rules.push(rule.clone());
                }
                PermissionAction::Allow => {
                    // Command-based allow rules are promoted to trusted_prefixes
                    // for arity-aware matching.  Path-only allow rules are
                    // handled through ask_rules (they skip the approval prompt).
                    if let Some(cmd) = &rule.command {
                        trusted.push(cmd.clone());
                    }
                    // Keep in ask_rules so path-only allow rules also work.
                    ask_rules.push(rule.clone());
                }
                PermissionAction::Ask => {
                    ask_rules.push(rule.clone());
                }
            }
        }

        Ruleset::user(trusted, denied).with_ask_rules(ask_rules)
    }
}

impl ProvidersToml {
    #[must_use]
    pub fn for_provider(&self, provider: ProviderKind) -> &ProviderConfigToml {
        match provider {
            ProviderKind::Deepseek => &self.deepseek,
            ProviderKind::DeepseekAnthropic => &self.deepseek_anthropic,
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
            ProviderKind::Qianfan => &self.qianfan,
            ProviderKind::OpenaiCodex => &self.openai_codex,
            ProviderKind::Anthropic => &self.anthropic,
            ProviderKind::Openmodel => &self.openmodel,
            ProviderKind::Zai => &self.zai,
            ProviderKind::Stepfun => &self.stepfun,
            ProviderKind::Minimax => &self.minimax,
            ProviderKind::Deepinfra => &self.deepinfra,
            ProviderKind::Sakana => &self.sakana,
            ProviderKind::LongCat => &self.longcat,
            ProviderKind::Xai => &self.xai,
            ProviderKind::Custom => &self.custom,
        }
    }

    pub fn for_provider_mut(&mut self, provider: ProviderKind) -> &mut ProviderConfigToml {
        match provider {
            ProviderKind::Deepseek => &mut self.deepseek,
            ProviderKind::DeepseekAnthropic => &mut self.deepseek_anthropic,
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
            ProviderKind::Qianfan => &mut self.qianfan,
            ProviderKind::OpenaiCodex => &mut self.openai_codex,
            ProviderKind::Anthropic => &mut self.anthropic,
            ProviderKind::Openmodel => &mut self.openmodel,
            ProviderKind::Zai => &mut self.zai,
            ProviderKind::Stepfun => &mut self.stepfun,
            ProviderKind::Minimax => &mut self.minimax,
            ProviderKind::Deepinfra => &mut self.deepinfra,
            ProviderKind::Sakana => &mut self.sakana,
            ProviderKind::LongCat => &mut self.longcat,
            ProviderKind::Xai => &mut self.xai,
            ProviderKind::Custom => &mut self.custom,
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
    /// Verifier-preview behavior (#2093). When absent, verifier tools keep the
    /// shipped defaults: disabled automatic preview and hunt verdict mapping.
    #[serde(default)]
    pub verifier: Option<VerifierConfigToml>,
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
    /// Workflow automatic-launch, approval, isolation, and activity
    /// persistence knobs (#4128 / Section 2.11). When absent, consumers use
    /// [`WorkflowConfigToml::default`].
    #[serde(default)]
    pub workflow: Option<WorkflowConfigToml>,
    #[serde(flatten)]
    pub extras: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderConfigField {
    ApiKey,
    BaseUrl,
    Model,
    ContextWindow,
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
            "context_window" | "context_window_tokens" => Self::ContextWindow,
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
            Self::ContextWindow => "context_window",
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
        ProviderConfigField::ContextWindow => config.context_window.map(|value| value.to_string()),
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

fn parse_context_window(value: &str) -> Result<u32> {
    let parsed = value.trim().parse::<u32>().with_context(|| {
        format!("invalid context_window '{value}': expected a positive token count")
    })?;
    if parsed == 0 {
        bail!("context_window must be greater than 0");
    }
    Ok(parsed)
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
        ProviderConfigField::ContextWindow => {
            config.providers.for_provider_mut(provider).context_window =
                Some(parse_context_window(value)?);
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
        ProviderConfigField::ContextWindow => {
            config.providers.for_provider_mut(provider).context_window = None;
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
    if let Some(v) = config.context_window {
        out.insert(
            provider_config_key(provider, ProviderConfigField::ContextWindow),
            v.to_string(),
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
    "mode.operate",
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

/// The default hotbar slots in on-disk (`[[hotbar]]`) form. Since #3807 an
/// absent `hotbar` key means "hidden", so `/hotbar on` persists these explicit
/// bindings rather than deleting the key. Kept in terms of
/// [`default_hotbar_bindings`] so `DEFAULT_HOTBAR_ACTIONS` stays the single
/// source of truth.
#[must_use]
pub fn default_hotbar_bindings_toml() -> Vec<HotbarBindingToml> {
    default_hotbar_bindings()
        .into_iter()
        .map(|binding| HotbarBindingToml {
            slot: binding.slot,
            action: binding.action,
            label: binding.label,
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
        // #3807: an absent `hotbar` key means the Hotbar is hidden until the
        // user opts in (via the setup wizard or `/hotbar on`). Only an explicit
        // `[[hotbar]]` config produces bindings. `Some([])` stays "disabled".
        None => Vec::new(),
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
            .or_else(|| self.providers.first().copied())
            .unwrap_or_default()
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

#[cfg(test)]
mod provider_chain_tests {
    use super::*;

    #[test]
    fn current_on_empty_chain_returns_default_provider() {
        let chain = ProviderChain {
            providers: vec![],
            position: 0,
        };
        assert_eq!(chain.current(), ProviderKind::default());
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
    /// Fleet profile vocabulary (#3167). Profiles group role semantics,
    /// loadout hints, permission defaults, and delegation bounds. They are
    /// config-only in this slice; executor/model routing wiring lands later.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, FleetProfile>,
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
pub const DEFAULT_STREAM_CHUNK_TIMEOUT_SECS: u64 = 900;
pub const MIN_STREAM_CHUNK_TIMEOUT_SECS: u64 = 1;
pub const MAX_STREAM_CHUNK_TIMEOUT_SECS: u64 = 3600;

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

/// Fleet org-chart profile.
///
/// A profile is an additive config record for future fleet scheduling policy.
/// Loading one must not grant runtime permissions by itself: shell and trust
/// escalation default off, and approvals default on.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct FleetProfile {
    /// Org-chart slot this profile describes.
    #[serde(default)]
    pub slot: FleetSlot,
    /// Semantic role name and optional instruction overlay.
    #[serde(default)]
    pub role: FleetRole,
    /// Model class / route-role hint. This is data only in this slice.
    #[serde(default)]
    pub loadout: FleetLoadout,
    /// Optional explicit model id for this profile on the active/resolved route.
    ///
    /// This is not an auth or endpoint selector. Provider-scoped routing still
    /// validates the executable provider/model/wire-model decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional explicit provider id for this profile's model (#4093).
    ///
    /// Present only when the profile was created against a specific,
    /// credential-checked provider (e.g. via the Fleet setup model picker),
    /// so a worker can be pinned to a route independent of the parent/current
    /// session provider. `None` means "no route pin" (inherit), matching
    /// `model: None`; a profile must never carry `provider` without `model`.
    ///
    /// EPIC #2608 explicit-config-only mandate: this field is the ONLY
    /// authority for the profile's provider. It is never inferred by sniffing
    /// a substring/prefix out of `model` — callers that need the provider for
    /// this profile must read this field, not guess from the model id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Optional explicit reasoning/thinking tier for this profile (#4137).
    ///
    /// This is a safe, non-secret route tuning value. `None` means inherit the
    /// operator/session reasoning tier. Concrete values are normalized by the
    /// TUI loader before they are used at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Permission defaults requested by the profile.
    #[serde(default)]
    pub permissions: FleetProfilePermissions,
    /// Delegation hints for future manager policy.
    #[serde(default)]
    pub delegation: FleetDelegationHints,
}

/// Semantic role declaration for a fleet profile.
///
/// TOML may use either `role = "reviewer"` or a role table with `name` and
/// `instructions`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FleetRole {
    /// Stable role name, e.g. `scout`, `implementer`, or `verifier`.
    pub name: String,
    /// Optional short description for config UIs and docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional instruction overlay to apply when the role is later consumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

impl Default for FleetRole {
    fn default() -> Self {
        Self {
            name: "general".to_string(),
            description: None,
            instructions: None,
        }
    }
}

impl<'de> Deserialize<'de> for FleetRole {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum FleetRoleWire {
            Name(String),
            Full {
                #[serde(default)]
                name: Option<String>,
                #[serde(default)]
                description: Option<String>,
                #[serde(default)]
                instructions: Option<String>,
            },
        }

        match FleetRoleWire::deserialize(deserializer)? {
            FleetRoleWire::Name(name) => Ok(Self {
                name,
                ..Self::default()
            }),
            FleetRoleWire::Full {
                name,
                description,
                instructions,
            } => Ok(Self {
                name: name.unwrap_or_else(|| Self::default().name),
                description,
                instructions,
            }),
        }
    }
}

/// Org-chart slot for grouping fleet profiles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FleetSlot {
    Manager,
    Scout,
    Implementer,
    Reviewer,
    Verifier,
    Operator,
    Summarizer,
    #[default]
    General,
    Custom(String),
}

impl FleetSlot {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Manager => "manager",
            Self::Scout => "scout",
            Self::Implementer => "implementer",
            Self::Reviewer => "reviewer",
            Self::Verifier => "verifier",
            Self::Operator => "operator",
            Self::Summarizer => "summarizer",
            Self::General => "general",
            Self::Custom(value) => value.as_str(),
        }
    }

    #[must_use]
    pub fn from_name(value: &str) -> Self {
        match value.trim() {
            "manager" | "coordinator" => Self::Manager,
            "scout" | "research" | "research-worker" => Self::Scout,
            "implementer" | "builder" => Self::Implementer,
            "reviewer" => Self::Reviewer,
            "verifier" | "tester" => Self::Verifier,
            "operator" | "incident" | "incident-worker" => Self::Operator,
            "summarizer" | "reducer" => Self::Summarizer,
            "general" | "" => Self::General,
            // Removed slots (e.g. the old "tool-heavy") and unknown names parse
            // as Custom, which dispatches on the General surface — identical to
            // the behavior the removed variants had.
            other => Self::Custom(other.to_string()),
        }
    }
}

impl Serialize for FleetSlot {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FleetSlot {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_name(&value))
    }
}

/// Model class or route-role hint for a profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FleetLoadout {
    /// Reuse the active session route (the operator's model). Default.
    #[default]
    Inherit,
    /// Route to the provider's faster/cheaper model class for wide fan-out.
    Fast,
    /// Unrecognized loadout names parse here (including the retired
    /// strong/balanced/deep-reasoning/code/review/tool-heavy tiers, which
    /// never routed differently). Treated as auto routing.
    Custom(String),
}

impl FleetLoadout {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Inherit => "inherit",
            Self::Fast => "fast",
            Self::Custom(value) => value.as_str(),
        }
    }

    #[must_use]
    pub fn from_name(value: &str) -> Self {
        match value.trim() {
            "inherit" | "default" | "auto" | "" => Self::Inherit,
            "fast" => Self::Fast,
            // Retired tiers (strong/balanced/deep-reasoning/code/review/
            // tool-heavy) and unknown names parse as Custom → auto routing,
            // exactly what those tiers resolved to before removal.
            other => Self::Custom(other.to_string()),
        }
    }
}

impl Serialize for FleetLoadout {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FleetLoadout {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_name(&value))
    }
}

/// Safe permission defaults attached to a fleet profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FleetProfilePermissions {
    /// Permit shell-capable tools for this profile when later consumed.
    #[serde(default)]
    pub allow_shell: bool,
    /// Permit trusted/elevated execution for this profile when later consumed.
    #[serde(default)]
    pub trust: bool,
    /// Require approval by default. This intentionally defaults on.
    #[serde(default = "default_fleet_profile_approval_required")]
    pub approval_required: bool,
}

fn default_fleet_profile_approval_required() -> bool {
    true
}

impl Default for FleetProfilePermissions {
    fn default() -> Self {
        Self {
            allow_shell: false,
            trust: false,
            approval_required: true,
        }
    }
}

/// Delegation hints for future fleet manager scheduling.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct FleetDelegationHints {
    /// Optional profile-level child spawn depth. `None` means inherit existing
    /// fleet/sub-agent config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_spawn_depth: Option<u32>,
    /// Optional profile-level worker concurrency hint.
    #[serde(
        default,
        alias = "concurrency",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_concurrency: Option<usize>,
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
            profiles: BTreeMap::new(),
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

/// On-disk schema for the `[workflow]` table (#4128 / Section 2.11).
///
/// Automatic Workflow launch, write/approval gates, child/isolation budgets,
/// and completed-activity persistence all read from this one model. When the
/// table is absent, consumers resolve [`WorkflowConfigToml::default`].
/// See `config.example.toml` for documentation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowConfigToml {
    /// Allow the parent agent to auto-launch Workflow for multi-agent work.
    /// Product default is on; set `false` to require explicit `/workflow`.
    #[serde(default = "default_workflow_automatic")]
    pub automatic: bool,
    /// When automatic launch is enabled, start read-only child plans without
    /// an approval card. Write/shell/network plans still consult
    /// [`Self::require_approval_for_writes`].
    #[serde(default = "default_workflow_auto_start_read_only")]
    pub auto_start_read_only: bool,
    /// Require an operator approval card before launching plans that write,
    /// elevate shell/network, or otherwise leave the read-only envelope.
    #[serde(default = "default_workflow_require_approval_for_writes")]
    pub require_approval_for_writes: bool,
    /// Soft upper bound on children admitted by automatic launch. Larger plans
    /// should ask the operator or use explicit `/workflow`.
    #[serde(default = "default_workflow_auto_start_child_limit")]
    pub auto_start_child_limit: u32,
    /// Hard ceiling on total children in one Workflow run (product: 1000).
    #[serde(default = "default_workflow_max_children")]
    pub max_children: u32,
    /// Maximum concurrently live agents inside one Workflow run (product: 16).
    #[serde(default = "default_workflow_max_concurrent")]
    pub max_concurrent: u32,
    /// Maximum nested Workflow / child-orchestration depth.
    #[serde(default = "default_workflow_max_depth")]
    pub max_depth: u32,
    /// Default shared token budget for a Workflow run and its children.
    #[serde(default = "default_workflow_default_token_budget")]
    pub default_token_budget: u64,
    /// How many parallel write children may share the parent worktree without
    /// isolation. `0` forces worktree isolation for parallel writes.
    #[serde(default = "default_workflow_max_parallel_writes_without_worktree")]
    pub max_parallel_writes_without_worktree: u32,
    /// Keep completed Workflow activity visible in the session activity surface
    /// until the next run (or explicit clear).
    #[serde(default = "default_workflow_persist_completed_activity")]
    pub persist_completed_activity: bool,
    /// Persist completed Workflow activity across process restarts via the
    /// durable run journal.
    #[serde(default = "default_workflow_persist_completed_across_restarts")]
    pub persist_completed_across_restarts: bool,
}

fn default_workflow_automatic() -> bool {
    true
}

fn default_workflow_auto_start_read_only() -> bool {
    true
}

fn default_workflow_require_approval_for_writes() -> bool {
    true
}

fn default_workflow_auto_start_child_limit() -> u32 {
    // Soft auto stays small; explicit launches may use the full concurrent cap.
    16
}

fn default_workflow_max_children() -> u32 {
    1000
}

fn default_workflow_max_concurrent() -> u32 {
    16
}

fn default_workflow_max_depth() -> u32 {
    2
}

fn default_workflow_default_token_budget() -> u64 {
    120_000
}

fn default_workflow_max_parallel_writes_without_worktree() -> u32 {
    0
}

fn default_workflow_persist_completed_activity() -> bool {
    true
}

fn default_workflow_persist_completed_across_restarts() -> bool {
    true
}

impl Default for WorkflowConfigToml {
    fn default() -> Self {
        Self {
            automatic: default_workflow_automatic(),
            auto_start_read_only: default_workflow_auto_start_read_only(),
            require_approval_for_writes: default_workflow_require_approval_for_writes(),
            auto_start_child_limit: default_workflow_auto_start_child_limit(),
            max_children: default_workflow_max_children(),
            max_concurrent: default_workflow_max_concurrent(),
            max_depth: default_workflow_max_depth(),
            default_token_budget: default_workflow_default_token_budget(),
            max_parallel_writes_without_worktree:
                default_workflow_max_parallel_writes_without_worktree(),
            persist_completed_activity: default_workflow_persist_completed_activity(),
            persist_completed_across_restarts: default_workflow_persist_completed_across_restarts(),
        }
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

/// Verdict policy for the verifier-preview surface (#2093).
///
/// Only the hunt vocabulary is shipped today. Keeping this typed lets future
/// policy additions reject misspellings instead of silently accepting unknown
/// strings.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerifierVerdictPolicy {
    #[default]
    Hunt,
}

/// On-disk schema for `[verifier]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifierConfigToml {
    /// Enable automatic verifier preview when the runtime wires a
    /// claim-of-done trigger. Manual `run_verifiers` remains available
    /// regardless.
    #[serde(default)]
    pub enabled: bool,
    /// How verifier verdicts map into the goal/hunt system.
    #[serde(default)]
    pub verdict_policy: VerifierVerdictPolicy,
}

impl Default for VerifierConfigToml {
    fn default() -> Self {
        Self {
            enabled: false,
            verdict_policy: VerifierVerdictPolicy::Hunt,
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

/// User-defined LSP server for one file extension (used inside
/// [`LspConfigToml::custom`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CustomLspDef {
    /// LSP `languageId` value used in `textDocument/didOpen`.
    pub language_id: String,
    /// Executable to spawn.
    pub command: String,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
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
    /// User-defined LSP servers for file extensions not in the built-in
    /// registry. Keyed by extension (e.g. `"php"`, `"rb"`).
    pub custom: Option<BTreeMap<String, CustomLspDef>>,
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
            "stream_chunk_timeout_secs" | "tui.stream_chunk_timeout_secs" => {
                Some(self.stream_chunk_timeout_secs().to_string())
            }
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

    #[must_use]
    pub fn stream_chunk_timeout_secs(&self) -> u64 {
        let raw = self
            .extras
            .get("tui")
            .and_then(toml::Value::as_table)
            .and_then(|table| table.get("stream_chunk_timeout_secs"))
            .and_then(toml_value_as_u64)
            .or_else(|| {
                self.extras
                    .get("tui.stream_chunk_timeout_secs")
                    .and_then(toml_value_as_u64)
            })
            .or_else(|| {
                self.extras
                    .get("stream_chunk_timeout_secs")
                    .and_then(toml_value_as_u64)
            })
            .unwrap_or(DEFAULT_STREAM_CHUNK_TIMEOUT_SECS);
        if raw == 0 {
            DEFAULT_STREAM_CHUNK_TIMEOUT_SECS
        } else {
            raw.clamp(MIN_STREAM_CHUNK_TIMEOUT_SECS, MAX_STREAM_CHUNK_TIMEOUT_SECS)
        }
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
                ProviderKind::DeepseekAnthropic => DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL.to_string(),
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
                ProviderKind::Qianfan => DEFAULT_QIANFAN_BASE_URL.to_string(),
                ProviderKind::OpenaiCodex => DEFAULT_OPENAI_CODEX_BASE_URL.to_string(),
                ProviderKind::Anthropic => DEFAULT_ANTHROPIC_BASE_URL.to_string(),
                ProviderKind::Openmodel => DEFAULT_OPENMODEL_BASE_URL.to_string(),
                ProviderKind::Zai => DEFAULT_ZAI_BASE_URL.to_string(),
                ProviderKind::Stepfun => DEFAULT_STEPFUN_BASE_URL.to_string(),
                ProviderKind::Minimax => DEFAULT_MINIMAX_BASE_URL.to_string(),
                ProviderKind::Deepinfra => DEFAULT_DEEPINFRA_BASE_URL.to_string(),
                ProviderKind::Sakana => DEFAULT_SAKANA_BASE_URL.to_string(),
                ProviderKind::LongCat => DEFAULT_LONGCAT_BASE_URL.to_string(),
                ProviderKind::Xai => DEFAULT_XAI_BASE_URL.to_string(),
                // The custom provider has no built-in endpoint; fall back to its
                // descriptor placeholder so the lookup is total. Real custom
                // routes always supply a configured base_url before this point.
                ProviderKind::Custom => provider.provider().default_base_url().to_string(),
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
        if !project_config_candidate_exists(&path) {
            continue;
        }
        let raw = match read_checked_config_file(&path) {
            Ok(raw) => raw,
            Err(e) => {
                tracing::warn!("Failed to read project config {}: {e:#}", path.display());
                return None;
            }
        };
        match toml::from_str(&raw) {
            Ok(config) => return Some(config),
            Err(e) => {
                tracing::warn!("Failed to parse project config {}: {e}", path.display());
                return None;
            }
        }
    }
    None
}

fn project_config_candidate_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        let file_type = metadata.file_type();
        file_type.is_file() || file_type.is_symlink()
    })
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
            | ProviderKind::Qianfan
            | ProviderKind::Ollama
            | ProviderKind::Xai
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
        XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL
        | "mimo-v2-5-pro-ultraspeed"
        | "xiaomi-mimo-v2.5-pro-ultraspeed"
        | "xiaomi-mimo-v2-5-pro-ultraspeed"
        | "ultraspeed"
        | "pro-ultraspeed" => Some(XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL),
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
        OPENROUTER_MINIMAX_M2_7_MODEL
        | "minimax-2.7"
        | "minimax-2-7"
        | "minimax-m2.7"
        | "minimax-m2-7"
        | "minimax-m-2.7"
        | "minimax-m-2-7" => Some(OPENROUTER_MINIMAX_M2_7_MODEL),
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
        ProviderKind::DeepseekAnthropic => DEFAULT_DEEPSEEK_ANTHROPIC_MODEL,
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
        ProviderKind::Qianfan => DEFAULT_QIANFAN_MODEL,
        ProviderKind::OpenaiCodex => DEFAULT_OPENAI_CODEX_MODEL,
        ProviderKind::Anthropic => DEFAULT_ANTHROPIC_MODEL,
        ProviderKind::Openmodel => DEFAULT_OPENMODEL_MODEL,
        ProviderKind::Zai => DEFAULT_ZAI_MODEL,
        ProviderKind::Stepfun => DEFAULT_STEPFUN_MODEL,
        ProviderKind::Minimax => DEFAULT_MINIMAX_MODEL,
        ProviderKind::Deepinfra => DEFAULT_DEEPINFRA_MODEL,
        ProviderKind::Sakana => DEFAULT_SAKANA_MODEL,
        ProviderKind::LongCat => DEFAULT_LONGCAT_MODEL,
        ProviderKind::Xai => DEFAULT_XAI_MODEL,
        // No built-in default model; the registry placeholder keeps this total.
        ProviderKind::Custom => provider.provider().default_model(),
    }
}

fn default_base_url_for_provider(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Deepseek => DEFAULT_DEEPSEEK_BASE_URL,
        ProviderKind::DeepseekAnthropic => DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL,
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
        ProviderKind::Qianfan => DEFAULT_QIANFAN_BASE_URL,
        ProviderKind::OpenaiCodex => DEFAULT_OPENAI_CODEX_BASE_URL,
        ProviderKind::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
        ProviderKind::Openmodel => DEFAULT_OPENMODEL_BASE_URL,
        ProviderKind::Zai => DEFAULT_ZAI_BASE_URL,
        ProviderKind::Stepfun => DEFAULT_STEPFUN_BASE_URL,
        ProviderKind::Minimax => DEFAULT_MINIMAX_BASE_URL,
        ProviderKind::Deepinfra => DEFAULT_DEEPINFRA_BASE_URL,
        ProviderKind::Sakana => DEFAULT_SAKANA_BASE_URL,
        ProviderKind::LongCat => DEFAULT_LONGCAT_BASE_URL,
        ProviderKind::Xai => DEFAULT_XAI_BASE_URL,
        // No built-in default base URL; the registry placeholder keeps this total.
        ProviderKind::Custom => provider.provider().default_base_url(),
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
        let (config, original_raw) = if checked_path_exists(&path)? {
            let raw = read_checked_config_file(&path)?;
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

    /// Render the exact body [`save`](Self::save) would write: the serialized
    /// config with comments and disabled keys from the originally-loaded file
    /// merged back in. Exposed so setup flows can stage this body into a
    /// [`persistence::SetupTransaction`] alongside sibling files and keep the
    /// comment-preserving write atomic with the rest of the transaction.
    pub fn rendered_body(&self) -> Result<String> {
        let serialized =
            toml::to_string_pretty(&self.config).context("failed to serialize config")?;
        if let Some(ref original_raw) = self.original_raw {
            Ok(
                merge_and_preserve_comments(&serialized, original_raw).unwrap_or_else(|e| {
                    tracing::warn!("failed to merge config comments, saving without them: {e:#}");
                    serialized
                }),
            )
        } else {
            Ok(serialized)
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = normalize_config_file_path(self.path.clone())?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }
        let body = self.rendered_body()?;
        if checked_path_exists(&path)? {
            let existing = read_checked_config_file(&path)?;
            if existing == body {
                return Ok(());
            }
            write_one_time_config_backup(&path)?;
        }
        persistence::atomic_write(&path, body.as_bytes())
            .with_context(|| format!("failed to write config at {}", path.display()))?;
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
        checked_permissions_path_for_config_path(&self.path)
            .expect("ConfigStore path is validated before construction")
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

        let path = checked_permissions_path_for_config_path(&self.path)?;
        let raw = if checked_path_exists(&path)? {
            read_checked_permissions_file(&path)?
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

fn config_backup_file_name(path: &Path) -> OsString {
    let mut file_name = path
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from(CONFIG_FILE_NAME));
    file_name.push(".bak");
    file_name
}

fn config_sibling_path_unchecked(config_path: &Path, file_name: &OsStr) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file_name)
}

fn checked_config_sibling_path(config_path: &Path, file_name: &OsStr) -> Result<PathBuf> {
    let config_path = normalize_config_file_path(config_path.to_path_buf())?;
    let parent = config_path
        .parent()
        .context("config path must include a parent directory")?;
    let path = parent.join(file_name);
    reject_path_symlink(&path)?;
    Ok(path)
}

#[cfg(test)]
fn config_backup_path(path: &Path) -> PathBuf {
    config_sibling_path_unchecked(path, &config_backup_file_name(path))
}

fn checked_config_backup_path(path: &Path) -> Result<PathBuf> {
    checked_config_sibling_path(path, &config_backup_file_name(path))
}

fn write_one_time_config_backup(path: &Path) -> Result<()> {
    let backup = checked_config_backup_path(path)?;
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
    if let Some(path) = codewhale_home_env_override() {
        return Ok(path);
    }
    let home = effective_home_dir().context("failed to resolve home directory")?;
    Ok(home.join(CODEWHALE_APP_DIR))
}

fn codewhale_home_env_override() -> Option<PathBuf> {
    let val = std::env::var("CODEWHALE_HOME").ok()?;
    let trimmed = val.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Whether `$CODEWHALE_HOME` is set to a non-empty value.
///
/// An explicit CodeWhale home is an isolation boundary: state/config resolvers
/// must not fall back to ambient legacy `~/.deepseek` data outside that root.
pub fn codewhale_home_is_explicit() -> bool {
    codewhale_home_env_override().is_some()
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
    let explicit_codewhale_home = codewhale_home_env_override().is_some();
    let primary = codewhale_home()?.join(subdir);
    if explicit_codewhale_home || primary.exists() {
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
    let (dir, migration) = ensure_state_dir_with_migration(subdir)?;
    if let Some(migration) = migration {
        eprintln!("{}", migration.user_notice());
    }
    Ok(dir)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateMigrationKind {
    Relocated,
    Copied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateMigration {
    pub subdir: String,
    pub legacy_path: PathBuf,
    pub primary_path: PathBuf,
    pub kind: StateMigrationKind,
}

impl StateMigration {
    pub fn user_notice(&self) -> String {
        let action = match self.kind {
            StateMigrationKind::Relocated => "relocated",
            StateMigrationKind::Copied => "copied",
        };
        let legacy_detail = match self.kind {
            StateMigrationKind::Relocated => {
                "The legacy .deepseek copy for this state path was removed by the move."
            }
            StateMigrationKind::Copied => {
                "The legacy .deepseek copy was left in place because a direct move failed."
            }
        };

        format!(
            "CodeWhale migrated legacy state ({action}):\n  {} -> {}\nYour data was preserved. Use .codewhale as the canonical state location from now on.\n{legacy_detail}\nIf no other apps use it, you can remove the legacy .deepseek tree after confirming everything looks right.",
            self.legacy_path.display(),
            self.primary_path.display(),
        )
    }
}

/// Variant of [`ensure_state_dir`] that exposes whether a legacy state path was
/// migrated. Most callers should use [`ensure_state_dir`]; this is kept for
/// tests and future UI surfaces that want to render the notice themselves.
pub fn ensure_state_dir_with_migration(subdir: &str) -> Result<(PathBuf, Option<StateMigration>)> {
    ensure_safe_state_subdir(subdir)?;
    let explicit_codewhale_home = codewhale_home_env_override().is_some();
    let dir = codewhale_home()?.join(subdir);
    let migration = if !explicit_codewhale_home {
        migrate_legacy_state_dir(&dir, subdir)?
    } else {
        None
    };
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}/", dir.display()))?;
    Ok((dir, migration))
}

/// One-time relocation of a legacy `~/.deepseek/<subdir>` state directory into
/// the primary `~/.codewhale/<subdir>` location (#3240). No-op once the primary
/// exists, for the root sentinel `"."` (a whole-tree move is owned by the
/// config-file migration), or when no legacy directory is present.
fn migrate_legacy_state_dir(primary: &Path, subdir: &str) -> Result<Option<StateMigration>> {
    if primary.exists() || subdir == "." || subdir.is_empty() {
        return Ok(None);
    }
    let legacy = match legacy_deepseek_home() {
        Ok(home) => home.join(subdir),
        Err(_) => return Ok(None),
    };
    if !legacy.exists() {
        return Ok(None);
    }
    // The primary's parent (the ~/.codewhale root) must exist for the rename.
    if let Some(parent) = primary.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(
            target: "config::migration",
            "Could not create {} for state migration ({}); writing to primary anyway",
            parent.display(),
            err
        );
    }
    match std::fs::rename(&legacy, primary) {
        Ok(()) => {
            tracing::info!(
                target: "config::migration",
                "Migrated legacy state directory {} -> {} (relocated). The .deepseek copy was removed.",
                legacy.display(),
                primary.display()
            );
            return Ok(Some(StateMigration {
                subdir: subdir.to_string(),
                legacy_path: legacy,
                primary_path: primary.to_path_buf(),
                kind: StateMigrationKind::Relocated,
            }));
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
                    return Ok(Some(StateMigration {
                        subdir: subdir.to_string(),
                        legacy_path: legacy,
                        primary_path: primary.to_path_buf(),
                        kind: StateMigrationKind::Copied,
                    }));
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
    Ok(None)
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
pub fn resolve_project_state_dir(workspace: &Path, subdir: &str) -> Result<(bool, PathBuf)> {
    ensure_safe_state_subdir(subdir)?;
    let workspace = normalize_project_workspace(workspace)?;
    let primary = workspace.join(CODEWHALE_APP_DIR).join(subdir);
    if primary.exists() {
        return Ok((true, primary));
    }
    let legacy = workspace.join(LEGACY_APP_DIR).join(subdir);
    Ok((false, legacy))
}

/// Ensure a project-local state subdirectory exists under `.codewhale/`,
/// creating it if necessary. Returns the directory path.
pub fn ensure_project_state_dir(workspace: &Path, subdir: &str) -> Result<PathBuf> {
    ensure_safe_state_subdir(subdir)?;
    let workspace = normalize_project_workspace(workspace)?;
    let dir = workspace.join(CODEWHALE_APP_DIR).join(subdir);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}/", dir.display()))?;
    Ok(dir)
}

pub fn resolve_config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return normalize_config_file_path(path);
    }
    if let Ok(path) = std::env::var("CODEWHALE_CONFIG_PATH") {
        if let Some(path) = config_path_from_env_value(&path)? {
            return Ok(path);
        }
        return default_config_path();
    }
    if let Ok(path) = std::env::var("DEEPSEEK_CONFIG_PATH") {
        if let Some(path) = config_path_from_env_value(&path)? {
            return Ok(path);
        }
        return default_config_path();
    }
    default_config_path()
}

fn config_path_from_env_value(path: &str) -> Result<Option<PathBuf>> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        normalize_config_file_path(PathBuf::from(trimmed)).map(Some)
    }
}

#[must_use]
pub fn permissions_path_for_config_path(config_path: &Path) -> PathBuf {
    config_sibling_path_unchecked(config_path, OsStr::new(PERMISSIONS_FILE_NAME))
}

fn checked_permissions_path_for_config_path(config_path: &Path) -> Result<PathBuf> {
    checked_config_sibling_path(config_path, OsStr::new(PERMISSIONS_FILE_NAME))
}

pub fn resolve_permissions_path(config_path: Option<PathBuf>) -> Result<PathBuf> {
    checked_permissions_path_for_config_path(&resolve_config_path(config_path)?)
}

/// Read a resolved `permissions.toml` path using the same checked/no-follow
/// path handling as config loading.
pub fn read_permissions_file(path: &Path) -> Result<String> {
    read_checked_permissions_file(path)
}

fn load_sibling_permissions(config_path: &Path) -> Result<PermissionsToml> {
    let permissions_path = checked_permissions_path_for_config_path(config_path)?;
    if !checked_path_exists(&permissions_path)? {
        return Ok(PermissionsToml::default());
    }

    let raw = read_checked_permissions_file(&permissions_path)?;
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
    if codewhale_home_is_explicit() || primary.exists() {
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
    if codewhale_home_is_explicit() {
        return Ok(None);
    }
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

fn toml_value_as_u64(value: &toml::Value) -> Option<u64> {
    match value {
        toml::Value::Integer(value) => u64::try_from(*value).ok(),
        toml::Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
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
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .context("failed to resolve current directory for config path")?
            .join(path)
    };
    let file_name = absolute
        .file_name()
        .map(OsString::from)
        .context("config path must include a file name")?;
    let parent = absolute
        .parent()
        .context("config path must include a parent directory")?;
    let parent = match parent.canonicalize() {
        Ok(parent) => parent,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => parent.to_path_buf(),
        Err(err) => {
            return Err(err).with_context(|| {
                format!("failed to resolve config directory {}", parent.display())
            });
        }
    };
    let normalized = parent.join(file_name);
    reject_path_symlink(&normalized)?;
    Ok(normalized)
}

fn normalize_project_workspace(workspace: &Path) -> Result<PathBuf> {
    if workspace.as_os_str().is_empty() {
        bail!("project workspace path cannot be empty");
    }
    if workspace
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("project workspace path cannot contain '..' components");
    }
    let absolute = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to resolve current directory for project workspace")?
            .join(workspace)
    };
    match absolute.canonicalize() {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(normalize_path_components(&absolute))
        }
        Err(err) => Err(err).with_context(|| {
            format!(
                "failed to resolve project workspace {}",
                workspace.display()
            )
        }),
    }
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn checked_path_exists(path: &Path) -> Result<bool> {
    let path = normalize_config_file_path(path.to_path_buf())?;
    path.try_exists()
        .with_context(|| format!("failed to inspect config path {}", path.display()))
}

fn read_checked_config_file(path: &Path) -> Result<String> {
    read_checked_toml_file(path, "config")
}

fn read_checked_permissions_file(path: &Path) -> Result<String> {
    read_checked_toml_file(path, "permissions")
}

fn read_checked_toml_file(path: &Path, label: &str) -> Result<String> {
    let path = normalize_config_file_path(path.to_path_buf())?;
    read_string_no_follow(&path)
        .with_context(|| format!("failed to read {label} at {}", path.display()))
}

#[cfg(unix)]
fn read_string_no_follow(path: &Path) -> std::io::Result<String> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    Ok(raw)
}

#[cfg(not(unix))]
fn read_string_no_follow(path: &Path) -> std::io::Result<String> {
    fs::read_to_string(path)
}

fn reject_path_symlink(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() {
        bail!("config path must not be a symlink: {}", path.display());
    }
    Ok(())
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
    deepseek_anthropic_base_url: Option<String>,
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
    qianfan_base_url: Option<String>,
    qianfan_model: Option<String>,
    openai_codex_base_url: Option<String>,
    openai_codex_model: Option<String>,
    anthropic_base_url: Option<String>,
    anthropic_model: Option<String>,
    openmodel_base_url: Option<String>,
    openmodel_model: Option<String>,
    zai_base_url: Option<String>,
    zai_model: Option<String>,
    stepfun_base_url: Option<String>,
    stepfun_model: Option<String>,
    minimax_base_url: Option<String>,
    minimax_model: Option<String>,
    deepinfra_base_url: Option<String>,
    deepinfra_model: Option<String>,
    sakana_base_url: Option<String>,
    sakana_model: Option<String>,
    longcat_base_url: Option<String>,
    longcat_model: Option<String>,
    xai_base_url: Option<String>,
    xai_model: Option<String>,
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
            deepseek_anthropic_base_url: std::env::var("DEEPSEEK_ANTHROPIC_BASE_URL")
                .or_else(|_| std::env::var("DEEPSEEK_CLAUDE_BASE_URL"))
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
            qianfan_base_url: std::env::var("QIANFAN_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    std::env::var("BAIDU_QIANFAN_BASE_URL")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                }),
            qianfan_model: std::env::var("QIANFAN_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    std::env::var("BAIDU_QIANFAN_MODEL")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                }),
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
            openmodel_base_url: std::env::var("OPENMODEL_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            openmodel_model: std::env::var("OPENMODEL_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            zai_base_url: std::env::var("ZAI_BASE_URL")
                .or_else(|_| std::env::var("Z_AI_BASE_URL"))
                .or_else(|_| std::env::var("ZHIPU_BASE_URL"))
                .or_else(|_| std::env::var("ZHIPUAI_BASE_URL"))
                .or_else(|_| std::env::var("BIGMODEL_BASE_URL"))
                .ok()
                .filter(|v| !v.trim().is_empty()),
            zai_model: std::env::var("ZAI_MODEL")
                .or_else(|_| std::env::var("Z_AI_MODEL"))
                .or_else(|_| std::env::var("ZHIPU_MODEL"))
                .or_else(|_| std::env::var("ZHIPUAI_MODEL"))
                .or_else(|_| std::env::var("BIGMODEL_MODEL"))
                .or_else(|_| std::env::var("GLM_MODEL"))
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
            sakana_base_url: std::env::var("SAKANA_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            sakana_model: std::env::var("SAKANA_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            longcat_base_url: std::env::var("LONGCAT_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            longcat_model: std::env::var("LONGCAT_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            xai_base_url: std::env::var("XAI_BASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            xai_model: std::env::var("XAI_MODEL")
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
            ProviderKind::DeepseekAnthropic => self.deepseek_anthropic_base_url.clone(),
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
            ProviderKind::Qianfan => self.qianfan_base_url.clone(),
            ProviderKind::OpenaiCodex => self.openai_codex_base_url.clone(),
            ProviderKind::Anthropic => self.anthropic_base_url.clone(),
            ProviderKind::Openmodel => self.openmodel_base_url.clone(),
            ProviderKind::Zai => self.zai_base_url.clone(),
            ProviderKind::Stepfun => self.stepfun_base_url.clone(),
            ProviderKind::Minimax => self.minimax_base_url.clone(),
            ProviderKind::Deepinfra => self.deepinfra_base_url.clone(),
            ProviderKind::Sakana => self.sakana_base_url.clone(),
            ProviderKind::LongCat => self.longcat_base_url.clone(),
            ProviderKind::Xai => self.xai_base_url.clone(),
            // No dedicated CODEWHALE_CUSTOM_BASE_URL env override: a custom
            // provider's base URL comes from its `[providers.<name>]` table.
            ProviderKind::Custom => None,
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
            ProviderKind::Qianfan => self.qianfan_model.clone(),
            ProviderKind::OpenaiCodex => self.openai_codex_model.clone(),
            ProviderKind::Anthropic => self.anthropic_model.clone(),
            ProviderKind::Openmodel => self.openmodel_model.clone(),
            ProviderKind::Zai => self.zai_model.clone(),
            ProviderKind::Stepfun => self.stepfun_model.clone(),
            ProviderKind::Minimax => self.minimax_model.clone(),
            ProviderKind::Deepinfra => self.deepinfra_model.clone(),
            ProviderKind::Sakana => self.sakana_model.clone(),
            ProviderKind::LongCat => self.longcat_model.clone(),
            ProviderKind::Xai => self.xai_model.clone(),
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
mod tests;
