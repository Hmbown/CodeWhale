//! Provider trait and concrete provider implementations.
//!
//! Each provider is a zero-sized struct that implements [`Provider`].
//! The global [`PROVIDER_REGISTRY`] maps canonical provider ids to trait objects.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

/// Which wire protocol the provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireFormat {
    /// OpenAI-compatible `/v1/chat/completions` (Bearer auth).
    ChatCompletions,
    /// Anthropic Messages API `/v1/messages` (`x-api-key` auth).
    AnthropicMessages,
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// A model provider — its identity, defaults, wire format, and per-provider
/// behaviour such as reasoning-effort mapping.
///
/// Implementations are zero-sized structs registered in [`PROVIDER_REGISTRY`].
pub trait Provider: Send + Sync {
    /// Canonical identifier (e.g. `"deepseek"`, `"openrouter"`).
    fn id(&self) -> &'static str;

    /// Human-readable label for UIs / status chips.
    fn display_name(&self) -> &'static str;

    /// Default base URL when none is configured.
    fn default_base_url(&self) -> &'static str;

    /// Environment variable names that supply the API key for this provider.
    fn env_vars(&self) -> &[&'static str];

    /// Default model when the user has not picked one.
    fn default_model(&self) -> &'static str;

    /// Which wire format this provider speaks.
    fn wire(&self) -> WireFormat;

    /// Key used in `[providers.<key>]` TOML sections.
    fn provider_config_key(&self) -> &'static str;

    /// Whether the provider supports thinking / reasoning mode.
    fn thinking_supported(&self) -> bool {
        false
    }

    /// Whether the provider returns prompt-cache telemetry fields.
    fn cache_telemetry_supported(&self) -> bool {
        false
    }

    /// Apply per-provider reasoning-effort fields to the outgoing request body.
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>);
}

// ---------------------------------------------------------------------------
// Concrete providers
// ---------------------------------------------------------------------------

macro_rules! wire {
    (chat_completions) => {
        WireFormat::ChatCompletions
    };
}

// --- DeepSeek ---

pub struct Deepseek;

impl Provider for Deepseek {
    fn id(&self) -> &'static str {
        "deepseek"
    }
    fn display_name(&self) -> &'static str {
        "DeepSeek"
    }
    fn default_base_url(&self) -> &'static str {
        "https://api.deepseek.com/beta"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["DEEPSEEK_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-v4-pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "deepseek"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn cache_telemetry_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        let Some(effort) = effort else { return };
        let normalized = effort.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "off" | "disabled" | "none" | "false" => {
                body["thinking"] = serde_json::json!({"type": "disabled"});
            }
            "low" | "minimal" | "medium" | "mid" | "high" | "" => {
                body["reasoning_effort"] = serde_json::json!("high");
                body["thinking"] = serde_json::json!({"type": "enabled"});
            }
            "xhigh" | "max" | "highest" => {
                body["reasoning_effort"] = serde_json::json!("max");
                body["thinking"] = serde_json::json!({"type": "enabled"});
            }
            _ => {}
        }
    }
}

// --- NVIDIA NIM ---

pub struct NvidiaNim;

impl Provider for NvidiaNim {
    fn id(&self) -> &'static str {
        "nvidia-nim"
    }
    fn display_name(&self) -> &'static str {
        "NVIDIA NIM"
    }
    fn default_base_url(&self) -> &'static str {
        "https://integrate.api.nvidia.com/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["NVIDIA_API_KEY", "NVIDIA_NIM_API_KEY", "DEEPSEEK_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-ai/deepseek-v4-pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "nvidia_nim"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn cache_telemetry_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        let Some(effort) = effort else { return };
        let normalized = effort.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "off" | "disabled" | "none" | "false" => {
                body["chat_template_kwargs"] = serde_json::json!({"thinking": false});
            }
            "low" | "minimal" | "medium" | "mid" | "high" | "" => {
                body["chat_template_kwargs"] = serde_json::json!({
                    "thinking": true,
                    "reasoning_effort": "high",
                });
            }
            "xhigh" | "max" | "highest" => {
                body["chat_template_kwargs"] = serde_json::json!({
                    "thinking": true,
                    "reasoning_effort": "max",
                });
            }
            _ => {}
        }
    }
}

