//! Truthful, session-local provider readiness.
//!
//! Static configuration can prove that credential material exists, but not
//! that an endpoint is reachable or an OAuth token is still entitled to a
//! model. `Ready` is therefore reserved for observed success in this session.

use std::borrow::Cow;

use crate::config::ApiProvider;
use crate::error_taxonomy::{ErrorCategory, ErrorEnvelope};
use codewhale_config::route::{LogicalModelRef, RouteRequest, RouteResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialState {
    MissingKey,
    MissingLogin,
    Saved,
    NoAuth,
    Local,
    Legacy,
}

/// Credential route whose observed health may be reused. A provider can
/// expose more than one auth route (notably xAI and Moonshot), so provider id
/// alone is not a safe cache key: a successful API-key request must not make a
/// newly selected OAuth login appear verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderAuthClass {
    ApiKey,
    OAuth,
    NoAuth,
    Local,
    Legacy,
}

/// Exact route whose observed health may be reused. This deliberately keeps
/// custom provider id, endpoint, model, and auth class together: success on
/// one private endpoint or model entitlement is not evidence for another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderRouteIdentity {
    provider: ApiProvider,
    provider_id: String,
    endpoint: String,
    model: String,
    auth_class: ProviderAuthClass,
}

pub(crate) fn route_identity_for_model(
    config: &crate::config::Config,
    provider: ApiProvider,
    model: &str,
) -> ProviderRouteIdentity {
    let configured = config.provider_config_for(provider);
    let provider_id = if provider == ApiProvider::Custom {
        config
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(provider.as_str())
    } else {
        provider.as_str()
    };
    let endpoint = if provider == config.api_provider() {
        config.deepseek_base_url()
    } else {
        configured
            .and_then(|entry| entry.base_url.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| provider.default_base_url())
            .to_string()
    }
    .trim_end_matches('/')
    .to_string();
    ProviderRouteIdentity {
        provider,
        provider_id: provider_id.to_string(),
        endpoint,
        model: model.trim().to_string(),
        auth_class: auth_class_for_provider(config, provider),
    }
}

pub(crate) fn auth_class_for_provider(
    config: &crate::config::Config,
    provider: ApiProvider,
) -> ProviderAuthClass {
    let auth_mode = config.auth_mode_for_provider(provider);
    if crate::config::auth_mode_disables_api_key(auth_mode.as_deref()) {
        return ProviderAuthClass::NoAuth;
    }
    let official_endpoint = !config.provider_uses_custom_endpoint(provider);
    if provider == ApiProvider::OpenaiCodex && official_endpoint {
        return ProviderAuthClass::OAuth;
    }
    if provider == ApiProvider::Moonshot
        && official_endpoint
        && auth_mode.as_deref().is_some_and(auth_mode_uses_kimi_oauth)
    {
        return ProviderAuthClass::OAuth;
    }
    if provider == ApiProvider::Xai
        && official_endpoint
        && auth_mode
            .as_deref()
            .is_some_and(crate::xai_oauth::auth_mode_uses_xai_oauth)
    {
        return ProviderAuthClass::OAuth;
    }
    match credential_state_for_provider(config, provider) {
        CredentialState::NoAuth => ProviderAuthClass::NoAuth,
        CredentialState::Local => ProviderAuthClass::Local,
        CredentialState::Legacy => ProviderAuthClass::Legacy,
        _ => ProviderAuthClass::ApiKey,
    }
}

