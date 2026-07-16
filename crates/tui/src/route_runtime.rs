use codewhale_config::route::{
    LogicalModelRef, ReadyRouteCandidate, RouteLimits, RouteRequest, RouteResolver, WireModelId,
};

use crate::client::DeepSeekClient;
use crate::codex_model_cache::{CodexModelCacheFreshness, model_roster};
use crate::config::{ApiProvider, Config, DEFAULT_NVIDIA_NIM_BASE_URL, ProviderIdentity};

#[derive(Clone)]
pub(crate) struct ResolvedRuntimeRoute {
    pub(crate) identity: ProviderIdentity,
    pub(crate) candidate: ReadyRouteCandidate,
    pub(crate) config: Box<Config>,
    pub(crate) model: String,
    preflighted_client: Option<DeepSeekClient>,
}

impl std::fmt::Debug for ResolvedRuntimeRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRuntimeRoute")
            .field("provider_identity", &self.identity.key)
            .field("provider", &self.identity.provider)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// One exact provider route, fully resolved and client-preflighted before a
/// host mutates session/runtime state. The config and client may contain
/// credentials, so diagnostics intentionally expose only non-secret receipt
/// fields.
#[derive(Clone)]
pub(crate) struct ValidatedRuntimeRoute {
    pub(crate) identity: ProviderIdentity,
    pub(crate) candidate: ReadyRouteCandidate,
    pub(crate) config: Box<Config>,
    pub(crate) model: String,
    pub(crate) client: DeepSeekClient,
}

impl std::fmt::Debug for ValidatedRuntimeRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidatedRuntimeRoute")
            .field("provider_identity", &self.identity.key)
            .field("provider", &self.identity.provider)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl ResolvedRuntimeRoute {
    pub(crate) fn preflight(mut self) -> Result<Self, String> {
        if self.preflighted_client.is_none() {
            self.preflighted_client = Some(
                DeepSeekClient::from_candidate(&self.config, &self.candidate).map_err(|err| {
                    format!(
                        "Failed to configure provider route {} / {}: {err}",
                        self.identity.key, self.model
                    )
                })?,
            );
        }
        Ok(self)
    }

    pub(crate) fn validate(mut self) -> Result<ValidatedRuntimeRoute, String> {
        let client = match self.preflighted_client.take() {
            Some(client) => client,
            None => {
                DeepSeekClient::from_candidate(&self.config, &self.candidate).map_err(|err| {
                    format!(
                        "Failed to configure provider route {} / {}: {err}",
                        self.identity.key, self.model
                    )
                })?
            }
        };
        Ok(ValidatedRuntimeRoute {
            identity: self.identity,
            candidate: self.candidate,
            config: self.config,
            model: self.model,
            client,
        })
    }

    pub(crate) fn take_preflighted_client(&mut self) -> Option<DeepSeekClient> {
        self.preflighted_client.take()
    }
}

impl ValidatedRuntimeRoute {
    /// Preserve the preflighted client with the exact resolved route receipt
    /// so the engine does not repeat environment-sensitive client discovery.
    pub(crate) fn into_resolved(self) -> ResolvedRuntimeRoute {
        ResolvedRuntimeRoute {
            identity: self.identity,
            candidate: self.candidate,
            config: self.config,
            model: self.model,
            preflighted_client: Some(self.client),
        }
    }
}