// --- OpenAI-compatible ---

pub struct Openai;

impl Provider for Openai {
    fn id(&self) -> &'static str {
        "openai"
    }
    fn display_name(&self) -> &'static str {
        "OpenAI-compatible"
    }
    fn default_base_url(&self) -> &'static str {
        "https://api.openai.com/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["OPENAI_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-v4-pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "openai"
    }
    fn apply_reasoning_effort(&self, _body: &mut Value, _effort: Option<&str>) {
        // No-op: reasoning effort is not supported.
    }
}

// --- AtlasCloud ---

pub struct Atlascloud;

impl Provider for Atlascloud {
    fn id(&self) -> &'static str {
        "atlascloud"
    }
    fn display_name(&self) -> &'static str {
        "AtlasCloud"
    }
    fn default_base_url(&self) -> &'static str {
        "https://api.atlascloud.ai/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["ATLASCLOUD_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-ai/deepseek-v4-flash"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "atlascloud"
    }
    fn apply_reasoning_effort(&self, _body: &mut Value, _effort: Option<&str>) {}
}

// --- Wanjie Ark ---

pub struct WanjieArk;

impl Provider for WanjieArk {
    fn id(&self) -> &'static str {
        "wanjie-ark"
    }
    fn display_name(&self) -> &'static str {
        "Wanjie Ark"
    }
    fn default_base_url(&self) -> &'static str {
        "https://maas-openapi.wanjiedata.com/api/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &[
            "WANJIE_ARK_API_KEY",
            "WANJIE_API_KEY",
            "WANJIE_MAAS_API_KEY",
        ]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-reasoner"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "wanjie_ark"
    }
    fn apply_reasoning_effort(&self, _body: &mut Value, _effort: Option<&str>) {}
}

// --- OpenRouter ---

pub struct Openrouter;

impl Provider for Openrouter {
    fn id(&self) -> &'static str {
        "openrouter"
    }
    fn display_name(&self) -> &'static str {
        "OpenRouter"
    }
    fn default_base_url(&self) -> &'static str {
        "https://openrouter.ai/api/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["OPENROUTER_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek/deepseek-v4-pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "openrouter"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        let Some(effort) = effort else { return };
        let normalized = effort.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "off" | "disabled" | "none" | "false" => {
                body["thinking"] = serde_json::json!({"type": "disabled"});
            }
            "low" | "minimal" | "medium" | "mid" | "high" | "" => {
                let value = match normalized.as_str() {
                    "low" | "minimal" => "low",
                    "medium" | "mid" => "medium",
                    _ => "high",
                };
                body["reasoning_effort"] = serde_json::json!(value);
                body["thinking"] = serde_json::json!({"type": "enabled"});
            }
            "xhigh" | "max" | "highest" => {
                body["reasoning_effort"] = serde_json::json!("xhigh");
                body["thinking"] = serde_json::json!({"type": "enabled"});
            }
            _ => {}
        }
    }
}

// --- Novita ---

pub struct Novita;

impl Provider for Novita {
    fn id(&self) -> &'static str {
        "novita"
    }
    fn display_name(&self) -> &'static str {
        "Novita AI"
    }
    fn default_base_url(&self) -> &'static str {
        "https://api.novita.ai/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["NOVITA_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek/deepseek-v4-pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "novita"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        // Same behaviour as OpenRouter.
        Openrouter.apply_reasoning_effort(body, effort);
    }
}

// --- Fireworks ---

pub struct Fireworks;

impl Provider for Fireworks {
    fn id(&self) -> &'static str {
        "fireworks"
    }
    fn display_name(&self) -> &'static str {
        "Fireworks AI"
    }
    fn default_base_url(&self) -> &'static str {
        "https://api.fireworks.ai/inference/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["FIREWORKS_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "accounts/fireworks/models/deepseek-v4-pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "fireworks"
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        let Some(effort) = effort else { return };
        let normalized = effort.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "off" | "disabled" | "none" | "false" => {
                // no-op
            }
            "low" | "minimal" | "medium" | "mid" | "high" | "" => {
                body["reasoning_effort"] = serde_json::json!("high");
            }
            "xhigh" | "max" | "highest" => {
                body["reasoning_effort"] = serde_json::json!("max");
            }
            _ => {}
        }
    }
}

