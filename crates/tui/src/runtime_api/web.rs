//! Embedded, loopback-only browser client for the Runtime API.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use super::RuntimeApiState;

const WEB_HTML: &str = include_str!("../runtime_web/index.html");
const WEB_CSS: &str = include_str!("../runtime_web/styles.css");
const WEB_JS: &str = include_str!("../runtime_web/app.mjs");
const BOOTSTRAP_TTL: Duration = Duration::from_secs(120);
const WEB_SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const BOOTSTRAP_PREFIX: &str = "cwwb_";
const WEB_SESSION_PREFIX: &str = "cwws_";
const WEB_SESSION_COOKIE_NAME: &str = "codewhale_web_session";
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'";

#[derive(Clone)]
pub(super) struct RuntimeWebState {
    bootstrap: Arc<Mutex<Option<BootstrapCapability>>>,
    session_token: Arc<str>,
    session_expires_at: Instant,
}

struct BootstrapCapability {
    nonce: String,
    expires_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootstrapError {
    Invalid,
    Expired,
    NonLoopback,
}

impl RuntimeWebState {
    pub(super) fn new() -> (Self, String) {
        Self::new_with_ttls(BOOTSTRAP_TTL, WEB_SESSION_TTL)
    }

    fn new_with_ttls(bootstrap_ttl: Duration, session_ttl: Duration) -> (Self, String) {
        let nonce = format!("{BOOTSTRAP_PREFIX}{}", Uuid::new_v4().simple());
        let session_token = format!(
            "{WEB_SESSION_PREFIX}{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let state = Self {
            bootstrap: Arc::new(Mutex::new(Some(BootstrapCapability {
                nonce: nonce.clone(),
                expires_at: Instant::now() + bootstrap_ttl,
            }))),
            session_token: session_token.into(),
            session_expires_at: Instant::now() + session_ttl,
        };
        (state, nonce)
    }

    fn consume(&self, nonce: &str, peer_ip: IpAddr) -> Result<String, BootstrapError> {
        if !peer_ip.is_loopback() {
            return Err(BootstrapError::NonLoopback);
        }
        if !valid_bootstrap_nonce(nonce) {
            return Err(BootstrapError::Invalid);
        }

        let mut slot = self
            .bootstrap
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(capability) = slot.as_ref() else {
            return Err(BootstrapError::Invalid);
        };
        if Instant::now() >= capability.expires_at {
            *slot = None;
            return Err(BootstrapError::Expired);
        }
        if !constant_time_eq(nonce.as_bytes(), capability.nonce.as_bytes()) {
            return Err(BootstrapError::Invalid);
        }

        let _capability = slot.take().expect("bootstrap capability checked above");
        Ok(self.session_token.to_string())
    }

    pub(super) fn matches_session_cookie(&self, cookie_header: Option<&str>) -> bool {
        let presented = cookie_value(cookie_header, WEB_SESSION_COOKIE_NAME).unwrap_or_default();
        let token_matches = constant_time_eq(presented.as_bytes(), self.session_token.as_bytes());
        token_matches & (Instant::now() < self.session_expires_at)
    }
}

pub(super) fn bootstrap_url(addr: SocketAddr, nonce: &str) -> String {
    format!("http://{addr}/__codewhale/bootstrap/{nonce}")
}

pub(super) async fn exchange_bootstrap(
    State(state): State<RuntimeApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(nonce): Path<String>,
) -> Response {
    let Some(web) = state.web.as_ref() else {
        return not_found();
    };
    let session_token = match web.consume(&nonce, peer.ip()) {
        Ok(token) => token,
        Err(BootstrapError::NonLoopback) => {
            return secured_text(StatusCode::FORBIDDEN, "bootstrap unavailable");
        }
        Err(BootstrapError::Invalid | BootstrapError::Expired) => {
            return secured_text(StatusCode::UNAUTHORIZED, "bootstrap unavailable");
        }
    };

    let cookie = web_session_cookie(&session_token);
    let mut response = (StatusCode::SEE_OTHER, "").into_response();
    response
        .headers_mut()
        .insert(header::LOCATION, HeaderValue::from_static("/"));
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("percent-encoded Runtime cookie is a valid header"),
    );
    secure_headers(&mut response, "text/plain; charset=utf-8");
    response
}

pub(super) async fn web_page(State(state): State<RuntimeApiState>) -> Response {
    if state.web.is_none() {
        return not_found();
    }
    secured_asset("text/html; charset=utf-8", WEB_HTML)
}

pub(super) async fn web_styles(State(state): State<RuntimeApiState>) -> Response {
    if state.web.is_none() {
        return not_found();
    }
    secured_asset("text/css; charset=utf-8", WEB_CSS)
}

pub(super) async fn web_script(State(state): State<RuntimeApiState>) -> Response {
    if state.web.is_none() {
        return not_found();
    }
    secured_asset("text/javascript; charset=utf-8", WEB_JS)
}

fn web_session_cookie(session_token: &str) -> String {
    format!("{WEB_SESSION_COOKIE_NAME}={session_token}; HttpOnly; SameSite=Strict; Path=/")
}

fn cookie_value<'a>(cookie_header: Option<&'a str>, name: &str) -> Option<&'a str> {
    cookie_header.and_then(|cookie| {
        cookie.split(';').find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then_some(value.trim())
        })
    })
}

