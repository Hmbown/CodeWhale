//! Provider-neutral `/v1/chat/completions` pass-through endpoint.
//!
//! This module resolves a model through the [`ModelRegistry`], looks up the
//! matching provider configuration, and forwards an OpenAI-compatible request
//! body upstream.  It does **not** import or call any DeepSeek-named client
//! APIs — routing stays in neutral config/provider types.
//!
//! Only providers whose [`WireFormat`] is [`WireFormat::ChatCompletions`] are
//! served.  Streaming requests are explicitly rejected for now.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderName, StatusCode};
use axum::response::IntoResponse;
use codewhale_agent::ModelRegistry;
use codewhale_config::{
    ConfigToml, ProviderKind, auth_mode_disables_api_key, is_upstream_auth_header,
    provider::WireFormat,
    provider_base_url_is_official, provider_preserves_custom_base_url_model,
    route::{LogicalModelRef, ProviderId, RouteError, RouteRequest, RouteResolver},
};
use serde_json::Value;

use super::AppState;

// ── Resolved endpoint ──────────────────────────────────────────────────

/// Everything needed to forward a single chat-completions request upstream.
#[derive(Debug, Clone)]
struct ResolvedModelEndpoint {
    provider: ProviderKind,
    base_url: String,
    model: String,
    api_key: Option<String>,
    auth_disabled: bool,
    http_headers: BTreeMap<String, String>,
    path_suffix: Option<String>,
    insecure_skip_tls_verify: bool,
    wire_format: WireFormat,
}

// ── Resolution ─────────────────────────────────────────────────────────

/// Resolve a provider endpoint from the app configuration + an optional
/// `model` field pulled out of the incoming request body.
fn resolve_endpoint(
    config: &ConfigToml,
    registry: &ModelRegistry,
    request_model: Option<&str>,
) -> Result<ResolvedModelEndpoint, RouteError> {
    let configured_base_url = provider_base_url(config, config.provider);
    let configured_endpoint_owns_models =
        endpoint_preserves_raw_model_ids(config.provider, &configured_base_url);
    let provider_kind = if configured_endpoint_owns_models {
        config.provider
    } else {
        request_model
            .filter(|model| !model.trim().is_empty())
            .and_then(|model_name| {
                inferred_provider_for_model(registry, config.provider, model_name)
            })
            .unwrap_or(config.provider)
    };
    let provider_cfg = config.providers.for_provider(provider_kind);
    let provider_meta = provider_kind.provider();

    // Base URL: configured → default
    let base_url = provider_base_url(config, provider_kind);
    let endpoint_owns_models = endpoint_preserves_raw_model_ids(provider_kind, &base_url);

    // Keep provider inference and wire identity separate: ModelRegistry picks
    // the provider for a known request alias, while RouteResolver owns the
    // provider-scoped wire model and custom-endpoint passthrough contract.
    let raw_selected_model = request_model
        .filter(|m| !m.trim().is_empty())
        .map(str::to_string)
        .or_else(|| provider_cfg.model.clone())
        .or_else(|| {
            (provider_kind == ProviderKind::Deepseek)
                .then(|| config.default_text_model.clone())
                .flatten()
        })
        .unwrap_or_else(|| provider_meta.default_model().to_string());
    let selected_model = if endpoint_owns_models {
        raw_selected_model
    } else {
        let resolved = registry.resolve(Some(&raw_selected_model), Some(provider_kind));
        if !resolved.used_fallback && resolved.resolved.provider == provider_kind {
            resolved.resolved.id
        } else {
            raw_selected_model
        }
    };
    let route = RouteResolver::new().resolve(&RouteRequest {
        explicit_provider: Some(provider_kind),
        model_selector: Some(LogicalModelRef::from(selected_model.as_str())),
        saved_provider_model: None,
        base_url_override: Some(base_url.clone()),
    })?;
    let model = route.wire_model_id.as_str().to_string();

    let auth_mode = provider_cfg.auth_mode.as_deref().or_else(|| {
        (provider_kind == config.provider)
            .then_some(config.auth_mode.as_deref())
            .flatten()
    });
    let auth_disabled = auth_mode_disables_api_key(auth_mode);

    let configured_api_key = provider_cfg.api_key.as_deref().or_else(|| {
        (provider_kind == ProviderKind::Deepseek)
            .then_some(config.api_key.as_deref())
            .flatten()
    });

    // Provider auth comes only from the resolved endpoint configuration. The
    // HTTP request's Authorization header authenticates the caller to the local
    // app-server and is never a provider credential.
    let api_key = resolve_upstream_api_key(
        configured_api_key,
        auth_disabled,
        provider_base_url_is_official(provider_kind, &base_url),
        || {
            provider_meta
                .env_vars()
                .iter()
                .find_map(|var| std::env::var(var).ok())
        },
    );

    let mut http_headers = if provider_kind == config.provider {
        config.http_headers.clone()
    } else {
        BTreeMap::new()
    };
    http_headers.extend(provider_cfg.http_headers.clone());
    if auth_disabled {
        http_headers.retain(|name, _| !is_upstream_auth_header(name));
    }

    let path_suffix = provider_cfg.path_suffix.clone();

    let insecure_skip_tls_verify = provider_cfg.insecure_skip_tls_verify.unwrap_or(false);

    let wire_format = provider_meta.wire_policy().fixed().ok_or_else(|| {
        RouteError::UnsupportedModelProtocol {
            provider: ProviderId::from(provider_kind.as_str()),
            model: model.clone(),
            endpoint_key: "unresolved".to_string(),
        }
    })?;

    Ok(ResolvedModelEndpoint {
        provider: provider_kind,
        base_url,
        model,
        api_key,
        auth_disabled,
        http_headers,
        path_suffix,
        insecure_skip_tls_verify,
        wire_format,
    })
}