// --- Moonshot / Kimi ---

pub struct Moonshot;

impl Provider for Moonshot {
    fn id(&self) -> &'static str {
        "moonshot"
    }
    fn display_name(&self) -> &'static str {
        "Moonshot/Kimi"
    }
    fn default_base_url(&self) -> &'static str {
        "https://api.moonshot.ai/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["MOONSHOT_API_KEY", "KIMI_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "kimi-k2.6"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "moonshot"
    }
    fn apply_reasoning_effort(&self, _body: &mut Value, _effort: Option<&str>) {}
}

// --- SGLang ---

pub struct Sglang;

impl Provider for Sglang {
    fn id(&self) -> &'static str {
        "sglang"
    }
    fn display_name(&self) -> &'static str {
        "SGLang"
    }
    fn default_base_url(&self) -> &'static str {
        "http://localhost:30000/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["SGLANG_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-ai/DeepSeek-V4-Pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "sglang"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        // Same behaviour as Deepseek for reasoning_effort/thinking.
        Deepseek.apply_reasoning_effort(body, effort);
    }
}

// --- vLLM ---

pub struct Vllm;

impl Provider for Vllm {
    fn id(&self) -> &'static str {
        "vllm"
    }
    fn display_name(&self) -> &'static str {
        "vLLM"
    }
    fn default_base_url(&self) -> &'static str {
        "http://localhost:8000/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["VLLM_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-ai/DeepSeek-V4-Pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "vllm"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        let Some(effort) = effort else { return };
        let normalized = effort.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "off" | "disabled" | "none" | "false" => {
                body["chat_template_kwargs"] = serde_json::json!({
                    "enable_thinking": false,
                });
            }
            "low" | "minimal" | "medium" | "mid" | "high" | "" => {
                body["chat_template_kwargs"] = serde_json::json!({
                    "enable_thinking": true,
                });
                let value = match normalized.as_str() {
                    "low" | "minimal" => "low",
                    "medium" | "mid" => "medium",
                    _ => "high",
                };
                body["reasoning_effort"] = serde_json::json!(value);
            }
            "xhigh" | "max" | "highest" => {
                body["chat_template_kwargs"] = serde_json::json!({
                    "enable_thinking": true,
                });
                // vLLM doesn't support "max" — downgrade to "high".
                body["reasoning_effort"] = serde_json::json!("high");
            }
            _ => {}
        }
    }
}

// --- Ollama ---

pub struct Ollama;

impl Provider for Ollama {
    fn id(&self) -> &'static str {
        "ollama"
    }
    fn display_name(&self) -> &'static str {
        "Ollama"
    }
    fn default_base_url(&self) -> &'static str {
        "http://localhost:11434/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["OLLAMA_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-coder:1.3b"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "ollama"
    }
    fn apply_reasoning_effort(&self, _body: &mut Value, _effort: Option<&str>) {}
}

// --- Volcengine ---

pub struct Volcengine;

impl Provider for Volcengine {
    fn id(&self) -> &'static str {
        "volcengine"
    }
    fn display_name(&self) -> &'static str {
        "Volcengine Ark"
    }
    fn default_base_url(&self) -> &'static str {
        "https://ark.cn-beijing.volces.com/api/coding/v3"
    }
    fn env_vars(&self) -> &[&'static str] {
        &[
            "VOLCENGINE_API_KEY",
            "VOLCENGINE_ARK_API_KEY",
            "ARK_API_KEY",
        ]
    }
    fn default_model(&self) -> &'static str {
        "DeepSeek-V4-Pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "volcengine"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        Deepseek.apply_reasoning_effort(body, effort);
    }
}

// --- Xiaomi Mimo ---

pub struct XiaomiMimo;

