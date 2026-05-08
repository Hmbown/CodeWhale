//! Direct-fetch HTTP tool. Complements `web_search` for cases where the user
//! already knows the URL — a known repo, a blog post, a spec page — and
//! search is overkill or actively unhelpful.
//!
//! Returns a structured `{url, status, content_type, content, truncated}`
//! payload. HTML responses are stripped to readable text by default
//! (`format = "markdown"`); pass `format = "raw"` to keep the bytes intact
//! when the model wants to do its own parsing.

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_u64,
};
use crate::network_policy::{Decision, NetworkPolicyDecider};
use async_trait::async_trait;
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::Duration;

const DEFAULT_MAX_BYTES: u64 = 1_000_000;
const HARD_MAX_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const HARD_MAX_TIMEOUT_MS: u64 = 60_000;
const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str =
    "Mozilla/5.0 (compatible; deepseek-tui/0.5; +https://github.com/Hmbown/DeepSeek-TUI)";

static SCRIPT_RE: OnceLock<Regex> = OnceLock::new();
static STYLE_RE: OnceLock<Regex> = OnceLock::new();
static TAG_RE: OnceLock<Regex> = OnceLock::new();
static WHITESPACE_RE: OnceLock<Regex> = OnceLock::new();

fn script_re() -> &'static Regex {
    SCRIPT_RE.get_or_init(|| Regex::new(r"(?is)<script[^>]*>.*?</script>").expect("script re"))
}
fn style_re() -> &'static Regex {
    STYLE_RE.get_or_init(|| Regex::new(r"(?is)<style[^>]*>.*?</style>").expect("style re"))
}
fn tag_re() -> &'static Regex {
    TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").expect("tag re"))
}
fn whitespace_re() -> &'static Regex {
    WHITESPACE_RE.get_or_init(|| Regex::new(r"\s+").expect("ws re"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Markdown,
    Raw,
}

impl Format {
    fn parse(value: Option<&str>) -> Result<Self, ToolError> {
        match value
            .unwrap_or("markdown")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "text" | "txt" | "plain" => Ok(Self::Text),
            "markdown" | "md" => Ok(Self::Markdown),
            "raw" | "html" | "bytes" => Ok(Self::Raw),
            other => Err(ToolError::invalid_input(format!(
                "unknown format `{other}` (allowed: text, markdown, raw)"
            ))),
        }
    }
}

#[derive(Debug, Serialize)]
struct FetchResponse {
    url: String,
    status: u16,
    content_type: String,
    content: String,
    truncated: bool,
}

pub struct FetchUrlTool;

