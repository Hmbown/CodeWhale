//! Unified guarded fetch pipeline for `fetch_url` and `web.run`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
#[cfg(not(test))]
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;

use super::cache::{self, CachedFetch};
use super::guard::{
    DnsPin, guarded_reqwest_client_builder, validate_fetch_target, validate_network_policy,
};
use crate::tools::spec::{ToolContext, ToolError};

pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const HARD_MAX_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const DEFAULT_MAX_BYTES: usize = 1_000_000;
pub(crate) const HARD_MAX_BYTES: usize = 10 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; codewhale/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/Hmbown/CodeWhale)"
);

#[derive(Debug, Clone)]
pub(crate) struct FetchOptions {
    pub(crate) timeout: Duration,
    pub(crate) max_bytes: usize,
    pub(crate) accept: &'static str,
}

impl FetchOptions {
    pub(crate) fn new(timeout: Duration, max_bytes: usize, accept: &'static str) -> Self {
        Self {
            timeout: timeout.min(HARD_MAX_TIMEOUT),
            max_bytes: max_bytes.clamp(1, HARD_MAX_BYTES),
            accept,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FetchedPayload {
    pub(crate) url: String,
    pub(crate) status: u16,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) content_type: String,
    pub(crate) bytes: Arc<Vec<u8>>,
    pub(crate) truncated: bool,
    pub(crate) cache_hit: bool,
    pub(crate) retries: usize,
    pub(crate) redirects: usize,
}

pub(crate) async fn fetch(
    url: &str,
    options: &FetchOptions,
    context: &ToolContext,
    tool_label: &str,
) -> Result<FetchedPayload, ToolError> {
    fetch_inner(url, options, context, tool_label, None).await
}

#[cfg(test)]
pub(crate) async fn fetch_with_initial_pin(
    url: &str,
    options: &FetchOptions,
    context: &ToolContext,
    tool_label: &str,
    initial_pin: DnsPin,
) -> Result<FetchedPayload, ToolError> {
    fetch_inner(url, options, context, tool_label, Some(initial_pin)).await
}

async fn fetch_inner(
    url: &str,
    options: &FetchOptions,
    context: &ToolContext,
    tool_label: &str,
    test_initial_pin: Option<DnsPin>,
) -> Result<FetchedPayload, ToolError> {
    let initial_url = reqwest::Url::parse(url)
        .map_err(|err| ToolError::invalid_input(format!("invalid URL: {err}")))?;
    if !matches!(initial_url.scheme(), "http" | "https") {
        return Err(ToolError::invalid_input(
            "only http:// and https:// URLs are supported",
        ));
    }

    // Validation precedes cache lookup so a policy tightened during the
    // session cannot be bypassed by a previously cached response.
    let validated_initial_pin = match test_initial_pin {
        Some(pin) => pin,
        None => validate_fetch_target(&initial_url, context, tool_label).await?,
    };

    if let Some(cached) = cache::get(
        &context.state_namespace,
        &initial_url,
        options.accept,
        options.max_bytes,
    ) {
        let cached_url = reqwest::Url::parse(&cached.url).map_err(|err| {
            ToolError::execution_failed(format!("cached response URL was invalid: {err}"))
        })?;
        let cached_host = cached_url.host_str().ok_or_else(|| {
            ToolError::execution_failed("cached response URL did not include a host")
        })?;
        // No network request occurs on a cache hit, so DNS/SSRF validation is
        // unnecessary. The final redirect destination still needs a policy
        // check in case the session policy was tightened after insertion.
        validate_network_policy(cached_host, context, tool_label)?;
        return Ok(from_cached(cached, true, 0));
    }

    let deadline = Instant::now() + options.timeout;
    let mut last_transient = None;
    for attempt in 0..=1 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match fetch_attempt(
            initial_url.clone(),
            options,
            context,
            tool_label,
            remaining,
            validated_initial_pin.clone(),
        )
        .await
        {
            Ok(payload) if is_transient_status(payload.status) && attempt == 0 => {
                last_transient = Some(format!("HTTP {}", payload.status));
            }
            Ok(payload) => {
                let fetched = from_cached(payload.clone(), false, attempt);
                if (200..300).contains(&payload.status) {
                    cache::insert(
                        &context.state_namespace,
                        &initial_url,
                        options.accept,
                        payload,
                    );
                }
                return Ok(fetched);
            }
            Err(AttemptError::Fatal(error)) => return Err(error),
            Err(AttemptError::Transient(message)) if attempt == 0 => {
                last_transient = Some(message);
            }
            Err(AttemptError::Transient(message)) => {
                return Err(ToolError::execution_failed(format!(
                    "request failed after one retry: {message}"
                )));
            }
        }

        let delay = retry_delay();
        if deadline.saturating_duration_since(Instant::now()) <= delay {
            break;
        }
        tokio::time::sleep(delay).await;
    }

    Err(ToolError::execution_failed(format!(
        "request timed out before retry completed{}",
        last_transient
            .map(|message| format!(" (last failure: {message})"))
            .unwrap_or_default()
    )))
}

#[derive(Debug)]
enum AttemptError {
    Fatal(ToolError),
    Transient(String),
}

async fn fetch_attempt(
    initial_url: reqwest::Url,
    options: &FetchOptions,
    context: &ToolContext,
    tool_label: &str,
    timeout: Duration,
    initial_pin: DnsPin,
) -> Result<CachedFetch, AttemptError> {
    let mut current_url = initial_url;
    let mut redirects = 0usize;
    let mut initial_pin = initial_pin;
    let deadline = Instant::now() + timeout;

    let response = loop {
        let dns_pin = if redirects == 0 {
            match initial_pin.take() {
                Some(pin) => Some(pin),
                None => validate_fetch_target(&current_url, context, tool_label)
                    .await
                    .map_err(AttemptError::Fatal)?,
            }
        } else {
            validate_fetch_target(&current_url, context, tool_label)
                .await
                .map_err(AttemptError::Fatal)?
        };

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AttemptError::Transient(
                "request timed out while following redirects".to_string(),
            ));
        }
        let mut builder = guarded_reqwest_client_builder()
            .timeout(remaining)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none());
        if let Some((hostname, validated_ip)) = dns_pin {
            builder = builder.resolve(&hostname, std::net::SocketAddr::new(validated_ip, 0));
        }
        let client = builder.build().map_err(|err| {
            AttemptError::Fatal(ToolError::execution_failed(format!(
                "failed to build HTTP client: {err}"
            )))
        })?;
        let response = client
            .get(current_url.clone())
            .header("Accept", options.accept)
            .header("Accept-Language", "en-US,en;q=0.5")
            .send()
            .await
            .map_err(|err| AttemptError::Transient(err.to_string()))?;

        if !response.status().is_redirection() {
            break response;
        }
        if redirects >= MAX_REDIRECTS {
            return Err(AttemptError::Fatal(ToolError::execution_failed(
                "request exceeded the five-redirect limit",
            )));
        }
        let Some(location) = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
        else {
            break response;
        };
        current_url = response.url().join(location).map_err(|err| {
            AttemptError::Fatal(ToolError::execution_failed(format!(
                "invalid redirect location: {err}"
            )))
        })?;
        redirects += 1;
    };

    let final_url = response.url().to_string();
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let headers = response_headers(response.headers());
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::with_capacity(options.max_bytes.min(64 * 1024));
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| AttemptError::Transient(err.to_string()))?;
        let remaining = options.max_bytes.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() == options.max_bytes {
            // A response exactly at the cap may be complete. Ask for one more
            // chunk to distinguish exact length from actual truncation.
            if let Some(next) = stream.next().await {
                let next = next.map_err(|err| AttemptError::Transient(err.to_string()))?;
                truncated = !next.is_empty();
            }
            break;
        }
    }

    Ok(CachedFetch {
        url: final_url,
        status,
        headers,
        content_type,
        bytes: Arc::new(bytes),
        truncated,
        redirects,
    })
}

