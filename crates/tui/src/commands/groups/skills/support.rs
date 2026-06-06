use std::fmt::Write;

use crate::network_policy::NetworkPolicy;
use crate::skills::SkillRegistry;
use crate::skills::install::{DEFAULT_MAX_SIZE_BYTES, DEFAULT_REGISTRY_URL};
use crate::tui::app::App;

pub(crate) fn discover_visible_skills(app: &App) -> SkillRegistry {
    crate::skills::discover_for_workspace_and_dir(&app.workspace, &app.skills_dir)
}

pub(crate) fn render_skill_warnings(registry: &SkillRegistry) -> String {
    if registry.warnings().is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let _ = writeln!(out, "\nWarnings ({}):", registry.warnings().len());
    for warning in registry.warnings() {
        let _ = writeln!(out, "  - {warning}");
    }
    out
}

/// Read the active config knobs for skill install/update/sync operations.
///
/// The TUI app does not carry a `Config` field, and the TOML load is cheap
/// compared with the network operation that follows.
pub(crate) fn installer_settings(_app: &App) -> (NetworkPolicy, u64, String) {
    let cfg = crate::config::Config::load(None, None).unwrap_or_default();
    let network = cfg
        .network
        .clone()
        .map(|policy| policy.into_runtime())
        .unwrap_or_default();
    let skills_cfg = cfg.skills.as_ref();
    let max_size = skills_cfg
        .and_then(|s| s.max_install_size_bytes)
        .unwrap_or(DEFAULT_MAX_SIZE_BYTES);
    let registry_url = skills_cfg
        .and_then(|s| s.registry_url.clone())
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());
    (network, max_size, registry_url)
}

pub(crate) fn run_async<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

pub(crate) fn path_or_default(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| {
            let parent = path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            if parent.is_empty() {
                name.to_string_lossy().to_string()
            } else {
                format!("{parent}/{}", name.to_string_lossy())
            }
        })
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn needs_approval_message(host: &str) -> String {
    format!(
        "Network policy requires approval for {host}.\n\
         Add it to your allow list with `/network allow {host}` (or set [network].default = \"allow\" in ~/.codewhale/config.toml), then retry."
    )
}

pub(crate) fn network_denied_message(host: &str) -> String {
    format!(
        "Network policy denied access to {host}.\n\
         Remove the deny entry from ~/.codewhale/config.toml under [network] or contact your administrator."
    )
}

fn registry_fetch_error_hint(err: &anyhow::Error) -> Option<&'static str> {
    let msg = format!("{err:#}").to_lowercase();
    if msg.contains("dns")
        || msg.contains("name resolution")
        || msg.contains("getaddrinfo")
        || msg.contains("nodename nor servname")
    {
        Some(
            "Hint: DNS lookup failed. Check internet/DNS connectivity, or override the registry URL in [skills] of ~/.codewhale/config.toml.",
        )
    } else if msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("connection aborted")
    {
        Some(
            "Hint: connection refused/reset. The registry host may be unreachable from this network (corporate proxy, firewall, offline).",
        )
    } else if msg.contains("tls")
        || msg.contains("certificate")
        || msg.contains("ssl")
        || msg.contains("handshake")
    {
        Some(
            "Hint: TLS handshake failed. The system trust store may be missing the registry's CA, or a TLS-intercepting proxy is rewriting the certificate.",
        )
    } else if msg.contains(" 404") || msg.contains("not found") {
        Some(
            "Hint: registry URL returned 404. Verify the registry URL in [skills] of ~/.codewhale/config.toml.",
        )
    } else if msg.contains(" 401") || msg.contains(" 403") || msg.contains("forbidden") {
        Some(
            "Hint: registry returned an auth error. The registry may require credentials or have been moved.",
        )
    } else if msg.contains(" 429") || msg.contains("rate limit") || msg.contains("too many") {
        Some("Hint: rate-limited by the registry. Try again in a moment.")
    } else if msg.contains("timed out") || msg.contains("timeout") {
        Some("Hint: request timed out. Network may be slow or the registry host may be down.")
    } else {
        None
    }
}

pub(crate) fn format_registry_error(prefix: &str, err: &anyhow::Error) -> String {
    let mut out = format!("{prefix}: {err:#}");
    if let Some(hint) = registry_fetch_error_hint(err) {
        out.push_str("\n\n");
        out.push_str(hint);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_fetch_error_hint_recognises_dns_failures() {
        let err = anyhow::Error::msg("error sending request: dns error: failed to lookup")
            .context("failed to fetch registry https://example.com/registry.json");
        let hint = registry_fetch_error_hint(&err).expect("dns hint");
        assert!(hint.contains("DNS"), "got: {hint}");
    }

    #[test]
    fn registry_fetch_error_hint_recognises_connection_refused() {
        let err = anyhow::Error::msg("error sending request: tcp connect: connection refused");
        let hint = registry_fetch_error_hint(&err).expect("refused hint");
        assert!(hint.contains("refused"), "got: {hint}");
    }

    #[test]
    fn registry_fetch_error_hint_recognises_tls_failures() {
        let err = anyhow::Error::msg("invalid peer certificate: UnknownIssuer (TLS handshake)");
        let hint = registry_fetch_error_hint(&err).expect("tls hint");
        assert!(hint.contains("TLS"), "got: {hint}");
    }

    #[test]
    fn registry_fetch_error_hint_recognises_http_status_codes() {
        let err_404 = anyhow::Error::msg("registry returned an error status: 404 Not Found");
        assert!(
            registry_fetch_error_hint(&err_404)
                .map(|h| h.contains("404"))
                .unwrap_or(false)
        );
        let err_429 =
            anyhow::Error::msg("registry returned an error status: 429 Too Many Requests");
        assert!(
            registry_fetch_error_hint(&err_429)
                .map(|h| h.contains("rate"))
                .unwrap_or(false)
        );
    }

    #[test]
    fn registry_fetch_error_hint_returns_none_for_unrecognised_errors() {
        let err = anyhow::Error::msg("a totally novel error nobody anticipated");
        assert!(registry_fetch_error_hint(&err).is_none());
    }

    #[test]
    fn format_registry_error_appends_hint_when_pattern_matches() {
        let err = anyhow::Error::msg("dns error: nodename nor servname provided");
        let formatted = format_registry_error("Failed to fetch registry", &err);
        assert!(formatted.starts_with("Failed to fetch registry: "));
        assert!(
            formatted.contains("Hint: DNS"),
            "expected hint, got: {formatted}"
        );
    }

    #[test]
    fn format_registry_error_omits_hint_when_no_pattern_matches() {
        let err = anyhow::Error::msg("inscrutable opaque failure");
        let formatted = format_registry_error("Sync failed", &err);
        assert_eq!(formatted, "Sync failed: inscrutable opaque failure");
    }
}
