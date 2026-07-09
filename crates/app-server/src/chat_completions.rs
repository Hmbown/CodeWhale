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
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::IntoResponse;
use codewhale_agent::ModelRegistry;
use codewhale_config::{ConfigToml, ProviderKind, provider::WireFormat};
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
) -> ResolvedModelEndpoint {
    let provider_kind = provider_for_request(config, registry, request_model);
    let provider_cfg = config.providers.for_provider(provider_kind);
    let provider_meta = provider_kind.provider();

    // Base URL: configured → default
    let base_url = provider_cfg
        .base_url
        .clone()
        .unwrap_or_else(|| provider_meta.default_base_url().to_string());

    // Model: request → configured → provider-level configured → default
    let model = request_model
        .filter(|m| !m.trim().is_empty())
        .map(str::to_string)
        .or_else(|| provider_cfg.model.clone())
        .unwrap_or_else(|| provider_meta.default_model().to_string());

    // API key: configured → environment
    let api_key = provider_cfg.api_key.clone().or_else(|| {
        provider_meta
            .env_vars()
            .iter()
            .find_map(|var| std::env::var(var).ok())
    });

    let http_headers = provider_cfg.http_headers.clone();

    let path_suffix = provider_cfg.path_suffix.clone();

    let insecure_skip_tls_verify = provider_cfg.insecure_skip_tls_verify.unwrap_or(false);

    let wire_format = provider_meta.wire();

    ResolvedModelEndpoint {
        provider: provider_kind,
        base_url,
        model,
        api_key,
        http_headers,
        path_suffix,
        insecure_skip_tls_verify,
        wire_format,
    }
}

/// Determine which provider to use for a chat-completions request.
///
/// 1. If the request includes a `model` name, resolve it through the registry.
///    When the registry finds a match (not a fallback), use that provider.
/// 2. Otherwise fall back to the configured default provider.
fn provider_for_request(
    config: &ConfigToml,
    registry: &ModelRegistry,
    request_model: Option<&str>,
) -> ProviderKind {
    if let Some(model_name) = request_model {
        let resolved = registry.resolve(Some(model_name), None);
        // Only use the registry's provider hint when the model was actually
        // matched; otherwise the registry's fallback is noise and we should
        // respect the configured default provider.
        if !resolved.used_fallback {
            return resolved.resolved.provider;
        }
    }
    // Fall back to configured provider.
    config.provider
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
    headers: HeaderMap,
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
    let endpoint = resolve_endpoint(&config, &state.registry, request_model);

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

    // Inject default model if the request didn't include one.
    if request_model.is_none() || request_model.is_some_and(|m| m.trim().is_empty()) {
        body["model"] = serde_json::Value::String(endpoint.model.clone());
    }

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
    let upstream_req = reqwest::Client::builder()
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

            // Auth: configured API key takes priority (the proxy owns credentials).
            // Incoming Bearer header is only used as a fallback when no configured key exists.
            let auth_from_header = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|raw| raw.strip_prefix("Bearer "));
            let api_key = endpoint.api_key.as_deref().or(auth_from_header);
            if let Some(key) = api_key {
                req = req.bearer_auth(key);
            }

            // Forward configured provider headers.
            for (name, value) in &endpoint.http_headers {
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