fn from_cached(payload: CachedFetch, cache_hit: bool, retries: usize) -> FetchedPayload {
    FetchedPayload {
        url: payload.url,
        status: payload.status,
        headers: payload.headers,
        content_type: payload.content_type,
        bytes: payload.bytes,
        truncated: payload.truncated,
        cache_hit,
        retries,
        redirects: payload.redirects,
    }
}

fn response_headers(headers: &reqwest::header::HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "authorization"
                    | "proxy-authorization"
                    | "cookie"
                    | "set-cookie"
                    | "set-cookie2"
                    | "x-api-key"
                    | "api-key"
            )
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

fn is_transient_status(status: u16) -> bool {
    (500..600).contains(&status)
}

fn retry_delay() -> Duration {
    #[cfg(test)]
    return Duration::ZERO;

    #[cfg(not(test))]
    {
        let jitter_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| u64::from(duration.subsec_nanos()) % 41)
            .unwrap_or(0);
        Duration::from_millis(30 + jitter_ms)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use super::*;

    fn context(namespace: &str) -> ToolContext {
        ToolContext::new(".").with_state_namespace(namespace)
    }

    fn pin() -> DnsPin {
        Some((
            "public.example".to_string(),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ))
    }

    #[derive(Clone)]
    struct FailOnce {
        calls: Arc<AtomicUsize>,
    }

    impl Respond for FailOnce {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503).set_body_json(json!({"error": "retry"}))
            } else {
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("recovered response")
            }
        }
    }

    #[tokio::test]
    async fn transient_server_error_retries_once_then_caches() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/retry"))
            .respond_with(FailOnce {
                calls: Arc::clone(&calls),
            })
            .mount(&server)
            .await;
        let url = format!("http://public.example:{}/retry", server.address().port());
        let options = FetchOptions::new(Duration::from_secs(5), 1_024, "text/plain");
        let context = context("fetch-retry-cache");

        let first = fetch_with_initial_pin(&url, &options, &context, "test", pin())
            .await
            .expect("retry succeeds");
        assert_eq!(first.status, 200);
        assert_eq!(first.retries, 1);
        assert!(!first.cache_hit);
        assert_eq!(&*first.bytes, b"recovered response");

        let second = fetch_with_initial_pin(&url, &options, &context, "test", pin())
            .await
            .expect("cache hit");
        assert!(second.cache_hit);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn truncated_cache_refetches_when_larger_body_is_requested() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/large"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("0123456789"),
            )
            .mount(&server)
            .await;
        let url = format!("http://public.example:{}/large", server.address().port());
        let context = context("fetch-truncated-refetch");

        let small = fetch_with_initial_pin(
            &url,
            &FetchOptions::new(Duration::from_secs(5), 4, "text/plain"),
            &context,
            "test",
            pin(),
        )
        .await
        .expect("small fetch");
        assert_eq!(&*small.bytes, b"0123");
        assert!(small.truncated);

        let large = fetch_with_initial_pin(
            &url,
            &FetchOptions::new(Duration::from_secs(5), 16, "text/plain"),
            &context,
            "test",
            pin(),
        )
        .await
        .expect("larger refetch");
        assert_eq!(&*large.bytes, b"0123456789");
        assert!(!large.truncated);
        assert!(!large.cache_hit);
    }

    #[test]
    fn fetch_user_agent_tracks_the_crate_version() {
        assert!(
            USER_AGENT.contains(concat!("codewhale/", env!("CARGO_PKG_VERSION"))),
            "guarded-fetch UA must never pin a stale release: {USER_AGENT}"
        );
    }

    #[test]
    fn response_headers_drop_set_cookie_values() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("content-type", "text/plain".parse().unwrap());
        headers.insert("set-cookie", "session=secret".parse().unwrap());
        headers.insert("x-api-key", "secret".parse().unwrap());

        let filtered = response_headers(&headers);

        assert_eq!(
            filtered.get("content-type").map(String::as_str),
            Some("text/plain")
        );
        assert!(!filtered.contains_key("set-cookie"));
        assert!(!filtered.contains_key("x-api-key"));
    }

    #[tokio::test]
    async fn tightened_network_policy_blocks_an_existing_cache_entry() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let url = reqwest::Url::parse("https://example.com/cached").unwrap();
        cache::insert(
            "policy-cache",
            &url,
            "text/plain",
            CachedFetch {
                url: url.to_string(),
                status: 200,
                headers: BTreeMap::new(),
                content_type: "text/plain".to_string(),
                bytes: Arc::new(b"cached".to_vec()),
                truncated: false,
                redirects: 0,
            },
        );
        let policy = NetworkPolicy {
            default: Decision::Deny.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: Vec::new(),
            proxy_fake_ip_cidrs: Vec::new(),
            audit: false,
        };
        let context =
            context("policy-cache").with_network_policy(NetworkPolicyDecider::new(policy, None));

        let error = fetch(
            url.as_str(),
            &FetchOptions::new(Duration::from_secs(1), 100, "text/plain"),
            &context,
            "fetch_url",
        )
        .await
        .expect_err("policy must win over cache");
        assert!(error.to_string().contains("blocked by network policy"));
    }

    #[tokio::test]
    async fn tightened_network_policy_checks_cached_redirect_destination() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let initial_url = reqwest::Url::parse("https://8.8.8.8/cached").unwrap();
        cache::insert(
            "redirect-policy-cache",
            &initial_url,
            "text/plain",
            CachedFetch {
                url: "https://1.1.1.1/redirected".to_string(),
                status: 200,
                headers: BTreeMap::new(),
                content_type: "text/plain".to_string(),
                bytes: Arc::new(b"cached".to_vec()),
                truncated: false,
                redirects: 1,
            },
        );
        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: vec!["1.1.1.1".to_string()],
            proxy: Vec::new(),
            proxy_fake_ip_cidrs: Vec::new(),
            audit: false,
        };
        let context = context("redirect-policy-cache")
            .with_network_policy(NetworkPolicyDecider::new(policy, None));

        let error = fetch(
            initial_url.as_str(),
            &FetchOptions::new(Duration::from_secs(1), 100, "text/plain"),
            &context,
            "fetch_url",
        )
        .await
        .expect_err("final redirect policy must win over cache");
        assert!(error.to_string().contains("1.1.1.1"));
        assert!(error.to_string().contains("blocked by network policy"));
    }
}
