//! Provider/model inventory for routing policy.
//!
//! This is the high-level "what can this user actually run?" object. Auto
//! routing, fleet workers, and sub-agent policy should consume this shape
//! instead of guessing model strings from global defaults.

use serde::Serialize;

use crate::config::{
    ApiProvider, Config, has_api_key_for, normalize_model_name_for_provider, provider_capability,
};
use crate::provider_lake::{all_catalog_models_for_provider, models_for_provider};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModelAuthSource {
    Config,
    Command,
    Env,
    OAuthCli,
    Secret,
    KeylessLocal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ModelRouteCandidate {
    pub(crate) provider: ApiProvider,
    pub(crate) provider_name: &'static str,
    pub(crate) provider_display_name: &'static str,
    pub(crate) model: String,
    pub(crate) context_window: u32,
    pub(crate) max_output: u32,
    pub(crate) thinking_supported: bool,
    pub(crate) cache_telemetry_supported: bool,
    pub(crate) auth_source: ModelAuthSource,
    pub(crate) default_for_provider: bool,
    pub(crate) tags: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ModelInventory {
    pub(crate) active_provider: ApiProvider,
    pub(crate) router_provider: ApiProvider,
    pub(crate) router_model: &'static str,
    pub(crate) router_available: bool,
    pub(crate) candidates: Vec<ModelRouteCandidate>,
}

impl ModelInventory {
    pub(crate) fn from_config(config: &Config) -> Self {
        let active_provider = config.api_provider();
        let mut candidates = Vec::new();

        for provider in ApiProvider::all().iter().copied() {
            let Some(auth_source) = auth_source_for_provider(config, provider) else {
                continue;
            };

            let default_model = provider_default_model(config, provider);
            let mut models = Vec::<String>::new();
            if let Some(model) = configured_model_for_provider(config, provider) {
                push_model(&mut models, provider, &model);
            }
            if provider == active_provider {
                let active_model = config.default_model();
                if !active_model.trim().eq_ignore_ascii_case("auto") {
                    push_model(&mut models, provider, &active_model);
                }
            }
            for model in models_for_provider(config, active_provider, provider) {
                push_model(&mut models, provider, &model);
            }
            if models.is_empty() {
                push_model(&mut models, provider, &default_model);
            }

            for model in models {
                let capability = provider_capability(provider, &model);
                let mut tags = Vec::new();
                if capability.context_window >= 1_000_000 {
                    tags.push("long_context");
                }
                if capability.thinking_supported {
                    tags.push("thinking");
                }
                if matches!(
                    provider,
                    ApiProvider::Ollama | ApiProvider::Sglang | ApiProvider::Vllm
                ) {
                    tags.push("local");
                }
                if model.eq_ignore_ascii_case(&default_model) {
                    tags.push("default");
                }

                candidates.push(ModelRouteCandidate {
                    provider,
                    provider_name: provider.as_str(),
                    provider_display_name: provider.display_name(),
                    default_for_provider: model.eq_ignore_ascii_case(&default_model),
                    model,
                    context_window: capability.context_window,
                    max_output: capability.max_output,
                    thinking_supported: capability.thinking_supported,
                    cache_telemetry_supported: capability.cache_telemetry_supported,
                    auth_source: auth_source.clone(),
                    tags,
                });
            }
        }

        Self {
            active_provider,
            router_provider: ApiProvider::Deepseek,
            router_model: "deepseek-v4-flash",
            router_available: has_api_key_for(config, ApiProvider::Deepseek),
            candidates,
        }
    }

    pub(crate) fn candidate(
        &self,
        provider: ApiProvider,
        model: &str,
    ) -> Option<&ModelRouteCandidate> {
        self.candidates.iter().find(|candidate| {
            candidate.provider == provider && candidate.model.eq_ignore_ascii_case(model.trim())
        })
    }

    pub(crate) fn active_default(&self) -> Option<&ModelRouteCandidate> {
        self.candidates
            .iter()
            .find(|candidate| {
                candidate.provider == self.active_provider && candidate.default_for_provider
            })
            .or_else(|| {
                self.candidates
                    .iter()
                    .find(|candidate| candidate.provider == self.active_provider)
            })
            .or_else(|| self.candidates.first())
    }

    pub(crate) fn router_context_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

fn push_model(models: &mut Vec<String>, provider: ApiProvider, model: &str) {
    let Some(model) = normalize_model_name_for_provider(provider, model)
        .or_else(|| crate::config::normalize_custom_model_id(model))
    else {
        return;
    };
    if !models
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&model))
    {
        models.push(model);
    }
}

fn configured_model_for_provider(config: &Config, provider: ApiProvider) -> Option<String> {
    config
        .provider_config_for(provider)
        .and_then(|entry| entry.model.clone())
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
}

fn provider_default_model(config: &Config, provider: ApiProvider) -> String {
    if provider == config.api_provider() {
        let model = config.default_model();
        if !model.trim().eq_ignore_ascii_case("auto") {
            return model;
        }
    }
    all_catalog_models_for_provider(provider)
        .first()
        .map(|model| model.as_str())
        .unwrap_or(match provider {
            ApiProvider::Ollama => crate::config::DEFAULT_OLLAMA_MODEL,
            ApiProvider::Sglang => crate::config::DEFAULT_SGLANG_MODEL,
            ApiProvider::Vllm => crate::config::DEFAULT_VLLM_MODEL,
            _ => crate::config::DEFAULT_TEXT_MODEL,
        })
        .to_string()
}

fn auth_source_for_provider(config: &Config, provider: ApiProvider) -> Option<ModelAuthSource> {
    if matches!(
        provider,
        ApiProvider::Ollama | ApiProvider::Sglang | ApiProvider::Vllm
    ) {
        return Some(ModelAuthSource::KeylessLocal);
    }
    if env_has_key_for(provider) {
        return Some(ModelAuthSource::Env);
    }
    if let Some(auth) = config
        .provider_config_for(provider)
        .and_then(|entry| entry.auth.as_ref())
    {
        return match auth.source {
            codewhale_config::AuthSourceKind::Command => Some(ModelAuthSource::Command),
            codewhale_config::AuthSourceKind::Secret => Some(ModelAuthSource::Secret),
        };
    }
    if provider_uses_oauth_cli(config, provider) && has_api_key_for(config, provider) {
        return Some(ModelAuthSource::OAuthCli);
    }
    has_api_key_for(config, provider).then_some(ModelAuthSource::Config)
}

fn provider_uses_oauth_cli(config: &Config, provider: ApiProvider) -> bool {
    match provider {
        ApiProvider::OpenaiCodex => true,
        ApiProvider::Moonshot => config
            .provider_config_for(provider)
            .and_then(|entry| entry.auth_mode.as_deref())
            .is_some_and(|mode| {
                let mode = mode.trim().to_ascii_lowercase().replace('-', "_");
                matches!(mode.as_str(), "kimi" | "kimi_oauth" | "kimi_cli" | "oauth")
            }),
        _ => false,
    }
}

fn env_has_key_for(provider: ApiProvider) -> bool {
    env_keys_for_provider(provider)
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()))
}

