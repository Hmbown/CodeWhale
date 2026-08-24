//! Secret redaction for model-bound credentials: collecting the exact
//! secret values that must never reach logs or transcripts, and the
//! fail-closed text redaction applied over provider traffic.
//!
//! Extracted verbatim from `client.rs` (#5586). The two entry points the
//! parent calls are `pub(super)`; the collectors stay module-private.

use codewhale_config::is_upstream_auth_header;

use crate::config::{ApiProvider, Config};

const MIN_EXACT_SECRET_CHARS: usize = 8;

fn push_model_bound_secret(values: &mut Vec<String>, value: Option<&str>) {
    let Some(value) = value
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() >= MIN_EXACT_SECRET_CHARS)
    else {
        return;
    };
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn model_bound_secret_store_slot(provider: ApiProvider) -> Option<&'static str> {
    match provider {
        ApiProvider::DeepseekCN => Some("deepseek"),
        ApiProvider::SiliconflowCn => Some("siliconflow"),
        ApiProvider::Custom => None,
        _ => Some(provider.as_str()),
    }
}

pub(super) fn push_file_backed_model_bound_secrets(values: &mut Vec<String>) {
    // Unit tests must never inspect the developer's real credential store.
    // The isolated regression below opts in with a temporary CODEWHALE_HOME,
    // matching Config's existing secret-store test discipline.
    #[cfg(test)]
    if !codewhale_paths::codewhale_home_is_explicit()
        || std::env::var_os("CODEWHALE_SECRET_BACKEND").is_none()
    {
        return;
    }

    // Redaction needs only a best-effort view of inactive file-backed
    // credentials. It must not cause a legacy-store migration merely because a
    // client is being constructed (notably for `doctor`'s live probe). Keep
    // this file-only to avoid a burst of OS-keychain prompts for inactive
    // providers; the active credential is already supplied by the route
    // resolver.
    let secrets = codewhale_secrets::Secrets::file_backed_read_only();
    let mut slots = Vec::new();
    for provider in ApiProvider::all()
        .iter()
        .copied()
        .chain(std::iter::once(ApiProvider::DeepseekCN))
    {
        let Some(slot) = model_bound_secret_store_slot(provider) else {
            continue;
        };
        if !slots.contains(&slot) {
            slots.push(slot);
        }
    }
    // The legacy literal `provider = "custom"` route owns this durable slot.
    slots.push("custom");

    for slot in slots {
        if let Ok(Some(secret)) = secrets.get(slot) {
            push_model_bound_secret(values, Some(&secret));
        }
    }
}

pub(super) fn configured_model_bound_secret_values(
    config: &Config,
    active_api_key: &str,
) -> Vec<String> {
    let mut values = Vec::new();
    push_model_bound_secret(&mut values, Some(active_api_key));
    push_model_bound_secret(&mut values, config.api_key.as_deref());
    push_model_bound_secret(&mut values, config.sandbox_api_key.as_deref());
    push_model_bound_secret(
        &mut values,
        config
            .search
            .as_ref()
            .and_then(|search| search.api_key.as_deref()),
    );
    push_model_bound_secret(
        &mut values,
        config
            .vision_model
            .as_ref()
            .and_then(|vision| vision.api_key.as_deref()),
    );

    if let Some(headers) = config.http_headers.as_ref() {
        for (name, value) in headers {
            if is_upstream_auth_header(name) {
                push_model_bound_secret(&mut values, Some(value));
            }
        }
    }

    for provider in ApiProvider::all()
        .iter()
        .copied()
        .chain(std::iter::once(ApiProvider::DeepseekCN))
        .filter(|provider| *provider != ApiProvider::Custom)
    {
        for env_name in provider.env_vars() {
            if let Ok(value) = std::env::var(env_name) {
                push_model_bound_secret(&mut values, Some(&value));
            }
        }
        let Some(provider_config) = config.provider_config_for(provider) else {
            continue;
        };
        push_model_bound_secret(&mut values, provider_config.api_key.as_deref());
        if let Some(headers) = provider_config.http_headers.as_ref() {
            for (name, value) in headers {
                if is_upstream_auth_header(name) {
                    push_model_bound_secret(&mut values, Some(value));
                }
            }
        }
    }

    if let Some(providers) = config.providers.as_ref() {
        for provider_config in providers.custom.values() {
            push_model_bound_secret(&mut values, provider_config.api_key.as_deref());
            if let Some(env_name) = provider_config
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                && let Ok(value) = std::env::var(env_name)
            {
                push_model_bound_secret(&mut values, Some(&value));
            }
            if let Some(headers) = provider_config.http_headers.as_ref() {
                for (name, value) in headers {
                    if is_upstream_auth_header(name) {
                        push_model_bound_secret(&mut values, Some(value));
                    }
                }
            }
        }
    }

    push_file_backed_model_bound_secrets(&mut values);

    // Replace longer values first in case one credential happens to contain
    // another as a prefix.
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values
}

pub(super) fn redact_model_bound_text(text: &str, exact_secret_values: &[String]) -> String {
    let mut redacted = text.to_string();
    for secret in exact_secret_values {
        redacted = redacted.replace(secret, codewhale_config::persistence::REDACTED);
    }
    // Tool results feed exact-match edits, so only credential-shaped values
    // are masked here; key-only hits (`password: credentials?.password`) stay
    // byte-exact. Logs and previews keep the broad key-based scrubber.
    codewhale_config::persistence::redact_model_bound_secrets(&redacted)
}