pub(crate) fn resolve_route_candidate(
    provider: ApiProvider,
    model_selector: Option<&str>,
    saved_provider_model: Option<&str>,
    base_url_override: Option<String>,
    context_window_override: Option<u32>,
) -> Result<ReadyRouteCandidate, String> {
    let route_request = RouteRequest {
        explicit_provider: provider.kind(),
        model_selector: model_selector.map(|model| LogicalModelRef::from(model.to_string())),
        saved_provider_model: saved_provider_model
            .map(|model| WireModelId::from(model.to_string())),
        base_url_override,
    };
    let mut candidate = RouteResolver::new()
        .resolve(&route_request)
        .map_err(|err| err.to_string())?;
    apply_context_window_override(&mut candidate.limits, context_window_override);
    if provider == ApiProvider::OpenaiCodex {
        // Models.dev describes the public API offering, not the account-scoped
        // ChatGPT OAuth route. Strip API-only limits, then carry the fresh
        // Codex roster's per-model context into every runtime consumer.
        candidate.limits.input_tokens = None;
        candidate.limits.output_tokens = None;
        if context_window_override.is_none() {
            let roster = model_roster();
            candidate.limits.context_tokens = if roster.freshness == CodexModelCacheFreshness::Fresh
            {
                roster
                    .metadata_for(candidate.wire_model_id.as_str())
                    .and_then(|metadata| metadata.context_window)
                    .map(u64::from)
            } else {
                None
            };
        }
    }
    Ok(candidate)
}

fn apply_context_window_override(limits: &mut RouteLimits, context_window: Option<u32>) {
    if let Some(context_window) = context_window.filter(|window| *window > 0) {
        limits.context_tokens = Some(u64::from(context_window));
    }
}

pub(crate) fn resolve_runtime_route(
    config: &Config,
    provider: ApiProvider,
    model_selector: Option<&str>,
) -> Result<ResolvedRuntimeRoute, String> {
    let identity = if provider == ApiProvider::Custom {
        config.active_provider_identity(provider)?
    } else {
        config
            .resolve_persisted_provider_identity(Some(provider.as_str()), Some(provider.as_str()))?
    };
    resolve_runtime_route_for_identity(config, &identity, model_selector)
}

/// Resolve one persisted/live identity into a scoped runtime config and route
/// candidate. Identity is revalidated against the live registry before any
/// endpoint, model, credential, or client material is read.
pub(crate) fn resolve_runtime_route_for_identity(
    config: &Config,
    identity: &ProviderIdentity,
    model_selector: Option<&str>,
) -> Result<ResolvedRuntimeRoute, String> {
    let identity = config.resolve_persisted_provider_identity(
        Some(identity.provider.as_str()),
        identity.persisted_id(),
    )?;
    let provider = identity.provider;
    let mut route_config = prepared_route_config(config, &identity, model_selector);
    let saved_provider_model = configured_model_for_route(&route_config, provider);
    let candidate = resolve_route_candidate(
        provider,
        model_selector,
        saved_provider_model,
        Some(route_config.deepseek_base_url()),
        route_config.context_window_for_provider_config(provider),
    )?;
    let model = candidate.wire_model_id.as_str().to_string();
    set_model_for_route(&mut route_config, provider, &model);

    Ok(ResolvedRuntimeRoute {
        identity,
        candidate,
        config: Box::new(route_config),
        model,
        preflighted_client: None,
    })
}

fn prepared_route_config(
    config: &Config,
    identity: &ProviderIdentity,
    model_selector: Option<&str>,
) -> Config {
    let mut route_config = config.clone();
    route_config.scope_to_provider_identity(identity);
    let provider = identity.provider;
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
        set_model_for_route(&mut route_config, provider, model);
    }
    route_config
}

fn configured_model_for_route(config: &Config, provider: ApiProvider) -> Option<&str> {
    if provider == ApiProvider::Custom && config.uses_legacy_literal_custom_route() {
        return config.default_text_model.as_deref();
    }
    config
        .provider_config_for(provider)
        .and_then(|provider| provider.model.as_deref())
}