pub(crate) fn credential_state_for_provider(
    config: &crate::config::Config,
    provider: ApiProvider,
) -> CredentialState {
    let auth_mode = config.auth_mode_for_provider(provider);
    if crate::config::auth_mode_disables_api_key(auth_mode.as_deref()) {
        return CredentialState::NoAuth;
    }
    let api_key_required = crate::config::auth_mode_requires_api_key(auth_mode.as_deref());
    let official_endpoint = !config.provider_uses_custom_endpoint(provider);

    // A built-in provider can intentionally target a local OpenAI-compatible
    // runtime. That route is keyless unless the operator explicitly declares
    // an API-key auth contract. Classify it before provider-specific hosted
    // branches (including the DeepSeek-CN compatibility alias) so readiness
    // and cache identity describe the effective endpoint, not just the
    // provider enum.
    if provider == config.api_provider()
        && !official_endpoint
        && crate::config::base_url_uses_local_host(&config.deepseek_base_url())
    {
        return if api_key_required {
            if crate::config::has_api_key_for(config, provider) {
                CredentialState::Saved
            } else {
                CredentialState::MissingKey
            }
        } else {
            CredentialState::Local
        };
    }

    // DeepSeek CN is a TUI compatibility alias without a shared
    // `ProviderKind`, but it is still a live route handled by the runtime.
    // Treating it as `Legacy` makes setup claim it cannot run at all.
    if provider == ApiProvider::DeepseekCN {
        return if crate::config::has_api_key_for(config, provider) {
            CredentialState::Saved
        } else {
            CredentialState::MissingKey
        };
    }
    if provider.kind().is_none() {
        return CredentialState::Legacy;
    }
    if provider == ApiProvider::Custom {
        if config.uses_legacy_literal_custom_route() {
            if config
                .base_url
                .as_deref()
                .is_some_and(crate::config::base_url_uses_local_host)
                && !api_key_required
            {
                return CredentialState::Local;
            }
            return if crate::config::has_api_key_for(config, provider) {
                CredentialState::Saved
            } else {
                CredentialState::MissingKey
            };
        }
        let Some(configured) = config.provider_config_for(provider) else {
            return CredentialState::MissingKey;
        };
        let auth_optional = configured
            .base_url
            .as_deref()
            .is_some_and(crate::config::base_url_uses_local_host)
            && !api_key_required;
        if auth_optional {
            return CredentialState::Local;
        }
        let has_auth = (provider == config.api_provider()
            && crate::config::explicit_cli_api_key_override().is_some())
            || configured
                .api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || configured
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .is_some_and(|name| {
                    std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
                });
        return if has_auth {
            CredentialState::Saved
        } else {
            CredentialState::MissingKey
        };
    }
    if provider.is_self_hosted() {
        return if api_key_required {
            if crate::config::has_api_key_for(config, provider) {
                CredentialState::Saved
            } else {
                CredentialState::MissingKey
            }
        } else {
            CredentialState::Local
        };
    }

    let uses_kimi_oauth = provider == ApiProvider::Moonshot
        && official_endpoint
        && auth_mode.as_deref().is_some_and(auth_mode_uses_kimi_oauth);
    if uses_kimi_oauth {
        return if crate::config::kimi_cli_credentials_valid() {
            CredentialState::Saved
        } else {
            CredentialState::MissingLogin
        };
    }
    if provider == ApiProvider::OpenaiCodex && official_endpoint {
        return if crate::config::has_api_key_for(config, provider) {
            CredentialState::Saved
        } else {
            CredentialState::MissingLogin
        };
    }
    let xai_oauth_selected = provider == ApiProvider::Xai
        && official_endpoint
        && auth_mode
            .as_deref()
            .is_some_and(crate::xai_oauth::auth_mode_uses_xai_oauth);
    if xai_oauth_selected {
        return if crate::xai_oauth::credentials_valid() {
            CredentialState::Saved
        } else {
            CredentialState::MissingLogin
        };
    }
    if provider == ApiProvider::Xai && explicit_provider_credential_present(config, provider) {
        return CredentialState::Saved;
    }
    if provider == ApiProvider::Xai {
        return CredentialState::MissingKey;
    }

    if crate::config::has_api_key_for(config, provider) {
        CredentialState::Saved
    } else {
        CredentialState::MissingKey
    }
}

fn explicit_provider_credential_present(
    config: &crate::config::Config,
    provider: ApiProvider,
) -> bool {
    (provider == config.api_provider() && crate::config::explicit_cli_api_key_override().is_some())
        || (!config.provider_uses_custom_endpoint(provider)
            && provider
                .env_vars()
                .iter()
                .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty())))
        || (config.config_credentials_are_bound_to_provider_endpoint(provider)
            && config.provider_config_for(provider).is_some_and(|entry| {
                entry
                    .api_key
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                    || entry
                        .api_key_env
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .is_some_and(|name| {
                            std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
                        })
            }))
}

fn auth_mode_uses_kimi_oauth(mode: &str) -> bool {
    matches!(
        mode.trim()
            .to_ascii_lowercase()
            .replace(['-', ' '], "_")
            .as_str(),
        "kimi" | "kimi_oauth" | "kimi_cli" | "oauth"
    )
}

