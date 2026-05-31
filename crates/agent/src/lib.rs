use std::collections::HashMap;

use codewhale_config::ProviderKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: ProviderKind,
    pub aliases: Vec<String>,
    pub supports_tools: bool,
    pub supports_reasoning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResolution {
    pub requested: Option<String>,
    pub resolved: ModelInfo,
    pub used_fallback: bool,
    pub fallback_chain: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ModelRegistry {
    models: Vec<ModelInfo>,
    alias_map: HashMap<String, usize>,
}

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
        Self::new(models)
    }
}

impl ModelRegistry {
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

    #[must_use]
    pub fn list(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

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
