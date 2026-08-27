//! Tailnet front for the loopback web client.
//!
//! `codewhale web --tailscale` is opt-in. Default `codewhale web` stays
//! loopback-only. This module prefers an embedded Tailscale node from the
//! official [`tailscale`](https://docs.rs/tailscale/0.5.0/tailscale/) 0.5.0
//! crate when the `tailscale` Cargo feature is enabled; otherwise (and when
//! embed cannot auth) it falls back to the Tailscale CLI
//! `tailscale serve --bg --https=443 localhost:<port>` design from PR #5628.
//!
//! ## Crate choice
//!
//! Official `tailscale` 0.5.0 **can listen**: `Device::tcp_listen` plus
//! `tailscale::axum::Listener` wrapping `netstack::TcpListener` for
//! `axum::serve`. `Config.requested_hostname` / `requested_tags` let us ask
//! for `codewhale.<tailnet>.ts.net` via `NodeInfo::fqdn`. That is why this
//! path uses the official crate rather than `geiserx_tailscale`.
//!
//! Official 0.5.0 **cannot mint HTTPS certificates**. The crate README lists
//! "HTTPS Certificates", "MagicDNS", and "Tailscale Serve" as unsupported.
//! Overlay traffic is still WireGuard-encrypted; browsers talking to the
//! embedded listener see HTTP on port 80. Browser-trusted TLS is the CLI
//! serve fallback (`https://<machine>.<tailnet>.ts.net`).
//!
//! Auth for embed: `CODEWHALE_TSNET_AUTHKEY` or `TS_AUTHKEY`. The crate also
//! requires `TS_RS_EXPERIMENT=this_is_unstable_software`. Platform support
//! in 0.5.0 is Linux (x86_64/ARM64) and macOS ARM64.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::process::Command;

/// Hostname advertised for an embedded tsnet node.
#[allow(dead_code)]
pub(crate) const DEFAULT_TSNET_HOSTNAME: &str = "codewhale";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TailscaleFrontKind {
    /// Embedded `Device::tcp_listen` + axum on tailnet :80 (no crate certs).
    #[allow(dead_code)]
    EmbeddedHttp,
    /// PR #5628 precursor: CLI `tailscale serve` HTTPS:443 → loopback.
    CliServeHttps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TailscaleWebFront {
    pub public_origin: String,
    pub magic_dns: String,
    pub kind: TailscaleFrontKind,
}

pub(crate) fn magic_dns_from_status_json(json: &str) -> Result<String> {
    let status: Value =
        serde_json::from_str(json).context("tailscale status --json was not valid JSON")?;
    let backend = status
        .get("BackendState")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if !backend.eq_ignore_ascii_case("Running") {
        bail!("tailscale is not connected (BackendState={backend}); run `tailscale up`");
    }
    let dns = status
        .pointer("/Self/DNSName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("tailscale status JSON missing Self.DNSName; is MagicDNS enabled?")?;
    let dns = dns.trim_end_matches('.');
    if !dns.contains('.') {
        bail!("tailscale MagicDNS name looks incomplete: {dns}");
    }
    Ok(dns.to_string())
}

pub(crate) fn public_origin_from_magic_dns(dns: &str, kind: TailscaleFrontKind) -> String {
    let host = dns.trim_end_matches('.');
    match kind {
        TailscaleFrontKind::EmbeddedHttp => format!("http://{host}"),
        TailscaleFrontKind::CliServeHttps => format!("https://{host}"),
    }
}

#[cfg(test)]
pub(crate) fn magic_dns_from_hostname_and_tailnet(hostname: &str, tailnet: &str) -> String {
    let hostname = hostname.trim_end_matches('.');
    let tailnet = tailnet.trim_end_matches('.');
    format!("{hostname}.{tailnet}")
}

pub(crate) fn serve_https_args(port: u16) -> Vec<String> {
    vec![
        "serve".to_string(),
        "--bg".to_string(),
        "--https=443".to_string(),
        format!("localhost:{port}"),
    ]
}

pub(crate) fn withdraw_https_args() -> Vec<String> {
    vec![
        "serve".to_string(),
        "--https=443".to_string(),
        "off".to_string(),
    ]
}

pub(crate) fn discover_cli_web_front() -> Result<TailscaleWebFront> {
    let status = run_tailscale(&["status", "--json"])?;
    let magic_dns = magic_dns_from_status_json(&status)?;
    Ok(TailscaleWebFront {
        public_origin: public_origin_from_magic_dns(&magic_dns, TailscaleFrontKind::CliServeHttps),
        magic_dns,
        kind: TailscaleFrontKind::CliServeHttps,
    })
}

pub(crate) fn publish_loopback_web(port: u16) -> Result<()> {
    let args = serve_https_args(port);
    let output = run_tailscale_output(&args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "tailscale serve failed (status {}): {}{}",
            output.status,
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!(" ({})", stdout.trim())
            }
        );
    }
    Ok(())
}

