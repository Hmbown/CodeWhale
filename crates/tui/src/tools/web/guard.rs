//! Shared SSRF guard for LLM-initiated HTTP fetches (`fetch_url`, `web.run`).
//!
//! Validates scheme/host, enforces network policy, resolves DNS and rejects
//! private/loopback/link-local/metadata addresses, and returns an optional
//! DNS pin so callers can bind the HTTP client to the validated address
//! (preventing TOCTOU rebinding). Callers that follow redirects must
//! re-invoke [`validate_fetch_target`] on every new Location.

use crate::network_policy::{Decision, NetworkPolicyDecider};
use crate::tools::spec::{ToolContext, ToolError};
use std::net::IpAddr;

/// DNS pin returned when a hostname was resolved to a validated public IP.
/// Callers should pass this to `reqwest::ClientBuilder::resolve` so the
/// connection uses the pre-validated address instead of re-resolving.
pub(crate) type DnsPin = Option<(String, IpAddr)>;

/// Build the transport used after a destination has passed SSRF validation.
/// Ambient HTTP(S)/SOCKS proxies are deliberately disabled: a proxy would
/// receive the original hostname, resolve it again outside this process, and
/// bypass the validated DNS pin.
pub(crate) fn guarded_reqwest_client_builder() -> reqwest::ClientBuilder {
    crate::tls::reqwest_client_builder().no_proxy()
}

/// Check if an IP address is loopback, private, link-local, cloud-metadata,
/// multicast, or reserved — all addresses that should not be reachable via
/// an LLM-initiated fetch request (SSRF prevention).
pub(crate) fn is_restricted_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                // 100.64.0.0/10 — Carrier-grade NAT (CGNAT / shared address space)
                || matches!(v4.octets(), [100, 64..=127, ..])
                // 169.254.169.254 — cloud metadata (AWS/GCP/Azure)
                || *ip == IpAddr::V4(std::net::Ipv4Addr::new(169, 254, 169, 254))
                // 198.18.0.0/15 — IETF benchmark testing
                || matches!(v4.octets(), [198, 18..=19, ..])
                // 240.0.0.0/4 — reserved (former Class E)
                || v4.octets()[0] >= 240
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped IPv6 addresses (::ffff:a.b.c.d) — unwrap and check as IPv4
            // to prevent bypass via ::ffff:127.0.0.1 etc.
            if v6.is_unspecified()
                || matches!(v6.octets(), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, ..])
            {
                return true;
            }
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_restricted_ip(&IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_multicast()
                || matches!(v6.segments(), [0xfc00..=0xfdff, ..]) // ULA fc00::/7
                || matches!(v6.segments(), [0xfe80..=0xfebf, ..]) // Link-local fe80::/10
        }
    }
}

/// Validate that `url` is a safe fetch target under SSRF and network policy.
///
/// On success returns an optional DNS pin `(hostname, ip)` for hostnames that
/// were resolved; literal public IPs return `None` (no pin needed).
///
/// `tool` is the policy/audit label (e.g. `"fetch_url"`, `"web_run"`).
pub(crate) async fn validate_fetch_target(
    url: &reqwest::Url,
    context: &ToolContext,
    tool: &str,
) -> Result<DnsPin, ToolError> {
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ToolError::invalid_input(
            "only http:// and https:// URLs are supported",
        ));
    }

    let host = url
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| ToolError::invalid_input("URL must include a host"))?;

    validate_network_policy(&host, context, tool)?;

    // SSRF protection: resolve hostname and reject private/link-local/loopback IPs.
    // Prevents LLM-prompted requests to cloud metadata (169.254.169.254),
    // localhost services, and internal networks.
    if host == "localhost" || host == "localhost.localdomain" {
        return Err(ToolError::permission_denied(
            "requests to localhost are not allowed",
        ));
    }
    // Normalize bracketed IPv6 literals before the literal-IP check so they
    // route through the same restricted-IP policy as unbracketed forms
    // (GHSA-88gh-2526-gfrr).
    let ip_candidate = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host.as_str());
    if let Ok(ip) = ip_candidate.parse::<IpAddr>() {
        if is_restricted_ip(&ip) {
            return Err(ToolError::permission_denied(format!(
                "IP {ip} is a restricted address (private/loopback/link-local)"
            )));
        }
        return Ok(None);
    }

    let addrs = tokio::net::lookup_host((host.as_str(), 0u16))
        .await
        .map_err(|e| {
            ToolError::permission_denied(format!(
                "could not resolve host before {tool} request: {e}"
            ))
        })?;
    let mut first_valid: Option<IpAddr> = None;
    for addr in addrs {
        validate_dns_resolved_ip(&host, &addr.ip(), context.network_policy.as_ref(), tool)?;
        if first_valid.is_none() {
            first_valid = Some(addr.ip());
        }
    }

    let Some(validated_ip) = first_valid else {
        return Err(ToolError::permission_denied(format!(
            "host resolved to no addresses before {tool} request"
        )));
    };
    Ok(Some((host, validated_ip)))
}