#[async_trait]
impl ToolSpec for FetchUrlTool {
    fn name(&self) -> &'static str {
        "fetch_url"
    }

    fn description(&self) -> &'static str {
        "Fetch a known URL directly (HTTP GET) and return its content. Use this when the user gives a URL or you already know the canonical link — it's faster and more reliable than web_search for known pages."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute HTTP/HTTPS URL to fetch."
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "markdown", "raw"],
                    "description": "Post-processing for the response body. `markdown` (default) and `text` strip HTML tags to readable text; `raw` returns the body bytes as-is."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Truncate response body after this many bytes (default 1,000,000; hard max 10,485,760)."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Request timeout in milliseconds (default 15,000; max 60,000)."
                }
            },
            "required": ["url"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Network]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid_input("`url` is required"))?
            .trim()
            .to_string();

        if url.is_empty() {
            return Err(ToolError::invalid_input("`url` cannot be empty"));
        }
        let scheme_ok = url.starts_with("http://") || url.starts_with("https://");
        if !scheme_ok {
            return Err(ToolError::invalid_input(
                "only http:// and https:// URLs are supported",
            ));
        }

        let format = Format::parse(input.get("format").and_then(Value::as_str))?;
        let max_bytes = optional_u64(&input, "max_bytes", DEFAULT_MAX_BYTES).min(HARD_MAX_BYTES);
        let timeout_ms =
            optional_u64(&input, "timeout_ms", DEFAULT_TIMEOUT_MS).min(HARD_MAX_TIMEOUT_MS);
        let mut current_url = reqwest::Url::parse(&url)
            .map_err(|e| ToolError::invalid_input(format!("invalid URL: {e}")))?;
        let mut redirects_followed = 0usize;

        let resp = loop {
            let dns_pinning = validate_fetch_target(&current_url, context).await?;
            let mut client_builder = reqwest::Client::builder()
                .timeout(Duration::from_millis(timeout_ms))
                .user_agent(USER_AGENT)
                .redirect(reqwest::redirect::Policy::none());

            // Pin validated IP to prevent DNS rebinding (TOCTOU) — reqwest will
            // connect to the validated IP directly instead of re-resolving.
            if let Some((hostname, validated_ip)) = dns_pinning {
                client_builder =
                    client_builder.resolve(&hostname, std::net::SocketAddr::new(validated_ip, 0));
            }

            let client = client_builder.build().map_err(|e| {
                ToolError::execution_failed(format!("failed to build HTTP client: {e}"))
            })?;

            let resp = client
                .get(current_url.clone())
                .header("Accept", "text/html,text/plain,application/json,*/*;q=0.5")
                .header("Accept-Language", "en-US,en;q=0.5")
                .send()
                .await
                .map_err(|e| ToolError::execution_failed(format!("request failed: {e}")))?;

            if !resp.status().is_redirection() || redirects_followed >= MAX_REDIRECTS {
                break resp;
            }

            let Some(location) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
            else {
                break resp;
            };

            current_url = resp.url().join(location).map_err(|e| {
                ToolError::execution_failed(format!("invalid redirect location: {e}"))
            })?;
            redirects_followed += 1;
        };

        let final_url = resp.url().to_string();
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ToolError::execution_failed(format!("failed to read body: {e}")))?;
        let total_bytes = bytes.len() as u64;
        let truncated = total_bytes > max_bytes;
        let usable = if truncated {
            &bytes[..max_bytes as usize]
        } else {
            &bytes[..]
        };

        let body_text = String::from_utf8_lossy(usable).to_string();
        let processed = match format {
            Format::Raw => body_text,
            Format::Text | Format::Markdown => {
                if content_type.contains("text/html") || body_text.contains("<html") {
                    html_to_text(&body_text)
                } else {
                    body_text
                }
            }
        };

        let response = FetchResponse {
            url: final_url,
            status: status.as_u16(),
            content_type,
            content: processed,
            truncated,
        };

        if !status.is_success() {
            // Don't `Err` on 4xx/5xx — the caller often wants to see the body
            // (e.g. a JSON error envelope). Mark the result as a failure so the
            // engine renders it as such.
            return Ok(ToolResult {
                content: serde_json::to_string_pretty(&response).map_err(|e| {
                    ToolError::execution_failed(format!("failed to serialize response: {e}"))
                })?,
                success: false,
                metadata: None,
            });
        }

        ToolResult::json(&response)
            .map_err(|e| ToolError::execution_failed(format!("failed to serialize response: {e}")))
    }
}

/// Check if an IP address is loopback, private, link-local, cloud-metadata,
/// multicast, or reserved — all addresses that should not be reachable via
/// an LLM-initiated fetch_url request (SSRF prevention).
fn is_restricted_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                // 100.64.0.0/10 — Carrier-grade NAT (CGNAT / shared address space)
                || matches!(v4.octets(), [100, 64..=127, ..])
                // 169.254.169.254 — cloud metadata (AWS/GCP/Azure)
                || *ip == std::net::IpAddr::V4(std::net::Ipv4Addr::new(169, 254, 169, 254))
                // 198.18.0.0/15 — IETF benchmark testing
                || matches!(v4.octets(), [198, 18..=19, ..])
                // 240.0.0.0/4 — reserved (former Class E)
                || v4.octets()[0] >= 240
        }
        std::net::IpAddr::V6(v6) => {
            // IPv4-mapped IPv6 addresses (::ffff:a.b.c.d) — unwrap and check as IPv4
            // to prevent bypass via ::ffff:127.0.0.1 etc.
            if v6.is_unspecified()
                || matches!(v6.octets(), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, ..])
            {
                return true;
            }
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_restricted_ip(&std::net::IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_multicast()
                || matches!(v6.segments(), [0xfc00..=0xfdff, ..]) // ULA fc00::/7
                || matches!(v6.segments(), [0xfe80..=0xfebf, ..]) // Link-local fe80::/10
        }
    }
}

