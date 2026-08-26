use axum::Json;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::RuntimeApiState;

const RUNTIME_TOKEN_COOKIE: &str = "codewhale_runtime_token";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedRuntimeAuth {
    pub(super) token: Option<String>,
    pub(super) generated: bool,
}

pub(super) fn resolve_runtime_auth(
    cli_token: Option<String>,
    env_token: Option<String>,
    insecure_no_auth: bool,
) -> ResolvedRuntimeAuth {
    if let Some(token) = first_nonblank_token(cli_token).or_else(|| first_nonblank_token(env_token))
    {
        return ResolvedRuntimeAuth {
            token: Some(token),
            generated: false,
        };
    }
    if insecure_no_auth {
        return ResolvedRuntimeAuth {
            token: None,
            generated: false,
        };
    }
    ResolvedRuntimeAuth {
        token: Some(generate_runtime_token()),
        generated: true,
    }
}

pub(super) fn runtime_auth_status_lines(auth: &ResolvedRuntimeAuth) -> Vec<String> {
    if auth.generated {
        return vec![
            "Runtime API auth: generated bearer token for this process (not printed).".to_string(),
            "  Set CODEWHALE_RUNTIME_TOKEN (or DEEPSEEK_RUNTIME_TOKEN as an alias) or pass --auth-token when another client needs to connect.".to_string(),
        ];
    }
    if auth.token.is_some() {
        return vec!["Runtime API auth: bearer token required for /v1/* routes.".to_string()];
    }
    vec!["Runtime API auth: disabled by explicit insecure mode.".to_string()]
}

fn first_nonblank_token(token: Option<String>) -> Option<String> {
    token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn generate_runtime_token() -> String {
    format!(
        "cwrt_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub(super) async fn require_runtime_token(
    State(state): State<RuntimeApiState>,
    req: Request,
    next: Next,
) -> Response {
    if runtime_request_is_authorized(&req, &state) {
        next.run(req).await
    } else {
        runtime_token_required_response()
    }
}

pub(super) fn runtime_request_is_authorized(req: &Request, state: &RuntimeApiState) -> bool {
    let Some(expected) = state.runtime_token.as_deref() else {
        return true;
    };
    let cookie_authorized = request_has_runtime_cookie(req, expected)
        || state.web.as_ref().is_some_and(|web| {
            web.matches_session_cookie(
                req.headers()
                    .get(header::COOKIE)
                    .and_then(|value| value.to_str().ok()),
            )
        });
    request_has_header_runtime_token(req, expected)
        || (cookie_authorized && web_cookie_request_is_same_origin(req, state))
}

fn request_has_header_runtime_token(req: &Request, expected: &str) -> bool {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected)
        || req
            .headers()
            .get("x-codewhale-runtime-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|token| token == expected)
        || req
            .headers()
            .get("x-deepseek-runtime-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|token| token == expected)
}

fn request_has_runtime_cookie(req: &Request, expected: &str) -> bool {
    token_from_cookie_header(
        req.headers()
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok()),
    )
    .is_some_and(|token| token == expected)
}

/// The web bootstrap adds cookie authentication to the existing bearer/header
/// boundary. SameSite is site-scoped rather than origin-scoped, so a sibling
/// loopback port can still receive the cookie. Require browser-origin evidence
/// for unsafe methods and reject Fetch Metadata that identifies any
/// cross-origin cookie request. Bearer and explicit runtime-token headers keep
/// their existing behavior.
fn web_cookie_request_is_same_origin(req: &Request, state: &RuntimeApiState) -> bool {
    if state.web.is_none() {
        return true;
    }

    if req
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| !site.eq_ignore_ascii_case("same-origin"))
    {
        return false;
    }

    if let Some(origin) = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        return web_cookie_origin_is_allowed(
            origin,
            &state.bind_host,
            state.bind_port,
            &state.web_public_origins,
        );
    }

    matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS)
}

pub(super) fn web_loopback_origin(bind_host: &str, bind_port: u16) -> String {
    if bind_port == 80 {
        format!("http://{bind_host}")
    } else {
        format!("http://{bind_host}:{bind_port}")
    }
}

pub(super) fn web_cookie_origin_is_allowed(
    origin: &str,
    bind_host: &str,
    bind_port: u16,
    public_origins: &[String],
) -> bool {
    origin == web_loopback_origin(bind_host, bind_port)
        || public_origins.iter().any(|allowed| allowed == origin)
}

pub(super) fn host_matches_public_origin(host: &str, public_origins: &[String]) -> bool {
    let request_host = host.split(':').next().unwrap_or(host);
    public_origins.iter().any(|origin| {
        origin
            .split_once("://")
            .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
            .map(|origin_host| origin_host.split(':').next().unwrap_or(origin_host))
            .is_some_and(|origin_host| origin_host.eq_ignore_ascii_case(request_host))
    })
}

fn runtime_token_required_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "message": "runtime API bearer token required",
                "status": StatusCode::UNAUTHORIZED.as_u16(),
            }
        })),
    )
        .into_response()
}

pub(super) fn token_from_cookie_header(cookie: Option<&str>) -> Option<String> {
    cookie.and_then(|cookie| {
        cookie.split(';').find_map(|pair| {
            let pair = pair.trim();
            let (key, value) = pair.split_once('=')?;
            (key == RUNTIME_TOKEN_COOKIE)
                .then(|| percent_decode_query_component(value.trim()))
                .flatten()
        })
    })
}

fn percent_decode_query_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hi = *bytes.get(index + 1)?;
                let lo = *bytes.get(index + 2)?;
                let hi = (hi as char).to_digit(16)? as u8;
                let lo = (lo as char).to_digit(16)? as u8;
                decoded.push((hi << 4) | lo);
                index += 3;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_origin_allows_loopback_and_explicit_tailnet() {
        let tailnet = "https://codewhale.tailnet.ts.net".to_string();
        assert!(web_cookie_origin_is_allowed(
            "http://127.0.0.1:7878",
            "127.0.0.1",
            7878,
            &[tailnet.clone()],
        ));
        assert!(web_cookie_origin_is_allowed(
            &tailnet,
            "127.0.0.1",
            7878,
            &[tailnet.clone()],
        ));
        assert!(!web_cookie_origin_is_allowed(
            "https://evil.example",
            "127.0.0.1",
            7878,
            &[tailnet],
        ));
    }

    #[test]
    fn public_origin_host_match_is_host_only() {
        let origins = ["https://codewhale.tailnet.ts.net".to_string()];
        assert!(host_matches_public_origin(
            "codewhale.tailnet.ts.net",
            &origins
        ));
        assert!(host_matches_public_origin(
            "codewhale.tailnet.ts.net:443",
            &origins
        ));
        assert!(!host_matches_public_origin("127.0.0.1:7878", &origins));
        assert!(!host_matches_public_origin("evil.example", &origins));
    }
}
