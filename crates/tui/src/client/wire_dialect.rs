//! Wire dialect helpers: default request headers per provider/dialect,
//! auth-header dialect classification, and provider wire-format
//! resolution (chat completions vs anthropic messages vs responses).
//!
//! Extracted verbatim from `client.rs` (#5586); visibility promoted to
//! pub(super) so the parent re-imports keep call sites unchanged.
//! Known follow-up (not changed by this move): wire_config_prefers_anthropic
//! is byte-identical to the private copy in config.rs and should dedup
//! onto one definition.

use std::collections::HashMap;

use anyhow::Result;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};

use codewhale_config::is_upstream_auth_header;

use super::WireFormat;
use super::{xiaomi_mimo_api_key_uses_token_plan, xiaomi_mimo_base_url_uses_token_plan};
use crate::config::ApiProvider;

pub(super) fn build_default_headers(
    api_key: &str,
    extra_headers: &HashMap<String, String>,
    api_provider: ApiProvider,
    base_url: &str,
    wire_format: WireFormat,
    auth_disabled: bool,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let api_key = api_key.trim();
    let uses_anthropic_messages = wire_format == WireFormat::AnthropicMessages;
    if uses_anthropic_messages {
        // #3014: most Messages API routes authenticate with `x-api-key`.
        // OpenModel also supports Bearer auth for Messages, and its `/models`
        // endpoint requires it, so the header chooser below keeps OpenModel on
        // Bearer while still pinning the Anthropic wire contract here.
        headers.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static("2023-06-01"),
        );
    }
    let auth_header_name = if auth_disabled {
        None
    } else if !api_key.is_empty()
        && uses_anthropic_messages
        && api_provider != ApiProvider::Openmodel
    {
        Some(HeaderName::from_static("x-api-key"))
    } else if !api_key.is_empty()
        && api_provider == ApiProvider::XiaomiMimo
        && (xiaomi_mimo_base_url_uses_token_plan(base_url)
            || xiaomi_mimo_api_key_uses_token_plan(api_key))
    {
        Some(HeaderName::from_static("api-key"))
    } else if !api_key.is_empty() {
        Some(AUTHORIZATION)
    } else {
        None
    };
    if let Some(header_name) = auth_header_name.as_ref() {
        let header_value = if *header_name == AUTHORIZATION {
            HeaderValue::from_str(&format!("Bearer {api_key}"))?
        } else {
            HeaderValue::from_str(api_key)?
        };
        headers.insert(header_name.clone(), header_value);
    }
    // OpenRouter app attribution: these two headers are how apps appear on
    // openrouter.ai's app rankings — there is no manual submission. They
    // identify the app, never the user, and a user-configured header of the
    // same name below still wins (the extra-header loop overwrites).
    if api_provider == ApiProvider::Openrouter {
        headers.insert(
            HeaderName::from_static("http-referer"),
            HeaderValue::from_static("https://codewhale.net"),
        );
        headers.insert(
            HeaderName::from_static("x-title"),
            HeaderValue::from_static("Codewhale"),
        );
    }
    for (name, value) in extra_headers {
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        if auth_disabled && is_upstream_auth_header(name) {
            continue;
        }
        let header_name = HeaderName::from_bytes(name.as_bytes())?;
        if header_name == AUTHORIZATION
            || header_name == CONTENT_TYPE
            || auth_header_name.as_ref() == Some(&header_name)
            || (auth_header_name.is_some() && is_auth_dialect_header(&header_name))
        {
            continue;
        }
        headers.insert(header_name, HeaderValue::from_str(value)?);
    }
    Ok(headers)
}

pub(super) fn is_auth_dialect_header(header_name: &HeaderName) -> bool {
    header_name == AUTHORIZATION
        || header_name == HeaderName::from_static("api-key")
        || header_name == HeaderName::from_static("x-api-key")
}

pub(super) fn provider_default_wire_format(api_provider: ApiProvider) -> WireFormat {
    provider_wire_format_for_config(api_provider, None)
}

/// Resolve the wire dialect for a dual-protocol vendor.
///
/// Power-user toggle: `providers.<id>.wire = "openai" | "anthropic"`.
/// Legacy dialect kinds (`*Anthropic`) still force Messages. Everyone else
/// keeps the descriptor's fixed policy (or Chat Completions).
pub(super) fn provider_wire_format_for_config(
    api_provider: ApiProvider,
    config: Option<&crate::config::Config>,
) -> WireFormat {
    let catalog = api_provider.catalog_identity();
    let wire = config
        .and_then(|cfg| cfg.provider_config_for(catalog))
        .and_then(|entry| entry.wire.as_deref());
    let prefers_anthropic = matches!(
        api_provider,
        ApiProvider::DeepseekAnthropic
            | ApiProvider::MinimaxAnthropic
            | ApiProvider::ModelstudioTokenPlanAnthropic
            | ApiProvider::ModelstudioCodingPlanAnthropic
    ) || wire_config_prefers_anthropic(wire);

    if prefers_anthropic
        && matches!(
            catalog,
            ApiProvider::Deepseek
                | ApiProvider::Minimax
                | ApiProvider::ModelstudioTokenPlan
                | ApiProvider::DeepseekAnthropic
                | ApiProvider::MinimaxAnthropic
                | ApiProvider::ModelstudioTokenPlanAnthropic
                | ApiProvider::ModelstudioCodingPlan
                | ApiProvider::ModelstudioCodingPlanAnthropic
        )
    {
        return WireFormat::AnthropicMessages;
    }

    api_provider
        .kind()
        .and_then(|kind| {
            codewhale_config::provider::provider_for_kind(kind)
                .wire_policy()
                .fixed()
        })
        .unwrap_or_else(|| {
            if api_provider == ApiProvider::OpencodeZen {
                WireFormat::Responses
            } else {
                WireFormat::ChatCompletions
            }
        })
}

pub(super) fn wire_config_prefers_anthropic(wire: Option<&str>) -> bool {
    let Some(raw) = wire.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let normalized = raw.to_ascii_lowercase().replace(['_', ' '], "-");
    matches!(
        normalized.as_str(),
        "anthropic"
            | "anthropic-messages"
            | "messages"
            | "claude"
            | "anthropic-compatible"
            | "anthropic-compat"
    )
}

pub(super) fn api_provider_skips_models_probe(api_provider: ApiProvider) -> bool {
    matches!(api_provider, ApiProvider::DeepseekAnthropic)
}
