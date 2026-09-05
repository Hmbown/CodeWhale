use axum::Json;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::{RuntimeApiState, mobile};

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
    if request_has_header_runtime_token(req, expected) {
        return true;
    }
    if state.web.as_ref().is_some_and(|web| {
        web.matches_session_cookie(
            req.headers()
                .get(header::COOKIE)
                .and_then(|value| value.to_str().ok()),
        ) && web_cookie_request_is_same_origin(req, state)
    }) {
        return true;
    }
    state
        .mobile
        .as_ref()
        .is_some_and(|mobile| mobile_session_request_is_authorized(req, state, mobile))
}

pub(super) fn request_has_header_runtime_token(req: &Request, expected: &str) -> bool {
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

/// The web bootstrap adds cookie authentication to the existing bearer/header
/// boundary. SameSite is site-scoped rather than origin-scoped, so a sibling
/// loopback port can still receive the cookie. Require browser-origin evidence
/// for unsafe methods and reject Fetch Metadata that identifies any
/// cross-origin cookie request. Bearer and explicit runtime-token headers keep
/// their existing behavior.
fn web_cookie_request_is_same_origin(req: &Request, state: &RuntimeApiState) -> bool {
    if req
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| !site.eq_ignore_ascii_case("same-origin"))
    {
        return false;
    }

    let expected_origin = runtime_http_origin(state);
    if let Some(origin) = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        return origin == expected_origin;
    }

    matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Mobile cookies are host-scoped and can be attached to a sibling loopback
/// port. A cookie alone is therefore never Runtime authority: normal fetches
/// must also present an origin-scoped proof and EventSource requests must
/// consume a short-lived stream ticket. The origin/Fetch Metadata check keeps
/// a sibling port from replaying a captured value.
pub(super) fn mobile_session_request_is_authorized(
    req: &Request,
    state: &RuntimeApiState,
    mobile_state: &mobile::RuntimeMobileState,
) -> bool {
    if !mobile_cookie_request_is_same_origin(req, state) {
        return false;
    }
    let cookie_header = req
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok());
    if is_mobile_stream_request(req) {
        return mobile_state.consume_stream_ticket(cookie_header, mobile_stream_ticket(req));
    }
    mobile_state.matches_request(
        cookie_header,
        req.headers()
            .get(mobile::MOBILE_REQUEST_HEADER)
            .and_then(|value| value.to_str().ok()),
    )
}

fn mobile_cookie_request_is_same_origin(req: &Request, state: &RuntimeApiState) -> bool {
    let fetch_metadata_is_same_origin = req
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site.eq_ignore_ascii_case("same-origin"));
    if req
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| !site.eq_ignore_ascii_case("same-origin"))
    {
        return false;
    }

    match req
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(origin) => origin == runtime_http_origin(state),
        None => fetch_metadata_is_same_origin,
    }
}

fn runtime_http_origin(state: &RuntimeApiState) -> String {
    runtime_http_origin_for_bind(&state.bind_host, state.bind_port)
}

fn runtime_http_origin_for_bind(bind_host: &str, bind_port: u16) -> String {
    let host = match bind_host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(address)) => address.to_string(),
        Ok(std::net::IpAddr::V6(address)) => format!("[{address}]"),
        Err(_) => bind_host.to_string(),
    };
    if bind_port == 80 {
        format!("http://{host}")
    } else {
        format!("http://{host}:{bind_port}")
    }
}

fn is_mobile_stream_request(req: &Request) -> bool {
    req.method() == Method::GET
        && req.uri().path().starts_with("/v1/threads/")
        && req.uri().path().ends_with("/events")
}

fn mobile_stream_ticket(req: &Request) -> Option<&str> {
    let mut tickets = req
        .uri()
        .query()?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter_map(|(key, value)| (key == mobile::MOBILE_STREAM_TICKET_QUERY).then_some(value));
    let ticket = tickets.next()?;
    tickets.next().is_none().then_some(ticket)
}

pub(super) fn runtime_token_required_response() -> Response {
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

#[cfg(test)]
mod tests {
    use super::runtime_http_origin_for_bind;

    #[test]
    fn expected_origin_canonicalizes_ipv6_loopback_literals() {
        assert_eq!(
            runtime_http_origin_for_bind("0:0:0:0:0:0:0:1", 7878),
            "http://[::1]:7878"
        );
        assert_eq!(runtime_http_origin_for_bind("::1", 80), "http://[::1]");
    }
}
