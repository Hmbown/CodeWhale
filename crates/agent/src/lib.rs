use std::error::Error;
use std::fmt;

use codewhale_config::{ProviderKind, opencode_go_chat_model_id};
use serde::{Deserialize, Serialize};

/// High-level model family used for shared identity affordances across clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelFamily {
    DeepSeek,
    Anthropic,
    OpenAI,
    Google,
    Meta,
    Mistral,
    Qwen,
    Grok,
    Cohere,
    GptOss,
    Inferencer,
}

/// Metadata for a single model entry in the registry.
///
/// Each model has a canonical `id` used by the provider, a list of `aliases`
/// that users may reference, and capability flags indicating whether the model
/// supports tool use and reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// The canonical model identifier used by the provider (e.g. `"deepseek-v4-pro"`).
    pub id: String,
    /// The provider that serves this model.
    pub provider: ProviderKind,
    /// Alternative names that users can use to reference this model (case-insensitive).
    pub aliases: Vec<String>,
    /// Whether this model supports tool/function calling.
    pub supports_tools: bool,
    /// Whether this model supports extended reasoning.
    pub supports_reasoning: bool,
}

/// The result of resolving a user-requested model name to a concrete model entry.
///
/// Contains the resolved [`ModelInfo`], whether a fallback was used, and the
/// chain of resolution strategies that were attempted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResolution {
    /// The original model name requested by the user, if any.
    pub requested: Option<String>,
    /// The concrete model that was resolved.
    pub resolved: ModelInfo,
    /// Whether the provider-owned default was used because no model was requested.
    pub used_fallback: bool,
    /// The ordered list of resolution strategies that were attempted.
    pub fallback_chain: Vec<String>,
}

/// A model lookup that cannot name a provider-owned result truthfully.
///
/// The registry is metadata, not route authority. In particular, a missing
/// provider must never be interpreted as permission to select DeepSeek (or
/// any other provider), and an explicit provider with no registered models
/// must never fall through to another provider's first catalog row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelResolutionError {
    /// No provider was supplied. A model name alone is never route authority,
    /// even when it happens to match one catalog entry.
    ProviderRequired { requested: Option<String> },
    /// The caller selected a provider for which this registry has no model
    /// metadata to return.
    ProviderHasNoModels {
        provider: ProviderKind,
        requested: Option<String>,
    },
    /// The caller selected a provider, then requested a model that provider's
    /// registry rows and explicit pass-through contract do not serve.
    ModelNotAvailableForProvider {
        provider: ProviderKind,
        requested: String,
    },
    /// The provider declares a default model, but the registry cannot return a
    /// matching provider-owned row for it.
    ProviderDefaultUnavailable {
        provider: ProviderKind,
        default_model: String,
    },
}

impl fmt::Display for ModelResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderRequired {
                requested: Some(requested),
            } => write!(
                formatter,
                "model '{requested}' does not identify an unambiguous provider; select a provider explicitly"
            ),
            Self::ProviderRequired { requested: None } => {
                formatter.write_str("model resolution requires an explicit provider")
            }
            Self::ProviderHasNoModels {
                provider,
                requested: Some(requested),
            } => write!(
                formatter,
                "provider '{}' has no registered model for '{requested}'",
                provider.as_str()
            ),
            Self::ProviderHasNoModels {
                provider,
                requested: None,
            } => write!(
                formatter,
                "provider '{}' has no registered default model",
                provider.as_str()
            ),
            Self::ModelNotAvailableForProvider {
                provider,
                requested,
            } => write!(
                formatter,
                "model '{requested}' is not available from provider '{}'",
                provider.as_str()
            ),
            Self::ProviderDefaultUnavailable {
                provider,
                default_model,
            } => write!(
                formatter,
                "provider '{}' declares default model '{default_model}', but that model is not registered for the provider",
                provider.as_str()
            ),
        }
    }
}

impl Error for ModelResolutionError {}

/// A registry of supported models and their aliases, used to resolve user-facing
/// model names to concrete provider-specific model entries.
///
/// The default registry is populated with all built-in models across supported
/// providers (DeepSeek, NVIDIA NIM, OpenAI-compatible, and others).
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    models: Vec<ModelInfo>,
}

