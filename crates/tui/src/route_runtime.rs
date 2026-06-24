use codewhale_config::route::{
    LogicalModelRef, ReadyRouteCandidate, RouteRequest, RouteResolver, WireModelId,
};

use crate::config::{ApiProvider, Config, DEFAULT_NVIDIA_NIM_BASE_URL};

#[derive(Debug, Clone)]
pub(crate) struct ResolvedRuntimeRoute {
    pub(crate) candidate: ReadyRouteCandidate,
    pub(crate) config: Config,
    pub(crate) model: String,
}

pub(crate) fn resolve_route_candidate(
    provider: ApiProvider,
    model_selector: Option<&str>,
    saved_provider_model: Option<&str>,
    base_url_override: Option<String>,
) -> Result<ReadyRouteCandidate, String> {
    let route_request = RouteRequest {
        explicit_provider: provider.kind(),
        model_selector: model_selector.map(|model| LogicalModelRef::from(model.to_string())),
        saved_provider_model: saved_provider_model
            .map(|model| WireModelId::from(model.to_string())),
        base_url_override,
    };
    RouteResolver::new()
        .resolve(&route_request)
        .map_err(|err| err.to_string())
}

pub(crate) fn resolve_runtime_route(
    config: &Config,
    provider: ApiProvider,
    model_selector: Option<&str>,
) -> Result<ResolvedRuntimeRoute, String> {
    let mut route_config = prepared_route_config(config, provider, model_selector);
    let saved_provider_model = route_config
        .provider_config_for(provider)
        .and_then(|provider| provider.model.as_deref());
    let candidate = resolve_route_candidate(
        provider,
        model_selector,
        saved_provider_model,
        Some(route_config.deepseek_base_url()),
    )?;
    let model = candidate.wire_model_id.as_str().to_string();
    route_config.provider_config_for_mut(provider).model = Some(model.clone());

    Ok(ResolvedRuntimeRoute {
        candidate,
        config: route_config,
        model,
    })
}

fn prepared_route_config(
    config: &Config,
    provider: ApiProvider,
    model_selector: Option<&str>,
) -> Config {
    let mut route_config = config.clone();
    route_config.provider = Some(provider.as_str().to_string());
    if matches!(provider, ApiProvider::NvidiaNim)
        && route_config
            .base_url
            .as_deref()
            .map(|base| !base.contains("integrate.api.nvidia.com"))
            .unwrap_or(true)
    {
        route_config.base_url = Some(DEFAULT_NVIDIA_NIM_BASE_URL.to_string());
    }
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && route_config
            .base_url
            .as_deref()
            .map(root_base_url_belongs_to_non_deepseek_provider)
            .unwrap_or(false)
    {
        route_config.base_url = None;
    }
    if let Some(model) = model_selector {
        route_config.provider_config_for_mut(provider).model = Some(model.to_string());
    }
    route_config
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
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_TEXT_MODEL, DEFAULT_ZAI_MODEL, ProviderConfig, ProvidersConfig};

    #[test]
    fn runtime_route_without_model_uses_target_provider_default() {
        let config = Config {
            provider: Some("openrouter".to_string()),
            providers: Some(ProvidersConfig {
                openrouter: ProviderConfig {
                    model: Some("deepseek/deepseek-v4-pro".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let route = resolve_runtime_route(&config, ApiProvider::Zai, None)
            .expect("target provider default should resolve");

        assert_eq!(route.model, DEFAULT_ZAI_MODEL);
        assert_eq!(route.config.provider.as_deref(), Some("zai"));
        assert_eq!(
            route
                .config
                .providers
                .as_ref()
                .and_then(|providers| providers.zai.model.as_deref()),
            Some(DEFAULT_ZAI_MODEL)
        );
        assert_eq!(
            route
                .config
                .providers
                .as_ref()
                .and_then(|providers| providers.openrouter.model.as_deref()),
            Some("deepseek/deepseek-v4-pro")
        );
    }

    #[test]
    fn runtime_route_rejects_foreign_direct_model_before_config_snapshot() {
        let config = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(ProvidersConfig {
                deepseek: ProviderConfig {
                    model: Some(DEFAULT_TEXT_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let err = resolve_runtime_route(&config, ApiProvider::Zai, Some("deepseek-v4-pro"))
            .expect_err("foreign direct-provider model should reject");

        assert!(err.contains("not served by direct provider zai"));
        assert_eq!(config.provider.as_deref(), Some("deepseek"));
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.zai.model.as_deref()),
            None
        );
    }
}