fn env_keys_for_provider(provider: ApiProvider) -> &'static [&'static str] {
    provider.env_vars()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_env_keys_follow_provider_metadata() {
        for provider in ApiProvider::all() {
            assert_eq!(env_keys_for_provider(*provider), provider.env_vars());
        }
    }

    #[test]
    fn inventory_includes_only_usable_authenticated_providers() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY", "ds-key");
        let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let _minimax = crate::test_support::EnvVarGuard::remove("MINIMAX_API_KEY");
        let config = Config {
            provider: Some("zai".to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..Default::default()
        };

        let inventory = ModelInventory::from_config(&config);

        assert!(inventory.router_available);
        assert!(
            inventory
                .candidate(ApiProvider::Zai, crate::config::ZAI_GLM_5_2_MODEL)
                .is_some()
        );
        assert!(
            inventory
                .candidates
                .iter()
                .all(|candidate| candidate.provider != ApiProvider::Minimax)
        );
    }

    #[test]
    fn inventory_marks_local_providers_keyless() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let config = Config::default();

        let inventory = ModelInventory::from_config(&config);

        assert!(
            inventory
                .candidates
                .iter()
                .any(|candidate| candidate.provider == ApiProvider::Ollama
                    && candidate.auth_source == ModelAuthSource::KeylessLocal)
        );
    }

    #[test]
    fn inventory_reports_command_auth_without_secret_value() {
        let _env_lock = crate::test_support::lock_test_env();
        let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _openai = crate::test_support::EnvVarGuard::remove("OPENAI_API_KEY");
        let mut providers = crate::config::ProvidersConfig::default();
        providers.openai.auth = Some(codewhale_config::ProviderAuthSourceToml {
            source: codewhale_config::AuthSourceKind::Command,
            command: vec!["secret-tool".to_string(), "lookup".to_string()],
            timeout_ms: Some(2000),
            secret_id: None,
        });
        let config = Config {
            provider: Some("openai".to_string()),
            providers: Some(providers),
            ..Default::default()
        };

        let inventory = ModelInventory::from_config(&config);
        let candidate = inventory
            .candidates
            .iter()
            .find(|candidate| candidate.provider == ApiProvider::Openai)
            .expect("openai candidate");

        assert_eq!(candidate.auth_source, ModelAuthSource::Command);
        let json = inventory.router_context_json();
        assert!(json.contains(r#""auth_source":"command""#));
        assert!(!json.contains("secret-tool"));
        assert!(!json.contains("lookup"));
    }
}