fn set_model_for_route(config: &mut Config, provider: ApiProvider, model: &str) {
    config.set_provider_model_override(provider, Some(model.to_string()));
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
    fn resolved_runtime_route_keeps_large_config_off_async_stacks() {
        assert!(
            std::mem::size_of::<ResolvedRuntimeRoute>() <= 1024,
            "resolved routes cross several async boundaries and must keep Config boxed"
        );
        assert!(
            std::mem::size_of::<ResolvedRuntimeRoute>() < std::mem::size_of::<Config>(),
            "resolved routes must remain smaller than their scoped Config payload"
        );
    }

    #[test]
    fn codex_route_uses_fresh_account_context_and_drops_api_only_limits() {
        let _lock = crate::test_support::lock_test_env();
        let codex_home = tempfile::tempdir().expect("Codex home");
        let _home = crate::test_support::EnvVarGuard::set("CODEX_HOME", codex_home.path());
        std::fs::write(
            codex_home.path().join("models_cache.json"),
            serde_json::to_vec(&serde_json::json!({
                "fetched_at": chrono::Utc::now(),
                "models": [{
                    "slug": crate::config::DEFAULT_OPENAI_CODEX_MODEL,
                    "priority": 1,
                    "context_window": 128000,
                    "supported_reasoning_levels": [{"effort": "high"}]
                }]
            }))
            .expect("serialize cache"),
        )
        .expect("write cache");

        let candidate = resolve_route_candidate(
            ApiProvider::OpenaiCodex,
            Some(crate::config::DEFAULT_OPENAI_CODEX_MODEL),
            None,
            None,
            None,
        )
        .expect("Codex route");

        assert_eq!(candidate.limits.context_tokens, Some(128_000));
        assert_eq!(candidate.limits.input_tokens, None);
        assert_eq!(candidate.limits.output_tokens, None);
        assert_eq!(
            crate::route_budget::route_context_window_tokens(
                ApiProvider::OpenaiCodex,
                crate::config::DEFAULT_OPENAI_CODEX_MODEL,
                Some(candidate.limits),
            ),
            128_000
        );
    }

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

    fn custom_config(base_url: &str, model: &str) -> Config {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "my_thing".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some(base_url.to_string()),
                model: Some(model.to_string()),
                api_key_env: Some("EXAMPLE_API_KEY".to_string()),
                ..Default::default()
            },
        );
        Config {
            provider: Some("my_thing".to_string()),
            providers: Some(ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn custom_provider_resolves_to_custom_endpoint_and_verbatim_model() {
        use codewhale_config::route::RequestProtocol;

        let config = custom_config("https://api.example.com/v1", "vendor/custom-model-v1");
        let route = resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom provider should resolve");

        // Endpoint + model come from the named table; the prefixed model id is
        // preserved verbatim as the wire id (no provider-prefix sniffing).
        assert_eq!(
            route.candidate.endpoint.base_url,
            "https://api.example.com/v1"
        );
        assert_eq!(
            route.candidate.wire_model_id.as_str(),
            "vendor/custom-model-v1"
        );
        assert_eq!(route.model, "vendor/custom-model-v1");
        assert_eq!(route.candidate.protocol, RequestProtocol::ChatCompletions);
        // HTTPS endpoint: route is valid with no insecure-http advisory.
        assert!(route.candidate.validation.ok);
        assert!(route.candidate.validation.messages.is_empty());
        // The selected provider name is preserved (not overwritten with "custom").
        assert_eq!(route.config.provider.as_deref(), Some("my_thing"));
    }

    #[test]
    fn custom_provider_context_window_overrides_unknown_route_limit() {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "dashscope".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://dashscope.example.com/compatible-mode/v1".to_string()),
                model: Some("qwen3.7".to_string()),
                context_window: Some(1_000_000),
                api_key_env: Some("DASHSCOPE_API_KEY".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("dashscope".to_string()),
            providers: Some(ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        let route = resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom route should resolve");

        assert_eq!(route.model, "qwen3.7");
        assert_eq!(route.candidate.limits.context_tokens, Some(1_000_000));
    }

    #[test]
    fn custom_provider_http_non_loopback_fires_insecure_advisory() {
        let config = custom_config("http://gpu.internal.example:8000/v1", "custom-model-v1");
        let route = resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom http provider should resolve");

        // Advisory only: the route still validates (ok == true) but warns that
        // credentials would be sent in plaintext over a non-loopback http URL.
        assert!(route.candidate.validation.ok);
        assert!(
            route
                .candidate
                .validation
                .messages
                .iter()
                .any(|message| message.contains("insecure http")),
            "expected insecure-http advisory, got {:?}",
            route.candidate.validation.messages
        );
        assert_eq!(
            route.candidate.endpoint.base_url,
            "http://gpu.internal.example:8000/v1"
        );
    }
}
