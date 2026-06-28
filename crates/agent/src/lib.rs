use std::collections::HashMap;

use codewhale_config::ProviderKind;
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
    /// Whether a fallback was used because the requested model was not found.
    pub used_fallback: bool,
    /// The ordered list of resolution strategies that were attempted.
    pub fallback_chain: Vec<String>,
}

/// A registry of supported models and their aliases, used to resolve user-facing
/// model names to concrete provider-specific model entries.
///
/// The default registry is populated with all built-in models across supported
/// providers (DeepSeek, NVIDIA NIM, OpenAI-compatible, and others).
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    models: Vec<ModelInfo>,
    alias_map: HashMap<String, usize>,
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
            ModelInfo {
                id: "z-ai/glm-5-turbo".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["glm-5-turbo".to_string(), "zai-glm-5-turbo".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
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
                aliases: vec!["hy3-preview".to_string(), "tencent-hy3-preview".to_string()],
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
                aliases: vec![
                    "deepseek-v4-pro".to_string(),
                    "deepseek-reasoner".to_string(),
                    "deepseek-r1".to_string(),
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
                id: "deepseek-coder:1.3b".to_string(),
                provider: ProviderKind::Ollama,
                aliases: vec![],
                supports_tools: true,
                supports_reasoning: false,
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
                aliases: vec![
                    "sakana-fugu".to_string(),
                    "sakana/fugu".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: false,
            },
            ModelInfo {
                id: "fugu-ultra-20260615".to_string(),
                provider: ProviderKind::Sakana,
                aliases: vec![
                    "fugu-ultra".to_string(),
                    "sakana-fugu-ultra".to_string(),
                ],
                supports_tools: true,
                supports_reasoning: true,
            },
        ];
        Self::new(models)
    }
}

impl ModelRegistry {
    /// Creates a new registry from a list of [`ModelInfo`] entries.
    ///
    /// Builds an internal alias map for fast lookup by model id or alias.
    /// If multiple models share the same id or alias, the first one registered
    /// takes priority.
    #[must_use]
    pub fn new(models: Vec<ModelInfo>) -> Self {
        let mut alias_map = HashMap::new();
        for (idx, model) in models.iter().enumerate() {
            alias_map.entry(normalize(&model.id)).or_insert(idx);
            for alias in &model.aliases {
                alias_map.entry(normalize(alias)).or_insert(idx);
            }
        }
        Self { models, alias_map }
    }

    /// Returns a clone of all models in the registry.
    #[must_use]
    pub fn list(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

    /// Resolves a user-requested model name to a concrete [`ModelInfo`].
    ///
    /// Resolution follows this priority order:
    /// 1. If the provider is Ollama, the requested name is used as-is (to
    ///    support arbitrary local model tags like `qwen2.5-coder:7b`).
    /// 2. If a `provider_hint` is given, search for a model matching that
    ///    provider whose id or alias matches the request (case-insensitive).
    /// 3. Look up the alias map for a case-insensitive match.
    /// 4. Fall back to the first model belonging to the hinted provider
    ///    (or DeepSeek if no hint was given).
    /// 5. As a last resort, fall back to the first model in the registry.
    #[must_use]
    pub fn resolve(
        &self,
        requested: Option<&str>,
        provider_hint: Option<ProviderKind>,
    ) -> ModelResolution {
        let mut fallback_chain = Vec::new();

        if let Some(name) = requested {
            fallback_chain.push(format!("requested:{name}"));
            if provider_hint == Some(ProviderKind::Ollama) {
                return ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: ModelInfo {
                        id: name.trim().to_string(),
                        provider: ProviderKind::Ollama,
                        aliases: Vec::new(),
                        supports_tools: true,
                        supports_reasoning: false,
                    },
                    used_fallback: false,
                    fallback_chain,
                };
            }
            if let Some(provider) = provider_hint
                && let Some(model) = self
                    .models
                    .iter()
                    .find(|m| m.provider == provider && model_matches(m, name))
                    .cloned()
            {
                return ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: model,
                    used_fallback: false,
                    fallback_chain,
                };
            }
            if provider_hint == Some(ProviderKind::Atlascloud)
                && let Some(model) = atlascloud_passthrough_model(name)
            {
                return ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: model,
                    used_fallback: false,
                    fallback_chain,
                };
            }
            if provider_hint == Some(ProviderKind::Arcee)
                && let Some(model) = arcee_passthrough_model(name)
            {
                return ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: model,
                    used_fallback: false,
                    fallback_chain,
                };
            }
            if provider_hint == Some(ProviderKind::XiaomiMimo)
                && let Some(model) = xiaomi_mimo_passthrough_model(name)
            {
                return ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: model,
                    used_fallback: false,
                    fallback_chain,
                };
            }
            if let Some(idx) = self.alias_map.get(&normalize(name)) {
                return ModelResolution {
                    requested: Some(name.to_string()),
                    resolved: preserve_requested_model_id_case(self.models[*idx].clone(), name),
                    used_fallback: false,
                    fallback_chain,
                };
            }
        }

        let provider = provider_hint.unwrap_or(ProviderKind::Deepseek);
        fallback_chain.push(format!("provider_default:{}", provider.as_str()));
        if let Some(model) = self.models.iter().find(|m| m.provider == provider).cloned() {
            return ModelResolution {
                requested: requested.map(ToOwned::to_owned),
                resolved: model,
                used_fallback: true,
                fallback_chain,
            };
        }

        let final_fallback = self.models.first().cloned().unwrap_or(ModelInfo {
            id: "deepseek-v4-pro".to_string(),
            provider: ProviderKind::Deepseek,
            aliases: Vec::new(),
            supports_tools: true,
            supports_reasoning: true,
        });
        fallback_chain.push("global_default:deepseek-v4-pro".to_string());
        ModelResolution {
            requested: requested.map(ToOwned::to_owned),
            resolved: final_fallback,
            used_fallback: true,
            fallback_chain,
        }
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
    if normalized.contains("llama") || normalized.contains("meta-") || normalized.contains("meta/")
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

