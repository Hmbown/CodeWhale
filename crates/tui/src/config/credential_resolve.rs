//! The one place provider credential precedence is decided.
//!
//! Ported from pi-mono `packages/ai/src/auth/resolve.ts` (MIT, Copyright (c)
//! 2025 Mario Zechner; full notice in `crate::credentials`). The idea taken is
//! pi's: a single resolver, one precedence rule stated in a doc comment beside
//! it, and a result that names the place it resolved from. The walk itself is
//! CodeWhale's — it is the former body of `has_api_key_for`, moved here
//! unchanged in order so no existing decision changes, with a
//! [`CredentialSource`] attached to each outcome.
//!
//! # Precedence rule
//!
//! **A stored credential owns the provider: ambient/env is consulted only when
//! nothing is stored. No silent env fallback after a failed refresh.**
//!
//! CodeWhale's order below is that rule instantiated over the stores it
//! actually has. Reading top to bottom:
//!
//! 1. `auth_mode = "none"` — the route sends no credential at all.
//! 2. An explicit `--api-key` on the active, non-OAuth provider.
//! 3. `[providers.<name>] api_key_env` — a credential the route *names*.
//! 4. An ambient provider environment variable (official endpoints only).
//! 5. Provider-owned login state: an explicitly consented external CLI
//!    credential file (Codex, DeepSeek Harness, Antigravity) or CodeWhale's own
//!    xAI OAuth storage.
//! 6. A keyless self-hosted / loopback route.
//! 7. `[providers.<name>] api_key` in the config file.
//! 8. CodeWhale's durable secret store.
//! 9. The root `api_key` compatibility slot.
//! 10. The user-global `~/.codewhale/config.toml`.
//!
//! Two departures from pi are deliberate and load-bearing here:
//!
//! * Ambient env outranks the secret store for a *named* binding (step 3) and
//!   for official-endpoint provider variables (step 4). That is CodeWhale's
//!   existing, documented behavior and users depend on it; changing it is not
//!   in this lane's scope. It is stated here so it is at least *visible*.
//! * External CLI credential files are only ever consulted through
//!   [`Config::external_credential_read_grant`], which enforces the read-only
//!   consent model (exact path, explicit consent, never refreshed, never
//!   rewritten). This resolver adds no new way to reach them.
//!
//! # Redaction
//!
//! This module never returns, logs, or renders secret material. It returns
//! only a [`CredentialSource`] label. Every probe that needs a value calls an
//! existing helper and discards the value with `.is_some()`.

use super::*;
use crate::credentials::{
    AuthContext, CredentialProbe, CredentialResolution, CredentialSource,
    context::ProcessAuthContext,
};

/// Resolve which place holds a credential for `provider`, using the real
/// process environment.
pub(crate) fn resolve_credential_source(
    config: &Config,
    provider: ApiProvider,
) -> CredentialResolution {
    resolve_credential_source_with(config, provider, &ProcessAuthContext)
}