pub(crate) fn withdraw_https_443() {
    let args = withdraw_https_args();
    let _ = Command::new("tailscale").args(&args).status();
}

pub(crate) fn embedded_tsnet_compiled() -> bool {
    cfg!(all(
        feature = "tailscale",
        any(
            all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            ),
            all(target_os = "macos", target_arch = "aarch64")
        )
    ))
}

pub(crate) fn embed_disabled_by_env() -> bool {
    std::env::var_os("CODEWHALE_TSNET_DISABLE").is_some_and(|value| value != "0")
}

/// Plan the tailnet front. Prefers embed when compiled and not disabled;
/// falls back to CLI serve (PR #5628) when embed cannot auth or is absent.
pub(crate) async fn plan_tailscale_front() -> Result<TailscalePublish> {
    if embedded_tsnet_compiled() && !embed_disabled_by_env() {
        match try_start_embedded_tsnet().await {
            Ok(publish) => return Ok(publish),
            Err(err) => {
                eprintln!(
                    "warning: embedded Tailscale node unavailable ({err}); falling back to `tailscale serve`"
                );
            }
        }
    }
    Ok(TailscalePublish::Cli(discover_cli_web_front()?))
}

pub(crate) enum TailscalePublish {
    Cli(TailscaleWebFront),
    /// Constructed when the `tailscale` feature is compiled in on a supported
    /// target. Default builds keep the CLI-serve fallback only.
    #[allow(dead_code)]
    Embedded(EmbeddedTsnet),
}

impl TailscalePublish {
    pub(crate) fn front(&self) -> &TailscaleWebFront {
        match self {
            Self::Cli(front) => front,
            Self::Embedded(node) => &node.front,
        }
    }
}

pub(crate) struct TailscaleGuard {
    inner: TailscaleGuardInner,
}

enum TailscaleGuardInner {
    Inactive,
    CliServe,
    #[allow(dead_code)]
    Embedded {
        serve: tokio::task::JoinHandle<()>,
    },
}

impl TailscaleGuard {
    pub(crate) fn new() -> Self {
        Self {
            inner: TailscaleGuardInner::Inactive,
        }
    }

    pub(crate) fn arm_cli(&mut self) {
        self.inner = TailscaleGuardInner::CliServe;
    }

    #[allow(dead_code)]
    pub(crate) fn arm_embedded(&mut self, serve: tokio::task::JoinHandle<()>) {
        self.inner = TailscaleGuardInner::Embedded { serve };
    }
}

impl Drop for TailscaleGuard {
    fn drop(&mut self) {
        match &mut self.inner {
            TailscaleGuardInner::Inactive => {}
            TailscaleGuardInner::CliServe => withdraw_https_443(),
            TailscaleGuardInner::Embedded { serve } => serve.abort(),
        }
    }
}

async fn try_start_embedded_tsnet() -> Result<TailscalePublish> {
    try_start_embedded_tsnet_impl().await
}

#[cfg(all(
    feature = "tailscale",
    any(
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(target_os = "macos", target_arch = "aarch64")
    )
))]
async fn try_start_embedded_tsnet_impl() -> Result<TailscalePublish> {
    embed::start().await.map(TailscalePublish::Embedded)
}

#[cfg(not(all(
    feature = "tailscale",
    any(
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(target_os = "macos", target_arch = "aarch64")
    )
)))]
async fn try_start_embedded_tsnet_impl() -> Result<TailscalePublish> {
    bail!(
        "embedded tsnet is not compiled in this binary (build with --features tailscale on Linux or macOS ARM64)"
    );
}

#[allow(dead_code)]
pub(crate) struct EmbeddedTsnet {
    pub front: TailscaleWebFront,
    #[cfg(all(
        feature = "tailscale",
        any(
            all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            ),
            all(target_os = "macos", target_arch = "aarch64")
        )
    ))]
    device: std::sync::Arc<::tailscale::Device>,
}