/// Creates a registry pre-populated with all built-in models and their aliases.
impl Default for ModelRegistry {
    fn default() -> Self {
        let models = vec![
            ModelInfo {
                id: "deepseek-v4-pro".to_string(),
                provider: ProviderKind::Deepseek,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                provider: ProviderKind::Deepseek,
                aliases: vec![
                    "deepseek-chat".to_string(),
                    "deepseek-reasoner".to_string(),
                    "deepseek-r1".to_string(),
                    "deepseek-v3".to_string(),
                    "deepseek-v3.2".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-v4-flash-vision-exp".to_string(),
                provider: ProviderKind::Deepseek,
                aliases: vec![
                    "flash-vision".to_string(),
                    "deepseek-v4flashvisionexp".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/deepseek-v4-pro".to_string(),
                provider: ProviderKind::NvidiaNim,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "nvidia-deepseek-v4-pro".to_string(),
                    "nim-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/deepseek-v4-flash".to_string(),
                provider: ProviderKind::NvidiaNim,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "deepseek-chat".to_string(),
                    "deepseek-reasoner".to_string(),
                    "nvidia-deepseek-v4-flash".to_string(),
                    "nim-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-v4-pro".to_string(),
                provider: ProviderKind::Openai,
                aliases: vec!["openai-compatible-deepseek-v4-pro".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                provider: ProviderKind::Openai,
                aliases: vec!["openai-compatible-deepseek-v4-flash".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // OpenAI public API models carried by the bundled catalog.
            ModelInfo {
                id: "gpt-5.3-codex".to_string(),
                provider: ProviderKind::Openai,
                aliases: vec!["gpt53-codex".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gpt-5.5".to_string(),
                provider: ProviderKind::Openai,
                aliases: vec!["openai-gpt-5.5".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gpt-5.5-pro".to_string(),
                provider: ProviderKind::Openai,
                aliases: vec!["openai-gpt-5.5-pro".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // OpenAI public API GPT-5.6 family.
            ModelInfo {
                id: "gpt-5.6".to_string(),
                provider: ProviderKind::Openai,
                aliases: vec!["gpt56".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gpt-5.6-sol".to_string(),
                provider: ProviderKind::Openai,
                aliases: vec!["gpt56-sol".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gpt-5.6-terra".to_string(),
                provider: ProviderKind::Openai,
                aliases: vec!["gpt56-terra".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gpt-5.6-luna".to_string(),
                provider: ProviderKind::Openai,
                aliases: vec!["gpt56-luna".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/deepseek-v4-flash".to_string(),
                provider: ProviderKind::Atlascloud,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "atlascloud-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/deepseek-v4-pro".to_string(),
                provider: ProviderKind::Atlascloud,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "atlascloud-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-reasoner".to_string(),
                provider: ProviderKind::WanjieArk,
                aliases: vec![
                    "wanjie-deepseek-reasoner".to_string(),
                    "ark-wanjie-deepseek-reasoner".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "DeepSeek-V4-Pro".to_string(),
                provider: ProviderKind::Volcengine,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "volcengine-deepseek-v4-pro".to_string(),
                    "ark-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "DeepSeek-V4-Flash".to_string(),
                provider: ProviderKind::Volcengine,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "deepseek-chat".to_string(),
                    "volcengine-deepseek-v4-flash".to_string(),
                    "ark-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "trinity-large-thinking".to_string(),
                provider: ProviderKind::Arcee,
                aliases: vec![
                    "trinity".to_string(),
                    "arcee-trinity".to_string(),
                    "arcee-trinity-large-thinking".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "trinity-mini".to_string(),
                provider: ProviderKind::Arcee,
                aliases: vec!["arcee-trinity-mini".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek/deepseek-v4-pro".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "openrouter-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek/deepseek-v4-flash".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "deepseek-chat".to_string(),
                    "deepseek-reasoner".to_string(),
                    "openrouter-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek/deepseek-v4-pro".to_string(),
                provider: ProviderKind::Orcarouter,
                aliases: vec!["orcarouter-deepseek-v4-pro".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek/deepseek-v4-flash".to_string(),
                provider: ProviderKind::Orcarouter,
                aliases: vec!["orcarouter-deepseek-v4-flash".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "orcarouter/auto".to_string(),
                provider: ProviderKind::Orcarouter,
                aliases: vec!["orcarouter-auto".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "arcee-ai/trinity-large-thinking".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "trinity".to_string(),
                    "trinity-large-thinking".to_string(),
                    "arcee-trinity-large-thinking".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "xiaomi/mimo-v2.5-pro".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "openrouter-mimo-v2.5-pro".to_string(),
                    "openrouter-xiaomi-mimo-v2.5-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "xiaomi/mimo-v2.5".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "openrouter-mimo-v2.5".to_string(),
                    "openrouter-xiaomi-mimo-v2.5".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "qwen/qwen3.6-flash".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["qwen3.6-flash".to_string(), "qwen-3.6-flash".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "qwen/qwen3.6-35b-a3b".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "qwen3.6-35b-a3b".to_string(),
                    "qwen-3.6-35b-a3b".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "qwen/qwen3.6-max-preview".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "qwen3.6-max-preview".to_string(),
                    "qwen-3.6-max-preview".to_string(),
                    "qwen-max-preview".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "qwen/qwen3.6-27b".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["qwen3.6-27b".to_string(), "qwen-3.6-27b".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "qwen/qwen3.6-plus".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["qwen3.6-plus".to_string(), "qwen-3.6-plus".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "qwen/qwen3.7-plus".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["qwen3.7-plus".to_string(), "qwen-3.7-plus".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "moonshotai/kimi-k2.7-code".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "kimi-k2.7-code".to_string(),
                    "openrouter-kimi-k2.7-code".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "moonshotai/kimi-k2.6".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["openrouter-kimi-k2.6".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "minimax/minimax-m3".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "minimax-m3".to_string(),
                    "minimax-m-3".to_string(),
                    "openrouter-minimax-m3".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "z-ai/glm-5.1".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["glm-5.1".to_string(), "zai-glm-5.1".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "z-ai/glm-5.2".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["glm-5.2".to_string(), "zai-glm-5.2".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // GLM-5.3 is live; capabilities still inherit from glm-5.2 until
            // Z.ai publishes distinct 5.3 numbers. See
            // crates/config/assets/models_dev.bundled.json
            // `_meta.pending_release_metadata`.
            ModelInfo {
                id: "z-ai/glm-5.3".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["glm-5.3".to_string(), "zai-glm-5.3".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "z-ai/glm-5.3-flash".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["glm-5.3-flash".to_string(), "zai-glm-5.3-flash".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "z-ai/glm-5-turbo".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["glm-5-turbo".to_string(), "zai-glm-5-turbo".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "GLM-5.3".to_string(),
                provider: ProviderKind::Zai,
                aliases: vec![
                    "glm-5.3".to_string(),
                    "glm-5-3".to_string(),
                    "zai-glm-5.3".to_string(),
                    "zai-glm-5-3".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "GLM-5.3-Flash".to_string(),
                provider: ProviderKind::Zai,
                aliases: vec![
                    "glm-5.3-flash".to_string(),
                    "glm-5-3-flash".to_string(),
                    "zai-glm-5.3-flash".to_string(),
                    "zai-glm-5-3-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            // The first Z.ai row is the provider default. Keep this ordering
            // aligned with `DEFAULT_ZAI_MODEL` in codewhale-config.
            ModelInfo {
                id: "GLM-5.2".to_string(),
                provider: ProviderKind::Zai,
                aliases: vec![
                    "glm-5.2".to_string(),
                    "glm-5-2".to_string(),
                    "zai-glm-5.2".to_string(),
                    "zai-glm-5-2".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "GLM-5.1".to_string(),
                provider: ProviderKind::Zai,
                aliases: vec![
                    "glm-5.1".to_string(),
                    "glm-5-1".to_string(),
                    "zai-glm-5.1".to_string(),
                    "zai-glm-5-1".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "GLM-5-Turbo".to_string(),
                provider: ProviderKind::Zai,
                aliases: vec![
                    "glm-5-turbo".to_string(),
                    "glm-5turbo".to_string(),
                    "zai-glm-5-turbo".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "tencent/hy3-preview".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "hy3-preview".to_string(),
                    "tencent-hy3-preview".to_string(),
                    "hy3".to_string(),
                    "hunyuan".to_string(),
                    "tencent-hunyuan".to_string(),
                    "hunyuan-hy3".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "google/gemma-4-31b-it".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["gemma-4-31b".to_string(), "gemma-4-31b-it".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "google/gemma-4-26b-a4b-it".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "gemma-4-26b-a4b".to_string(),
                    "gemma-4-26b-a4b-it".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "nemotron-3-nano-omni".to_string(),
                    "nemotron-3-nano-omni-reasoning".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "mimo-v2.5-pro".to_string(),
                provider: ProviderKind::XiaomiMimo,
                aliases: vec![
                    "mimo".to_string(),
                    "pro".to_string(),
                    "xiaomi-mimo-v2.5-pro".to_string(),
                    "xiaomi-mimo-v2-5-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "mimo-v2.5".to_string(),
                provider: ProviderKind::XiaomiMimo,
                aliases: vec![
                    "omni".to_string(),
                    "mimo-omni".to_string(),
                    "v2.5-omni".to_string(),
                    "mimo-v2.5-omni".to_string(),
                    "xiaomi-mimo-v2.5".to_string(),
                    "xiaomi-mimo-v2.5-omni".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "mimo-v2.5-asr".to_string(),
                provider: ProviderKind::XiaomiMimo,
                aliases: vec![
                    "asr".to_string(),
                    "speech-to-text".to_string(),
                    "transcribe".to_string(),
                ],
                supports_tools: false,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "mimo-v2.5-tts".to_string(),
                provider: ProviderKind::XiaomiMimo,
                aliases: vec![
                    "tts".to_string(),
                    "speech".to_string(),
                    "mimo-tts".to_string(),
                ],
                supports_tools: false,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "mimo-v2.5-tts-voicedesign".to_string(),
                provider: ProviderKind::XiaomiMimo,
                aliases: vec![
                    "voicedesign".to_string(),
                    "voice-design".to_string(),
                    "mimo-voice-design".to_string(),
                ],
                supports_tools: false,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "mimo-v2.5-tts-voiceclone".to_string(),
                provider: ProviderKind::XiaomiMimo,
                aliases: vec![
                    "voiceclone".to_string(),
                    "voice-clone".to_string(),
                    "mimo-voice-clone".to_string(),
                ],
                supports_tools: false,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "mimo-v2-tts".to_string(),
                provider: ProviderKind::XiaomiMimo,
                aliases: vec!["mimo-v2-speech".to_string()],
                supports_tools: false,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "deepseek/deepseek-v4-pro".to_string(),
                provider: ProviderKind::Novita,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "novita-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek/deepseek-v4-flash".to_string(),
                provider: ProviderKind::Novita,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "deepseek-chat".to_string(),
                    "deepseek-reasoner".to_string(),
                    "novita-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "accounts/fireworks/models/deepseek-v4-pro".to_string(),
                provider: ProviderKind::Fireworks,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "fireworks-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Pro".to_string(),
                provider: ProviderKind::Siliconflow,
                // `deepseek-reasoner` and `deepseek-r1` deliberately do NOT
                // appear here. Every other provider maps both to V4-Flash, so
                // listing them on a Pro row made one alias mean two tiers —
                // and Pro costs ~3x Flash per input token. An alias must name
                // one model everywhere or it is a silent substitution.
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "siliconflow-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Flash".to_string(),
                provider: ProviderKind::Siliconflow,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "deepseek-chat".to_string(),
                    "deepseek-v3".to_string(),
                    "siliconflow-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "trinity-large-preview".to_string(),
                provider: ProviderKind::Arcee,
                aliases: vec!["arcee-trinity-large-preview".to_string()],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "kimi-k2.7-code".to_string(),
                provider: ProviderKind::Moonshot,
                aliases: vec![
                    "kimi".to_string(),
                    "kimi-k2".to_string(),
                    "kimi-k2.7".to_string(),
                    "kimi-code".to_string(),
                    "moonshot-kimi-k2.7-code".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "kimi-k2.6".to_string(),
                provider: ProviderKind::Moonshot,
                aliases: vec!["kimi-k2.6".to_string(), "moonshot-kimi-k2.6".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // Moonshot ships K3 as two distinct products under one provider
            // id, separated by endpoint (v0.9.1 kimi-k3 dogfood report):
            //   * `kimi-k3` on the direct platform API (api.moonshot.ai/v1)
            //   * `k3` on the Kimi Code coding-plan API (api.kimi.com/coding/v1)
            // Both must be resolvable here or `--model kimi-k3` silently
            // reports the provider default instead. The endpoint pairing is
            // enforced separately by `validate_kimi_code_api_model_id`; keep
            // the two ids in separate entries so neither one's alias set can
            // launder a request onto the other product's route.
            ModelInfo {
                id: "kimi-k3".to_string(),
                provider: ProviderKind::Moonshot,
                aliases: vec!["moonshot-kimi-k3".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "k3".to_string(),
                provider: ProviderKind::Moonshot,
                aliases: vec!["kimi-code-k3".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Pro".to_string(),
                provider: ProviderKind::Sglang,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "sglang-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Flash".to_string(),
                provider: ProviderKind::Sglang,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "deepseek-chat".to_string(),
                    "deepseek-reasoner".to_string(),
                    "sglang-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Pro".to_string(),
                provider: ProviderKind::Vllm,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "vllm-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Flash".to_string(),
                provider: ProviderKind::Vllm,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "deepseek-chat".to_string(),
                    "deepseek-reasoner".to_string(),
                    "vllm-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                provider: ProviderKind::Ollama,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gpt-oss:120b".to_string(),
                provider: ProviderKind::OllamaCloud,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Pro".to_string(),
                provider: ProviderKind::Huggingface,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "hf-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Flash".to_string(),
                provider: ProviderKind::Huggingface,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "deepseek-chat".to_string(),
                    "deepseek-reasoner".to_string(),
                    "hf-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            // Together AI provider models
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Pro".to_string(),
                provider: ProviderKind::Together,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "together-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Flash".to_string(),
                provider: ProviderKind::Together,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "deepseek-chat".to_string(),
                    "together-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                // Together's published hosted endpoint is lowercase even though
                // the open-weight Hugging Face repository uses `Inkling`.
                id: "thinkingmachines/inkling".to_string(),
                provider: ProviderKind::Together,
                aliases: vec!["inkling".to_string(), "together-inkling".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // Qwen 3.7 Max (OpenRouter)
            ModelInfo {
                id: "qwen/qwen3.7-max".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["qwen3.7-max".to_string(), "qwen-3.7-max".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // OpenAI Codex (ChatGPT OAuth) models
            ModelInfo {
                id: "gpt-5.5".to_string(),
                provider: ProviderKind::OpenaiCodex,
                aliases: vec!["codex-gpt-5.5".to_string(), "chatgpt-gpt-5.5".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // Anthropic native Messages API models (#3014)
            ModelInfo {
                id: "claude-opus-4-8".to_string(),
                provider: ProviderKind::Anthropic,
                aliases: vec!["opus".to_string(), "claude-opus".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // Claude Opus 5 (GA 2026-07-24; API id/alias `claude-opus-5`, 1M
            // context / 128K output per
            // https://platform.claude.com/docs/en/about-claude/models/overview).
            ModelInfo {
                id: "claude-opus-5".to_string(),
                provider: ProviderKind::Anthropic,
                aliases: vec!["opus-5".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "claude-sonnet-4-6".to_string(),
                provider: ProviderKind::Anthropic,
                aliases: vec!["sonnet".to_string(), "claude-sonnet".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "claude-haiku-4-5".to_string(),
                provider: ProviderKind::Anthropic,
                aliases: vec!["haiku".to_string(), "claude-haiku".to_string()],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "claude-sonnet-5".to_string(),
                provider: ProviderKind::Anthropic,
                aliases: vec!["sonnet-5".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "claude-fable-5".to_string(),
                provider: ProviderKind::Anthropic,
                aliases: vec!["fable".to_string(), "fable-5".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // OpenModel Anthropic-compatible Messages route
            ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                provider: ProviderKind::Openmodel,
                aliases: vec!["openmodel".to_string(), "openmodel-deepseek".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // MiniMax 2.7 (OpenRouter)
            ModelInfo {
                id: "minimax/minimax-m2.7".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "minimax-2.7".to_string(),
                    "minimax-2-7".to_string(),
                    "openrouter-minimax-2.7".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "step-3.7-flash".to_string(),
                provider: ProviderKind::Stepfun,
                aliases: vec!["stepfun".to_string(), "stepflash".to_string()],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "MiniMax-M3".to_string(),
                provider: ProviderKind::Minimax,
                aliases: vec![
                    "minimax".to_string(),
                    "minimax-m3".to_string(),
                    "minimax-m-3".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "MiniMax-M2.7".to_string(),
                provider: ProviderKind::Minimax,
                aliases: vec![
                    "minimax-m2.7".to_string(),
                    "minimax-m2-7".to_string(),
                    "minimax-m-2.7".to_string(),
                    "minimax-m-2-7".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "MiniMax-M3".to_string(),
                provider: ProviderKind::MinimaxAnthropic,
                aliases: vec![
                    "minimax-anthropic".to_string(),
                    "minimax-anthropic-m3".to_string(),
                    "minimax-m3".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "MiniMax-M2.7".to_string(),
                provider: ProviderKind::MinimaxAnthropic,
                aliases: vec![
                    "minimax-anthropic-m2.7".to_string(),
                    "minimax-anthropic-m2-7".to_string(),
                    "minimax-m2.7".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "MiniMax-M2.7-highspeed".to_string(),
                provider: ProviderKind::Minimax,
                aliases: vec![
                    "minimax-m2.7-highspeed".to_string(),
                    "minimax-m2-7-highspeed".to_string(),
                    "minimax-m-2.7-highspeed".to_string(),
                    "minimax-m-2-7-highspeed".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "MiniMax-M2.5".to_string(),
                provider: ProviderKind::Minimax,
                aliases: vec![
                    "minimax-m2.5".to_string(),
                    "minimax-m2-5".to_string(),
                    "minimax-m-2.5".to_string(),
                    "minimax-m-2-5".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "MiniMax-M2.5-highspeed".to_string(),
                provider: ProviderKind::Minimax,
                aliases: vec![
                    "minimax-m2.5-highspeed".to_string(),
                    "minimax-m2-5-highspeed".to_string(),
                    "minimax-m-2.5-highspeed".to_string(),
                    "minimax-m-2-5-highspeed".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "MiniMax-M2.1".to_string(),
                provider: ProviderKind::Minimax,
                aliases: vec![
                    "minimax-m2.1".to_string(),
                    "minimax-m2-1".to_string(),
                    "minimax-m-2.1".to_string(),
                    "minimax-m-2-1".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "MiniMax-M2.1-highspeed".to_string(),
                provider: ProviderKind::Minimax,
                aliases: vec![
                    "minimax-m2.1-highspeed".to_string(),
                    "minimax-m2-1-highspeed".to_string(),
                    "minimax-m-2.1-highspeed".to_string(),
                    "minimax-m-2-1-highspeed".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "MiniMax-M2".to_string(),
                provider: ProviderKind::Minimax,
                aliases: vec!["minimax-m2".to_string(), "minimax-m-2".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // NVIDIA Nemotron 3 Ultra (OpenRouter)
            ModelInfo {
                id: "nvidia/nemotron-3-ultra-550b-a55b".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec![
                    "nvidia/nemotron-3-ultra".to_string(),
                    "nemotron-3-ultra".to_string(),
                    "nemotron-3-ultra-550b-a55b".to_string(),
                    "nvidia-nemotron-3-ultra".to_string(),
                    "nvidia-nemotron-3-ultra-550b-a55b".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            // DeepInfra (https://deepinfra.com)
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Pro".to_string(),
                provider: ProviderKind::Deepinfra,
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "di-deepseek-v4-pro".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-V4-Flash".to_string(),
                provider: ProviderKind::Deepinfra,
                aliases: vec![
                    "deepseek-v4-flash".to_string(),
                    "di-deepseek-v4-flash".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            // Sakana AI Fugu (https://api.sakana.ai)
            ModelInfo {
                id: "fugu".to_string(),
                provider: ProviderKind::Sakana,
                aliases: vec!["sakana-fugu".to_string(), "sakana/fugu".to_string()],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "fugu-ultra-20260615".to_string(),
                provider: ProviderKind::Sakana,
                aliases: vec!["fugu-ultra".to_string(), "sakana-fugu-ultra".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // Meituan LongCat (https://longcat.chat/platform)
            ModelInfo {
                id: "LongCat-2.0".to_string(),
                provider: ProviderKind::LongCat,
                aliases: vec!["longcat".to_string(), "longcat-2.0".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // OpenCode Go Chat Completions models (https://opencode.ai/docs/go/).
            // Go models documented only on `/messages` are intentionally not
            // advertised by this OpenAI-compatible provider slice.
            ModelInfo {
                id: "deepseek-v4-pro".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/deepseek-v4-pro".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "grok-4.5".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/grok-4.5".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // No glm-5.3 row (2026-08-03): OpenCode Go publishes no glm-5.3
            // model. The Z.ai/OpenRouter glm-5.3 rows inherit glm-5.2 metadata;
            // that inheritance is not evidence this gateway serves it.
            ModelInfo {
                id: "glm-5.2".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/glm-5.2".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "glm-5.1".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/glm-5.1".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "kimi-k3".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/kimi-k3".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "kimi-k2.7-code".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/kimi-k2.7-code".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "kimi-k2.6".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/kimi-k2.6".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/deepseek-v4-flash".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "mimo-v2.5".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/mimo-v2.5".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "mimo-v2.5-pro".to_string(),
                provider: ProviderKind::OpencodeGo,
                aliases: vec!["opencode-go/mimo-v2.5-pro".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // Meta Model API / Muse Spark. Keep these in step with
            // `DEFAULT_META_MODEL` in config's provider_defaults and with the
            // bundled models.dev catalog: this registry resolves the `muse`
            // aliases for the CLI and app-server, so a stale id here silently
            // routes them somewhere the configured default never points.
            ModelInfo {
                id: "muse-spark-1.2".to_string(),
                provider: ProviderKind::Meta,
                aliases: vec!["muse-spark".to_string(), "muse".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "muse-spark-1.2-contributor".to_string(),
                provider: ProviderKind::Meta,
                aliases: vec!["muse-spark-contributor".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // xAI / Grok (https://api.x.ai/v1)
            ModelInfo {
                id: "grok-4.6".to_string(),
                provider: ProviderKind::Xai,
                aliases: vec!["grok".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "grok-4.5".to_string(),
                provider: ProviderKind::Xai,
                aliases: vec!["xai-grok-4.5".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "grok-4.3".to_string(),
                provider: ProviderKind::Xai,
                aliases: vec!["xai-grok-4.3".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "grok-build".to_string(),
                provider: ProviderKind::Xai,
                aliases: vec!["xai-grok-build".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "grok-composer-2.5-fast".to_string(),
                provider: ProviderKind::Xai,
                aliases: vec!["xai-grok-composer".to_string()],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "grok-4.20-0309-reasoning".to_string(),
                provider: ProviderKind::Xai,
                aliases: vec!["xai-grok-reasoning".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "grok-4.20-0309-non-reasoning".to_string(),
                provider: ProviderKind::Xai,
                aliases: vec!["xai-grok-fast".to_string()],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "gemini-3.1-pro-preview".to_string(),
                provider: ProviderKind::Google,
                aliases: vec!["gemini-3.1-pro".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gemini-3-pro-preview".to_string(),
                provider: ProviderKind::Google,
                aliases: vec!["gemini-3-pro".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            // Gemini 3.7 Flash (2026-08 latest Flash; 1,048,576 in / 65,536 out,
            // https://ai.google.dev/gemini-api/docs/models/gemini-3.7-flash).
            ModelInfo {
                id: "gemini-3.7-flash".to_string(),
                provider: ProviderKind::Google,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gemini-3.6-flash".to_string(),
                provider: ProviderKind::Google,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gemini-3.5-flash".to_string(),
                provider: ProviderKind::Google,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gemini-3.5-flash-lite".to_string(),
                provider: ProviderKind::Google,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "gemini-2.5-pro".to_string(),
                provider: ProviderKind::Google,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "gemini-2.5-flash".to_string(),
                provider: ProviderKind::Google,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "mistral-code-latest".to_string(),
                provider: ProviderKind::Mistral,
                aliases: vec![
                    "codestral".to_string(),
                    "codestral-latest".to_string(),
                    "mistral-code".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "mistral-medium-latest".to_string(),
                provider: ProviderKind::Mistral,
                aliases: vec![
                    "mistral-medium".to_string(),
                    "mistral-medium-3-5".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "mistral-small-latest".to_string(),
                provider: ProviderKind::Mistral,
                aliases: vec![
                    "mistral-small".to_string(),
                    "mistral-small-2603".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "magistral-small-latest".to_string(),
                provider: ProviderKind::Mistral,
                aliases: vec!["magistral".to_string(), "magistral-small".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "mistral-large-latest".to_string(),
                provider: ProviderKind::Mistral,
                aliases: vec!["mistral-large".to_string()],
                supports_tools: true,
                supports_reasoning: false,
            },
        ];
        Self::new(models)
    }
}

impl ModelRegistry {
    /// Creates a new registry from a list of [`ModelInfo`] entries.
    ///
    #[must_use]
    pub fn new(models: Vec<ModelInfo>) -> Self {
        Self { models }
    }

    /// Returns a clone of all models in the registry.
    #[must_use]
    pub fn list(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

    /// Returns whether a selector is known only outside the selected provider.
    ///
    /// This is rejection metadata, never route authority: callers may use it
    /// to reject a clearly foreign model, but must not use the matching row to
    /// select a provider or credential slot.
    #[must_use]
    pub fn is_known_for_other_provider(
        &self,
        requested: &str,
        selected_provider: ProviderKind,
    ) -> bool {
        let known_here = self
            .models
            .iter()
            .any(|model| model.provider == selected_provider && model_matches(model, requested));
        !known_here
            && self
                .models
                .iter()
                .any(|model| model.provider != selected_provider && model_matches(model, requested))
    }

    /// Resolves a user-requested model name to a concrete [`ModelInfo`].
    ///
    /// Resolution follows this priority order:
    /// 1. If the provider is Ollama, the requested name is used as-is (to
    ///    support arbitrary local model tags like `qwen2.5-coder:7b`).
    /// 2. If a `provider_hint` is given, search for a model matching that
    ///    provider whose id or alias matches the request (case-insensitive).
    /// 3. Provider-specific pass-through contracts may preserve arbitrary
    ///    model ids.
    /// 4. An omitted model falls back to the explicitly selected provider's
    ///    documented default.
    /// 5. A requested model outside that provider fails closed. Model text is
    ///    metadata and never authorizes a provider or credential switch.
    pub fn resolve(
        &self,
        requested: Option<&str>,
        provider_hint: Option<ProviderKind>,
    ) -> Result<ModelResolution, ModelResolutionError> {
        let requested = requested.filter(|name| !name.trim().is_empty());
        let mut fallback_chain = Vec::new();
        let Some(provider) = provider_hint else {
            return Err(ModelResolutionError::ProviderRequired {
                requested: requested.map(ToOwned::to_owned),
            });
        };

        if let Some(name) = requested {
            fallback_chain.push(format!("requested:{name}"));
            if matches!(
                provider_hint,
                Some(ProviderKind::Ollama | ProviderKind::OllamaCloud)
            ) {
                return Ok(ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: ModelInfo {
                        id: name.trim().to_string(),
                        provider: provider_hint.expect("matched provider hint"),
                        aliases: Vec::new(),
                        supports_tools: true,
                        supports_reasoning: false,
                    },
                    used_fallback: false,
                    fallback_chain,
                });
            }
            // OpenCode Go's catalog spans Chat Completions and Anthropic
            // Messages, while Codewhale's provider slice intentionally speaks
            // Chat only. Resolve a hinted Go model through the shared Chat
            // allowlist and never fall through to a same-named global alias on
            // OpenRouter or MiniMax.
            if provider_hint == Some(ProviderKind::OpencodeGo)
                && let Some(canonical) = opencode_go_chat_model_id(name)
                && let Some(model) = self
                    .models
                    .iter()
                    .find(|model| {
                        model.provider == ProviderKind::OpencodeGo
                            && model.id.eq_ignore_ascii_case(canonical)
                    })
                    .cloned()
            {
                return Ok(ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: model,
                    used_fallback: false,
                    fallback_chain,
                });
            }
            if provider_hint != Some(ProviderKind::OpencodeGo)
                && let Some(provider) = provider_hint
                && let Some(model) = self
                    .models
                    .iter()
                    .find(|m| m.provider == provider && model_matches(m, name))
                    .cloned()
            {
                return Ok(ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: model,
                    used_fallback: false,
                    fallback_chain,
                });
            }
            if provider_hint == Some(ProviderKind::Atlascloud)
                && let Some(model) = atlascloud_passthrough_model(name)
            {
                return Ok(ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: model,
                    used_fallback: false,
                    fallback_chain,
                });
            }
            if provider_hint == Some(ProviderKind::Arcee)
                && let Some(model) = arcee_passthrough_model(name)
            {
                return Ok(ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: model,
                    used_fallback: false,
                    fallback_chain,
                });
            }
            if provider_hint == Some(ProviderKind::XiaomiMimo)
                && let Some(model) = xiaomi_mimo_passthrough_model(name)
            {
                return Ok(ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: model,
                    used_fallback: false,
                    fallback_chain,
                });
            }
            if !self.models.iter().any(|model| model.provider == provider) {
                return Err(ModelResolutionError::ProviderHasNoModels {
                    provider,
                    requested: Some(name.to_string()),
                });
            }
            return Err(ModelResolutionError::ModelNotAvailableForProvider {
                provider,
                requested: name.to_string(),
            });
        }

        fallback_chain.push(format!("provider_default:{}", provider.as_str()));
        if !self.models.iter().any(|model| model.provider == provider) {
            return Err(ModelResolutionError::ProviderHasNoModels {
                provider,
                requested: None,
            });
        }
        let default_model = provider.provider().default_model();
        if let Some(model) = self
            .models
            .iter()
            .find(|model| model.provider == provider && model_matches(model, default_model))
            .cloned()
        {
            return Ok(ModelResolution {
                requested: None,
                resolved: model,
                used_fallback: true,
                fallback_chain,
            });
        }

        Err(ModelResolutionError::ProviderDefaultUnavailable {
            provider,
            default_model: default_model.to_string(),
        })
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[must_use]
/// Classify a model identifier by its underlying model family.
pub fn model_family(model_id: &str) -> ModelFamily {
    let normalized = normalize(model_id);
    if normalized.is_empty() {
        return ModelFamily::Inferencer;
    }

    if normalized.contains("deepseek") {
        return ModelFamily::DeepSeek;
    }
    if normalized.contains("claude") || normalized.contains("anthropic") {
        return ModelFamily::Anthropic;
    }
    if normalized.contains("gpt-oss") || normalized.contains("gpt_oss") {
        return ModelFamily::GptOss;
    }
    if normalized.starts_with("gpt-")
        || normalized.contains("/gpt-")
        || normalized.contains("openai/")
    {
        return ModelFamily::OpenAI;
    }
    if normalized.contains("gemini")
        || normalized.contains("gemma")
        || normalized.contains("google/")
    {
        return ModelFamily::Google;
    }
    if normalized.contains("llama")
        || normalized.contains("muse-spark")
        || normalized.contains("meta-")
        || normalized.contains("meta/")
    {
        return ModelFamily::Meta;
    }
    if normalized.contains("mistral")
        || normalized.contains("mixtral")
        || normalized.contains("codestral")
    {
        return ModelFamily::Mistral;
    }
    if normalized.contains("qwen") {
        return ModelFamily::Qwen;
    }
    if normalized.contains("grok") {
        return ModelFamily::Grok;
    }
    if normalized.contains("cohere") || normalized.contains("command-r") {
        return ModelFamily::Cohere;
    }

    ModelFamily::Inferencer
}

fn model_matches(model: &ModelInfo, requested: &str) -> bool {
    let requested = normalize(requested);
    normalize(&model.id) == requested
        || model
            .aliases
            .iter()
            .any(|alias| normalize(alias) == requested)
}

fn atlascloud_passthrough_model(requested: &str) -> Option<ModelInfo> {
    let requested = requested.trim();
    if requested.is_empty() || !requested.contains('/') {
        return None;
    }

    Some(ModelInfo {
        id: requested.to_string(),
        provider: ProviderKind::Atlascloud,
        aliases: Vec::new(),
        supports_tools: true,
        supports_reasoning: true,
    })
}

fn arcee_passthrough_model(requested: &str) -> Option<ModelInfo> {
    let requested = requested.trim();
    if requested.is_empty() {
        return None;
    }
    let supports_reasoning = requested.to_ascii_lowercase().contains("thinking");

    Some(ModelInfo {
        id: requested.to_string(),
        provider: ProviderKind::Arcee,
        aliases: Vec::new(),
        supports_tools: true,
        supports_reasoning,
    })
}

fn xiaomi_mimo_passthrough_model(requested: &str) -> Option<ModelInfo> {
    let requested = requested.trim();
    if requested.is_empty() || requested.chars().any(char::is_control) {
        return None;
    }

    Some(ModelInfo {
        id: requested.to_string(),
        provider: ProviderKind::XiaomiMimo,
        aliases: Vec::new(),
        supports_tools: true,
        supports_reasoning: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    trait ModelRegistryTestExt {
        fn resolve_ok(
            &self,
            requested: Option<&str>,
            provider_hint: Option<ProviderKind>,
        ) -> ModelResolution;
    }

    impl ModelRegistryTestExt for ModelRegistry {
        fn resolve_ok(
            &self,
            requested: Option<&str>,
            provider_hint: Option<ProviderKind>,
        ) -> ModelResolution {
            self.resolve(requested, provider_hint)
                .expect("test route should resolve")
        }
    }

    #[test]
    fn model_registry_new_preserves_model_rows_and_aliases() {
        let models = vec![
            ModelInfo {
                id: "Model-A".to_string(),
                provider: ProviderKind::Deepseek,
                aliases: vec!["alias-1".to_string(), " ALIAS-2 ".to_string()],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "model-b".to_string(),
                provider: ProviderKind::Deepseek,
                aliases: vec!["alias-1".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
        ];

        let registry = ModelRegistry::new(models);

        let rows = registry.list();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "Model-A");
        assert_eq!(rows[0].aliases, ["alias-1", " ALIAS-2 "]);
        assert_eq!(rows[1].id, "model-b");
    }

    #[test]
    fn deepseek_v4_pro_alias_stays_deepseek_when_provider_selected() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("deepseek-v4-pro"), Some(ProviderKind::Deepseek));

        assert_eq!(resolved.resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.resolved.id, "deepseek-v4-pro");
    }

    #[test]
    fn providerless_unknown_model_requires_explicit_route_authority() {
        let registry = ModelRegistry::default();

        for requested in [None, Some("deepseek-v4-pro"), Some("not-in-the-catalog")] {
            let error = ModelRegistry::resolve(&registry, requested, None)
                .expect_err("provider-less fallback must fail closed");
            assert_eq!(
                error,
                ModelResolutionError::ProviderRequired {
                    requested: requested.map(str::to_string),
                }
            );
            if requested.is_none() || requested == Some("not-in-the-catalog") {
                assert!(!error.to_string().to_ascii_lowercase().contains("deepseek"));
            }
        }
    }

    #[test]
    fn providerless_unknown_selectors_never_mint_provider_authority() {
        let registry = ModelRegistry::default();

        for requested in [
            "deepseek-v4-not-a-real-model",
            "gpt-not-a-real-model",
            "provider/model-that-does-not-exist",
        ] {
            assert!(matches!(
                ModelRegistry::resolve(&registry, Some(requested), None),
                Err(ModelResolutionError::ProviderRequired {
                    requested: Some(returned),
                }) if returned == requested
            ));
        }
        for requested in ["", "   "] {
            assert!(matches!(
                ModelRegistry::resolve(&registry, Some(requested), None),
                Err(ModelResolutionError::ProviderRequired { requested: None })
            ));
        }
    }

    #[test]
    fn explicit_provider_with_no_registry_rows_never_borrows_global_default() {
        let registry = ModelRegistry::new(Vec::new());

        let error = ModelRegistry::resolve(
            &registry,
            Some("provider-owned-model"),
            Some(ProviderKind::Openrouter),
        )
        .expect_err("an empty provider catalog must not borrow another route");
        assert_eq!(
            error,
            ModelResolutionError::ProviderHasNoModels {
                provider: ProviderKind::Openrouter,
                requested: Some("provider-owned-model".to_string()),
            }
        );
        assert!(!error.to_string().to_ascii_lowercase().contains("deepseek"));
    }

    #[test]
    fn explicit_deepseek_selection_retains_its_provider_owned_default() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::Deepseek));

        assert_eq!(resolved.resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.resolved.id, "deepseek-v4-pro");
        assert!(resolved.used_fallback);
        assert_eq!(resolved.fallback_chain, ["provider_default:deepseek"]);
    }

    #[test]
    fn explicit_openai_selection_uses_its_documented_default_not_first_catalog_row() {
        let registry = ModelRegistry::default();

        for requested in [None, Some(""), Some("   ")] {
            let resolved = registry.resolve_ok(requested, Some(ProviderKind::Openai));
            assert_eq!(resolved.requested, None);
            assert_eq!(resolved.resolved.provider, ProviderKind::Openai);
            assert_eq!(resolved.resolved.id, "gpt-5.6");
            assert!(resolved.used_fallback);
            assert_eq!(resolved.fallback_chain, ["provider_default:openai"]);
        }
    }

    #[test]
    fn missing_provider_default_row_fails_instead_of_borrowing_another_model() {
        let registry = ModelRegistry::new(vec![ModelInfo {
            id: "not-the-openai-default".to_string(),
            provider: ProviderKind::Openai,
            aliases: Vec::new(),
            supports_tools: true,
            supports_reasoning: true,
        }]);

        let error = registry
            .resolve(None, Some(ProviderKind::Openai))
            .expect_err("a missing provider default must fail closed");
        assert_eq!(
            error,
            ModelResolutionError::ProviderDefaultUnavailable {
                provider: ProviderKind::Openai,
                default_model: "gpt-5.6".to_string(),
            }
        );
    }

    #[test]
    fn deepseek_vision_model_lists_and_resolves_with_aliases() {
        let registry = ModelRegistry::default();
        let listed = registry.list();

        assert!(listed.iter().any(|model| {
            model.provider == ProviderKind::Deepseek
                && model.id == "deepseek-v4-flash-vision-exp"
                && model.aliases
                    == [
                        "flash-vision".to_string(),
                        "deepseek-v4flashvisionexp".to_string(),
                    ]
        }));

        for selector in [
            "deepseek-v4-flash-vision-exp",
            "flash-vision",
            "deepseek-v4flashvisionexp",
        ] {
            let resolved = registry.resolve_ok(Some(selector), Some(ProviderKind::Deepseek));
            assert_eq!(
                resolved.resolved.id, "deepseek-v4-flash-vision-exp",
                "{selector} must resolve to the experimental vision model"
            );
            assert_eq!(resolved.resolved.provider, ProviderKind::Deepseek);
            assert!(!resolved.used_fallback, "{selector} must not fall back");
        }
    }

    #[test]
    fn deepseek_v4_pro_alias_resolves_to_nvidia_nim_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("deepseek-v4-pro"), Some(ProviderKind::NvidiaNim));

        assert_eq!(resolved.resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-pro");
    }

    #[test]
    fn nvidia_nim_default_uses_catalog_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::NvidiaNim));

        assert_eq!(resolved.resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-pro");
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_nvidia_nim_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve_ok(Some("deepseek-v4-flash"), Some(ProviderKind::NvidiaNim));

        assert_eq!(resolved.resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-flash");
    }

    #[test]
    fn atlascloud_default_uses_namespaced_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-flash");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_atlascloud_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve_ok(Some("deepseek-v4-flash"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-flash");
    }

    #[test]
    fn deepseek_v4_pro_alias_resolves_to_atlascloud_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("deepseek-v4-pro"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-pro");
    }

    #[test]
    fn atlascloud_provider_hint_passes_through_explicit_model_id() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve_ok(Some("openai/gpt-5.2-chat"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "openai/gpt-5.2-chat");
        assert!(resolved.resolved.supports_tools);
        assert!(resolved.resolved.supports_reasoning);
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn atlascloud_provider_hint_preserves_explicit_model_id_case() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve_ok(Some("Qwen/Qwen3-Coder"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "Qwen/Qwen3-Coder");
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn atlascloud_plain_unknown_model_rejects_instead_of_using_default() {
        let registry = ModelRegistry::default();
        let error = registry
            .resolve(Some("not-in-atlas"), Some(ProviderKind::Atlascloud))
            .expect_err("a requested unknown model must not become the provider default");

        assert_eq!(
            error,
            ModelResolutionError::ModelNotAvailableForProvider {
                provider: ProviderKind::Atlascloud,
                requested: "not-in-atlas".to_string(),
            }
        );
    }

    #[test]
    fn openrouter_default_uses_namespaced_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::Openrouter));

        assert_eq!(resolved.resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.resolved.id, "deepseek/deepseek-v4-pro");
    }

    #[test]
    fn xiaomi_mimo_default_uses_canonical_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::XiaomiMimo));

        assert_eq!(resolved.resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.resolved.id, "mimo-v2.5-pro");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn moonshot_default_and_aliases_use_kimi_k27_code() {
        let registry = ModelRegistry::default();

        for requested in [None, Some("kimi"), Some("kimi-k2.7-code")] {
            let resolved = registry.resolve_ok(requested, Some(ProviderKind::Moonshot));

            assert_eq!(resolved.resolved.provider, ProviderKind::Moonshot);
            assert_eq!(resolved.resolved.id, "kimi-k2.7-code");
            assert!(resolved.resolved.supports_tools);
            assert!(resolved.resolved.supports_reasoning);
        }
    }

    #[test]
    fn moonshot_explicit_kimi_k26_remains_available() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("kimi-k2.6"), Some(ProviderKind::Moonshot));

        assert_eq!(resolved.resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.resolved.id, "kimi-k2.6");
        assert!(resolved.resolved.supports_reasoning);
    }

    /// v0.9.1 dogfood report: a user ran `--provider moonshot --model kimi-k3` and was told
    /// the model was `kimi-k2.7-code`. The registry knew neither Moonshot K3
    /// product, so the explicit request fell through to the provider default.
    #[test]
    fn moonshot_resolves_both_k3_products_without_crossing_them() {
        let registry = ModelRegistry::default();

        for (requested, expected) in [("kimi-k3", "kimi-k3"), ("k3", "k3")] {
            let resolved = registry.resolve_ok(Some(requested), Some(ProviderKind::Moonshot));

            assert_eq!(resolved.resolved.provider, ProviderKind::Moonshot);
            assert_eq!(resolved.resolved.id, expected, "{resolved:?}");
            assert!(
                !resolved.used_fallback,
                "an explicit Moonshot K3 request is not a fallback: {resolved:?}"
            );
        }
    }

    /// The bare `k3` id belongs to the Kimi Code coding-plan endpoint and
    /// `kimi-k3` to the direct platform endpoint. Neither may be laundered
    /// into the other's id by alias expansion.
    #[test]
    fn moonshot_k3_ids_are_never_rewritten_into_each_other() {
        let registry = ModelRegistry::default();

        assert_eq!(
            registry
                .resolve_ok(Some("kimi-k3"), Some(ProviderKind::Moonshot))
                .resolved
                .id,
            "kimi-k3"
        );
        assert_eq!(
            registry
                .resolve_ok(Some("k3"), Some(ProviderKind::Moonshot))
                .resolved
                .id,
            "k3"
        );
    }

    /// A provider-scoped question must never be answered with another
    /// vendor's model. `kimi-k3` also exists in the OpenCode Go catalog;
    /// before this fix that entry answered `--provider moonshot` requests.
    #[test]
    fn a_provider_hint_never_resolves_to_another_providers_model() {
        let registry = ModelRegistry::default();

        let error = registry
            .resolve(Some("glm-5.2"), Some(ProviderKind::Moonshot))
            .expect_err("a Moonshot request must not be answered by Z.ai or a default");
        assert_eq!(
            error,
            ModelResolutionError::ModelNotAvailableForProvider {
                provider: ProviderKind::Moonshot,
                requested: "glm-5.2".to_string(),
            }
        );

        let go = registry.resolve_ok(Some("kimi-k3"), Some(ProviderKind::OpencodeGo));
        assert_eq!(go.resolved.provider, ProviderKind::OpencodeGo);
        assert_eq!(go.resolved.id, "kimi-k3");
    }

    #[test]
    fn xiaomi_mimo_tts_aliases_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("tts"), Some(ProviderKind::XiaomiMimo));
        assert_eq!(resolved.resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.resolved.id, "mimo-v2.5-tts");
        assert!(!resolved.resolved.supports_tools);
        assert!(!resolved.resolved.supports_reasoning);

        let resolved = registry.resolve_ok(Some("voice-design"), Some(ProviderKind::XiaomiMimo));
        assert_eq!(resolved.resolved.id, "mimo-v2.5-tts-voicedesign");

        let resolved = registry.resolve_ok(Some("voiceclone"), Some(ProviderKind::XiaomiMimo));
        assert_eq!(resolved.resolved.id, "mimo-v2.5-tts-voiceclone");
    }

    #[test]
    fn xiaomi_mimo_chat_aliases_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();

        let resolved = registry.resolve_ok(Some("omni"), Some(ProviderKind::XiaomiMimo));
        assert_eq!(resolved.resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.resolved.id, "mimo-v2.5");
        assert!(resolved.resolved.supports_tools);
    }

    #[test]
    fn xiaomi_mimo_provider_hint_preserves_custom_model_id() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve_ok(Some("account-custom-mimo"), Some(ProviderKind::XiaomiMimo));

        assert_eq!(resolved.resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.resolved.id, "account-custom-mimo");
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn xiaomi_mimo_provider_hint_does_not_reclassify_openrouter_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(
            Some("deepseek/deepseek-v4-pro"),
            Some(ProviderKind::XiaomiMimo),
        );

        assert_eq!(resolved.resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.resolved.id, "deepseek/deepseek-v4-pro");
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn wanjie_ark_default_uses_reasoner_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::WanjieArk));

        assert_eq!(resolved.resolved.provider, ProviderKind::WanjieArk);
        assert_eq!(resolved.resolved.id, "deepseek-reasoner");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn novita_default_uses_namespaced_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::Novita));

        assert_eq!(resolved.resolved.provider, ProviderKind::Novita);
        assert_eq!(resolved.resolved.id, "deepseek/deepseek-v4-pro");
    }

    #[test]
    fn fireworks_default_uses_canonical_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::Fireworks));

        assert_eq!(resolved.resolved.provider, ProviderKind::Fireworks);
        assert_eq!(
            resolved.resolved.id,
            "accounts/fireworks/models/deepseek-v4-pro"
        );
    }

    #[test]
    fn siliconflow_default_uses_canonical_pro_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::Siliconflow));

        assert_eq!(resolved.resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Pro");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn arcee_default_uses_direct_trinity_large_thinking_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::Arcee));

        assert_eq!(resolved.resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.resolved.id, "trinity-large-thinking");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn arcee_trinity_alias_resolves_to_direct_large_thinking_not_openrouter() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("trinity"), Some(ProviderKind::Arcee));

        assert_eq!(resolved.resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.resolved.id, "trinity-large-thinking");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn arcee_trinity_mini_remains_explicit_compatibility_model() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("trinity-mini"), Some(ProviderKind::Arcee));

        assert_eq!(resolved.resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.resolved.id, "trinity-mini");
        assert!(resolved.resolved.supports_reasoning);
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn arcee_provider_hint_preserves_explicit_future_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("trinity-large-next"), Some(ProviderKind::Arcee));

        assert_eq!(resolved.resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.resolved.id, "trinity-large-next");
        assert!(!resolved.resolved.supports_reasoning);
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn deepseek_reasoner_does_not_silently_substitute_siliconflow_pro() {
        let registry = ModelRegistry::default();
        let error = registry
            .resolve(Some("deepseek-reasoner"), Some(ProviderKind::Siliconflow))
            .expect_err("an absent alias must not become SiliconFlow's first/default row");

        assert_eq!(
            error,
            ModelResolutionError::ModelNotAvailableForProvider {
                provider: ProviderKind::Siliconflow,
                requested: "deepseek-reasoner".to_string(),
            }
        );
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_siliconflow_flash_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve_ok(Some("deepseek-v4-flash"), Some(ProviderKind::Siliconflow));

        assert_eq!(resolved.resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Flash");
    }

    #[test]
    fn sglang_default_uses_canonical_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::Sglang));

        assert_eq!(resolved.resolved.provider, ProviderKind::Sglang);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Pro");
    }

    #[test]
    fn zai_direct_models_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();

        // Keep the agent registry fallback aligned with codewhale-config's
        // DEFAULT_ZAI_MODEL.
        let default = registry.resolve_ok(None, Some(ProviderKind::Zai));
        assert_eq!(default.resolved.provider, ProviderKind::Zai);
        assert_eq!(default.resolved.id, "GLM-5.3");
        assert!(default.used_fallback);
        assert_eq!(default.fallback_chain, ["provider_default:zai"]);

        for (alias, expected) in [
            ("GLM-5.1", "GLM-5.1"),
            ("glm-5-1", "GLM-5.1"),
            ("GLM-5.2", "GLM-5.2"),
            ("glm-5.2", "GLM-5.2"),
            ("zai-glm-5-2", "GLM-5.2"),
            ("GLM-5.3", "GLM-5.3"),
            ("glm-5.3", "GLM-5.3"),
            ("glm-5-3", "GLM-5.3"),
            ("zai-glm-5-3", "GLM-5.3"),
            ("GLM-5.3-Flash", "GLM-5.3-Flash"),
            ("glm-5.3-flash", "GLM-5.3-Flash"),
            ("glm-5-3-flash", "GLM-5.3-Flash"),
            ("zai-glm-5.3-flash", "GLM-5.3-Flash"),
            ("GLM-5-Turbo", "GLM-5-Turbo"),
            ("glm-5-turbo", "GLM-5-Turbo"),
            ("zai-glm-5-turbo", "GLM-5-Turbo"),
        ] {
            let resolved = registry.resolve_ok(Some(alias), Some(ProviderKind::Zai));

            assert_eq!(resolved.resolved.provider, ProviderKind::Zai);
            assert_eq!(resolved.resolved.id, expected);
            assert!(!resolved.used_fallback);
            assert!(resolved.resolved.supports_tools);
            assert!(resolved.resolved.supports_reasoning);
        }
    }

    #[test]
    fn first_party_recent_provider_models_are_listed() {
        let registry = ModelRegistry::default();
        let models = registry.list();

        for (provider, id) in [
            (ProviderKind::Zai, "GLM-5.2"),
            (ProviderKind::Stepfun, "step-3.7-flash"),
            (ProviderKind::Minimax, "MiniMax-M2.1"),
            (ProviderKind::MinimaxAnthropic, "MiniMax-M3"),
            (ProviderKind::Openmodel, "deepseek-v4-flash"),
            (ProviderKind::Meta, "muse-spark-1.2"),
            (ProviderKind::Xai, "grok-4.6"),
        ] {
            assert!(
                models
                    .iter()
                    .any(|model| model.provider == provider && model.id == id),
                "expected {provider:?} model {id} in registry"
            );
        }
    }

    #[test]
    fn opencode_go_lists_only_current_chat_completions_models() {
        let registry = ModelRegistry::default();
        let listed = registry.list();
        let models: Vec<&str> = listed
            .iter()
            .filter(|model| model.provider == ProviderKind::OpencodeGo)
            .map(|model| model.id.as_str())
            .collect();

        assert_eq!(
            models,
            vec![
                "deepseek-v4-pro",
                "grok-4.5",
                "glm-5.2",
                "glm-5.1",
                "kimi-k3",
                "kimi-k2.7-code",
                "kimi-k2.6",
                "deepseek-v4-flash",
                "mimo-v2.5",
                "mimo-v2.5-pro",
            ]
        );

        let default = registry.resolve_ok(None, Some(ProviderKind::OpencodeGo));
        assert_eq!(default.resolved.provider, ProviderKind::OpencodeGo);
        assert_eq!(default.resolved.id, "deepseek-v4-pro");

        for model in ["grok-4.5", "kimi-k3"] {
            for requested in [model.to_string(), format!("opencode-go/{model}")] {
                let resolved =
                    registry.resolve_ok(Some(&requested), Some(ProviderKind::OpencodeGo));
                assert_eq!(resolved.resolved.provider, ProviderKind::OpencodeGo);
                assert_eq!(resolved.resolved.id, model);
                assert!(!resolved.used_fallback);
            }
        }

        for messages_only in [
            "minimax-m3",
            "minimax-m2.7",
            "minimax-m2.5",
            "qwen3.7-max",
            "qwen3.7-plus",
            "qwen3.6-plus",
        ] {
            for requested in [
                messages_only.to_string(),
                format!("opencode-go/{messages_only}"),
            ] {
                let rejected = registry
                    .resolve(Some(&requested), Some(ProviderKind::OpencodeGo))
                    .expect_err("Messages-only id must not fall back on the Chat-only route");
                assert_eq!(
                    rejected,
                    ModelResolutionError::ModelNotAvailableForProvider {
                        provider: ProviderKind::OpencodeGo,
                        requested,
                    }
                );
            }
        }
    }

    #[test]
    fn xai_grok_models_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();

        let default = registry.resolve_ok(None, Some(ProviderKind::Xai));
        assert_eq!(default.resolved.provider, ProviderKind::Xai);
        assert_eq!(default.resolved.id, "grok-4.6");
        assert!(default.used_fallback);

        let alias = registry.resolve_ok(Some("grok"), Some(ProviderKind::Xai));
        assert_eq!(alias.resolved.provider, ProviderKind::Xai);
        assert_eq!(alias.resolved.id, "grok-4.6");
        assert!(!alias.used_fallback);

        let fast = registry.resolve_ok(
            Some("grok-4.20-0309-non-reasoning"),
            Some(ProviderKind::Xai),
        );
        assert_eq!(fast.resolved.provider, ProviderKind::Xai);
        assert_eq!(fast.resolved.id, "grok-4.20-0309-non-reasoning");
        assert!(!fast.resolved.supports_reasoning);
    }

    #[test]
    fn meta_muse_spark_resolves_when_provider_hinted() {
        let registry = ModelRegistry::default();

        let default = registry.resolve_ok(None, Some(ProviderKind::Meta));
        assert_eq!(default.resolved.provider, ProviderKind::Meta);
        assert_eq!(default.resolved.id, "muse-spark-1.2");
        assert!(default.used_fallback);

        let alias = registry.resolve_ok(Some("muse-spark"), Some(ProviderKind::Meta));
        assert_eq!(alias.resolved.provider, ProviderKind::Meta);
        assert_eq!(alias.resolved.id, "muse-spark-1.2");
        assert!(!alias.used_fallback);
        assert_eq!(model_family("muse-spark-1.2"), ModelFamily::Meta);
    }

    #[test]
    fn openai_gpt56_family_resolves_when_provider_hinted() {
        let registry = ModelRegistry::default();
        for model in ["gpt-5.6", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let resolved = registry.resolve_ok(Some(model), Some(ProviderKind::Openai));
            assert_eq!(resolved.resolved.provider, ProviderKind::Openai, "{model}");
            assert_eq!(resolved.resolved.id, model, "{model}");
            assert!(resolved.resolved.supports_tools, "{model}");
            assert!(resolved.resolved.supports_reasoning, "{model}");
            assert!(!resolved.used_fallback, "{model}");
        }
    }

    #[test]
    fn grok_ids_stay_in_grok_family() {
        assert_eq!(model_family("grok-4.6"), ModelFamily::Grok);
        assert_eq!(model_family("grok-4.5"), ModelFamily::Grok);
        assert_eq!(
            model_family("grok-4.20-0309-non-reasoning"),
            ModelFamily::Grok
        );
    }

    #[test]
    fn stepfun_and_minimax_direct_models_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();

        let stepfun = registry.resolve_ok(None, Some(ProviderKind::Stepfun));
        assert_eq!(stepfun.resolved.provider, ProviderKind::Stepfun);
        assert_eq!(stepfun.resolved.id, "step-3.7-flash");

        for (alias, expected) in [
            ("minimax", "MiniMax-M3"),
            ("minimax-m3", "MiniMax-M3"),
            ("minimax-m2.7", "MiniMax-M2.7"),
            ("minimax-m2-7-highspeed", "MiniMax-M2.7-highspeed"),
            ("minimax-m2.1", "MiniMax-M2.1"),
            ("minimax-m2", "MiniMax-M2"),
        ] {
            let resolved = registry.resolve_ok(Some(alias), Some(ProviderKind::Minimax));

            assert_eq!(resolved.resolved.provider, ProviderKind::Minimax);
            assert_eq!(resolved.resolved.id, expected);
            assert!(!resolved.used_fallback);
            assert!(resolved.resolved.supports_tools);
            assert!(resolved.resolved.supports_reasoning);
        }
    }

    #[test]
    fn minimax_anthropic_models_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();

        for (alias, expected) in [
            ("minimax-anthropic", "MiniMax-M3"),
            ("minimax-m3", "MiniMax-M3"),
            ("minimax-m2.7", "MiniMax-M2.7"),
        ] {
            let resolved = registry.resolve_ok(Some(alias), Some(ProviderKind::MinimaxAnthropic));

            assert_eq!(resolved.resolved.provider, ProviderKind::MinimaxAnthropic);
            assert_eq!(resolved.resolved.id, expected);
            assert!(!resolved.used_fallback);
            assert!(resolved.resolved.supports_tools);
            assert!(resolved.resolved.supports_reasoning);
        }
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_openrouter_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve_ok(Some("deepseek-v4-flash"), Some(ProviderKind::Openrouter));

        assert_eq!(resolved.resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.resolved.id, "deepseek/deepseek-v4-flash");
    }

    #[test]
    fn recent_openrouter_large_model_aliases_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();

        for (alias, expected) in [
            ("trinity-large-thinking", "arcee-ai/trinity-large-thinking"),
            ("qwen3.6-flash", "qwen/qwen3.6-flash"),
            ("qwen3.6-35b-a3b", "qwen/qwen3.6-35b-a3b"),
            ("qwen3.6-max-preview", "qwen/qwen3.6-max-preview"),
            ("qwen3.6-plus", "qwen/qwen3.6-plus"),
            ("gemma-4-31b-it", "google/gemma-4-31b-it"),
            ("glm-5.1", "z-ai/glm-5.1"),
            ("glm-5.2", "z-ai/glm-5.2"),
            ("glm-5.3", "z-ai/glm-5.3"),
            ("glm-5.3-flash", "z-ai/glm-5.3-flash"),
            ("minimax-m3", "minimax/minimax-m3"),
            ("minimax-2.7", "minimax/minimax-m2.7"),
            ("openrouter-mimo-v2.5-pro", "xiaomi/mimo-v2.5-pro"),
            ("openrouter-kimi-k2.7-code", "moonshotai/kimi-k2.7-code"),
            ("openrouter-kimi-k2.6", "moonshotai/kimi-k2.6"),
            ("nemotron-3-ultra", "nvidia/nemotron-3-ultra-550b-a55b"),
            (
                "nvidia/nemotron-3-ultra",
                "nvidia/nemotron-3-ultra-550b-a55b",
            ),
        ] {
            let resolved = registry.resolve_ok(Some(alias), Some(ProviderKind::Openrouter));

            assert_eq!(resolved.resolved.provider, ProviderKind::Openrouter);
            assert_eq!(resolved.resolved.id, expected);
            assert!(resolved.resolved.supports_tools);
            assert!(resolved.resolved.supports_reasoning);
        }
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_novita_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("deepseek-v4-flash"), Some(ProviderKind::Novita));

        assert_eq!(resolved.resolved.provider, ProviderKind::Novita);
        assert_eq!(resolved.resolved.id, "deepseek/deepseek-v4-flash");
    }

    #[test]
    fn together_inkling_keeps_published_wire_identity() {
        let registry = ModelRegistry::default();
        for requested in ["thinkingmachines/inkling", "inkling", "together-inkling"] {
            let resolved = registry.resolve_ok(Some(requested), Some(ProviderKind::Together));

            assert_eq!(resolved.resolved.provider, ProviderKind::Together);
            assert_eq!(resolved.resolved.id, "thinkingmachines/inkling");
            assert!(resolved.resolved.supports_tools);
            assert!(resolved.resolved.supports_reasoning);
            assert!(!resolved.used_fallback);
        }

        assert!(matches!(
            registry.resolve(Some("inkling"), None),
            Err(ModelResolutionError::ProviderRequired { .. })
        ));
    }

    #[test]
    fn registry_lists_and_resolves_every_v090_catalog_addition() {
        let registry = ModelRegistry::default();
        let advertised = [
            (ProviderKind::Anthropic, "claude-sonnet-5"),
            (ProviderKind::Anthropic, "claude-fable-5"),
            (ProviderKind::Openai, "gpt-5.3-codex"),
            (ProviderKind::Openai, "gpt-5.5"),
            (ProviderKind::Openai, "gpt-5.5-pro"),
            (ProviderKind::Openrouter, "qwen/qwen3.7-plus"),
            (ProviderKind::Arcee, "trinity-mini"),
        ];

        let listed = registry.list();
        for (provider, model_id) in advertised {
            assert!(
                listed
                    .iter()
                    .any(|model| model.provider == provider && model.id == model_id),
                "missing {model_id} ({}) from model list",
                provider.as_str()
            );
            let resolved = registry.resolve_ok(Some(model_id), Some(provider));
            assert_eq!(resolved.resolved.provider, provider, "{model_id}");
            assert_eq!(resolved.resolved.id, model_id, "{model_id}");
            assert!(!resolved.used_fallback, "{model_id}");
        }
    }

    #[test]
    fn gpt_55_stays_provider_scoped_between_openai_and_codex() {
        let registry = ModelRegistry::default();

        assert!(matches!(
            registry.resolve(Some("gpt-5.5"), None),
            Err(ModelResolutionError::ProviderRequired { .. })
        ));

        let codex = registry.resolve_ok(Some("gpt-5.5"), Some(ProviderKind::OpenaiCodex));
        assert_eq!(codex.resolved.provider, ProviderKind::OpenaiCodex);
        assert_eq!(codex.resolved.id, "gpt-5.5");
        assert!(!codex.used_fallback);
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_sglang_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("deepseek-v4-flash"), Some(ProviderKind::Sglang));

        assert_eq!(resolved.resolved.provider, ProviderKind::Sglang);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Flash");
    }

    #[test]
    fn vllm_default_uses_canonical_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::Vllm));

        assert_eq!(resolved.resolved.provider, ProviderKind::Vllm);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Pro");
    }

    #[test]
    fn ollama_default_is_unavailable_until_the_local_catalog_answers() {
        // Y-2: `DEFAULT_OLLAMA_MODEL` is deliberately "unknown". The real
        // default comes from the live local catalog, so the header never names
        // a model the session cannot reach; without that catalog the registry
        // must say so instead of resolving a costume.
        let registry = ModelRegistry::default();
        let error = registry
            .resolve(None, Some(ProviderKind::Ollama))
            .expect_err("the placeholder default must not resolve");

        assert!(matches!(
            error,
            ModelResolutionError::ProviderDefaultUnavailable {
                provider: ProviderKind::Ollama,
                ref default_model,
            } if default_model == "unknown"
        ));
    }

    #[test]
    fn ollama_cloud_default_uses_the_hosted_catalog_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(None, Some(ProviderKind::OllamaCloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::OllamaCloud);
        assert_eq!(resolved.resolved.id, "gpt-oss:120b");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn ollama_requested_model_tag_is_preserved() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("qwen2.5-coder:7b"), Some(ProviderKind::Ollama));

        assert_eq!(resolved.resolved.provider, ProviderKind::Ollama);
        assert_eq!(resolved.resolved.id, "qwen2.5-coder:7b");
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_vllm_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("deepseek-v4-flash"), Some(ProviderKind::Vllm));

        assert_eq!(resolved.resolved.provider, ProviderKind::Vllm);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Flash");
    }

    #[test]
    fn providerless_cased_model_text_does_not_authorize_deepseek() {
        let registry = ModelRegistry::default();
        assert!(matches!(
            registry.resolve(Some("DeepSeek-V4-Pro"), None),
            Err(ModelResolutionError::ProviderRequired { .. })
        ));
    }

    #[test]
    fn registry_casing_takes_priority_over_requested_casing_with_provider_hint() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("DeepSeek-V4-Pro"), Some(ProviderKind::Deepseek));

        assert_eq!(resolved.resolved.provider, ProviderKind::Deepseek);
        // Registry's canonical id is used even when user provides different casing
        assert_eq!(resolved.resolved.id, "deepseek-v4-pro");
    }

    #[test]
    fn providerless_whitespace_model_text_does_not_authorize_deepseek() {
        let registry = ModelRegistry::default();
        assert!(matches!(
            registry.resolve(Some("  DeepSeek-V4-Pro  "), None),
            Err(ModelResolutionError::ProviderRequired { .. })
        ));
    }

    #[test]
    fn alias_match_does_not_override_requested_casing() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve_ok(Some("deepseek-reasoner"), Some(ProviderKind::Deepseek));

        assert_eq!(resolved.resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.resolved.id, "deepseek-v4-flash");
    }

    #[test]
    fn model_family_classifies_known_model_ids() {
        assert_eq!(model_family("deepseek-v4-pro"), ModelFamily::DeepSeek);
        assert_eq!(model_family("openai/gpt-5.4"), ModelFamily::OpenAI);
        assert_eq!(
            model_family("anthropic/claude-opus-4-7"),
            ModelFamily::Anthropic
        );
        assert_eq!(
            model_family("meta-llama/llama-3.3-70b-instruct"),
            ModelFamily::Meta
        );
        assert_eq!(model_family("Qwen/Qwen3-Coder"), ModelFamily::Qwen);
    }

    #[test]
    fn model_family_uses_underlying_model_for_router_ids() {
        assert_eq!(
            model_family("groq/llama-3.3-70b-versatile"),
            ModelFamily::Meta
        );
        assert_eq!(
            model_family("openrouter/openai/gpt-5.4"),
            ModelFamily::OpenAI
        );
        assert_eq!(
            model_family("fireworks/accounts/fireworks/models/deepseek-v4-pro"),
            ModelFamily::DeepSeek
        );
    }

    #[test]
    fn model_family_covers_prominent_google_and_mistral_model_names() {
        assert_eq!(model_family("google/gemma-3-27b-it"), ModelFamily::Google);
        assert_eq!(
            model_family("mistralai/mixtral-8x22b"),
            ModelFamily::Mistral
        );
        assert_eq!(model_family("codestral-latest"), ModelFamily::Mistral);
    }

    #[test]
    fn model_family_falls_back_to_inferencer_for_unknown_models() {
        assert_eq!(
            model_family("custom-gateway/my-private-model"),
            ModelFamily::Inferencer
        );
        assert_eq!(model_family(""), ModelFamily::Inferencer);
    }
}