/// Resolve with an injected [`AuthContext`].
///
/// Only the ambient reads this function performs *itself* go through `ctx`.
/// The provider-specific helpers it delegates to (secret store, external
/// grants, xAI OAuth) still read the real environment and filesystem; making
/// those injectable means threading a context through config.rs and is not in
/// this lane.
pub(crate) fn resolve_credential_source_with(
    config: &Config,
    provider: ApiProvider,
    ctx: &dyn AuthContext,
) -> CredentialResolution {
    let mut probed: Vec<CredentialProbe> = Vec::new();

    let auth_mode = config.auth_mode_for_provider(provider);
    if auth_mode_disables_api_key(auth_mode.as_deref()) {
        return CredentialResolution::found(CredentialSource::AuthModeNone);
    }

    if provider == config.api_provider()
        && !provider_uses_oauth_credentials(config, provider)
        && explicit_cli_api_key_override().is_some()
    {
        return CredentialResolution::found(CredentialSource::CliOverride);
    }

    if let Some(var) = bound_provider_api_key_env_name(config, provider) {
        if provider_config_env_api_key(config, provider).is_some() {
            return CredentialResolution::found(CredentialSource::ProviderConfigEnv { var });
        }
        probed.push(CredentialProbe::with_fix(
            format!("env {var} (bound by api_key_env)"),
            format!("export {var}=<key>"),
        ));
    }

    let skip_secret_store = config.should_skip_secret_store_for_provider(provider);
    if !skip_secret_store {
        if let Some(var) = provider
            .env_vars()
            .iter()
            .find(|var| ctx.env(var).is_some())
        {
            return CredentialResolution::found(CredentialSource::AmbientEnv {
                var: (*var).to_string(),
            });
        }
        if let Some(var) = provider.env_vars().first() {
            probed.push(CredentialProbe::with_fix(
                format!("env {}", provider.env_vars_label()),
                format!("export {var}=<key>"),
            ));
        }
    }

    if provider == ApiProvider::Moonshot && provider_uses_oauth_credentials(config, provider) {
        // Kimi CLI credentials are never imported; the route needs its own key.
        probed.push(CredentialProbe::with_fix(
            "Kimi CLI credentials (never imported)",
            "codewhale auth set --provider moonshot",
        ));
        return CredentialResolution::missing(probed);
    }
    if provider == ApiProvider::OpenaiCodex && !config.provider_uses_custom_endpoint(provider) {
        // Token env overrides are checked above. An external Codex login is
        // considered only after exact read-only consent has been validated.
        let path = crate::oauth::auth_file_path();
        let granted = config
            .external_credential_read_grant(
                provider,
                codewhale_config::ExternalCredentialSource::CodexCli,
                &path,
            )
            .is_ok_and(|grant| crate::oauth::stored_credentials_present(&grant));
        return if granted {
            CredentialResolution::found(CredentialSource::ExternalGrant {
                cli: "Codex CLI".to_string(),
                path: path.display().to_string(),
            })
        } else {
            probed.push(external_grant_probe(
                "Codex CLI",
                &path,
                "codewhale auth external-consent --provider openai-codex --mode read-only",
                ctx,
            ));
            CredentialResolution::missing(probed)
        };
    }
    if provider == ApiProvider::Xai
        && !config.provider_uses_custom_endpoint(provider)
        && crate::xai_oauth::credentials_present(config)
    {
        // xAI supports both API keys and OAuth. A Grok-compatible token file is
        // sufficient, but its absence must fall through to the ordinary API-key
        // checks below instead of masking a configured key.
        return CredentialResolution::found(CredentialSource::OAuth {
            flow: "xAI".to_string(),
        });
    }
    if provider == ApiProvider::Antigravity && !config.provider_uses_custom_endpoint(provider) {
        let path = codewhale_config::default_agy_credentials_path();
        if config
            .external_credential_read_grant(
                provider,
                codewhale_config::ExternalCredentialSource::AgyCli,
                &path,
            )
            .is_ok_and(|grant| {
                crate::agy_credentials::antigravity_oauth_token_from_grant(&grant)
                    .ok()
                    .flatten()
                    .is_some()
            })
        {
            return CredentialResolution::found(CredentialSource::ExternalGrant {
                cli: "Antigravity CLI".to_string(),
                path: path.display().to_string(),
            });
        }
        probed.push(external_grant_probe(
            "Antigravity CLI",
            &path,
            "codewhale auth external-consent --provider antigravity --mode read-only",
            ctx,
        ));
    }
    if matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekAnthropic
    ) && !config.provider_uses_custom_endpoint(provider)
    {
        let path = codewhale_config::default_dsh_credentials_path();
        if config
            .external_credential_read_grant(
                provider,
                codewhale_config::ExternalCredentialSource::DshCli,
                &path,
            )
            .is_ok_and(|grant| {
                crate::dsh_credentials::deepseek_api_key_from_grant(&grant)
                    .ok()
                    .flatten()
                    .is_some()
            })
        {
            return CredentialResolution::found(CredentialSource::ExternalGrant {
                cli: "DeepSeek Harness".to_string(),
                path: path.display().to_string(),
            });
        }
        probed.push(external_grant_probe(
            "DeepSeek Harness",
            &path,
            "codewhale auth external-consent --provider deepseek --mode read-only",
            ctx,
        ));
    }

    if !auth_mode_requires_api_key(auth_mode.as_deref())
        && (provider_route_is_keyless_self_hosted(provider, &config.base_url_for_route(provider))
            || (provider == config.api_provider()
                && base_url_uses_local_host(&config.deepseek_base_url())))
    {
        return CredentialResolution::found(CredentialSource::KeylessRoute {
            base_url: config.base_url_for_route(provider),
        });
    }

    if config.config_credentials_are_bound_to_provider_endpoint(provider) {
        if config
            .provider_config_string_with_runtime_fallback(provider, |entry| entry.api_key.clone())
            .is_some_and(|key| {
                classify_config_api_key_value(&key) == ConfigApiKeyValueKind::Literal
            })
        {
            return CredentialResolution::found(CredentialSource::ProviderConfigApiKey {
                table: provider_config_table_name(provider)
                    .unwrap_or_else(|_| format!("providers.{}", provider.as_str())),
            });
        }
        if let Ok(table) = provider_config_table_name(provider) {
            probed.push(CredentialProbe::with_fix(
                format!("[{table}] api_key"),
                format!("add api_key to [{table}] in ~/.codewhale/config.toml"),
            ));
        }
    }
    // Probe the active provider, plus any provider whose persisted
    // `[providers.<name>]` table carries the marker the secret-store save
    // path itself writes (an api-key auth mode with no config literal). A
    // configured-but-inactive provider must not render as unconfigured just
    // because the operator switched providers after saving its key (#5033).
    // Shared-slot families (one account, several provider variants — e.g.
    // Model Studio Token/Coding Plan × OpenAI/Anthropic dialects) honor the
    // marker written by ANY sibling variant, since the save path stores one
    // key under the family's canonical slot. The probe stays bounded to
    // explicitly configured providers, and the non-active case is strictly
    // read-only so rendering the catalog never migrates a legacy store or
    // opens a write-capable backend.
    if !skip_secret_store {
        let slot = provider_secret_store_slot(provider).to_string();
        if provider == config.api_provider() {
            if provider_secret_store_api_key(config, provider).is_some() {
                return CredentialResolution::found(CredentialSource::SecretStore { slot });
            }
            probed.push(secret_store_probe(&slot, provider));
        } else if secret_slot_save_marker_on_shared_slot(config, provider) {
            if provider_secret_store_api_key_with_mode(config, provider, true).is_some() {
                return CredentialResolution::found(CredentialSource::SecretStore { slot });
            }
            probed.push(secret_store_probe(&slot, provider));
        } else {
            // #5033's marker gate: without a `[providers.<name>]` api-key
            // auth-mode marker the store is not read at all for an inactive
            // provider. Say so, because the row is otherwise indistinguishable
            // from a genuinely empty slot — and the request path *would* read
            // it once this provider became active.
            probed.push(CredentialProbe::with_fix(
                format!(
                    "secret store \"{slot}\" (not read: inactive provider, no api-key marker)"
                ),
                format!(
                    "codewhale auth set --provider {} writes the marker that makes this slot readable while inactive",
                    provider.as_str()
                ),
            ));
        }
    }

    if (matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        || (provider == ApiProvider::Custom && config.uses_legacy_literal_custom_route()))
        && config.config_credentials_are_bound_to_provider_endpoint(provider)
        && config
            .api_key
            .as_ref()
            .is_some_and(|key| classify_config_api_key_value(key) == ConfigApiKeyValueKind::Literal)
    {
        return CredentialResolution::found(CredentialSource::RootConfigApiKey);
    }

    // Last resort: the user-global config file. A key saved there must not
    // disappear just because this process loaded a workspace config.
    if user_global_config_api_key(provider).is_some() {
        return CredentialResolution::found(CredentialSource::UserGlobalConfig);
    }
    probed.push(CredentialProbe::with_fix(
        "~/.codewhale/config.toml",
        format!("codewhale auth set --provider {}", provider.as_str()),
    ));

    CredentialResolution::missing(probed)
}

