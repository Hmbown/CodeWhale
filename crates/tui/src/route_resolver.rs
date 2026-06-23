//! Atomic provider/model route resolution.
//!
//! This module owns the point where provider state and model state come
//! together. Callers should resolve a route candidate here before mutating UI
//! state, persisting settings, or restarting the engine.

use crate::config::{
    ApiProvider, Config, DEFAULT_NVIDIA_NIM_BASE_URL, ProviderCapability, provider_capability,
    validate_route, wire_model_for_provider,
};

#[derive(Debug, Clone)]
pub(crate) struct RouteCandidate {
    pub(crate) config: Config,
    pub(crate) provider: ApiProvider,
    pub(crate) model: String,
    pub(crate) wire_model: String,
    pub(crate) base_url: String,
    pub(crate) model_ids_passthrough: bool,
    #[allow(dead_code)]
    pub(crate) capability: ProviderCapability,
}

pub(crate) fn resolve_provider_switch(
    current: &Config,
    target: ApiProvider,
    model_override: Option<&str>,
) -> Result<RouteCandidate, String> {
    let mut config = current.clone();
    apply_provider_selection(&mut config, target, model_override);
    resolve_config_route(config)
}

pub(crate) fn apply_provider_selection(
    config: &mut Config,
    target: ApiProvider,
    model_override: Option<&str>,
) {
    config.provider = Some(target.as_str().to_string());
    if matches!(target, ApiProvider::NvidiaNim)
        && config
            .base_url
            .as_deref()
            .map(|base| !base.contains("integrate.api.nvidia.com"))
            .unwrap_or(true)
    {
        config.base_url = Some(DEFAULT_NVIDIA_NIM_BASE_URL.to_string());
    }
    if matches!(target, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && config
            .base_url
            .as_deref()
            .is_some_and(root_base_url_belongs_to_non_deepseek_provider)
    {
        config.base_url = None;
    }
    if let Some(model) = model_override {
        config.provider_config_for_mut(target).model = Some(model.to_string());
    }
}

pub(crate) fn resolve_config_route(config: Config) -> Result<RouteCandidate, String> {
    let provider = config.api_provider();
    let model = config.default_model();
    let model_ids_passthrough = config.model_ids_pass_through();
    if !model_ids_passthrough {
        validate_route(provider, &model)?;
    }
    let wire_model = wire_model_for_provider(provider, &model);
    let base_url = config.deepseek_base_url();
    let capability = provider_capability(provider, &wire_model);
    Ok(RouteCandidate {
        config,
        provider,
        model,
        wire_model,
        base_url,
        model_ids_passthrough,
        capability,
    })
}

fn root_base_url_belongs_to_non_deepseek_provider(base_url: &str) -> bool {
    let lower = base_url.to_ascii_lowercase();
    [
        "integrate.api.nvidia.com",
        "api.openai.com",
        "api.atlascloud.ai",
        "maas-openapi.wanjiedata.com",
        "volces.com",
        "openrouter.ai",
        "xiaomimimo.com",
        "novita.ai",
        "fireworks.ai",
        "siliconflow",
        "arcee.ai",
        "moonshot.ai",
        "api.kimi.com",
        "api.together.xyz",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_TOGETHER_BASE_URL, DEFAULT_TOGETHER_MODEL};

    #[test]
    fn together_default_resolves_as_atomic_route_candidate() {
        let config = Config::default();
        let route = resolve_provider_switch(&config, ApiProvider::Together, None)
            .expect("together default should be a valid hosted DeepSeek route");

        assert_eq!(route.provider, ApiProvider::Together);
        assert_eq!(route.model, DEFAULT_TOGETHER_MODEL);
        assert_eq!(route.wire_model, DEFAULT_TOGETHER_MODEL);
        assert_eq!(route.base_url, DEFAULT_TOGETHER_BASE_URL);
        assert!(!route.model_ids_passthrough);
        assert!(route.capability.thinking_supported);
    }

    #[test]
    fn direct_provider_rejects_foreign_deepseek_route_override() {
        let config = Config::default();

        let err = resolve_provider_switch(&config, ApiProvider::Zai, Some("deepseek-v4-pro"))
            .expect_err("explicit zai/deepseek route override must be rejected");
        assert!(err.contains("DeepSeek model"), "{err}");
        assert!(err.contains("zai"), "{err}");
    }
}