impl EmbeddedTsnet {
    /// Serve the Runtime router on tailnet HTTP :80.
    ///
    /// Official 0.5.0 has no certificate API, so this is HTTP over WireGuard,
    /// not browser-trusted HTTPS.
    #[allow(dead_code)]
    pub(crate) async fn spawn_http80(
        &self,
        app: axum::Router,
    ) -> Result<tokio::task::JoinHandle<()>> {
        #[cfg(all(
            feature = "tailscale",
            any(
                all(
                    target_os = "linux",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(target_os = "macos", target_arch = "aarch64")
            )
        ))]
        {
            let ipv4 = self
                .device
                .ipv4_addr()
                .await
                .context("embedded tsnet has no tailnet IPv4 address yet")?;
            let listener = self
                .device
                .tcp_listen((ipv4, 80).into())
                .await
                .context("embedded tsnet tcp_listen(:80) failed")?;
            let listener: ::tailscale::axum::Listener = listener.into();
            Ok(tokio::spawn(async move {
                let _ = axum::serve(listener, app.into_make_service()).await;
            }))
        }
        #[cfg(not(all(
            feature = "tailscale",
            any(
                all(
                    target_os = "linux",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                ),
                all(target_os = "macos", target_arch = "aarch64")
            )
        )))]
        {
            let _ = (self, app);
            bail!("embedded tsnet is not compiled in this binary");
        }
    }
}

fn run_tailscale(args: &[&str]) -> Result<String> {
    let output = run_tailscale_output(args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "tailscale {} failed: {}",
            args.first().copied().unwrap_or("command"),
            stderr.trim()
        );
    }
    String::from_utf8(output.stdout).context("tailscale stdout was not UTF-8")
}

fn run_tailscale_output(args: &[impl AsRef<std::ffi::OsStr>]) -> Result<std::process::Output> {
    Command::new("tailscale")
        .args(args)
        .output()
        .context("tailscale CLI not found; install Tailscale and confirm `tailscale status` works")
}

#[cfg(all(
    feature = "tailscale",
    any(
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(target_os = "macos", target_arch = "aarch64")
    )
))]
mod embed {
    use super::*;
    use ::tailscale::{AuthState, Config, Device};
    use anyhow::Context;
    use std::sync::Arc;

    const EXPERIMENT_ENV: &str = "TS_RS_EXPERIMENT";
    const EXPERIMENT_VALUE: &str = "this_is_unstable_software";