async fn validate_fetch_target(
    url: &reqwest::Url,
    context: &ToolContext,
) -> Result<Option<(String, std::net::IpAddr)>, ToolError> {
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ToolError::invalid_input(
            "only http:// and https:// URLs are supported",
        ));
    }

    let host = url
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| ToolError::invalid_input("URL must include a host"))?;

    validate_network_policy(&host, context)?;

    // SSRF protection: resolve hostname and reject private/link-local/loopback IPs.
    // Prevents LLM-prompted requests to cloud metadata (169.254.169.254),
    // localhost services, and internal networks.
    if host == "localhost" || host == "localhost.localdomain" {
        return Err(ToolError::permission_denied(
            "requests to localhost are not allowed",
        ));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_restricted_ip(&ip) {
            return Err(ToolError::permission_denied(format!(
                "IP {ip} is a restricted address (private/loopback/link-local)"
            )));
        }
        return Ok(None);
    }

    let mut first_valid: Option<std::net::IpAddr> = None;
    if let Ok(addrs) = tokio::net::lookup_host((host.as_str(), 0u16)).await {
        for addr in addrs {
            validate_dns_resolved_ip(&host, &addr.ip(), context.network_policy.as_ref())?;
            if first_valid.is_none() {
                first_valid = Some(addr.ip());
            }
        }
    }

    // If DNS resolution fails, let the HTTP request proceed and fail naturally.
    Ok(first_valid.map(|validated_ip| (host, validated_ip)))
}

fn validate_network_policy(host: &str, context: &ToolContext) -> Result<(), ToolError> {
    let Some(decider) = context.network_policy.as_ref() else {
        return Ok(());
    };

    match decider.evaluate(host, "fetch_url") {
        Decision::Allow => Ok(()),
        Decision::Deny => Err(ToolError::permission_denied(format!(
            "network call to '{host}' blocked by network policy"
        ))),
        Decision::Prompt => Err(ToolError::permission_denied(format!(
            "network call to '{host}' requires approval; \
             re-run after `/network allow {host}` or set network.default = \"allow\" in config"
        ))),
    }
}

fn validate_dns_resolved_ip(
    host: &str,
    ip: &std::net::IpAddr,
    decider: Option<&NetworkPolicyDecider>,
) -> Result<(), ToolError> {
    if !is_restricted_ip(ip) {
        return Ok(());
    }

    if let Some(decider) = decider
        && decider.trusts_proxy_fakeip_host(host)
    {
        decider.record_trusted_proxy_fakeip_allow(host, "fetch_url");
        return Ok(());
    }

    Err(ToolError::permission_denied(format!(
        "resolved IP {ip} is a restricted address (private/loopback/link-local)"
    )))
}

/// Strip `<script>` / `<style>` blocks, drop remaining tags, and collapse
/// whitespace. Good enough for "let the model read this page" — not a full
/// HTML-to-Markdown converter.
fn html_to_text(html: &str) -> String {
    let no_script = script_re().replace_all(html, "");
    let no_style = style_re().replace_all(&no_script, "");
    let no_tags = tag_re().replace_all(&no_style, " ");
    let decoded = decode_entities(&no_tags);
    whitespace_re()
        .replace_all(&decoded, " ")
        .trim()
        .to_string()
}