/// Validate the configured provider/model/endpoint route without making a
/// network request. This is shared by model inventory, `/model`, and Fleet so
/// none of them can mark a route selectable when `/provider` would reject it.
pub(crate) fn route_is_valid_for_model(
    config: &crate::config::Config,
    provider: ApiProvider,
    model: Option<&str>,
) -> bool {
    let compatibility_kind =
        (provider == ApiProvider::DeepseekCN).then_some(codewhale_config::ProviderKind::Deepseek);
    let Some(kind) = provider.kind().or(compatibility_kind) else {
        return true;
    };
    let configured = config.provider_config_for(provider);
    let configured_model = model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            configured
                .and_then(|entry| entry.model.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });
    let active_model = (provider == config.api_provider())
        .then(|| config.default_model())
        .filter(|model| !model.trim().is_empty() && !model.eq_ignore_ascii_case("auto"));
    let request = RouteRequest {
        explicit_provider: Some(kind),
        model_selector: configured_model.or(active_model).map(LogicalModelRef::from),
        saved_provider_model: None,
        base_url_override: if provider == config.api_provider() {
            Some(config.deepseek_base_url())
        } else if provider == ApiProvider::Custom && config.uses_legacy_literal_custom_route() {
            config
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        } else {
            configured
                .and_then(|entry| entry.base_url.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        },
    };
    RouteResolver::new()
        .resolve(&request)
        .is_ok_and(|candidate| candidate.validation.ok)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LastProviderCheck {
    Passed,
    Failed {
        category: ErrorCategory,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResolvedProviderReadiness {
    MissingKey,
    MissingLogin,
    SavedUnchecked,
    NoAuthUnchecked,
    LocalUnchecked,
    Ready,
    SavedLastCheckFailed {
        category: ErrorCategory,
        message: String,
    },
    InvalidRoute,
    Legacy,
}

impl ResolvedProviderReadiness {
    pub(crate) fn label(&self) -> Cow<'static, str> {
        match self {
            Self::MissingKey => Cow::Borrowed("missing key"),
            Self::MissingLogin => Cow::Borrowed("missing login"),
            Self::SavedUnchecked => Cow::Borrowed("key saved · not checked"),
            Self::NoAuthUnchecked => Cow::Borrowed("no auth · not checked"),
            Self::LocalUnchecked => Cow::Borrowed("local · not checked"),
            Self::Ready => Cow::Borrowed("ready"),
            Self::SavedLastCheckFailed { category, .. } => {
                Cow::Owned(format!("last check failed ({category})"))
            }
            Self::InvalidRoute => Cow::Borrowed("invalid route"),
            Self::Legacy => Cow::Borrowed("legacy"),
        }
    }

    pub(crate) fn detail(&self) -> Option<&str> {
        match self {
            Self::SavedLastCheckFailed { message, .. } => Some(message),
            _ => None,
        }
    }

    pub(crate) fn can_attempt(&self) -> bool {
        matches!(
            self,
            Self::SavedUnchecked
                | Self::NoAuthUnchecked
                | Self::LocalUnchecked
                | Self::Ready
                | Self::SavedLastCheckFailed { .. }
        )
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProviderReadinessSnapshot {
    checks: Vec<(ProviderRouteIdentity, LastProviderCheck)>,
}

impl ProviderReadinessSnapshot {
    fn last(&self, identity: &ProviderRouteIdentity) -> Option<&LastProviderCheck> {
        self.checks
            .iter()
            .rev()
            .find_map(|(candidate, check)| (candidate == identity).then_some(check))
    }

    pub(crate) fn record_success(
        &mut self,
        config: &crate::config::Config,
        provider: ApiProvider,
        model: &str,
    ) {
        self.replace(
            route_identity_for_model(config, provider, model),
            LastProviderCheck::Passed,
        );
    }

    pub(crate) fn record_failure(
        &mut self,
        config: &crate::config::Config,
        provider: ApiProvider,
        model: &str,
        envelope: &ErrorEnvelope,
    ) {
        if !provider_owned_failure(envelope) {
            return;
        }
        self.replace(
            route_identity_for_model(config, provider, model),
            LastProviderCheck::Failed {
                category: envelope.category,
                message: sanitize_message(&envelope.message),
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn record_failure_message(
        &mut self,
        config: &crate::config::Config,
        provider: ApiProvider,
        model: &str,
        category: ErrorCategory,
        message: &str,
    ) {
        self.replace(
            route_identity_for_model(config, provider, model),
            LastProviderCheck::Failed {
                category,
                message: sanitize_message(message),
            },
        );
    }

    fn replace(&mut self, identity: ProviderRouteIdentity, check: LastProviderCheck) {
        self.checks.retain(|(candidate, _)| candidate != &identity);
        self.checks.push((identity, check));
    }
}

pub(crate) fn resolve_with_identity(
    identity: &ProviderRouteIdentity,
    credentials: CredentialState,
    route_ok: bool,
    checks: &ProviderReadinessSnapshot,
) -> ResolvedProviderReadiness {
    if !route_ok {
        return ResolvedProviderReadiness::InvalidRoute;
    }
    match credentials {
        CredentialState::Legacy => ResolvedProviderReadiness::Legacy,
        CredentialState::MissingKey => ResolvedProviderReadiness::MissingKey,
        CredentialState::MissingLogin => ResolvedProviderReadiness::MissingLogin,
        CredentialState::Saved | CredentialState::NoAuth | CredentialState::Local => {
            match checks.last(identity) {
                Some(LastProviderCheck::Passed) => ResolvedProviderReadiness::Ready,
                Some(LastProviderCheck::Failed { category, message }) => {
                    ResolvedProviderReadiness::SavedLastCheckFailed {
                        category: *category,
                        message: message.clone(),
                    }
                }
                None if credentials == CredentialState::NoAuth => {
                    ResolvedProviderReadiness::NoAuthUnchecked
                }
                None if credentials == CredentialState::Local => {
                    ResolvedProviderReadiness::LocalUnchecked
                }
                None => ResolvedProviderReadiness::SavedUnchecked,
            }
        }
    }
}

pub(crate) fn resolve_for_model(
    config: &crate::config::Config,
    provider: ApiProvider,
    model: &str,
    checks: &ProviderReadinessSnapshot,
) -> ResolvedProviderReadiness {
    resolve_with_identity(
        &route_identity_for_model(config, provider, model),
        credential_state_for_provider(config, provider),
        route_is_valid_for_model(config, provider, Some(model)),
        checks,
    )
}

fn provider_owned_failure(envelope: &ErrorEnvelope) -> bool {
    matches!(
        envelope.category,
        ErrorCategory::Network
            | ErrorCategory::Authentication
            | ErrorCategory::Authorization
            | ErrorCategory::RateLimit
            | ErrorCategory::Timeout
    )
}

fn sanitize_message(message: &str) -> String {
    crate::utils::truncate_with_ellipsis(message.trim(), 120, "…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_taxonomy::ErrorSeverity;

    fn resolve_test_route(
        config: &crate::config::Config,
        provider: ApiProvider,
        model: &str,
        credentials: CredentialState,
        route_ok: bool,
        checks: &ProviderReadinessSnapshot,
    ) -> ResolvedProviderReadiness {
        resolve_with_identity(
            &route_identity_for_model(config, provider, model),
            credentials,
            route_ok,
            checks,
        )
    }

    #[test]
    fn saved_credentials_are_never_ready_without_observed_success() {
        let config = crate::config::Config::default();
        let checks = ProviderReadinessSnapshot::default();
        assert_eq!(
            resolve_test_route(
                &config,
                ApiProvider::Deepseek,
                "deepseek-v4-pro",
                CredentialState::Saved,
                true,
                &checks,
            ),
            ResolvedProviderReadiness::SavedUnchecked
        );
    }

    #[test]
    fn deepseek_cn_compatibility_alias_uses_real_key_readiness() {
        let _lock = crate::test_support::lock_test_env();
        let _key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let missing = crate::config::Config {
            provider: Some("deepseek-cn".to_string()),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&missing, ApiProvider::DeepseekCN),
            CredentialState::MissingKey
        );

        let configured = crate::config::Config {
            provider: Some("deepseek-cn".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                deepseek_cn: crate::config::ProviderConfig {
                    api_key: Some("deepseek-cn-test-key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&configured, ApiProvider::DeepseekCN),
            CredentialState::Saved
        );
    }

    #[test]
    fn custom_readiness_identity_preserves_case_sensitive_route_parts() {
        let custom = std::collections::HashMap::from([
            (
                "CUSTOM".to_string(),
                crate::config::ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    base_url: Some("https://example.test/TenantA/v1".to_string()),
                    model: Some("Vendor/ModelA".to_string()),
                    api_key: Some("test-key-a".to_string()),
                    ..Default::default()
                },
            ),
            (
                "custom".to_string(),
                crate::config::ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    base_url: Some("https://example.test/tenanta/v1".to_string()),
                    model: Some("vendor/modela".to_string()),
                    api_key: Some("test-key-b".to_string()),
                    ..Default::default()
                },
            ),
        ]);
        let upper = crate::config::Config {
            provider: Some("CUSTOM".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom: custom.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let lower = crate::config::Config {
            provider: Some("custom".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        };
        let upper_identity = route_identity_for_model(&upper, ApiProvider::Custom, "Vendor/ModelA");
        let lower_identity = route_identity_for_model(&lower, ApiProvider::Custom, "vendor/modela");

        assert_ne!(upper_identity, lower_identity);
        assert_eq!(upper_identity.provider_id, "CUSTOM");
        assert_eq!(upper_identity.endpoint, "https://example.test/TenantA/v1");
        assert_eq!(upper_identity.model, "Vendor/ModelA");

        let mut checks = ProviderReadinessSnapshot::default();
        checks.record_success(&upper, ApiProvider::Custom, "Vendor/ModelA");
        assert_eq!(
            resolve_for_model(&lower, ApiProvider::Custom, "vendor/modela", &checks),
            ResolvedProviderReadiness::SavedUnchecked
        );
    }

    #[test]
    fn success_and_provider_failure_replace_session_evidence() {
        let config = crate::config::Config::default();
        let mut checks = ProviderReadinessSnapshot::default();
        checks.record_success(&config, ApiProvider::Zai, "glm-5.2");
        assert_eq!(
            resolve_test_route(
                &config,
                ApiProvider::Zai,
                "glm-5.2",
                CredentialState::Saved,
                true,
                &checks,
            ),
            ResolvedProviderReadiness::Ready
        );

        checks.record_failure(
            &config,
            ApiProvider::Zai,
            "glm-5.2",
            &ErrorEnvelope::new(
                ErrorCategory::Authentication,
                ErrorSeverity::Error,
                false,
                "auth_failed",
                "token rejected",
            ),
        );
        let resolved = resolve_test_route(
            &config,
            ApiProvider::Zai,
            "glm-5.2",
            CredentialState::Saved,
            true,
            &checks,
        );
        assert!(matches!(
            resolved,
            ResolvedProviderReadiness::SavedLastCheckFailed {
                category: ErrorCategory::Authentication,
                ..
            }
        ));
        assert!(resolved.can_attempt());
    }

    #[test]
    fn tool_failures_do_not_poison_provider_health() {
        let config = crate::config::Config::default();
        let mut checks = ProviderReadinessSnapshot::default();
        checks.record_success(&config, ApiProvider::Deepseek, "deepseek-v4-pro");
        checks.record_failure(
            &config,
            ApiProvider::Deepseek,
            "deepseek-v4-pro",
            &ErrorEnvelope::new(
                ErrorCategory::Tool,
                ErrorSeverity::Error,
                false,
                "tool_failed",
                "shell failed",
            ),
        );
        assert!(matches!(
            resolve_test_route(
                &config,
                ApiProvider::Deepseek,
                "deepseek-v4-pro",
                CredentialState::Saved,
                true,
                &checks,
            ),
            ResolvedProviderReadiness::Ready
        ));
    }

    #[test]
    fn route_and_missing_auth_states_dominate_health() {
        let config = crate::config::Config {
            provider: Some("openai-codex".to_string()),
            ..Default::default()
        };
        let mut checks = ProviderReadinessSnapshot::default();
        checks.record_success(&config, ApiProvider::OpenaiCodex, "gpt-5.5");
        assert_eq!(
            resolve_test_route(
                &config,
                ApiProvider::OpenaiCodex,
                "gpt-5.5",
                CredentialState::MissingLogin,
                true,
                &checks
            ),
            ResolvedProviderReadiness::MissingLogin
        );
        assert_eq!(
            resolve_test_route(
                &config,
                ApiProvider::OpenaiCodex,
                "gpt-5.5",
                CredentialState::Saved,
                false,
                &checks
            ),
            ResolvedProviderReadiness::InvalidRoute
        );
    }

    #[test]
    fn api_key_success_does_not_verify_new_xai_oauth_route() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("oauth fixture root");
        let oauth_path = temp.path().join("grok-auth.json");
        std::fs::write(
            &oauth_path,
            serde_json::to_vec(&serde_json::json!({
                "test-scope": {
                    "key": "expired-access-token",
                    "refresh_token": "saved-refresh-token",
                    "expires_at": "2000-01-01T00:00:00Z",
                    "auth_mode": "oidc"
                }
            }))
            .expect("oauth json"),
        )
        .expect("oauth fixture");
        let _oauth_path = crate::test_support::EnvVarGuard::set("GROK_AUTH_PATH", &oauth_path);

        let api_key_config = crate::config::Config {
            provider: Some("xai".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    api_key: Some("xai-test-key".to_string()),
                    auth_mode: Some("api_key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut checks = ProviderReadinessSnapshot::default();
        let api_key_model = api_key_config.default_model();
        checks.record_success(&api_key_config, ApiProvider::Xai, &api_key_model);

        let mut oauth_config = api_key_config;
        oauth_config
            .providers
            .as_mut()
            .expect("providers")
            .xai
            .auth_mode = Some("oauth".to_string());
        assert_eq!(
            credential_state_for_provider(&oauth_config, ApiProvider::Xai),
            CredentialState::Saved,
            "fixture must have structurally valid OAuth material"
        );
        let model = oauth_config.default_model();
        assert_eq!(
            resolve_for_model(&oauth_config, ApiProvider::Xai, &model, &checks),
            ResolvedProviderReadiness::SavedUnchecked,
            "API-key evidence must not cross the auth-class boundary"
        );
    }

    #[test]
    fn observed_success_is_scoped_to_exact_model_endpoint_and_custom_provider() {
        let deepseek = crate::config::Config {
            api_key: Some("deepseek-test-key".to_string()),
            ..Default::default()
        };
        let mut checks = ProviderReadinessSnapshot::default();
        checks.record_success(&deepseek, ApiProvider::Deepseek, "deepseek-v4-pro");
        assert_eq!(
            resolve_for_model(
                &deepseek,
                ApiProvider::Deepseek,
                "deepseek-v4-flash",
                &checks,
            ),
            ResolvedProviderReadiness::SavedUnchecked,
            "one model entitlement must not verify a sibling model"
        );

        let custom_config = |id: &str, endpoint: &str| crate::config::Config {
            provider: Some(id.to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom: std::collections::HashMap::from([(
                    id.to_string(),
                    crate::config::ProviderConfig {
                        kind: Some("openai-compatible".to_string()),
                        base_url: Some(endpoint.to_string()),
                        model: Some("private-coder".to_string()),
                        api_key: Some("custom-test-key".to_string()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let alpha = custom_config("alpha", "https://alpha.example/v1");
        checks.record_success(&alpha, ApiProvider::Custom, "private-coder");

        let beta = custom_config("beta", "https://alpha.example/v1");
        assert_eq!(
            resolve_for_model(&beta, ApiProvider::Custom, "private-coder", &checks),
            ResolvedProviderReadiness::SavedUnchecked,
            "named custom providers must not share observed health"
        );

        let alpha_moved = custom_config("alpha", "https://other.example/v1");
        assert_eq!(
            resolve_for_model(&alpha_moved, ApiProvider::Custom, "private-coder", &checks,),
            ResolvedProviderReadiness::SavedUnchecked,
            "changing endpoints must invalidate observed health"
        );
    }

    #[test]
    fn malformed_oauth_files_are_missing_login_not_ready() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("oauth fixture root");
        let kimi_home = temp.path().join("kimi");
        std::fs::create_dir_all(kimi_home.join("credentials")).expect("kimi credentials dir");
        std::fs::write(kimi_home.join("credentials/kimi-code.json"), "{not-json")
            .expect("malformed kimi fixture");
        let _kimi_home = crate::test_support::EnvVarGuard::set(
            "KIMI_CODE_HOME",
            kimi_home.to_str().expect("utf8 path"),
        );
        let grok_path = temp.path().join("grok-auth.json");
        std::fs::write(&grok_path, "{}").expect("empty grok fixture");
        let _grok_path = crate::test_support::EnvVarGuard::set(
            "GROK_AUTH_PATH",
            grok_path.to_str().expect("utf8 path"),
        );

        let config = crate::config::Config {
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    auth_mode: Some("kimi_oauth".to_string()),
                    ..Default::default()
                },
                xai: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(
            credential_state_for_provider(&config, ApiProvider::Moonshot),
            CredentialState::MissingLogin
        );
        assert_eq!(
            credential_state_for_provider(&config, ApiProvider::Xai),
            CredentialState::MissingLogin
        );

        let api_key_config = crate::config::Config {
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    api_key: Some("explicit-xai-key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&api_key_config, ApiProvider::Xai),
            CredentialState::Saved,
            "an unrelated stale Grok OAuth file must not shadow an explicit xAI API key"
        );
        assert_eq!(
            credential_state_for_provider(&crate::config::Config::default(), ApiProvider::Xai),
            CredentialState::MissingKey,
            "a Grok file is not active until xAI OAuth is selected in config"
        );
        let stale_root_config = crate::config::Config {
            provider: Some("xai".to_string()),
            api_key: Some("legacy-deepseek-root-key".to_string()),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&stale_root_config, ApiProvider::Xai),
            CredentialState::MissingKey,
            "the legacy root DeepSeek key is not an xAI credential"
        );

        let _cli_source = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY_SOURCE", "cli");
        let _cli_key =
            crate::test_support::EnvVarGuard::set("CODEWHALE_CLI_API_KEY", "explicit-cli-key");
        let cli_config = crate::config::Config {
            provider: Some("xai".to_string()),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&cli_config, ApiProvider::Xai),
            CredentialState::Saved,
            "the source-marked CLI override is valid for the active xAI provider"
        );
    }

    #[test]
    fn custom_provider_env_and_local_no_auth_states_match_runtime() {
        let _lock = crate::test_support::lock_test_env();
        let _custom_key = crate::test_support::EnvVarGuard::set("ACME_CUSTOM_KEY", "custom-secret");
        let remote = crate::config::Config {
            provider: Some("acme".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom: std::collections::HashMap::from([(
                    "acme".to_string(),
                    crate::config::ProviderConfig {
                        kind: Some("openai-compatible".to_string()),
                        base_url: Some("https://api.acme.test/v1".to_string()),
                        model: Some("acme-coder".to_string()),
                        api_key_env: Some("ACME_CUSTOM_KEY".to_string()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&remote, ApiProvider::Custom),
            CredentialState::Saved
        );

        let local = crate::config::Config {
            provider: Some("local-acme".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom: std::collections::HashMap::from([(
                    "local-acme".to_string(),
                    crate::config::ProviderConfig {
                        kind: Some("openai-compatible".to_string()),
                        base_url: Some("http://127.0.0.1:8080/v1".to_string()),
                        model: Some("local-model".to_string()),
                        auth_mode: Some("none".to_string()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&local, ApiProvider::Custom),
            CredentialState::NoAuth
        );
        assert_eq!(
            resolve_for_model(
                &local,
                ApiProvider::Custom,
                "local-model",
                &ProviderReadinessSnapshot::default(),
            ),
            ResolvedProviderReadiness::NoAuthUnchecked
        );
    }

    #[test]
    fn no_auth_has_distinct_readiness_and_cache_identity_from_implicit_local() {
        let local = crate::config::Config {
            provider: Some("vllm".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                vllm: crate::config::ProviderConfig {
                    base_url: Some("http://127.0.0.1:8000/v1".to_string()),
                    model: Some("local-model".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut no_auth = local.clone();
        no_auth
            .providers
            .as_mut()
            .expect("providers")
            .vllm
            .auth_mode = Some("no-auth".to_string());

        assert_eq!(
            credential_state_for_provider(&local, ApiProvider::Vllm),
            CredentialState::Local
        );
        assert_eq!(
            credential_state_for_provider(&no_auth, ApiProvider::Vllm),
            CredentialState::NoAuth
        );
        assert_eq!(
            auth_class_for_provider(&local, ApiProvider::Vllm),
            ProviderAuthClass::Local
        );
        assert_eq!(
            auth_class_for_provider(&no_auth, ApiProvider::Vllm),
            ProviderAuthClass::NoAuth
        );

        let mut checks = ProviderReadinessSnapshot::default();
        checks.record_success(&local, ApiProvider::Vllm, "local-model");
        assert_eq!(
            resolve_for_model(&no_auth, ApiProvider::Vllm, "local-model", &checks),
            ResolvedProviderReadiness::NoAuthUnchecked,
            "implicit-local success must not verify an explicitly no-auth route"
        );
        assert!(ResolvedProviderReadiness::NoAuthUnchecked.can_attempt());
        assert_eq!(
            ResolvedProviderReadiness::NoAuthUnchecked.label(),
            "no auth · not checked"
        );
    }

    #[test]
    fn explicit_api_key_mode_on_loopback_requires_a_real_credential() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("isolated credential home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path());
        let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _vllm_key = crate::test_support::EnvVarGuard::remove("VLLM_API_KEY");
        let _cli_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

        let missing = crate::config::Config {
            provider: Some("vllm".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                vllm: crate::config::ProviderConfig {
                    base_url: Some("http://127.0.0.1:8000/v1".to_string()),
                    model: Some("local-model".to_string()),
                    auth_mode: Some("api_key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&missing, ApiProvider::Vllm),
            CredentialState::MissingKey
        );
        assert!(missing.deepseek_api_key().is_err());

        let mut configured = missing.clone();
        configured
            .providers
            .as_mut()
            .expect("providers")
            .vllm
            .api_key = Some("protected-local-key".to_string());
        assert_eq!(
            credential_state_for_provider(&configured, ApiProvider::Vllm),
            CredentialState::Saved
        );
        assert_eq!(
            configured.deepseek_api_key().expect("configured key"),
            "protected-local-key"
        );

        let named_custom = crate::config::Config {
            provider: Some("protected-local".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom: std::collections::HashMap::from([(
                    "protected-local".to_string(),
                    crate::config::ProviderConfig {
                        kind: Some("openai-compatible".to_string()),
                        base_url: Some("http://127.0.0.1:9000/v1".to_string()),
                        model: Some("private-model".to_string()),
                        auth_mode: Some("bearer".to_string()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&named_custom, ApiProvider::Custom),
            CredentialState::MissingKey
        );
        assert!(named_custom.deepseek_api_key().is_err());
    }

    #[test]
    fn provider_auth_metadata_is_not_a_runtime_credential() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("isolated credential home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path());
        let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _openai_key = crate::test_support::EnvVarGuard::remove("OPENAI_API_KEY");
        let _xai_key = crate::test_support::EnvVarGuard::remove("XAI_API_KEY");
        let _cli_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

        let command = crate::config::Config {
            provider: Some("openai".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                openai: crate::config::ProviderConfig {
                    auth: Some(codewhale_config::ProviderAuthSourceToml {
                        source: codewhale_config::AuthSourceKind::Command,
                        command: vec!["secret-tool".to_string(), "lookup".to_string()],
                        timeout_ms: Some(2_000),
                        secret_id: None,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&command, ApiProvider::Openai),
            CredentialState::MissingKey
        );

        let secret = crate::config::Config {
            provider: Some("xai".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    auth: Some(codewhale_config::ProviderAuthSourceToml {
                        source: codewhale_config::AuthSourceKind::Secret,
                        command: Vec::new(),
                        timeout_ms: None,
                        secret_id: Some("codewhale/xai".to_string()),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&secret, ApiProvider::Xai),
            CredentialState::MissingKey
        );
    }

    #[test]
    fn oauth_readiness_is_limited_to_official_endpoints() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("isolated oauth home");
        let missing_grok_auth = temp.path().join("missing-grok-auth.json");
        let _grok_auth =
            crate::test_support::EnvVarGuard::set("GROK_AUTH_PATH", &missing_grok_auth);
        let _xai_key = crate::test_support::EnvVarGuard::remove("XAI_API_KEY");
        let _codex_key = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _legacy_codex_key = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");

        let custom_xai = crate::config::Config {
            provider: Some("xai".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    base_url: Some("https://gateway.example.test/v1".to_string()),
                    auth_mode: Some("oauth".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            auth_class_for_provider(&custom_xai, ApiProvider::Xai),
            ProviderAuthClass::ApiKey
        );
        assert_eq!(
            credential_state_for_provider(&custom_xai, ApiProvider::Xai),
            CredentialState::MissingKey
        );

        let official_xai = crate::config::Config {
            provider: Some("xai".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            auth_class_for_provider(&official_xai, ApiProvider::Xai),
            ProviderAuthClass::OAuth
        );
        assert_eq!(
            credential_state_for_provider(&official_xai, ApiProvider::Xai),
            CredentialState::MissingLogin
        );

        let custom_codex = crate::config::Config {
            provider: Some("openai-codex".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                openai_codex: crate::config::ProviderConfig {
                    base_url: Some("https://gateway.example.test/v1".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            auth_class_for_provider(&custom_codex, ApiProvider::OpenaiCodex),
            ProviderAuthClass::ApiKey
        );
        assert_eq!(
            credential_state_for_provider(&custom_codex, ApiProvider::OpenaiCodex),
            CredentialState::MissingKey
        );
    }

    #[test]
    fn xai_custom_endpoint_does_not_count_ambient_official_key() {
        let _lock = crate::test_support::lock_test_env();
        let _source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");
        let _ambient = crate::test_support::EnvVarGuard::set("XAI_API_KEY", "ambient-xai-key");
        let config = crate::config::Config {
            provider: Some("xai".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    base_url: Some("https://unrelated-gateway.example.test/v1".to_string()),
                    model: Some("private-grok-model".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        assert!(!crate::config::has_api_key_for(&config, ApiProvider::Xai));
        assert_eq!(
            credential_state_for_provider(&config, ApiProvider::Xai),
            CredentialState::MissingKey
        );
    }

    #[test]
    fn active_deepseek_routes_validate_models_against_effective_custom_base_url() {
        let official = crate::config::Config::default();
        assert!(!route_is_valid_for_model(
            &official,
            ApiProvider::Deepseek,
            Some("anthropic/private-model")
        ));

        for provider_name in ["deepseek", "deepseek-cn"] {
            let config = crate::config::Config {
                provider: Some(provider_name.to_string()),
                base_url: Some("https://tenant-gateway.example.test/v1".to_string()),
                default_text_model: Some("anthropic/private-model".to_string()),
                ..Default::default()
            };
            let provider = config.api_provider();
            assert!(matches!(
                provider,
                ApiProvider::Deepseek | ApiProvider::DeepseekCN
            ));
            assert!(route_is_valid_for_model(
                &config,
                provider,
                Some("anthropic/private-model")
            ));
        }
    }

    #[test]
    fn cli_forwarded_deepseek_custom_route_validates_prefixed_model() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("isolated config home");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"api_key = "saved-file-key"
base_url = "https://api.deepseek.com/v1"
default_text_model = "deepseek-chat"
"#,
        )
        .expect("write config");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path());
        let _provider = crate::test_support::EnvVarGuard::set("CODEWHALE_PROVIDER", "deepseek");
        let _legacy_provider = crate::test_support::EnvVarGuard::remove("DEEPSEEK_PROVIDER");
        let _base = crate::test_support::EnvVarGuard::set(
            "CODEWHALE_BASE_URL",
            "https://tenant-gateway.example.test/v1",
        );
        let _legacy_base = crate::test_support::EnvVarGuard::remove("DEEPSEEK_BASE_URL");
        let _model =
            crate::test_support::EnvVarGuard::set("CODEWHALE_MODEL", "anthropic/private-model");
        let _source = crate::test_support::EnvVarGuard::set("DEEPSEEK_API_KEY_SOURCE", "cli");
        let _cli_key =
            crate::test_support::EnvVarGuard::set("CODEWHALE_CLI_API_KEY", "explicit-cli-key");

        let config = crate::config::Config::load(Some(config_path), None).expect("load config");
        assert_eq!(config.default_model(), "anthropic/private-model");
        assert_eq!(
            config.deepseek_api_key().expect("explicit CLI key"),
            "explicit-cli-key"
        );
        assert!(route_is_valid_for_model(
            &config,
            ApiProvider::Deepseek,
            Some("anthropic/private-model")
        ));
    }

    #[test]
    fn builtin_loopback_local_and_api_key_routes_have_distinct_cache_identity() {
        let _lock = crate::test_support::lock_test_env();
        let _openai_key = crate::test_support::EnvVarGuard::remove("OPENAI_API_KEY");
        let _source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");
        let local = crate::config::Config {
            provider: Some("openai".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                openai: crate::config::ProviderConfig {
                    base_url: Some("http://127.0.0.1:8080/v1".to_string()),
                    model: Some("local-model".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut protected = local.clone();
        let protected_route = &mut protected.providers.as_mut().expect("providers").openai;
        protected_route.auth_mode = Some("api_key".to_string());
        protected_route.api_key = Some("protected-local-key".to_string());

        assert_eq!(
            credential_state_for_provider(&local, ApiProvider::Openai),
            CredentialState::Local
        );
        assert_eq!(
            auth_class_for_provider(&local, ApiProvider::Openai),
            ProviderAuthClass::Local
        );
        assert_eq!(
            credential_state_for_provider(&protected, ApiProvider::Openai),
            CredentialState::Saved
        );
        assert_eq!(
            auth_class_for_provider(&protected, ApiProvider::Openai),
            ProviderAuthClass::ApiKey
        );
        assert_ne!(
            route_identity_for_model(&local, ApiProvider::Openai, "local-model"),
            route_identity_for_model(&protected, ApiProvider::Openai, "local-model")
        );

        let mut checks = ProviderReadinessSnapshot::default();
        checks.record_success(&local, ApiProvider::Openai, "local-model");
        assert_eq!(
            resolve_for_model(&protected, ApiProvider::Openai, "local-model", &checks),
            ResolvedProviderReadiness::SavedUnchecked
        );

        let deepseek_cn_local = crate::config::Config {
            provider: Some("deepseek-cn".to_string()),
            base_url: Some("http://127.0.0.1:9090/v1".to_string()),
            default_text_model: Some("local-cn-model".to_string()),
            ..Default::default()
        };
        assert_eq!(
            credential_state_for_provider(&deepseek_cn_local, ApiProvider::DeepseekCN),
            CredentialState::Local
        );
    }
}