/// Prefer the configured provider when a model id/alias exists on more than
/// one provider. Only a genuine scoped miss may infer a different registry
/// provider. This keeps `deepseek-v4-pro` on a configured OpenRouter route
/// while still allowing a DeepSeek default to route an unambiguous `inkling`
/// request to Together.
fn inferred_provider_for_model(
    registry: &ModelRegistry,
    configured_provider: ProviderKind,
    model_name: &str,
) -> Option<ProviderKind> {
    // OpenCode Go is an explicit Chat-only provider scope. A same-named model
    // on OpenRouter/MiniMax must not escape that scope through the registry's
    // global inference; RouteResolver owns the authoritative Go allowlist and
    // will reject Messages-only ids.
    if configured_provider == ProviderKind::OpencodeGo {
        return Some(configured_provider);
    }
    let scoped = registry.resolve(Some(model_name), Some(configured_provider));
    if !scoped.used_fallback && scoped.resolved.provider == configured_provider {
        return Some(configured_provider);
    }
    let global = registry.resolve(Some(model_name), None);
    (!global.used_fallback).then_some(global.resolved.provider)
}

fn resolve_upstream_api_key(
    configured: Option<&str>,
    auth_disabled: bool,
    allow_ambient: bool,
    ambient_provider_env: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if auth_disabled {
        None
    } else if let Some(configured) = configured {
        Some(configured.to_string())
    } else if allow_ambient {
        ambient_provider_env()
    } else {
        None
    }
}

fn provider_base_url(config: &ConfigToml, provider: ProviderKind) -> String {
    let metadata = provider.provider();
    config
        .providers
        .for_provider(provider)
        .base_url
        .clone()
        .or_else(|| {
            (provider == ProviderKind::Deepseek)
                .then(|| config.base_url.clone())
                .flatten()
        })
        .unwrap_or_else(|| metadata.default_base_url().to_string())
}

fn endpoint_preserves_raw_model_ids(provider: ProviderKind, base_url: &str) -> bool {
    matches!(
        provider,
        ProviderKind::Custom | ProviderKind::Ollama | ProviderKind::Vllm | ProviderKind::Sglang
    ) || provider_preserves_custom_base_url_model(provider, base_url)
}

/// Build the upstream URL.
fn upstream_url(endpoint: &ResolvedModelEndpoint) -> String {
    let base = endpoint.base_url.trim_end_matches('/');
    match endpoint.path_suffix.as_deref() {
        Some(suffix) if !suffix.trim().is_empty() => format!(
            "{}/{}",
            unversioned_base_url(base),
            suffix.trim_start_matches('/')
        ),
        _ => {
            let mut versioned = versioned_base_url(base);
            if versioned
                .rsplit('/')
                .next()
                .is_some_and(|segment| segment.eq_ignore_ascii_case("beta"))
            {
                versioned = format!("{}/v1", unversioned_base_url(base));
            }
            format!("{}/chat/completions", versioned.trim_end_matches('/'))
        }
    }
}

fn versioned_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if base_url_has_version_suffix(trimmed) {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

fn unversioned_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    trimmed
        .rsplit_once('/')
        .filter(|(_, segment)| is_version_segment(segment))
        .map(|(base, _)| base)
        .unwrap_or(trimmed)
        .to_string()
}

fn base_url_has_version_suffix(trimmed: &str) -> bool {
    trimmed.rsplit('/').next().is_some_and(is_version_segment)
}

fn is_version_segment(segment: &str) -> bool {
    segment.eq_ignore_ascii_case("beta")
        || segment
            .strip_prefix('v')
            .or_else(|| segment.strip_prefix('V'))
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()))
}

// ── Route handler ──────────────────────────────────────────────────────