    pub(super) async fn start() -> Result<EmbeddedTsnet> {
        ensure_experiment_env();
        let key_path = tsnet_key_path()?;
        if let Some(parent) = key_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create tsnet state dir {}", parent.display()))?;
        }
        let mut config = Config::default_with_key_file(&key_path)
            .await
            .context("load embedded tsnet key file")?;
        config.requested_hostname = Some(DEFAULT_TSNET_HOSTNAME.to_string());
        if let Some(tags) = requested_tags_from_env() {
            config.requested_tags = tags;
        }
        let auth_key = auth_key_from_env();
        let device = Device::new(&config, auth_key)
            .await
            .context("start embedded Tailscale device")?;
        match device
            .is_authorized()
            .await
            .context("query embedded Tailscale auth state")?
        {
            AuthState::Authorized => {}
            AuthState::NotAuthorized(url) => {
                bail!(
                    "embedded Tailscale node is not authorized; open {url} or set CODEWHALE_TSNET_AUTHKEY / TS_AUTHKEY"
                );
            }
        }
        let node = device
            .self_node()
            .await
            .context("read embedded tsnet self node")?;
        let magic_dns = node
            .fqdn_opt(false)
            .filter(|dns| dns.contains('.'))
            .unwrap_or_else(|| node.fqdn(false));
        let magic_dns = magic_dns.trim_end_matches('.').to_string();
        if !magic_dns.contains('.') {
            bail!("embedded tsnet FQDN looks incomplete: {magic_dns}");
        }
        Ok(EmbeddedTsnet {
            front: TailscaleWebFront {
                public_origin: public_origin_from_magic_dns(
                    &magic_dns,
                    TailscaleFrontKind::EmbeddedHttp,
                ),
                magic_dns,
                kind: TailscaleFrontKind::EmbeddedHttp,
            },
            device: Arc::new(device),
        })
    }

    fn ensure_experiment_env() {
        if std::env::var_os(EXPERIMENT_ENV).is_none() {
            // SAFETY: process-local gate required by tailscale-rs 0.5.0
            // until its third-party audit lands. Only set when unset, and
            // only on the embed path.
            unsafe {
                std::env::set_var(EXPERIMENT_ENV, EXPERIMENT_VALUE);
            }
        }
    }

    fn tsnet_key_path() -> Result<std::path::PathBuf> {
        let home = codewhale_paths::codewhale_home()
            .ok()
            .flatten()
            .context("CODEWHALE_HOME / user home is required for embedded tsnet state")?;
        Ok(home.join("tsnet").join("keys.json"))
    }

    fn auth_key_from_env() -> Option<String> {
        ["CODEWHALE_TSNET_AUTHKEY", "TS_AUTHKEY"]
            .into_iter()
            .find_map(|name| {
                std::env::var(name)
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
    }

    fn requested_tags_from_env() -> Option<Vec<String>> {
        std::env::var("CODEWHALE_TSNET_TAGS").ok().map(|raw| {
            raw.split(',')
                .map(|tag| tag.trim().to_string())
                .filter(|tag| !tag.is_empty())
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_api::{RuntimeApiOptions, validate_runtime_api_options};

    #[test]
    fn reads_magic_dns_and_strips_trailing_dot() {
        let json = r#"{
            "BackendState": "Running",
            "Self": { "DNSName": "codewhale.tailnet.ts.net." }
        }"#;
        assert_eq!(
            magic_dns_from_status_json(json).unwrap(),
            "codewhale.tailnet.ts.net"
        );
        assert_eq!(
            public_origin_from_magic_dns(
                "codewhale.tailnet.ts.net.",
                TailscaleFrontKind::CliServeHttps
            ),
            "https://codewhale.tailnet.ts.net"
        );
        assert_eq!(
            public_origin_from_magic_dns(
                "codewhale.tailnet.ts.net",
                TailscaleFrontKind::EmbeddedHttp
            ),
            "http://codewhale.tailnet.ts.net"
        );
    }

    #[test]
    fn requested_hostname_is_codewhale() {
        assert_eq!(DEFAULT_TSNET_HOSTNAME, "codewhale");
        assert_eq!(
            magic_dns_from_hostname_and_tailnet("codewhale", "tailnet.ts.net."),
            "codewhale.tailnet.ts.net"
        );
    }

    #[test]
    fn rejects_disconnected_or_incomplete_status() {
        let stopped = r#"{"BackendState":"Stopped","Self":{"DNSName":"x.tailnet.ts.net."}}"#;
        let error = magic_dns_from_status_json(stopped).unwrap_err().to_string();
        assert!(error.contains("not connected"), "{error}");

        let empty = r#"{"BackendState":"Running","Self":{"DNSName":""}}"#;
        assert!(magic_dns_from_status_json(empty).is_err());
    }

    #[test]
    fn serve_mapping_is_https_443_only_and_does_not_reset() {
        assert_eq!(
            serve_https_args(7878),
            ["serve", "--bg", "--https=443", "localhost:7878"]
        );
        let withdraw = withdraw_https_args();
        assert_eq!(withdraw, ["serve", "--https=443", "off"]);
        assert!(
            !withdraw.iter().any(|arg| arg.contains("reset")),
            "withdraw must not wipe other serve config"
        );
    }

    #[test]
    fn default_web_does_not_enable_tailscale() {
        let options = RuntimeApiOptions::default();
        assert!(!options.tailscale);
        assert!(!options.web);
        validate_runtime_api_options(&options).unwrap();
    }

    #[test]
    fn tailscale_without_web_is_rejected() {
        let err = validate_runtime_api_options(&RuntimeApiOptions {
            tailscale: true,
            web: false,
            ..RuntimeApiOptions::default()
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("--tailscale requires --web"), "{err}");
    }

    #[test]
    fn tailscale_with_web_passes_option_validation() {
        validate_runtime_api_options(&RuntimeApiOptions {
            web: true,
            tailscale: true,
            ..RuntimeApiOptions::default()
        })
        .unwrap();
    }

    #[test]
    fn embed_is_absent_from_default_builds() {
        if cfg!(not(feature = "tailscale")) {
            assert!(
                !embedded_tsnet_compiled(),
                "default builds must not link the tailscale crate"
            );
        }
    }
}