impl Provider for XiaomiMimo {
    fn id(&self) -> &'static str {
        "xiaomi-mimo"
    }
    fn display_name(&self) -> &'static str {
        "Xiaomi Mimo"
    }
    fn default_base_url(&self) -> &'static str {
        "https://token-plan-sgp.xiaomimimo.com/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["XIAOMI_MIMO_API_KEY", "XIAOMI_API_KEY", "MIMO_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "mimo-v2.5-pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "xiaomi_mimo"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        let Some(effort) = effort else { return };
        let normalized = effort.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "off" | "disabled" | "none" | "false" => {
                body["thinking"] = serde_json::json!({"type": "disabled"});
            }
            "low" | "minimal" | "medium" | "mid" | "high" | "" | "xhigh" | "max" | "highest" => {
                body["thinking"] = serde_json::json!({"type": "enabled"});
            }
            _ => {}
        }
    }
}

// --- SiliconFlow ---

pub struct Siliconflow;

impl Provider for Siliconflow {
    fn id(&self) -> &'static str {
        "siliconflow"
    }
    fn display_name(&self) -> &'static str {
        "SiliconFlow"
    }
    fn default_base_url(&self) -> &'static str {
        "https://api.siliconflow.com/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["SILICONFLOW_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-ai/DeepSeek-V4-Pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "siliconflow"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        Deepseek.apply_reasoning_effort(body, effort);
    }
}

// --- Arcee ---

pub struct Arcee;

impl Provider for Arcee {
    fn id(&self) -> &'static str {
        "arcee"
    }
    fn display_name(&self) -> &'static str {
        "Arcee AI"
    }
    fn default_base_url(&self) -> &'static str {
        "https://api.arcee.ai/api/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["ARCEE_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "trinity-large-thinking"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "arcee"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        let Some(effort) = effort else { return };
        let normalized = effort.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "off" | "disabled" | "none" | "false" => {
                // no-op
            }
            "low" | "minimal" | "medium" | "mid" | "high" | "" => {
                let value = match normalized.as_str() {
                    "minimal" => "minimal",
                    "low" => "low",
                    "medium" | "mid" => "medium",
                    _ => "high",
                };
                body["reasoning_effort"] = serde_json::json!(value);
            }
            "xhigh" | "max" | "highest" => {
                body["reasoning_effort"] = serde_json::json!("high");
            }
            _ => {}
        }
    }
}

// --- SiliconFlow CN ---

pub struct SiliconflowCn;

impl Provider for SiliconflowCn {
    fn id(&self) -> &'static str {
        "siliconflow-cn"
    }
    fn display_name(&self) -> &'static str {
        "SiliconFlow (China)"
    }
    fn default_base_url(&self) -> &'static str {
        "https://api.siliconflow.cn/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["SILICONFLOW_API_KEY"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-ai/DeepSeek-V4-Pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "siliconflow_cn"
    }
    fn thinking_supported(&self) -> bool {
        true
    }
    fn apply_reasoning_effort(&self, body: &mut Value, effort: Option<&str>) {
        Deepseek.apply_reasoning_effort(body, effort);
    }
}

// --- Hugging Face ---

pub struct Huggingface;

impl Provider for Huggingface {
    fn id(&self) -> &'static str {
        "huggingface"
    }
    fn display_name(&self) -> &'static str {
        "Hugging Face"
    }
    fn default_base_url(&self) -> &'static str {
        "https://router.huggingface.co/v1"
    }
    fn env_vars(&self) -> &[&'static str] {
        &["HUGGINGFACE_API_KEY", "HF_TOKEN"]
    }
    fn default_model(&self) -> &'static str {
        "deepseek-ai/DeepSeek-V4-Pro"
    }
    fn wire(&self) -> WireFormat {
        wire!(chat_completions)
    }
    fn provider_config_key(&self) -> &'static str {
        "huggingface"
    }
    fn apply_reasoning_effort(&self, _body: &mut Value, _effort: Option<&str>) {}
}

// ---------------------------------------------------------------------------
// Global registry
// ---------------------------------------------------------------------------

static PROVIDER_REGISTRY: OnceLock<HashMap<&'static str, &'static dyn Provider>> = OnceLock::new();