fn secret_store_probe(slot: &str, provider: ApiProvider) -> CredentialProbe {
    CredentialProbe::with_fix(
        format!("secret store \"{slot}\""),
        format!("codewhale auth set --provider {}", provider.as_str()),
    )
}

fn external_grant_probe(
    cli: &str,
    path: &std::path::Path,
    consent_command: &str,
    ctx: &dyn AuthContext,
) -> CredentialProbe {
    let exists = ctx.file_exists(path);
    let place = if exists {
        format!(
            "{cli} credentials at {} (present, not consented)",
            path.display()
        )
    } else {
        format!("{cli} credentials at {} (absent)", path.display())
    };
    if exists {
        CredentialProbe::with_fix(place, consent_command.to_string())
    } else {
        CredentialProbe::new(place)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::context::MapAuthContext;
    use crate::test_support::{EnvVarGuard, lock_test_env};

    fn deepseek_config() -> Config {
        Config {
            provider: Some("deepseek".to_string()),
            ..Config::default()
        }
    }

    /// The precedence rule has to be enforced somewhere a test can see it.
    #[test]
    fn a_named_env_binding_resolves_and_names_itself() {
        let _lock = lock_test_env();
        let _key = EnvVarGuard::set("CW_TEST_BOUND_KEY", "bound-value");
        let config = Config {
            provider: Some("openrouter".to_string()),
            providers: Some(
                toml::from_str("[openrouter]\napi_key_env = \"CW_TEST_BOUND_KEY\"\n")
                    .expect("provider table"),
            ),
            ..Config::default()
        };
        let resolution = resolve_credential_source(&config, ApiProvider::Openrouter);
        assert_eq!(
            resolution.source,
            CredentialSource::ProviderConfigEnv {
                var: "CW_TEST_BOUND_KEY".to_string()
            },
            "a route that names its variable must resolve from it and say so"
        );
        assert_eq!(resolution.source.label(), "api_key_env CW_TEST_BOUND_KEY");
    }

    /// An ambient export must name the exact variable that won, not just
    /// "configured" — this is pi's `source: "ANTHROPIC_API_KEY"`.
    #[test]
    fn ambient_env_names_the_variable_that_won() {
        let _lock = lock_test_env();
        let ctx = MapAuthContext::new().with_env("OPENROUTER_API_KEY", "value");
        let config = Config::default();
        let resolution = resolve_credential_source_with(&config, ApiProvider::Openrouter, &ctx);
        assert_eq!(
            resolution.source,
            CredentialSource::AmbientEnv {
                var: "OPENROUTER_API_KEY".to_string()
            }
        );
        assert_eq!(resolution.source.label(), "OPENROUTER_API_KEY");
    }

    /// The regression this whole lane exists for: a provider with no
    /// credential anywhere used to report a bare boolean. It must now name
    /// every place that was probed, in precedence order, and offer a fix.
    #[test]
    fn a_missing_credential_names_every_place_that_was_checked() {
        let _lock = lock_test_env();
        let ctx = MapAuthContext::new();
        let config = Config::default();
        let resolution = resolve_credential_source_with(&config, ApiProvider::Openrouter, &ctx);
        assert!(!resolution.is_present());

        let checked = resolution.checked_places();
        assert!(
            checked.contains("OPENROUTER_API_KEY"),
            "the ambient variable must be named: {checked}"
        );
        assert!(
            checked.contains("secret store \"openrouter\""),
            "the durable slot must be named: {checked}"
        );
        assert!(
            checked.contains("~/.codewhale/config.toml"),
            "the user-global config must be named: {checked}"
        );
        assert_eq!(
            resolution.first_fix(),
            Some("export OPENROUTER_API_KEY=<key>"),
            "the first probed place must carry the command that fixes it"
        );
    }

    /// #5033's marker gate is a real asymmetry between what the picker
    /// reports and what the request path would find: for a provider that is
    /// not active and whose config table carries no api-key marker, the
    /// durable slot is *not read at all*. That is defensible, but it must be
    /// visible — a user staring at "missing key" has to be told the slot was
    /// skipped rather than found empty.
    #[test]
    fn an_unread_secret_slot_says_it_was_not_read_and_why() {
        let _lock = lock_test_env();
        let ctx = MapAuthContext::new();
        let config = deepseek_config();
        let resolution = resolve_credential_source_with(&config, ApiProvider::Openrouter, &ctx);

        let checked = resolution.checked_places();
        assert!(
            checked.contains("(not read: inactive provider, no api-key marker)"),
            "an unread slot must not look like an empty one: {checked}"
        );
    }

    /// `auth_mode = "none"` is a resolution, not an absence.
    #[test]
    fn no_auth_routes_resolve_to_the_auth_mode_itself() {
        let _lock = lock_test_env();
        let config = Config {
            providers: Some(
                toml::from_str("[openrouter]\nauth_mode = \"none\"\n").expect("provider table"),
            ),
            ..Config::default()
        };
        let resolution = resolve_credential_source(&config, ApiProvider::Openrouter);
        assert_eq!(resolution.source, CredentialSource::AuthModeNone);
        assert!(resolution.is_present());
        assert!(resolution.checked_places().is_empty());
    }

    /// The resolver is the sole authority; `has_api_key_for` must agree with
    /// it for every provider, or two surfaces can disagree again.
    #[test]
    fn has_api_key_for_agrees_with_the_resolver_for_every_provider() {
        let _lock = lock_test_env();
        let config = Config::default();
        for provider in ApiProvider::all() {
            let resolution = resolve_credential_source(&config, *provider);
            assert_eq!(
                has_api_key_for(&config, *provider),
                resolution.is_present(),
                "{provider:?} disagreed: {:?}",
                resolution.source
            );
        }
    }
}
