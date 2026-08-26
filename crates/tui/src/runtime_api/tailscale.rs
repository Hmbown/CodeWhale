//! Tailscale Serve front for the loopback web client.
//!
//! `codewhale web --tailscale` keeps the HTTP listener on `127.0.0.1` and
//! asks the local Tailscale CLI to publish HTTPS:443 to that port. Reachability
//! is ACL-gated by the tailnet. This is not Funnel and not a public tunnel.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TailscaleWebFront {
    pub public_origin: String,
    pub magic_dns: String,
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

pub(crate) fn public_origin_from_magic_dns(dns: &str) -> String {
    format!("https://{}", dns.trim_end_matches('.'))
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

pub(crate) fn discover_web_front() -> Result<TailscaleWebFront> {
    let status = run_tailscale(&["status", "--json"])?;
    let magic_dns = magic_dns_from_status_json(&status)?;
    Ok(TailscaleWebFront {
        public_origin: public_origin_from_magic_dns(&magic_dns),
        magic_dns,
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

pub(crate) struct TailscaleServeGuard {
    active: bool,
}

impl TailscaleServeGuard {
    pub(crate) fn new() -> Self {
        Self { active: true }
    }
}

impl Drop for TailscaleServeGuard {
    fn drop(&mut self) {
        if self.active {
            withdraw_https_443();
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

#[cfg(test)]
mod tests {
    use super::*;

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
            public_origin_from_magic_dns("codewhale.tailnet.ts.net."),
            "https://codewhale.tailnet.ts.net"
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
}