/// Decode the handful of HTML entities we expect to hit in stripped text.
/// Pulling in `html-escape` for the long tail isn't worth the dep weight.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::spec::ToolContext;
    use std::path::PathBuf;

    fn ctx() -> ToolContext {
        ToolContext::new(PathBuf::from("."))
    }

    #[test]
    fn html_to_text_strips_scripts_styles_and_tags() {
        let html = r#"
            <html>
              <head>
                <style>body { color: red; }</style>
                <script>alert("nope");</script>
              </head>
              <body>
                <h1>Hello &amp; welcome</h1>
                <p>This is <b>important</b>.</p>
              </body>
            </html>
        "#;
        let text = html_to_text(html);
        assert!(text.contains("Hello & welcome"));
        assert!(text.contains("This is important"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("color: red"));
    }

    #[test]
    fn format_parse_accepts_aliases_and_rejects_unknown() {
        assert_eq!(Format::parse(Some("markdown")).unwrap(), Format::Markdown);
        assert_eq!(Format::parse(Some("MD")).unwrap(), Format::Markdown);
        assert_eq!(Format::parse(Some("text")).unwrap(), Format::Text);
        assert_eq!(Format::parse(Some("raw")).unwrap(), Format::Raw);
        assert_eq!(Format::parse(None).unwrap(), Format::Markdown);
        assert!(Format::parse(Some("yaml")).is_err());
    }

    #[tokio::test]
    async fn rejects_non_http_schemes() {
        let tool = FetchUrlTool;
        let res = tool
            .execute(json!({"url": "file:///etc/passwd"}), &ctx())
            .await;
        let err = res.unwrap_err();
        assert!(format!("{err:?}").contains("http"));
    }

    #[tokio::test]
    async fn rejects_empty_url() {
        let tool = FetchUrlTool;
        let res = tool.execute(json!({"url": "   "}), &ctx()).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn rejects_missing_url() {
        let tool = FetchUrlTool;
        let res = tool.execute(json!({}), &ctx()).await;
        assert!(res.is_err());
    }

    #[test]
    fn rejects_private_localhost_literal() {
        assert!(is_restricted_ip(&"127.0.0.1".parse().unwrap()));
        assert!(is_restricted_ip(&"::1".parse().unwrap()));
    }

    #[test]
    fn rejects_private_rfc1918() {
        assert!(is_restricted_ip(&"10.0.0.1".parse().unwrap()));
        assert!(is_restricted_ip(&"172.16.0.1".parse().unwrap()));
        assert!(is_restricted_ip(&"192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn rejects_cloud_metadata() {
        assert!(is_restricted_ip(&"169.254.169.254".parse().unwrap()));
    }

    #[test]
    fn rejects_link_local() {
        assert!(is_restricted_ip(&"169.254.1.1".parse().unwrap()));
    }

    #[test]
    fn rejects_cgnat() {
        assert!(is_restricted_ip(&"100.64.0.1".parse().unwrap()));
        assert!(!is_restricted_ip(&"100.63.0.1".parse().unwrap()));
        assert!(!is_restricted_ip(&"100.128.0.1".parse().unwrap()));
    }

    #[test]
    fn rejects_ipv6_ula() {
        assert!(is_restricted_ip(&"fc00::1".parse().unwrap()));
        assert!(is_restricted_ip(&"fd12:3456::1".parse().unwrap()));
    }

    #[test]
    fn rejects_ipv4_mapped_ipv6() {
        // ::ffff:127.0.0.1 — IPv4-mapped IPv6 loopback bypass
        assert!(is_restricted_ip(&"::ffff:127.0.0.1".parse().unwrap()));
        assert!(is_restricted_ip(&"::ffff:10.0.0.1".parse().unwrap()));
        assert!(is_restricted_ip(&"::ffff:169.254.169.254".parse().unwrap()));
        assert!(is_restricted_ip(&"::ffff:192.168.1.1".parse().unwrap()));
        // :: (unspecified)
        assert!(is_restricted_ip(&"::".parse().unwrap()));
    }

    #[test]
    fn allows_public_ips() {
        assert!(!is_restricted_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_restricted_ip(&"1.1.1.1".parse().unwrap()));
        assert!(!is_restricted_ip(&"93.184.216.34".parse().unwrap()));
        assert!(!is_restricted_ip(&"2606:4700::1".parse().unwrap()));
    }

    #[tokio::test]
    async fn rejects_localhost_hostname() {
        let tool = FetchUrlTool;
        let res = tool
            .execute(json!({"url": "http://localhost:8080/admin"}), &ctx())
            .await;
        let err = res.unwrap_err();
        assert!(format!("{err}").contains("localhost"));
    }

    #[tokio::test]
    async fn network_policy_denies_blocked_host() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};
        let policy = NetworkPolicy {
            default: Decision::Deny.into(),
            allow: vec!["api.deepseek.com".to_string()],
            deny: vec![],
            proxy: Vec::new(),
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ctx = ToolContext::new(PathBuf::from(".")).with_network_policy(decider);
        let tool = FetchUrlTool;
        let res = tool
            .execute(json!({"url": "https://example.com/foo"}), &ctx)
            .await;
        let err = res.expect_err("blocked host should fail");
        assert!(format!("{err}").contains("blocked"));
    }

    #[tokio::test]
    async fn redirected_localhost_hostname_is_rejected() {
        let url = reqwest::Url::parse("http://localhost:8080/admin").unwrap();
        let err = validate_fetch_target(&url, &ctx()).await.unwrap_err();
        assert!(format!("{err}").contains("localhost"));
    }

    #[tokio::test]
    async fn redirected_private_ip_literal_is_rejected() {
        let url = reqwest::Url::parse("http://169.254.169.254/latest/meta-data").unwrap();
        let err = validate_fetch_target(&url, &ctx()).await.unwrap_err();
        assert!(format!("{err}").contains("restricted address"));
    }

    #[tokio::test]
    async fn redirected_host_respects_network_policy() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};
        let policy = NetworkPolicy {
            default: Decision::Deny.into(),
            allow: vec!["api.deepseek.com".to_string()],
            deny: vec![],
            proxy: Vec::new(),
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ctx = ToolContext::new(PathBuf::from(".")).with_network_policy(decider);
        let url = reqwest::Url::parse("https://example.com/redirect-target").unwrap();
        let err = validate_fetch_target(&url, &ctx).await.unwrap_err();
        assert!(format!("{err}").contains("blocked"));
    }

    #[test]
    fn restricted_dns_result_is_denied_without_proxy_opt_in() {
        let ip = "198.18.0.1".parse().unwrap();

        let err = validate_dns_resolved_ip("github.com", &ip, None)
            .expect_err("fake-IP DNS result must be denied by default");

        assert!(format!("{err}").contains("resolved IP 198.18.0.1 is a restricted address"));
    }

    #[test]
    fn proxy_opt_in_allows_restricted_dns_for_matching_host() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: vec!["github.com".to_string()],
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ip = "198.18.0.1".parse().unwrap();

        validate_dns_resolved_ip("github.com", &ip, Some(&decider))
            .expect("proxy opt-in should allow fake-IP DNS for matching host");
    }

    #[test]
    fn proxy_opt_in_does_not_allow_unlisted_host() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: vec!["github.com".to_string()],
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ip = "198.18.0.1".parse().unwrap();

        let err = validate_dns_resolved_ip("example.com", &ip, Some(&decider))
            .expect_err("proxy opt-in must be scoped to configured hosts");

        assert!(format!("{err}").contains("resolved IP 198.18.0.1 is a restricted address"));
    }

    #[tokio::test]
    async fn proxy_opt_in_does_not_allow_restricted_ip_literal() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: vec!["198.18.0.1".to_string()],
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ctx = ToolContext::new(PathBuf::from(".")).with_network_policy(decider);
        let tool = FetchUrlTool;

        let err = tool
            .execute(json!({"url": "http://198.18.0.1/status"}), &ctx)
            .await
            .expect_err("literal restricted IP URLs must stay blocked");

        assert!(format!("{err}").contains("IP 198.18.0.1 is a restricted address"));
    }

    #[test]
    fn proxy_dns_allow_is_audited() {
        use crate::network_policy::{
            Decision, NetworkAuditor, NetworkPolicy, NetworkPolicyDecider,
        };
        use tempfile::tempdir;

        let dir = tempdir().expect("tempdir");
        let auditor = NetworkAuditor::new(dir.path().join("audit.log"), true);
        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: vec!["github.com".to_string()],
            audit: true,
        };
        let decider = NetworkPolicyDecider::new(policy, Some(auditor));
        let ip = "198.18.0.1".parse().unwrap();

        validate_dns_resolved_ip("github.com", &ip, Some(&decider)).expect("proxy DNS allow");

        let body = std::fs::read_to_string(dir.path().join("audit.log")).expect("audit log");
        assert!(body.contains("github.com"));
        assert!(body.contains("TrustedProxyFakeIp-Allow"));
    }
}