fn preserve_requested_model_id_case(mut model: ModelInfo, requested: &str) -> ModelInfo {
    let requested = requested.trim();
    if model.id.eq_ignore_ascii_case(requested) {
        model.id = requested.to_string();
    }
    model
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

    #[test]
    fn deepseek_v4_pro_alias_stays_deepseek_by_default() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-pro"), None);

        assert_eq!(resolved.resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.resolved.id, "deepseek-v4-pro");
    }

    #[test]
    fn deepseek_v4_pro_alias_resolves_to_nvidia_nim_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-pro"), Some(ProviderKind::NvidiaNim));

        assert_eq!(resolved.resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-pro");
    }

    #[test]
    fn nvidia_nim_default_uses_catalog_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::NvidiaNim));

        assert_eq!(resolved.resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-pro");
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_nvidia_nim_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-flash"), Some(ProviderKind::NvidiaNim));

        assert_eq!(resolved.resolved.provider, ProviderKind::NvidiaNim);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-flash");
    }

    #[test]
    fn atlascloud_default_uses_namespaced_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-flash");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_atlascloud_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-flash"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-flash");
    }

    #[test]
    fn deepseek_v4_pro_alias_resolves_to_atlascloud_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-pro"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-pro");
    }

    #[test]
    fn atlascloud_provider_hint_passes_through_explicit_model_id() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve(Some("openai/gpt-5.2-chat"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "openai/gpt-5.2-chat");
        assert!(resolved.resolved.supports_tools);
        assert!(resolved.resolved.supports_reasoning);
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn atlascloud_provider_hint_preserves_explicit_model_id_case() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("Qwen/Qwen3-Coder"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "Qwen/Qwen3-Coder");
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn atlascloud_plain_unknown_model_still_uses_provider_default() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("not-in-atlas"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-flash");
        assert!(resolved.used_fallback);
    }

    #[test]
    fn openrouter_default_uses_namespaced_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::Openrouter));

        assert_eq!(resolved.resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.resolved.id, "deepseek/deepseek-v4-pro");
    }

    #[test]
    fn xiaomi_mimo_default_uses_canonical_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::XiaomiMimo));

        assert_eq!(resolved.resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.resolved.id, "mimo-v2.5-pro");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn moonshot_default_and_aliases_use_kimi_k27_code() {
        let registry = ModelRegistry::default();

        for requested in [None, Some("kimi"), Some("kimi-k2.7-code")] {
            let resolved = registry.resolve(requested, Some(ProviderKind::Moonshot));

            assert_eq!(resolved.resolved.provider, ProviderKind::Moonshot);
            assert_eq!(resolved.resolved.id, "kimi-k2.7-code");
            assert!(resolved.resolved.supports_tools);
            assert!(resolved.resolved.supports_reasoning);
        }
    }

    #[test]
    fn moonshot_explicit_kimi_k26_remains_available() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("kimi-k2.6"), Some(ProviderKind::Moonshot));

        assert_eq!(resolved.resolved.provider, ProviderKind::Moonshot);
        assert_eq!(resolved.resolved.id, "kimi-k2.6");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn xiaomi_mimo_tts_aliases_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("tts"), Some(ProviderKind::XiaomiMimo));
        assert_eq!(resolved.resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.resolved.id, "mimo-v2.5-tts");
        assert!(!resolved.resolved.supports_tools);
        assert!(!resolved.resolved.supports_reasoning);

        let resolved = registry.resolve(Some("voice-design"), Some(ProviderKind::XiaomiMimo));
        assert_eq!(resolved.resolved.id, "mimo-v2.5-tts-voicedesign");

        let resolved = registry.resolve(Some("voiceclone"), Some(ProviderKind::XiaomiMimo));
        assert_eq!(resolved.resolved.id, "mimo-v2.5-tts-voiceclone");
    }

    #[test]
    fn xiaomi_mimo_chat_aliases_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();

        let resolved = registry.resolve(Some("omni"), Some(ProviderKind::XiaomiMimo));
        assert_eq!(resolved.resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.resolved.id, "mimo-v2.5");
        assert!(resolved.resolved.supports_tools);
    }

    #[test]
    fn xiaomi_mimo_provider_hint_preserves_custom_model_id() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve(Some("account-custom-mimo"), Some(ProviderKind::XiaomiMimo));

        assert_eq!(resolved.resolved.provider, ProviderKind::XiaomiMimo);
        assert_eq!(resolved.resolved.id, "account-custom-mimo");
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn xiaomi_mimo_provider_hint_does_not_reclassify_openrouter_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(
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
        let resolved = registry.resolve(None, Some(ProviderKind::WanjieArk));

        assert_eq!(resolved.resolved.provider, ProviderKind::WanjieArk);
        assert_eq!(resolved.resolved.id, "deepseek-reasoner");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn novita_default_uses_namespaced_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::Novita));

        assert_eq!(resolved.resolved.provider, ProviderKind::Novita);
        assert_eq!(resolved.resolved.id, "deepseek/deepseek-v4-pro");
    }

    #[test]
    fn fireworks_default_uses_canonical_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::Fireworks));

        assert_eq!(resolved.resolved.provider, ProviderKind::Fireworks);
        assert_eq!(
            resolved.resolved.id,
            "accounts/fireworks/models/deepseek-v4-pro"
        );
    }

    #[test]
    fn siliconflow_default_uses_canonical_pro_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::Siliconflow));

        assert_eq!(resolved.resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Pro");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn arcee_default_uses_direct_trinity_large_thinking_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::Arcee));

        assert_eq!(resolved.resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.resolved.id, "trinity-large-thinking");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn arcee_trinity_alias_resolves_to_direct_large_thinking_not_openrouter() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("trinity"), Some(ProviderKind::Arcee));

        assert_eq!(resolved.resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.resolved.id, "trinity-large-thinking");
        assert!(resolved.resolved.supports_reasoning);
    }

    #[test]
    fn arcee_trinity_mini_remains_explicit_compatibility_model() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("trinity-mini"), Some(ProviderKind::Arcee));

        assert_eq!(resolved.resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.resolved.id, "trinity-mini");
        assert!(!resolved.resolved.supports_reasoning);
    }

    #[test]
    fn arcee_provider_hint_preserves_explicit_future_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("trinity-large-next"), Some(ProviderKind::Arcee));

        assert_eq!(resolved.resolved.provider, ProviderKind::Arcee);
        assert_eq!(resolved.resolved.id, "trinity-large-next");
        assert!(!resolved.resolved.supports_reasoning);
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn deepseek_reasoner_alias_resolves_to_siliconflow_pro_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-reasoner"), Some(ProviderKind::Siliconflow));

        assert_eq!(resolved.resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Pro");
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_siliconflow_flash_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-flash"), Some(ProviderKind::Siliconflow));

        assert_eq!(resolved.resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Flash");
    }

    #[test]
    fn sglang_default_uses_canonical_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::Sglang));

        assert_eq!(resolved.resolved.provider, ProviderKind::Sglang);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Pro");
    }

    #[test]
    fn zai_direct_models_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();

        // GLM-5.2 is now the default direct Z.AI model.
        let default = registry.resolve(None, Some(ProviderKind::Zai));
        assert_eq!(default.resolved.provider, ProviderKind::Zai);
        assert_eq!(default.resolved.id, "GLM-5.2");

        for (alias, expected) in [
            ("GLM-5.1", "GLM-5.1"),
            ("glm-5-1", "GLM-5.1"),
            ("GLM-5.2", "GLM-5.2"),
            ("glm-5.2", "GLM-5.2"),
            ("zai-glm-5-2", "GLM-5.2"),
            ("GLM-5-Turbo", "GLM-5-Turbo"),
            ("glm-5-turbo", "GLM-5-Turbo"),
            ("zai-glm-5-turbo", "GLM-5-Turbo"),
        ] {
            let resolved = registry.resolve(Some(alias), Some(ProviderKind::Zai));

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
            (ProviderKind::Openmodel, "deepseek-v4-flash"),
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
    fn stepfun_and_minimax_direct_models_resolve_when_provider_hinted() {
        let registry = ModelRegistry::default();

        let stepfun = registry.resolve(None, Some(ProviderKind::Stepfun));
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
            let resolved = registry.resolve(Some(alias), Some(ProviderKind::Minimax));

            assert_eq!(resolved.resolved.provider, ProviderKind::Minimax);
            assert_eq!(resolved.resolved.id, expected);
            assert!(!resolved.used_fallback);
            assert!(resolved.resolved.supports_tools);
            assert!(resolved.resolved.supports_reasoning);
        }
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_openrouter_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-flash"), Some(ProviderKind::Openrouter));

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
            let resolved = registry.resolve(Some(alias), Some(ProviderKind::Openrouter));

            assert_eq!(resolved.resolved.provider, ProviderKind::Openrouter);
            assert_eq!(resolved.resolved.id, expected);
            assert!(resolved.resolved.supports_tools);
            assert!(resolved.resolved.supports_reasoning);
        }
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_novita_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-flash"), Some(ProviderKind::Novita));

        assert_eq!(resolved.resolved.provider, ProviderKind::Novita);
        assert_eq!(resolved.resolved.id, "deepseek/deepseek-v4-flash");
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_sglang_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-flash"), Some(ProviderKind::Sglang));

        assert_eq!(resolved.resolved.provider, ProviderKind::Sglang);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Flash");
    }

    #[test]
    fn vllm_default_uses_canonical_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::Vllm));

        assert_eq!(resolved.resolved.provider, ProviderKind::Vllm);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Pro");
    }

    #[test]
    fn ollama_default_uses_small_local_model_id() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(None, Some(ProviderKind::Ollama));

        assert_eq!(resolved.resolved.provider, ProviderKind::Ollama);
        assert_eq!(resolved.resolved.id, "deepseek-coder:1.3b");
        assert!(!resolved.resolved.supports_reasoning);
    }

    #[test]
    fn ollama_requested_model_tag_is_preserved() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("qwen2.5-coder:7b"), Some(ProviderKind::Ollama));

        assert_eq!(resolved.resolved.provider, ProviderKind::Ollama);
        assert_eq!(resolved.resolved.id, "qwen2.5-coder:7b");
        assert!(!resolved.used_fallback);
    }

    #[test]
    fn deepseek_v4_flash_alias_resolves_to_vllm_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-flash"), Some(ProviderKind::Vllm));

        assert_eq!(resolved.resolved.provider, ProviderKind::Vllm);
        assert_eq!(resolved.resolved.id, "deepseek-ai/DeepSeek-V4-Flash");
    }

    #[test]
    fn preserves_requested_model_casing_for_third_party_providers() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("DeepSeek-V4-Pro"), None);

        assert_eq!(resolved.resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.resolved.id, "DeepSeek-V4-Pro");
    }

    #[test]
    fn registry_casing_takes_priority_over_requested_casing_with_provider_hint() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("DeepSeek-V4-Pro"), Some(ProviderKind::Deepseek));

        assert_eq!(resolved.resolved.provider, ProviderKind::Deepseek);
        // Registry's canonical id is used even when user provides different casing
        assert_eq!(resolved.resolved.id, "deepseek-v4-pro");
    }

    #[test]
    fn preserves_requested_model_casing_without_surrounding_whitespace() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("  DeepSeek-V4-Pro  "), None);

        assert_eq!(resolved.resolved.provider, ProviderKind::Deepseek);
        assert_eq!(resolved.resolved.id, "DeepSeek-V4-Pro");
    }

    #[test]
    fn alias_match_does_not_override_requested_casing() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-reasoner"), None);

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