fn registry() -> &'static HashMap<&'static str, &'static dyn Provider> {
    PROVIDER_REGISTRY.get_or_init(|| {
        let providers: [(&str, &dyn Provider); 18] = [
            ("deepseek", &Deepseek),
            ("nvidia-nim", &NvidiaNim),
            ("openai", &Openai),
            ("atlascloud", &Atlascloud),
            ("wanjie-ark", &WanjieArk),
            ("volcengine", &Volcengine),
            ("openrouter", &Openrouter),
            ("xiaomi-mimo", &XiaomiMimo),
            ("novita", &Novita),
            ("fireworks", &Fireworks),
            ("siliconflow", &Siliconflow),
            ("arcee", &Arcee),
            ("siliconflow-cn", &SiliconflowCn),
            ("moonshot", &Moonshot),
            ("sglang", &Sglang),
            ("vllm", &Vllm),
            ("ollama", &Ollama),
            ("huggingface", &Huggingface),
        ];
        let mut map = HashMap::with_capacity(providers.len());
        for (key, provider) in providers {
            map.insert(key, provider);
        }
        map
    })
}

/// Look up a provider by its canonical id.
#[must_use]
pub fn lookup_provider(id: &str) -> Option<&'static dyn Provider> {
    registry().get(id).copied()
}

/// Look up a provider by id string (with canonicalization for legacy aliases).
/// Returns `None` when the id is not recognised.
#[must_use]
pub fn resolve_provider(id: &str) -> Option<&'static dyn Provider> {
    let normalized = match id.trim().to_ascii_lowercase().as_str() {
        "deepseek" | "deep-seek" => "deepseek",
        "deepseek-cn" | "deepseek_china" | "deepseekcn" | "deepseek-china" => "deepseek",
        "nvidia" | "nvidia-nim" | "nvidia_nim" | "nim" => "nvidia-nim",
        "openai" | "open-ai" => "openai",
        "atlascloud" | "atlas-cloud" | "atlas_cloud" | "atlas" => "atlascloud",
        "wanjie" | "wanjie-ark" | "wanjie_ark" | "ark-wanjie" | "ark_wanjie" | "wanjieark"
        | "wanjie-maas" | "wanjie_maas" | "wanjiemaas" => "wanjie-ark",
        "volcengine" | "volcengine-ark" | "volcengine_ark" | "ark" | "volc-ark"
        | "volcengineark" => "volcengine",
        "openrouter" | "open_router" => "openrouter",
        "xiaomi-mimo" | "xiaomi_mimo" | "xiaomimimo" | "mimo" | "xiaomi" => "xiaomi-mimo",
        "novita" => "novita",
        "fireworks" | "fireworks-ai" => "fireworks",
        "siliconflow" | "silicon-flow" | "silicon_flow" => "siliconflow",
        "siliconflow-cn" | "siliconflow-CN" | "siliconflow_cn" => "siliconflow-cn",
        "arcee" | "arcee-ai" | "arcee_ai" => "arcee",
        "moonshot" | "moonshot-ai" | "kimi" | "kimi-k2" => "moonshot",
        "sglang" | "sg-lang" => "sglang",
        "vllm" | "v-llm" => "vllm",
        "ollama" | "ollama-local" => "ollama",
        "huggingface" | "hugging-face" | "hugging_face" | "hf" => "huggingface",
        _ => return None,
    };
    lookup_provider(normalized)
}

/// Default provider (DeepSeek).
#[must_use]
pub fn default_provider() -> &'static dyn Provider {
    &Deepseek
}

/// All providers, in the canonical order shown in picker UIs.
/// Does not include `deepseek-cn` — that's a legacy alias that resolves to
/// the same `Deepseek` trait object.
#[must_use]
pub fn all_providers() -> &'static [&'static dyn Provider] {
    static ALL: OnceLock<Vec<&'static dyn Provider>> = OnceLock::new();
    ALL.get_or_init(|| {
        vec![
            &Deepseek,
            &NvidiaNim,
            &Openai,
            &Atlascloud,
            &WanjieArk,
            &Volcengine,
            &Openrouter,
            &XiaomiMimo,
            &Novita,
            &Fireworks,
            &Siliconflow,
            &Arcee,
            &SiliconflowCn,
            &Moonshot,
            &Sglang,
            &Vllm,
            &Ollama,
            &Huggingface,
        ]
    })
}