fn valid_bootstrap_nonce(value: &str) -> bool {
    value.strip_prefix(BOOTSTRAP_PREFIX).is_some_and(|random| {
        random.len() == 32 && random.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn secured_asset(content_type: &'static str, body: &'static str) -> Response {
    let mut response = body.into_response();
    secure_headers(&mut response, content_type);
    response
}

fn secured_text(status: StatusCode, body: &'static str) -> Response {
    let mut response = (status, body).into_response();
    secure_headers(&mut response, "text/plain; charset=utf-8");
    response
}

fn not_found() -> Response {
    secured_text(StatusCode::NOT_FOUND, "not found")
}

fn secure_headers(response: &mut Response, content_type: &'static str) {
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_is_loopback_only_one_time_and_expires() {
        let (state, nonce) =
            RuntimeWebState::new_with_ttls(Duration::from_secs(60), Duration::from_secs(60));
        assert_eq!(
            state.consume(&nonce, "192.0.2.4".parse().unwrap()),
            Err(BootstrapError::NonLoopback)
        );
        let session_token = state
            .consume(&nonce, "127.0.0.1".parse().unwrap())
            .expect("valid loopback bootstrap");
        assert!(session_token.starts_with(WEB_SESSION_PREFIX));
        assert!(state.matches_session_cookie(Some(&format!(
            "theme=dark; {WEB_SESSION_COOKIE_NAME}={session_token}"
        ))));
        assert_eq!(
            state.consume(&nonce, "127.0.0.1".parse().unwrap()),
            Err(BootstrapError::Invalid)
        );

        let (expired, expired_nonce) =
            RuntimeWebState::new_with_ttls(Duration::ZERO, Duration::from_secs(60));
        assert_eq!(
            expired.consume(&expired_nonce, "::1".parse().unwrap()),
            Err(BootstrapError::Expired)
        );
    }

    #[test]
    fn web_session_survives_reload_then_expires_and_rejects_wrong_tokens() {
        let (state, nonce) =
            RuntimeWebState::new_with_ttls(Duration::from_secs(60), Duration::from_secs(60));
        let session_token = state
            .consume(&nonce, "127.0.0.1".parse().unwrap())
            .expect("valid loopback bootstrap");
        let cookie = format!("{WEB_SESSION_COOKIE_NAME}={session_token}");
        assert!(state.matches_session_cookie(Some(&cookie)));
        assert!(
            state.matches_session_cookie(Some(&cookie)),
            "the same process-local session remains valid across a page reload"
        );
        assert!(!state.matches_session_cookie(Some(
            "codewhale_web_session=cwws_0000000000000000000000000000000000000000000000000000000000000000"
        )));

        let (expired, _nonce) =
            RuntimeWebState::new_with_ttls(Duration::from_secs(60), Duration::ZERO);
        let expired_cookie = format!(
            "{WEB_SESSION_COOKIE_NAME}={}",
            expired.session_token.as_ref()
        );
        assert!(!expired.matches_session_cookie(Some(&expired_cookie)));
    }

    #[test]
    fn bootstrap_rejects_malformed_or_wrong_capabilities_without_consuming() {
        let (state, nonce) = RuntimeWebState::new();
        for invalid in ["", "cwwb_short", "cwwb_gggggggggggggggggggggggggggggggg"] {
            assert_eq!(
                state.consume(invalid, "127.0.0.1".parse().unwrap()),
                Err(BootstrapError::Invalid)
            );
        }
        let mut wrong = nonce.clone();
        wrong.replace_range(wrong.len() - 1.., "0");
        if wrong == nonce {
            wrong.replace_range(wrong.len() - 1.., "1");
        }
        assert_eq!(
            state.consume(&wrong, "127.0.0.1".parse().unwrap()),
            Err(BootstrapError::Invalid)
        );
        assert!(
            state
                .consume(&nonce, "127.0.0.1".parse().unwrap())
                .expect("valid bootstrap remains available")
                .starts_with(WEB_SESSION_PREFIX)
        );
    }

    #[test]
    fn cookie_has_exact_security_attributes_without_the_runtime_bearer() {
        let session_token = format!("{WEB_SESSION_PREFIX}{}", "01".repeat(16));
        let runtime_bearer = "cwrt_runtime_secret_never_in_browser_storage";
        let cookie = web_session_cookie(&session_token);
        assert_eq!(
            cookie,
            format!("codewhale_web_session={session_token}; HttpOnly; SameSite=Strict; Path=/")
        );
        assert!(!cookie.contains(runtime_bearer));
        assert!(!cookie.contains("Domain="));
    }

    #[test]
    fn launcher_url_contains_only_the_one_time_capability() {
        let token = "cwrt_runtime_secret_never_in_browser_arguments";
        let nonce = format!("{BOOTSTRAP_PREFIX}{}", "01".repeat(16));
        let url = bootstrap_url("127.0.0.1:7878".parse().unwrap(), &nonce);
        assert!(url.ends_with(&nonce));
        assert!(!url.contains(token));
        assert!(!url.contains('?'));
        assert!(!url.contains('#'));
    }

    #[test]
    fn embedded_client_has_no_secret_storage_or_unsafe_dynamic_html_sink() {
        for asset in [WEB_HTML, WEB_JS] {
            assert!(!asset.contains("localStorage"));
            assert!(!asset.contains("sessionStorage"));
            assert!(!asset.contains("codewhale_runtime_token"));
            assert!(!asset.contains("innerHTML"));
            assert!(!asset.contains("http://"));
            assert!(!asset.contains("https://"));
        }
    }
}