pub(crate) fn validate_network_policy(
    host: &str,
    context: &ToolContext,
    tool: &str,
) -> Result<(), ToolError> {
    let Some(decider) = context.network_policy.as_ref() else {
        return Ok(());
    };

    match decider.evaluate(host, tool) {
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

pub(crate) fn validate_dns_resolved_ip(
    host: &str,
    ip: &IpAddr,
    decider: Option<&NetworkPolicyDecider>,
    tool: &str,
) -> Result<(), ToolError> {
    if !is_restricted_ip(ip) {
        return Ok(());
    }

    // A fake-IP exception requires both an explicitly trusted hostname and an
    // explicitly trusted placeholder CIDR. The CIDR parser admits only subnets
    // inside 198.18.0.0/15, so real private/loopback/link-local/metadata/ULA
    // addresses remain blocked even when the hostname is trusted.
    if let Some(decider) = decider
        && decider.is_trusted_fakeip_addr(ip)
        && decider.trusts_proxy_fakeip_host(host)
    {
        decider.record_trusted_proxy_fakeip_allow(host, tool);
        return Ok(());
    }

    Err(ToolError::permission_denied(format!(
        "resolved IP {ip} is a restricted address (private/loopback/link-local)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::spec::ToolContext;
    #[cfg(not(windows))]
    use std::io::{Read, Write};
    #[cfg(not(windows))]
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::path::PathBuf;
    #[cfg(not(windows))]
    use std::process::Command;
    #[cfg(not(windows))]
    use std::sync::Arc;
    #[cfg(not(windows))]
    use std::sync::atomic::{AtomicBool, Ordering};
    #[cfg(not(windows))]
    use std::time::{Duration, Instant};

    fn ctx() -> ToolContext {
        ToolContext::new(PathBuf::from("."))
    }

    #[cfg(not(windows))]
    #[derive(Clone, Copy, Debug)]
    enum AmbientProxyKind {
        Http,
        HttpsConnect,
        SocksRemoteDns,
    }

    #[cfg(not(windows))]
    fn spawn_accept_probe(
        stop: Arc<AtomicBool>,
        respond_http: bool,
        drain_http_headers: bool,
    ) -> (u16, std::thread::JoinHandle<bool>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind probe listener");
        let port = listener.local_addr().expect("probe address").port();
        listener
            .set_nonblocking(true)
            .expect("nonblocking probe listener");
        let handle = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Accepted sockets inherit nonblocking from the listener
                        // on several Unixes. Restore blocking mode before timed
                        // header reads so a not-yet-ready socket is not treated
                        // as a hard failure (WouldBlock / EAGAIN).
                        stream
                            .set_nonblocking(false)
                            .expect("blocking probe stream");
                        if drain_http_headers {
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .expect("probe read timeout");
                            let mut request = Vec::new();
                            let mut chunk = [0_u8; 1024];
                            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                                let read = match stream.read(&mut chunk) {
                                    Ok(n) => n,
                                    Err(err)
                                        if matches!(
                                            err.kind(),
                                            std::io::ErrorKind::WouldBlock
                                                | std::io::ErrorKind::TimedOut
                                                | std::io::ErrorKind::Interrupted
                                        ) =>
                                    {
                                        continue;
                                    }
                                    Err(err) => panic!("read probe request: {err}"),
                                };
                                if read == 0 {
                                    break;
                                }
                                request.extend_from_slice(&chunk[..read]);
                                if Instant::now() >= deadline {
                                    break;
                                }
                            }
                        }
                        if respond_http {
                            let _ = stream.write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                            );
                        }
                        return true;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(err) => panic!("probe accept failed: {err}"),
                }
                if stop.load(Ordering::SeqCst) || Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        (port, handle)
    }

    #[cfg(not(windows))]
    fn run_ambient_proxy_probe(kind: AmbientProxyKind, guarded: bool) -> (bool, bool) {
        let stop = Arc::new(AtomicBool::new(false));
        let (target_port, target_handle) = spawn_accept_probe(
            Arc::clone(&stop),
            matches!(
                kind,
                AmbientProxyKind::Http | AmbientProxyKind::SocksRemoteDns
            ),
            matches!(
                kind,
                AmbientProxyKind::Http | AmbientProxyKind::SocksRemoteDns
            ),
        );
        let (proxy_port, proxy_handle) = spawn_accept_probe(Arc::clone(&stop), true, false);

        let mut command = Command::new(std::env::current_exe().expect("current test executable"));
        command.args([
            "--exact",
            "tools::web::guard::tests::guarded_transport_proxy_probe_child",
            "--ignored",
            "--nocapture",
        ]);
        for key in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
            "REQUEST_METHOD",
        ] {
            command.env_remove(key);
        }
        command
            .env("CODEWHALE_PROXY_PROBE_CHILD", "1")
            .env(
                "CODEWHALE_PROXY_PROBE_GUARDED",
                if guarded { "1" } else { "0" },
            )
            .env("CODEWHALE_PROXY_PROBE_TARGET_PORT", target_port.to_string());
        match kind {
            AmbientProxyKind::Http => {
                let proxy = format!("http://127.0.0.1:{proxy_port}");
                command
                    .env("CODEWHALE_PROXY_PROBE_SCHEME", "http")
                    .env("HTTP_PROXY", &proxy)
                    .env("http_proxy", proxy);
            }
            AmbientProxyKind::HttpsConnect => {
                let proxy = format!("http://127.0.0.1:{proxy_port}");
                command
                    .env("CODEWHALE_PROXY_PROBE_SCHEME", "https")
                    .env("HTTPS_PROXY", &proxy)
                    .env("https_proxy", proxy);
            }
            AmbientProxyKind::SocksRemoteDns => {
                let proxy = format!("socks5h://127.0.0.1:{proxy_port}");
                command
                    .env("CODEWHALE_PROXY_PROBE_SCHEME", "http")
                    .env("ALL_PROXY", &proxy)
                    .env("all_proxy", proxy);
            }
        }

        let output = command.output().expect("run proxy probe child");
        stop.store(true, Ordering::SeqCst);
        let target_hit = target_handle.join().expect("join target probe");
        let proxy_hit = proxy_handle.join().expect("join proxy probe");
        assert!(
            output.status.success(),
            "proxy probe child failed for {kind:?} guarded={guarded}:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        (target_hit, proxy_hit)
    }

    #[cfg(not(windows))]
    #[tokio::test]
    #[ignore = "subprocess helper for ambient proxy regression test"]
    async fn guarded_transport_proxy_probe_child() {
        if std::env::var_os("CODEWHALE_PROXY_PROBE_CHILD").is_none() {
            return;
        }
        let guarded = std::env::var("CODEWHALE_PROXY_PROBE_GUARDED").as_deref() == Ok("1");
        let scheme = std::env::var("CODEWHALE_PROXY_PROBE_SCHEME").expect("probe scheme");
        let target_port = std::env::var("CODEWHALE_PROXY_PROBE_TARGET_PORT")
            .expect("probe target port")
            .parse::<u16>()
            .expect("numeric target port");
        let host = "guarded-proxy-probe.example.invalid";
        let builder = if guarded {
            guarded_reqwest_client_builder()
        } else {
            crate::tls::reqwest_client_builder()
        };
        let client = builder
            .timeout(Duration::from_secs(2))
            .resolve(
                host,
                SocketAddr::new(Ipv4Addr::LOCALHOST.into(), target_port),
            )
            .build()
            .expect("build proxy probe client");
        let result = client
            .get(format!("{scheme}://{host}:{target_port}/"))
            .send()
            .await;
        if guarded && scheme == "http" {
            assert!(
                result.is_ok(),
                "guarded HTTP target should answer: {result:?}"
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn guarded_transport_bypasses_ambient_http_https_and_remote_dns_socks_proxies() {
        for kind in [
            AmbientProxyKind::Http,
            AmbientProxyKind::HttpsConnect,
            AmbientProxyKind::SocksRemoteDns,
        ] {
            let (unguarded_target, unguarded_proxy) = run_ambient_proxy_probe(kind, false);
            assert!(
                unguarded_proxy && !unguarded_target,
                "control client must demonstrate ambient {kind:?} proxy interception"
            );

            let (guarded_target, guarded_proxy) = run_ambient_proxy_probe(kind, true);
            assert!(
                guarded_target && !guarded_proxy,
                "guarded client must preserve its DNS pin and bypass ambient {kind:?} proxy"
            );
        }
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
    async fn redirected_localhost_hostname_is_rejected() {
        let url = reqwest::Url::parse("http://localhost:8080/admin").unwrap();
        let err = validate_fetch_target(&url, &ctx(), "fetch_url")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("localhost"));
    }

    #[tokio::test]
    async fn redirected_private_ip_literal_is_rejected() {
        let url = reqwest::Url::parse("http://169.254.169.254/latest/meta-data").unwrap();
        let err = validate_fetch_target(&url, &ctx(), "fetch_url")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("restricted address"));
    }

    // GHSA-88gh-2526-gfrr — regression coverage for bracketed IPv6 literals.
    #[tokio::test]
    async fn rejects_ipv6_literal_loopback() {
        let url = reqwest::Url::parse("http://[::1]/").unwrap();
        let err = validate_fetch_target(&url, &ctx(), "fetch_url")
            .await
            .expect_err("[::1] must be rejected as restricted");
        assert!(format!("{err}").contains("restricted"));
    }

    #[tokio::test]
    async fn rejects_ipv6_literal_ula() {
        let url = reqwest::Url::parse("http://[fc00::1]/").unwrap();
        let err = validate_fetch_target(&url, &ctx(), "fetch_url")
            .await
            .expect_err("[fc00::1] must be rejected as restricted");
        assert!(format!("{err}").contains("restricted"));
    }

    #[tokio::test]
    async fn rejects_ipv6_literal_link_local() {
        let url = reqwest::Url::parse("http://[fe80::1]/").unwrap();
        let err = validate_fetch_target(&url, &ctx(), "fetch_url")
            .await
            .expect_err("[fe80::1] must be rejected as restricted");
        assert!(format!("{err}").contains("restricted"));
    }

    #[tokio::test]
    async fn rejects_ipv6_literal_ipv4_mapped_loopback() {
        let url = reqwest::Url::parse("http://[::ffff:127.0.0.1]/").unwrap();
        let err = validate_fetch_target(&url, &ctx(), "fetch_url")
            .await
            .expect_err("[::ffff:127.0.0.1] must be rejected as restricted");
        assert!(format!("{err}").contains("restricted"));
    }

    #[tokio::test]
    async fn rejects_ipv6_literal_unspecified() {
        let url = reqwest::Url::parse("http://[::]/").unwrap();
        let err = validate_fetch_target(&url, &ctx(), "fetch_url")
            .await
            .expect_err("[::] must be rejected as restricted");
        assert!(format!("{err}").contains("restricted"));
    }

    #[tokio::test]
    async fn redirected_host_respects_network_policy() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};
        let policy = NetworkPolicy {
            default: Decision::Deny.into(),
            allow: vec!["api.deepseek.com".to_string()],
            deny: vec![],
            proxy: Vec::new(),
            proxy_fake_ip_cidrs: Vec::new(),
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ctx = ToolContext::new(PathBuf::from(".")).with_network_policy(decider);
        let url = reqwest::Url::parse("https://example.com/redirect-target").unwrap();
        let err = validate_fetch_target(&url, &ctx, "fetch_url")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("blocked"));
    }

    #[tokio::test]
    async fn unresolved_hostname_is_rejected_before_request() {
        let url =
            reqwest::Url::parse("https://codewhale-unresolvable-fetch-target.invalid/resource")
                .unwrap();
        let err = validate_fetch_target(&url, &ctx(), "fetch_url")
            .await
            .expect_err("unresolved host must fail preflight");
        let message = format!("{err}");
        assert!(
            message.contains("could not resolve host") || message.contains("restricted address"),
            "error must identify preflight DNS or restricted-IP failure; got {err}"
        );
    }

    #[test]
    fn restricted_dns_result_is_denied_without_proxy_opt_in() {
        let ip = "198.18.0.1".parse().unwrap();

        let err = validate_dns_resolved_ip("github.com", &ip, None, "fetch_url")
            .expect_err("fake-IP DNS result must be denied by default");

        assert!(format!("{err}").contains("resolved IP 198.18.0.1 is a restricted address"));
    }

    #[test]
    fn proxy_host_and_fakeip_cidr_allow_matching_placeholder() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: vec!["github.com".to_string()],
            proxy_fake_ip_cidrs: vec!["198.18.0.0/15".to_string()],
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ip = "198.18.0.1".parse().unwrap();

        validate_dns_resolved_ip("github.com", &ip, Some(&decider), "fetch_url")
            .expect("matching host and fake-IP CIDR should allow the placeholder");
    }

    #[test]
    fn proxy_host_without_fakeip_cidr_does_not_allow_restricted_dns() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: vec!["github.com".to_string()],
            proxy_fake_ip_cidrs: Vec::new(),
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ip = "198.18.0.1".parse().unwrap();

        validate_dns_resolved_ip("github.com", &ip, Some(&decider), "fetch_url")
            .expect_err("hostname trust alone must not allow a restricted address");
    }

    #[test]
    fn fakeip_cidr_without_proxy_host_does_not_allow_restricted_dns() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: Vec::new(),
            proxy_fake_ip_cidrs: vec!["198.18.0.0/15".to_string()],
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ip = "198.18.0.1".parse().unwrap();

        validate_dns_resolved_ip("github.com", &ip, Some(&decider), "fetch_url")
            .expect_err("fake-IP CIDR alone must not allow an untrusted hostname");
    }

    #[test]
    fn proxy_host_never_exempts_real_private_or_local_addresses() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: vec!["github.com".to_string()],
            proxy_fake_ip_cidrs: vec![
                "198.18.0.0/15".to_string(),
                "127.0.0.0/8".to_string(),
                "10.0.0.0/8".to_string(),
                "169.254.0.0/16".to_string(),
            ],
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);

        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "fc00::1",
        ] {
            let ip = ip.parse().unwrap();
            assert!(
                validate_dns_resolved_ip("github.com", &ip, Some(&decider), "fetch_url").is_err(),
                "{ip} must remain restricted"
            );
        }
    }

    #[test]
    fn proxy_opt_in_does_not_allow_unlisted_host() {
        use crate::network_policy::{Decision, NetworkPolicy, NetworkPolicyDecider};

        let policy = NetworkPolicy {
            default: Decision::Allow.into(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: vec!["github.com".to_string()],
            proxy_fake_ip_cidrs: vec!["198.18.0.0/15".to_string()],
            audit: false,
        };
        let decider = NetworkPolicyDecider::new(policy, None);
        let ip = "198.18.0.1".parse().unwrap();

        let err = validate_dns_resolved_ip("example.com", &ip, Some(&decider), "fetch_url")
            .expect_err("proxy opt-in must be scoped to configured hosts");

        assert!(format!("{err}").contains("resolved IP 198.18.0.1 is a restricted address"));
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
            proxy_fake_ip_cidrs: vec!["198.18.0.0/15".to_string()],
            audit: true,
        };
        let decider = NetworkPolicyDecider::new(policy, Some(auditor));
        let ip = "198.18.0.1".parse().unwrap();

        validate_dns_resolved_ip("github.com", &ip, Some(&decider), "fetch_url")
            .expect("proxy DNS allow");

        let body = std::fs::read_to_string(dir.path().join("audit.log")).expect("audit log");
        assert!(body.contains("github.com"));
        assert!(body.contains("TrustedProxyFakeIp-Allow"));
    }

    #[tokio::test]
    async fn web_run_tool_label_is_used_in_dns_error() {
        let url =
            reqwest::Url::parse("https://codewhale-unresolvable-web-run-target.invalid/resource")
                .unwrap();
        let err = validate_fetch_target(&url, &ctx(), "web_run")
            .await
            .expect_err("unresolved host must fail preflight");
        let message = format!("{err}");
        // Either DNS failure (mentions web_run) or a restricted resolution.
        assert!(
            message.contains("web_run") || message.contains("restricted address"),
            "error should be labeled for web_run or report restricted IP; got {err}"
        );
    }
}
