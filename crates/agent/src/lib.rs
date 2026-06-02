use std::collections::HashMap;

use codewhale_config::ProviderKind;
use serde::{Deserialize, Serialize};

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

const ATLASCLOUD_VALIDATED_MODELS: &[&str] = &[
    "deepseek-ai/DeepSeek-V3-0324",
    "deepseek-ai/deepseek-r1-0528",
    "moonshotai/Kimi-K2-Instruct",
    "Qwen/Qwen3-Coder",
    "Qwen/Qwen3-235B-A22B-Instruct-2507",
    "deepseek-ai/DeepSeek-V3.1",
    "moonshotai/Kimi-K2-Instruct-0905",
    "Qwen/Qwen3-Next-80B-A3B-Instruct",
    "Qwen/Qwen3-Next-80B-A3B-Thinking",
    "Qwen/Qwen3-30B-A3B-Instruct-2507",
    "deepseek-ai/DeepSeek-V3.1-Terminus",
    "deepseek-ai/DeepSeek-V3.2-Exp",
    "zai-org/GLM-4.6",
    "MiniMaxAI/MiniMax-M2",
    "Qwen/Qwen3-VL-235B-A22B-Instruct",
    "moonshotai/Kimi-K2-Thinking",
    "google/gemini-2.5-flash",
    "google/gemini-2.5-flash-lite",
    "openai/gpt-5.1",
    "openai/gpt-5.1-chat",
    "openai/gpt-4o",
    "openai/gpt-4o-mini",
    "openai/gpt-4.1",
    "openai/gpt-4.1-mini",
    "openai/gpt-4.1-nano",
    "openai/o1",
    "openai/o3",
    "openai/o3-mini",
    "openai/o4-mini",
    "anthropic/claude-sonnet-4.5-20250929",
    "deepseek-ai/deepseek-v3.2",
    "openai/gpt-5",
    "openai/gpt-5-chat",
    "openai/gpt-5-mini",
    "openai/gpt-5-nano",
    "openai/gpt-5.2",
    "openai/gpt-5.2-chat",
    "google/gemini-2.5-pro",
    "anthropic/claude-opus-4.5-20251101",
    "google/gemini-3-flash-preview",
    "zai-org/glm-4.7",
    "minimaxai/minimax-m2.1",
    "google/gemini-2.0-flash",
    "qwen/qwen3-8b",
    "qwen/qwen3-235b-a22b-thinking-2507",
    "qwen/qwen3-vl-235b-a22b-thinking",
    "qwen/qwen3-30b-a3b",
    "qwen/qwen3-30b-a3b-thinking-2507",
    "deepseek-ai/deepseek-ocr",
    "xai/grok-4-0709",
];

fn atlascloud_model(id: &str, aliases: &[&str]) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        provider: ProviderKind::Atlascloud,
        aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
        supports_tools: true,
        supports_reasoning: true,
    }
}

fn atlascloud_models() -> Vec<ModelInfo> {
    let mut models = vec![
        atlascloud_model(
            "deepseek-ai/deepseek-v4-flash",
            &["deepseek-v4-flash", "atlascloud-deepseek-v4-flash"],
        ),
        atlascloud_model(
            "deepseek-ai/deepseek-v4-pro",
            &["deepseek-v4-pro", "atlascloud-deepseek-v4-pro"],
        ),
    ];
    models.extend(
        ATLASCLOUD_VALIDATED_MODELS
            .iter()
            .map(|model_id| atlascloud_model(model_id, &[])),
    );
    models
}

/// Creates a registry pre-populated with all built-in models and their aliases.
impl Default for ModelRegistry {
    fn default() -> Self {
        let mut models = vec![
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
                id: "qwen/qwen3.6-27b".to_string(),
                provider: ProviderKind::Openrouter,
                aliases: vec!["qwen3.6-27b".to_string(), "qwen-3.6-27b".to_string()],
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
                aliases: vec!["mimo".to_string()],
                supports_tools: true,
                supports_reasoning: true,
            },
            ModelInfo {
                id: "mimo-v2.5".to_string(),
                provider: ProviderKind::XiaomiMimo,
                aliases: vec!["xiaomi-mimo-v2.5".to_string()],
                supports_tools: true,
                supports_reasoning: true,
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
                id: "kimi-k2.6".to_string(),
                provider: ProviderKind::Moonshot,
                aliases: vec![
                    "kimi".to_string(),
                    "kimi-k2".to_string(),
                    "moonshot-kimi-k2.6".to_string(),
                ],
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
        ];
        models.splice(6..6, atlascloud_models());
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
    fn atlascloud_validated_model_resolves_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved =
            registry.resolve(Some("openai/gpt-5.2-chat"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "openai/gpt-5.2-chat");
    }

    #[test]
    fn atlascloud_registry_includes_validated_model_pool() {
        let registry = ModelRegistry::default();
        let atlas_count = registry
            .list()
            .into_iter()
            .filter(|model| model.provider == ProviderKind::Atlascloud)
            .count();

        assert!(atlas_count >= ATLASCLOUD_VALIDATED_MODELS.len() + 2);
    }

    #[test]
    fn deepseek_v4_pro_alias_resolves_to_atlascloud_when_provider_hinted() {
        let registry = ModelRegistry::default();
        let resolved = registry.resolve(Some("deepseek-v4-pro"), Some(ProviderKind::Atlascloud));

        assert_eq!(resolved.resolved.provider, ProviderKind::Atlascloud);
        assert_eq!(resolved.resolved.id, "deepseek-ai/deepseek-v4-pro");
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
            ("qwen3.6-35b-a3b", "qwen/qwen3.6-35b-a3b"),
            ("gemma-4-31b-it", "google/gemma-4-31b-it"),
            ("glm-5.1", "z-ai/glm-5.1"),
            ("minimax-m3", "minimax/minimax-m3"),
            ("openrouter-mimo-v2.5-pro", "xiaomi/mimo-v2.5-pro"),
            ("openrouter-kimi-k2.6", "moonshotai/kimi-k2.6"),
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
}