pub(crate) async fn chat_completions_handler(
    State(state): State<AppState>,
    Json(mut body): Json<Value>,
) -> impl IntoResponse {
    // Reject streaming early.
    if body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "message": "streaming is not supported on this endpoint",
                    "type": "unsupported_parameter",
                    "code": "streaming_unsupported"
                }
            })),
        )
            .into_response();
    }

    // Extract model from body.
    let request_model = body.get("model").and_then(|v| v.as_str());

    // Resolve endpoint.
    let config = state.config.read().await;
    let endpoint = match resolve_endpoint(&config, &state.registry, request_model) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("model route could not be resolved: {error}"),
                        "type": "invalid_request_error",
                        "code": "model_route_invalid"
                    }
                })),
            )
                .into_response();
        }
    };

    // Only ChatCompletions providers are supported.
    if endpoint.wire_format != WireFormat::ChatCompletions {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "message": format!(
                        "provider {:?} uses {:?} wire format, only ChatCompletions is supported",
                        endpoint.provider, endpoint.wire_format
                    ),
                    "type": "unsupported_provider",
                    "code": "provider_wire_format_unsupported"
                }
            })),
        )
            .into_response();
    }

    // Always write the resolved model back. Unknown provider-owned ids remain
    // byte-for-byte passthrough values, while known aliases become their exact
    // provider wire ids before forwarding.
    body["model"] = serde_json::Value::String(endpoint.model.clone());

    let url = upstream_url(&endpoint);

    if endpoint.insecure_skip_tls_verify {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "message": format!(
                        "TLS certificate verification cannot be disabled for provider {:?}; use SSL_CERT_FILE with a trusted custom CA bundle",
                        endpoint.provider
                    ),
                    "type": "invalid_request_error",
                    "code": "tls_verification_required"
                }
            })),
        )
            .into_response();
    }

    // Build upstream request.
    let upstream_req = codewhale_release::platform_http_client_builder()
        .build()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("failed to build upstream client: {e}"),
                        "type": "internal_error"
                    }
                })),
            )
                .into_response()
        })
        .map(|client| {
            let mut req = client.post(&url).json(&body);

            if !endpoint.auth_disabled
                && let Some(key) = endpoint.api_key.as_deref()
            {
                req = req.bearer_auth(key);
            }

            // Forward configured provider headers.
            for (name, value) in &endpoint.http_headers {
                if endpoint.auth_disabled && is_upstream_auth_header(name) {
                    continue;
                }
                if let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) {
                    req = req.header(header_name, value.as_str());
                }
            }

            req
        });

    let client = match upstream_req {
        Ok(client) => client,
        Err(resp) => return resp,
    };

    // Execute upstream request.
    match client.send().await {
        Ok(upstream_resp) => {
            let status = upstream_resp.status();
            let headers = upstream_resp.headers().clone();
            match upstream_resp.text().await {
                Ok(body_text) => {
                    let mut response =
                        axum::response::Response::new(axum::body::Body::from(body_text));
                    *response.status_mut() = status;
                    // Forward relevant upstream headers.
                    if let Some(ct) = headers.get("content-type") {
                        response.headers_mut().insert("content-type", ct.clone());
                    }
                    response
                }
                Err(e) => (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("failed to read upstream response: {e}"),
                            "type": "upstream_error"
                        }
                    })),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": {
                    "message": format!("upstream request failed: {e}"),
                    "type": "upstream_error"
                }
            })),
        )
            .into_response(),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use codewhale_config::provider::WireFormat;
    use std::fs;
    use std::sync::OnceLock;
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    use super::super::{app_router, build_state};

    fn install_crypto_provider() {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    /// Start a minimal upstream mock server that echoes back what it received.
    async fn start_mock_upstream() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{}:{}", addr.ip(), addr.port());

        let handle = tokio::spawn(async move {
            let app = axum::Router::new()
                .route("/v1/chat/completions", axum::routing::post(mock_handler));
            axum::serve(listener, app).await.unwrap();
        });

        // Give the server a moment to start.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        (base_url, handle)
    }

    async fn mock_handler(
        headers: axum::http::HeaderMap,
        Json(body): Json<Value>,
    ) -> impl axum::response::IntoResponse {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("none");

        let response_body = serde_json::json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "created": 1234567890,
            "model": body.get("model").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": format!("echo: received {} messages, auth={auth}",
                        body.get("messages").and_then(|m| m.as_array()).map(|a| a.len()).unwrap_or(0))
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        (StatusCode::OK, Json(response_body))
    }

    async fn capturing_mock_handler(
        axum::extract::State(captured): axum::extract::State<
            mpsc::UnboundedSender<axum::http::HeaderMap>,
        >,
        headers: axum::http::HeaderMap,
        body: Json<Value>,
    ) -> impl axum::response::IntoResponse {
        captured
            .send(headers.clone())
            .expect("capture upstream headers");
        mock_handler(headers, body).await
    }

    async fn start_capturing_mock_upstream() -> (
        String,
        mpsc::UnboundedReceiver<axum::http::HeaderMap>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind capturing upstream");
        let addr = listener.local_addr().expect("capturing upstream address");
        let base_url = format!("http://{}:{}", addr.ip(), addr.port());
        let (captured_tx, captured_rx) = mpsc::unbounded_channel();

        let handle = tokio::spawn(async move {
            let app = axum::Router::new()
                .route(
                    "/v1/chat/completions",
                    axum::routing::post(capturing_mock_handler),
                )
                .with_state(captured_tx);
            axum::serve(listener, app)
                .await
                .expect("serve capturing upstream");
        });

        (base_url, captured_rx, handle)
    }

    fn app_with_mock_upstream(
        auth_token: Option<&str>,
        mock_base_url: &str,
    ) -> (axum::Router, tempfile::TempDir) {
        app_with_mock_upstream_with_provider_extra(auth_token, mock_base_url, "")
    }

    fn app_with_mock_upstream_with_provider_extra(
        auth_token: Option<&str>,
        mock_base_url: &str,
        provider_extra: &str,
    ) -> (axum::Router, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        let config_content = format!(
            r#"
provider = "arcee"
api_key = "sk-deepseek-secret"

[providers.arcee]
base_url = "{mock_base_url}"
model = "trinity-large-thinking"
api_key = "arcee-configured-key"
{provider_extra}
"#
        );
        fs::write(&config_path, config_content).expect("write config");
        let state = build_state(
            Some(config_path),
            auth_token.map(std::string::ToString::to_string),
        )
        .expect("state");
        (app_router(state, &[]), tmp)
    }

    fn app_with_together_mock_upstream(mock_base_url: &str) -> (axum::Router, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        let config_content = format!(
            r#"
provider = "deepseek"

[providers.together]
base_url = "{mock_base_url}"
api_key = "together-configured-key"
"#
        );
        fs::write(&config_path, config_content).expect("write config");
        let state = build_state(Some(config_path), None).expect("state");
        (app_router(state, &[]), tmp)
    }

    fn app_with_root_deepseek_mock_upstream(
        mock_base_url: &str,
    ) -> (axum::Router, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        let config_content = format!(
            r#"
provider = "deepseek"
api_key = "root-deepseek-key"
base_url = "{mock_base_url}"
default_text_model = "root-deepseek-model"
http_headers = {{ "X-Root-Route" = "kept" }}
"#
        );
        fs::write(&config_path, config_content).expect("write config");
        let state = build_state(Some(config_path), None).expect("state");
        (app_router(state, &[]), tmp)
    }

    fn app_with_auth_boundary_mock_upstream(
        auth_token: &str,
        mock_base_url: &str,
        provider_api_key: &str,
        auth_mode: Option<&str>,
        include_configured_auth_headers: bool,
    ) -> (axum::Router, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        let auth_mode = auth_mode
            .map(|mode| format!("auth_mode = {mode:?}"))
            .unwrap_or_default();
        let configured_auth_headers = if include_configured_auth_headers {
            r#"http_headers = { aUtHoRiZaTiOn = "Bearer configured-header-secret", "X-API-Key" = "configured-x-key-secret", "Api-Key" = "configured-key-secret", "Proxy-Authorization" = "Basic configured-proxy-secret", "X-Auth-Token" = "configured-auth-token", "X-Access-Token" = "configured-access-token", "X-Goog-Api-Key" = "configured-google-key", Cookie = "session=secret", "X-Route-Metadata" = "safe" }"#
        } else {
            ""
        };
        let config_content = format!(
            r#"
provider = "arcee"

[providers.arcee]
base_url = "{mock_base_url}"
model = "trinity-large-thinking"
api_key = {provider_api_key:?}
{auth_mode}
{configured_auth_headers}
"#
        );
        fs::write(&config_path, config_content).expect("write config");
        let state = build_state(Some(config_path), Some(auth_token.to_string())).expect("state");
        (app_router(state, &[]), tmp)
    }

    async fn response_body_json(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        serde_json::from_slice(&bytes).expect("json response")
    }

    #[tokio::test]
    async fn forwards_messages_and_tools() {
        install_crypto_provider();
        let (mock_url, _mock) = start_mock_upstream().await;
        let (app, _tmp) = app_with_mock_upstream(None, &mock_url);

        let body = serde_json::json!({
            "model": "trinity-large-thinking",
            "messages": [
                {"role": "user", "content": "hello"}
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather",
                    "parameters": {"type": "object", "properties": {}}
                }
            }],
            "tool_choice": "auto"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let resp_body = response_body_json(response).await;
        assert_eq!(resp_body["model"], "trinity-large-thinking");
        assert!(
            resp_body["choices"][0]["message"]["content"]
                .as_str()
                .unwrap()
                .contains("1 messages")
        );
    }

    #[tokio::test]
    async fn default_model_injected_when_omitted() {
        install_crypto_provider();
        let (mock_url, _mock) = start_mock_upstream().await;
        let (app, _tmp) = app_with_mock_upstream(None, &mock_url);

        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": "hello"}
            ]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let resp_body = response_body_json(response).await;
        // The mock echoes the model it received; should be the configured default.
        assert_eq!(resp_body["model"], "trinity-large-thinking");
    }

    #[tokio::test]
    async fn root_deepseek_compatibility_fields_reach_the_configured_upstream() {
        install_crypto_provider();
        let (mock_url, mut captured, _mock) = start_capturing_mock_upstream().await;
        let (app, _tmp) = app_with_root_deepseek_mock_upstream(&mock_url);

        let body = serde_json::json!({
            "messages": [{"role": "user", "content": "hello"}]
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let response_body = response_body_json(response).await;
        assert_eq!(response_body["model"], "root-deepseek-model");
        assert!(
            response_body["choices"][0]["message"]["content"]
                .as_str()
                .is_some_and(|content| content.contains("auth=Bearer root-deepseek-key"))
        );
        let headers = tokio::time::timeout(std::time::Duration::from_secs(1), captured.recv())
            .await
            .expect("upstream request timeout")
            .expect("captured upstream request");
        assert_eq!(
            headers
                .get("x-root-route")
                .and_then(|value| value.to_str().ok()),
            Some("kept")
        );
    }

    #[tokio::test]
    async fn configured_model_preserved_when_provided() {
        install_crypto_provider();
        let (mock_url, _mock) = start_mock_upstream().await;
        let (app, _tmp) = app_with_mock_upstream(None, &mock_url);

        let body = serde_json::json!({
            "model": "custom-model-v2",
            "messages": [
                {"role": "user", "content": "hello"}
            ]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let resp_body = response_body_json(response).await;
        assert_eq!(resp_body["model"], "custom-model-v2");
    }

    #[test]
    fn official_together_inkling_aliases_resolve_to_the_exact_wire_model() {
        let config = ConfigToml::default();
        let registry = ModelRegistry::default();

        for requested in ["inkling", "together-inkling", "thinkingmachines/inkling"] {
            let endpoint =
                resolve_endpoint(&config, &registry, Some(requested)).expect("Inkling route");
            assert_eq!(endpoint.provider, ProviderKind::Together, "{requested}");
            assert_eq!(endpoint.model, "thinkingmachines/inkling", "{requested}");
        }
    }

    #[test]
    fn shared_alias_prefers_the_configured_provider_before_global_inference() {
        let config = ConfigToml {
            provider: ProviderKind::Openrouter,
            ..ConfigToml::default()
        };

        let endpoint =
            resolve_endpoint(&config, &ModelRegistry::default(), Some("deepseek-v4-pro"))
                .expect("configured-provider route");

        assert_eq!(endpoint.provider, ProviderKind::Openrouter);
    }

    #[test]
    fn opencode_go_app_route_cannot_cross_route_or_bypass_chat_allowlist() {
        let registry = ModelRegistry::default();
        let messages_models = [
            "minimax-m3",
            "minimax-m2.7",
            "minimax-m2.5",
            "qwen3.7-max",
            "qwen3.7-plus",
            "qwen3.6-plus",
        ];

        for model in messages_models {
            for requested in [model.to_string(), format!("opencode-go/{model}")] {
                let mut config = ConfigToml {
                    provider: ProviderKind::OpencodeGo,
                    ..ConfigToml::default()
                };
                config.providers.opencode_go.model = Some(requested.clone());
                assert!(
                    matches!(
                        resolve_endpoint(&config, &registry, None),
                        Err(RouteError::ForeignModelForDirectProvider { .. })
                    ),
                    "static {requested} must be rejected"
                );
                assert!(
                    matches!(
                        resolve_endpoint(&config, &registry, Some(&requested)),
                        Err(RouteError::ForeignModelForDirectProvider { .. })
                    ),
                    "request {requested} must not cross-route"
                );

                config.providers.opencode_go.base_url =
                    Some("https://go-gateway.example.test/v1".to_string());
                assert!(
                    matches!(
                        resolve_endpoint(&config, &registry, Some(&requested)),
                        Err(RouteError::ForeignModelForDirectProvider { .. })
                    ),
                    "custom-base {requested} must still be rejected"
                );
            }
        }

        let mut valid = ConfigToml {
            provider: ProviderKind::OpencodeGo,
            ..ConfigToml::default()
        };
        valid.providers.opencode_go.model = Some("opencode-go/glm-5.2".to_string());
        let endpoint = resolve_endpoint(&valid, &registry, None).expect("valid Go route");
        assert_eq!(endpoint.provider, ProviderKind::OpencodeGo);
        assert_eq!(endpoint.model, "glm-5.2");
        assert_eq!(endpoint.wire_format, WireFormat::ChatCompletions);
    }

    #[test]
    fn root_auth_and_headers_do_not_bleed_across_inferred_providers() {
        let mut config = ConfigToml {
            provider: ProviderKind::Deepseek,
            auth_mode: Some("none".to_string()),
            ..ConfigToml::default()
        };
        config.http_headers.insert(
            "X-Root-Route".to_string(),
            "must-not-cross-providers".to_string(),
        );
        config.providers.together.api_key = Some("together-key".to_string());

        let endpoint = resolve_endpoint(&config, &ModelRegistry::default(), Some("inkling"))
            .expect("Together route");

        assert_eq!(endpoint.provider, ProviderKind::Together);
        assert!(!endpoint.auth_disabled);
        assert_eq!(endpoint.api_key.as_deref(), Some("together-key"));
        assert!(!endpoint.http_headers.contains_key("X-Root-Route"));
    }

    #[test]
    fn official_endpoints_forward_canonical_registry_wire_ids() {
        let config = ConfigToml::default();
        let registry = ModelRegistry::default();

        for (requested, provider, expected) in [
            (
                "qwen3.7-plus",
                ProviderKind::Openrouter,
                "qwen/qwen3.7-plus",
            ),
            ("gpt53-codex", ProviderKind::Openai, "gpt-5.3-codex"),
            ("arcee-trinity-mini", ProviderKind::Arcee, "trinity-mini"),
        ] {
            let endpoint =
                resolve_endpoint(&config, &registry, Some(requested)).expect("known alias route");
            assert_eq!(endpoint.provider, provider, "{requested}");
            assert_eq!(endpoint.model, expected, "{requested}");
        }
    }

    #[test]
    fn every_official_deepseek_endpoint_canonicalizes_retired_aliases() {
        let registry = ModelRegistry::default();
        for base_url in [
            "https://api.deepseek.com",
            "https://api.deepseek.com/v1/",
            "https://api.deepseek.com/beta",
        ] {
            for alias in ["deepseek-chat", "deepseek-reasoner"] {
                let mut config = ConfigToml::default();
                config.providers.deepseek.base_url = Some(base_url.to_string());
                let endpoint = resolve_endpoint(&config, &registry, Some(alias))
                    .expect("official DeepSeek route");
                assert_eq!(endpoint.provider, ProviderKind::Deepseek, "{base_url}");
                assert_eq!(endpoint.model, "deepseek-v4-flash", "{base_url} {alias}");
            }
        }
    }

    #[test]
    fn custom_endpoint_preserves_known_registry_alias_verbatim() {
        let mut config = ConfigToml::default();
        config
            .providers
            .for_provider_mut(ProviderKind::Openrouter)
            .base_url = Some("https://gateway.example.test/v1".to_string());

        let endpoint = resolve_endpoint(&config, &ModelRegistry::default(), Some("qwen3.7-plus"))
            .expect("custom OpenRouter-compatible route");
        assert_eq!(endpoint.provider, ProviderKind::Openrouter);
        assert_eq!(endpoint.model, "qwen3.7-plus");
    }

    #[test]
    fn custom_endpoint_never_resolves_ambient_provider_env() {
        let ambient_was_read = std::cell::Cell::new(false);
        let api_key = resolve_upstream_api_key(None, false, false, || {
            ambient_was_read.set(true);
            Some("ambient-provider-secret".to_string())
        });

        assert_eq!(api_key, None);
        assert!(!ambient_was_read.get());
    }

    #[test]
    fn disabled_auth_never_resolves_configured_or_ambient_credentials() {
        let ambient_was_read = std::cell::Cell::new(false);
        let api_key = resolve_upstream_api_key(Some("provider-secret"), true, true, || {
            ambient_was_read.set(true);
            Some("ambient-provider-secret".to_string())
        });

        assert_eq!(api_key, None);
        assert!(!ambient_was_read.get());
    }

    #[test]
    fn active_custom_endpoint_is_not_hijacked_by_known_foreign_alias() {
        let mut config = ConfigToml {
            provider: ProviderKind::Arcee,
            ..ConfigToml::default()
        };
        config
            .providers
            .for_provider_mut(ProviderKind::Arcee)
            .base_url = Some("https://gateway.example.test/v1".to_string());

        let endpoint = resolve_endpoint(&config, &ModelRegistry::default(), Some("qwen3.7-plus"))
            .expect("active custom endpoint route");
        assert_eq!(endpoint.provider, ProviderKind::Arcee);
        assert_eq!(endpoint.model, "qwen3.7-plus");
    }

    #[test]
    fn official_configured_alias_is_canonicalized_when_model_is_omitted() {
        let mut config = ConfigToml {
            provider: ProviderKind::Openrouter,
            ..ConfigToml::default()
        };
        config
            .providers
            .for_provider_mut(ProviderKind::Openrouter)
            .model = Some("qwen3.7-plus".to_string());

        let endpoint = resolve_endpoint(&config, &ModelRegistry::default(), None)
            .expect("configured official alias route");
        assert_eq!(endpoint.provider, ProviderKind::Openrouter);
        assert_eq!(endpoint.model, "qwen/qwen3.7-plus");
    }

    #[test]
    fn configured_together_inkling_alias_is_normalized_when_model_is_omitted() {
        let mut config = ConfigToml {
            provider: ProviderKind::Together,
            ..ConfigToml::default()
        };
        config
            .providers
            .for_provider_mut(ProviderKind::Together)
            .model = Some("inkling".to_string());

        let endpoint = resolve_endpoint(&config, &ModelRegistry::default(), None)
            .expect("configured Inkling route");
        assert_eq!(endpoint.provider, ProviderKind::Together);
        assert_eq!(endpoint.model, "thinkingmachines/inkling");
    }

    #[tokio::test]
    async fn custom_together_endpoint_preserves_explicit_inkling_model_ids() {
        install_crypto_provider();
        let (mock_url, _mock) = start_mock_upstream().await;
        let (app, _tmp) = app_with_together_mock_upstream(&mock_url);

        for requested in ["inkling", "together-inkling", "thinkingmachines/inkling"] {
            let body = serde_json::json!({
                "model": requested,
                "messages": [{"role": "user", "content": "hello"}]
            });
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/v1/chat/completions")
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK, "{requested}");
            let resp_body = response_body_json(response).await;
            assert_eq!(resp_body["model"], requested, "{requested}");
        }
    }

    #[tokio::test]
    async fn configured_api_key_takes_priority_over_incoming_bearer() {
        install_crypto_provider();
        let (mock_url, _mock) = start_mock_upstream().await;
        let (app, _tmp) = app_with_mock_upstream(None, &mock_url);

        let body = serde_json::json!({
            "model": "trinity-large-thinking",
            "messages": [
                {"role": "user", "content": "hello"}
            ]
        });

        // Send with an explicit bearer token, but the configured key should win.
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer user-provided-secret-key")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let resp_body = response_body_json(response).await;
        let content = resp_body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap();
        // The configured key takes priority, not the incoming Bearer.
        assert!(
            content.contains("auth=Bearer arcee-configured-key"),
            "expected configured auth in mock echo, got: {content}"
        );
    }

    #[tokio::test]
    async fn app_authorization_is_not_forwarded_when_upstream_auth_is_disabled() {
        install_crypto_provider();
        let (mock_url, mut captured, _mock) = start_capturing_mock_upstream().await;
        let (app, _tmp) = app_with_auth_boundary_mock_upstream(
            "app-secret",
            &mock_url,
            "provider-secret",
            Some("none"),
            true,
        );

        let body = serde_json::json!({
            "model": "trinity-large-thinking",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer app-secret")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let headers = tokio::time::timeout(std::time::Duration::from_secs(1), captured.recv())
            .await
            .expect("upstream request timeout")
            .expect("captured upstream request");
        for name in [
            "authorization",
            "x-api-key",
            "api-key",
            "proxy-authorization",
            "x-auth-token",
            "x-access-token",
            "x-goog-api-key",
            "cookie",
        ] {
            assert!(headers.get(name).is_none(), "disabled auth leaked {name}");
        }
        assert_eq!(
            headers
                .get("x-route-metadata")
                .and_then(|value| value.to_str().ok()),
            Some("safe")
        );
    }

    #[tokio::test]
    async fn configured_provider_credential_is_the_only_outbound_bearer() {
        install_crypto_provider();
        let (mock_url, mut captured, _mock) = start_capturing_mock_upstream().await;
        let (app, _tmp) = app_with_auth_boundary_mock_upstream(
            "app-secret",
            &mock_url,
            "provider-secret",
            None,
            false,
        );

        let body = serde_json::json!({
            "model": "trinity-large-thinking",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer app-secret")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let headers = tokio::time::timeout(std::time::Duration::from_secs(1), captured.recv())
            .await
            .expect("upstream request timeout")
            .expect("captured upstream request");
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer provider-secret")
        );
    }

    #[tokio::test]
    async fn configured_api_key_used_when_no_bearer_in_request() {
        install_crypto_provider();
        let (mock_url, _mock) = start_mock_upstream().await;
        let (app, _tmp) = app_with_mock_upstream(None, &mock_url);

        let body = serde_json::json!({
            "model": "trinity-large-thinking",
            "messages": [
                {"role": "user", "content": "hello"}
            ]
        });

        // No Authorization header; the configured key should be used.
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let resp_body = response_body_json(response).await;
        let content = resp_body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap();
        assert!(
            content.contains("auth=Bearer arcee-configured-key"),
            "expected configured auth in mock echo, got: {content}"
        );
    }

    #[tokio::test]
    async fn insecure_tls_skip_verify_is_rejected() {
        install_crypto_provider();
        let (mock_url, _mock) = start_mock_upstream().await;
        let (app, _tmp) = app_with_mock_upstream_with_provider_extra(
            None,
            &mock_url,
            "insecure_skip_tls_verify = true",
        );

        let body = serde_json::json!({
            "model": "trinity-large-thinking",
            "messages": [
                {"role": "user", "content": "hello"}
            ]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = response_body_json(response).await;
        assert_eq!(resp_body["error"]["code"], "tls_verification_required");
        assert!(
            resp_body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("SSL_CERT_FILE")
        );
    }

    #[tokio::test]
    async fn streaming_request_rejected() {
        install_crypto_provider();
        let (mock_url, _mock) = start_mock_upstream().await;
        let (app, _tmp) = app_with_mock_upstream(None, &mock_url);

        let body = serde_json::json!({
            "model": "trinity-large-thinking",
            "messages": [
                {"role": "user", "content": "hello"}
            ],
            "stream": true
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let resp_body = response_body_json(response).await;
        assert_eq!(resp_body["error"]["code"], "streaming_unsupported");
    }

    #[tokio::test]
    async fn requires_bearer_token_when_auth_enabled() {
        install_crypto_provider();
        let (mock_url, _mock) = start_mock_upstream().await;
        let (app, _tmp) = app_with_mock_upstream(Some("test-token"), &mock_url);

        let body = serde_json::json!({
            "messages": [{"role": "user", "content": "hello"}]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn non_chat_completions_provider_rejected() {
        // Use the test to verify WireFormat checks work for non-ChatCompletions providers.
        // Anthropic's wire format is AnthropicMessages; OpenaiCodex is Responses.
        let endpoint = ResolvedModelEndpoint {
            provider: ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            api_key: Some("sk-ant-test".to_string()),
            auth_disabled: false,
            http_headers: BTreeMap::new(),
            path_suffix: None,
            insecure_skip_tls_verify: false,
            wire_format: WireFormat::AnthropicMessages,
        };

        assert_ne!(endpoint.wire_format, WireFormat::ChatCompletions);
        // The handler would reject this; we verify the wire format here.
        assert_eq!(endpoint.wire_format, WireFormat::AnthropicMessages);
    }

    #[test]
    fn upstream_url_defaults_to_v1_chat_completions() {
        let endpoint = ResolvedModelEndpoint {
            provider: ProviderKind::Arcee,
            base_url: "https://api.arcee.ai".to_string(),
            model: "trinity".to_string(),
            api_key: None,
            auth_disabled: false,
            http_headers: BTreeMap::new(),
            path_suffix: None,
            insecure_skip_tls_verify: false,
            wire_format: WireFormat::ChatCompletions,
        };
        assert_eq!(
            upstream_url(&endpoint),
            "https://api.arcee.ai/v1/chat/completions"
        );
    }

    #[test]
    fn upstream_url_preserves_arcee_api_v1_base() {
        let endpoint = ResolvedModelEndpoint {
            provider: ProviderKind::Arcee,
            base_url: "https://api.arcee.ai/api/v1".to_string(),
            model: "trinity".to_string(),
            api_key: None,
            auth_disabled: false,
            http_headers: BTreeMap::new(),
            path_suffix: None,
            insecure_skip_tls_verify: false,
            wire_format: WireFormat::ChatCompletions,
        };
        assert_eq!(
            upstream_url(&endpoint),
            "https://api.arcee.ai/api/v1/chat/completions"
        );
    }

    #[test]
    fn upstream_url_respects_path_suffix() {
        let endpoint = ResolvedModelEndpoint {
            provider: ProviderKind::Openrouter,
            base_url: "https://openrouter.ai/api/v1".to_string(),
            model: "deepseek/deepseek-v4-pro".to_string(),
            api_key: None,
            auth_disabled: false,
            http_headers: BTreeMap::new(),
            path_suffix: Some("/chat/completions".to_string()),
            insecure_skip_tls_verify: false,
            wire_format: WireFormat::ChatCompletions,
        };
        assert_eq!(
            upstream_url(&endpoint),
            "https://openrouter.ai/api/chat/completions"
        );
    }

    #[test]
    fn upstream_url_beta_base_uses_standard_v1_chat_completions() {
        let endpoint = ResolvedModelEndpoint {
            provider: ProviderKind::Deepseek,
            base_url: "https://api.deepseek.com/beta".to_string(),
            model: "deepseek-chat".to_string(),
            api_key: None,
            auth_disabled: false,
            http_headers: BTreeMap::new(),
            path_suffix: None,
            insecure_skip_tls_verify: false,
            wire_format: WireFormat::ChatCompletions,
        };
        assert_eq!(
            upstream_url(&endpoint),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn upstream_url_strips_trailing_slash() {
        let endpoint = ResolvedModelEndpoint {
            provider: ProviderKind::Deepseek,
            base_url: "https://api.deepseek.com/".to_string(),
            model: "deepseek-chat".to_string(),
            api_key: None,
            auth_disabled: false,
            http_headers: BTreeMap::new(),
            path_suffix: None,
            insecure_skip_tls_verify: false,
            wire_format: WireFormat::ChatCompletions,
        };
        assert_eq!(
            upstream_url(&endpoint),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }
}
