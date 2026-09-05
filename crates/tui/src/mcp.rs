//! Async MCP (Model Context Protocol) Implementation
//!
//! This module provides full async support for MCP servers with:
//! - Connection pooling for server reuse
//! - Automatic tool discovery via `tools/list`
//! - Configurable timeouts per-server and globally

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::future::Future;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

pub mod external_import;
mod headers;
mod http;
pub mod oauth;
mod sse;
mod stdio;
mod streamable_http;
mod wire;

use self::http::{HttpTransport, McpHttpAuth};
use self::sse::SseTransport;
use self::stdio::StdioTransport;
#[cfg(all(test, unix))]
use self::stdio::{STDIO_SHUTDOWN_GRACE, StderrTail};
use self::wire::{is_mcp_stale_session_body, is_mcp_stale_session_error};
use crate::network_policy::{Decision, NetworkPolicyDecider, host_from_url};
use crate::utils::write_atomic;

// === Error diagnostics helpers (#71) ===

/// Bytes of a non-2xx response body to surface in connection errors.
const ERROR_BODY_PREVIEW_BYTES: usize = 200;

fn validate_mcp_config_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        anyhow::bail!("MCP config path cannot be empty");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("MCP config path cannot contain '..' components");
    }
    Ok(())
}

/// Expand `${NAME}` placeholders in an MCP config value from the process
/// environment. This lets secrets (API keys, bearer tokens, …) be supplied
/// through environment variables instead of being written in cleartext into
/// the MCP config file on disk.
///
/// On a missing or malformed placeholder the error names only the offending
/// variable, never the surrounding value, so a secret-bearing string is never
/// echoed into logs or error output.
fn expand_env_placeholders_with(
    value: &str,
    environment: Option<&crate::plugins::HostEnvironment>,
) -> Result<String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            anyhow::bail!("unterminated environment placeholder in MCP config value");
        };
        let name = &after[..end];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            anyhow::bail!("invalid environment placeholder in MCP config value");
        }
        let env_value = environment
            .map_or_else(|| std::env::var(name), |env| env.var(name))
            .with_context(|| {
                format!("environment variable {name} required by MCP config is not set")
            })?;
        out.push_str(&env_value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
fn expand_env_placeholders(value: &str) -> Result<String> {
    expand_env_placeholders_with(value, None)
}

/// Expand `${NAME}` placeholders across every value of an MCP config map
/// (e.g. the stdio child `env`). `context` only labels expansion errors so a
/// failure can be attributed to the right map.
fn expand_env_placeholders_map_with_environment(
    values: &HashMap<String, String>,
    context: &str,
    environment: Option<&crate::plugins::HostEnvironment>,
) -> Result<HashMap<String, String>> {
    let mut expanded = HashMap::with_capacity(values.len());
    for (key, value) in values {
        expanded.insert(
            key.clone(),
            expand_env_placeholders_with(value, environment)
                .with_context(|| format!("failed to expand MCP {context} value for {key}"))?,
        );
    }
    Ok(expanded)
}

#[cfg(test)]
fn expand_env_placeholders_map(
    values: &HashMap<String, String>,
    context: &str,
) -> Result<HashMap<String, String>> {
    expand_env_placeholders_map_with_environment(values, context, None)
}

fn expanded_mcp_stdio_env(config: &McpServerConfig) -> Result<HashMap<String, String>> {
    let environment = config
        .reviewed_plugin
        .as_ref()
        .map(|source| source.host_environment.as_ref());
    expand_env_placeholders_map_with_environment(&config.env, "env", environment)
}

/// Mirror the exact expanded and sanitized environment applied by the MCP
/// stdio spawn path, without constructing or starting a process.
fn mcp_stdio_child_env(config: &McpServerConfig) -> Result<Vec<(OsString, OsString)>> {
    let expanded_env = expanded_mcp_stdio_env(config)?;
    let overrides = crate::child_env::string_map_env(&expanded_env);
    Ok(if let Some(source) = config.reviewed_plugin.as_ref() {
        // Plugin reviews name every extra environment source explicitly. Do
        // not silently widen that consent to the compatibility-oriented MCP
        // bootstrap namespace (for example NPM_CONFIG_*).
        crate::child_env::sanitized_plugin_mcp_env_from(
            source.host_environment.entries().iter().cloned(),
            overrides,
        )
    } else {
        crate::child_env::sanitized_mcp_env(overrides)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpCommandAvailability {
    Available,
    Missing,
    NotApplicable,
    NotChecked,
}

impl McpCommandAvailability {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
            Self::NotApplicable => "not_applicable",
            Self::NotChecked => "not_checked",
        }
    }
}

pub(crate) fn is_relative_stdio_path_arg(value: &str) -> bool {
    if value.is_empty() || value.starts_with('-') || value.contains("://") || value.starts_with('~')
    {
        return false;
    }
    let looks_like_path = value.contains('/') || value.contains('\\');
    if !looks_like_path {
        return false;
    }
    let bytes = value.as_bytes();
    let windows_absolute = value.starts_with("\\\\")
        || (bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/'));
    !Path::new(value).is_absolute() && !windows_absolute
}

fn env_value<'a>(env: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsStr> {
    env.iter()
        .rev()
        .find(|(key, _)| {
            #[cfg(windows)]
            {
                key.to_string_lossy().eq_ignore_ascii_case(name)
            }
            #[cfg(not(windows))]
            {
                key == OsStr::new(name)
            }
        })
        .map(|(_, value)| value.as_os_str())
}

#[cfg(unix)]
fn spawnable_command_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.is_file()
        && fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(windows)]
fn spawnable_command_file(path: &Path) -> bool {
    path.is_file() || (path.extension().is_none() && path.with_extension("exe").is_file())
}

#[cfg(not(any(unix, windows)))]
fn spawnable_command_file(path: &Path) -> bool {
    path.is_file()
}

fn path_candidate(dir: &Path, name: &str, cwd: Option<&Path>) -> PathBuf {
    #[cfg(unix)]
    {
        // Unix performs PATH lookup after applying Command::current_dir. That
        // includes empty PATH entries, which mean the child's current dir.
        if dir.is_relative()
            && let Some(cwd) = cwd
        {
            return cwd.join(dir).join(name);
        }
    }
    #[cfg(not(unix))]
    let _ = cwd;
    dir.join(name)
}

fn command_availability_on_path(
    name: &str,
    env: &[(OsString, OsString)],
    cwd: Option<&Path>,
) -> McpCommandAvailability {
    let Some(path) = env_value(env, "PATH") else {
        // On Unix execvp falls back to an OS-defined path. On Windows Rust's
        // resolver still checks system and parent locations. We cannot prove a
        // miss without reproducing platform internals, so remain conservative.
        return McpCommandAvailability::NotChecked;
    };
    for dir in std::env::split_paths(path) {
        let candidate = path_candidate(&dir, name, cwd);
        if spawnable_command_file(&candidate) {
            return McpCommandAvailability::Available;
        }
    }

    #[cfg(windows)]
    {
        // Windows Command resolution also checks the running executable's
        // directory, system directories, and the parent PATH after an explicit
        // child PATH. A static miss in the child PATH is therefore not proof
        // that spawn will fail. PATHEXT is intentionally not consulted: Rust
        // only supplies an omitted `.exe`; `.cmd`/`.bat` must be explicit.
        return McpCommandAvailability::NotChecked;
    }
    #[cfg(not(windows))]
    {
        McpCommandAvailability::Missing
    }
}

/// Inspect an MCP stdio command using the same expanded, sanitized environment
/// as the real spawn path, without starting the configured process.
pub(crate) fn static_mcp_command_availability(
    server: &McpServerConfig,
) -> Result<McpCommandAvailability> {
    if server.url.is_some() {
        return Ok(McpCommandAvailability::NotApplicable);
    }
    let Some(cmd) = server.command.as_deref() else {
        return Ok(McpCommandAvailability::NotChecked);
    };
    if cmd.is_empty() {
        return Ok(McpCommandAvailability::Missing);
    }

    // StdioTransport expands every configured env value before spawning, even
    // when the command itself is absolute. Mirror that failure boundary here.
    let child_env = mcp_stdio_child_env(server)?;
    let path = Path::new(cmd);
    let is_absolute = path.is_absolute() || cmd.starts_with('/');
    if is_absolute {
        return Ok(if spawnable_command_file(path) {
            McpCommandAvailability::Available
        } else {
            McpCommandAvailability::Missing
        });
    }

    if is_relative_stdio_path_arg(cmd) {
        let Some(cwd) = server.cwd.as_deref() else {
            return Ok(McpCommandAvailability::NotChecked);
        };
        return Ok(if spawnable_command_file(&cwd.join(path)) {
            McpCommandAvailability::Available
        } else {
            McpCommandAvailability::Missing
        });
    }

    Ok(command_availability_on_path(
        cmd,
        &child_env,
        server.cwd.as_deref(),
    ))
}

/// Mask a URL so any embedded credentials in the userinfo portion (e.g.
/// `https://user:secret@host`) are replaced with `***`. Failures fall back to
/// the original string so we don't lose context — we never want masking to
/// produce an empty error.
fn mask_url_secrets(url: &str) -> String {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        let mut clone = parsed.clone();
        if !parsed.username().is_empty() || parsed.password().is_some() {
            let _ = clone.set_username("***");
            let _ = clone.set_password(Some("***"));
        }
        if parsed.query().is_some() {
            clone.set_query(Some("***"));
        }
        clone.set_fragment(None);
        return clone.to_string();
    }
    url.to_string()
}

/// Redact the userinfo segment (`username[:password]@…` portion) from
/// a proxy URL so it can be safely included in `tracing::warn!` output
/// without leaking the
/// password into the on-disk log. URLs without userinfo are returned
/// unchanged. Garbage input (no `://` scheme separator) is also returned
/// unchanged — the malformed-URL warning path is the only caller, so an
/// unparseable input is already the failure case.
fn redact_proxy_userinfo(proxy_url: &str) -> String {
    let Some(scheme_end) = proxy_url.find("://") else {
        return proxy_url.to_string();
    };
    let after_scheme = scheme_end + 3;
    // The userinfo segment ends at the next `@`, but only if that `@`
    // comes before the next `/`, `?`, or `#` (otherwise the `@` is in a
    // path / query and the URL has no userinfo at all).
    let rest = &proxy_url[after_scheme..];
    let at_idx = rest.find('@');
    let path_idx = rest.find(['/', '?', '#']);
    let userinfo_end = match (at_idx, path_idx) {
        (Some(a), Some(p)) if a < p => Some(a),
        (Some(a), None) => Some(a),
        _ => None,
    };
    if let Some(end) = userinfo_end {
        let mut out = String::with_capacity(proxy_url.len());
        out.push_str(&proxy_url[..after_scheme]);
        out.push_str("***@");
        out.push_str(&rest[end + 1..]);
        out
    } else {
        proxy_url.to_string()
    }
}

fn redact_values_after_ascii_needle(
    output: &mut String,
    needle: &str,
    terminates: impl Fn(char) -> bool,
) {
    let needle = needle.as_bytes();
    let mut search_from = 0_usize;
    while search_from.saturating_add(needle.len()) <= output.len() {
        let Some(relative) = output.as_bytes()[search_from..]
            .windows(needle.len())
            .position(|candidate| candidate.eq_ignore_ascii_case(needle))
        else {
            break;
        };
        let value_start = search_from + relative + needle.len();
        let value_end = output[value_start..]
            .char_indices()
            .find(|(_, ch)| terminates(*ch))
            .map_or(output.len(), |(offset, _)| value_start + offset);
        if value_end == value_start {
            if value_start == output.len() {
                break;
            }
            // The empty value is already safe. Advance over its ASCII
            // separator so a second occurrence later in the body is found.
            search_from = value_start + 1;
            continue;
        }
        output.replace_range(value_start..value_end, "***");
        search_from = value_start + 3;
    }
}

/// Mask obvious token-like substrings in a body excerpt before surfacing it.
/// Every occurrence is replaced, not only the first one.
fn redact_body_preview(body: &str) -> String {
    let mut out = body.to_string();
    redact_values_after_ascii_needle(&mut out, "bearer ", |ch| {
        ch.is_whitespace() || ch == '"' || ch == ','
    });
    for needle in ["api_key=", "apikey=", "api-key=", "token="] {
        redact_values_after_ascii_needle(&mut out, needle, |ch| {
            ch.is_whitespace() || ch == '&' || ch == '"' || ch == ','
        });
    }
    out
}

/// Read at most `max_bytes` of a reqwest response body and produce a
/// single-line excerpt suitable for an error message. The stream is dropped as
/// soon as the cap is reached, so an unbounded or never-ending error response
/// cannot make diagnostics retain the entire body. Best-effort — if the body
/// can't be read, returns the literal string `<no body>`.
async fn bounded_body_excerpt(response: reqwest::Response, max_bytes: usize) -> String {
    use futures_util::StreamExt;

    let declared_truncated = response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64);
    let mut stream = response.bytes_stream();
    let mut body = Vec::with_capacity(max_bytes.min(8 * 1024));
    let mut truncated = declared_truncated;

    while body.len() < max_bytes {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let Ok(chunk) = chunk else {
            break;
        };
        let remaining = max_bytes - body.len();
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
        if body.len() == max_bytes {
            // For a chunked response there is no length that proves EOF. Stop
            // now rather than polling an attacker-controlled stream again.
            truncated = true;
        }
    }

    if body.is_empty() {
        return "<no body>".to_string();
    }

    let one_line = String::from_utf8_lossy(&body).replace(['\n', '\r'], " ");
    let suffix = if truncated { "…" } else { "" };
    format!("{}{}", redact_body_preview(&one_line), suffix)
}

fn invalid_json_preview(bytes: &[u8]) -> String {
    let body_text = String::from_utf8_lossy(bytes);
    if body_text.is_empty() {
        return "<empty>".to_string();
    }

    let trimmed: String = body_text.chars().take(ERROR_BODY_PREVIEW_BYTES).collect();
    let suffix = if body_text.chars().count() > ERROR_BODY_PREVIEW_BYTES {
        "…"
    } else {
        ""
    };
    let one_line = trimmed.replace(['\n', '\r'], " ");
    format!("{}{}", redact_body_preview(&one_line), suffix)
}

// === Configuration Types ===

/// Full MCP configuration from mcp.json
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct McpConfig {
    #[serde(default)]
    pub timeouts: McpTimeouts,
    #[serde(default, alias = "mcpServers")]
    pub servers: HashMap<String, McpServerConfig>,
}

/// Global timeout configuration
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[allow(clippy::struct_field_names)]
pub struct McpTimeouts {
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: u64,
    #[serde(default = "default_execute_timeout")]
    pub execute_timeout: u64,
    #[serde(default = "default_read_timeout")]
    pub read_timeout: u64,
}

fn default_connect_timeout() -> u64 {
    10
}
fn default_execute_timeout() -> u64 {
    60
}
fn default_read_timeout() -> u64 {
    120
}

impl Default for McpTimeouts {
    fn default() -> Self {
        Self {
            connect_timeout: default_connect_timeout(),
            execute_timeout: default_execute_timeout(),
            read_timeout: default_read_timeout(),
        }
    }
}

/// Configuration for a single MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub url: Option<String>,
    /// Optional explicit HTTP transport override.
    ///
    /// By default URL-based MCP servers use Streamable HTTP first and fall
    /// back to legacy SSE only when the server rejects Streamable HTTP with
    /// a known incompatible status. Set this to `"sse"` for legacy SSE
    /// endpoints that must start with a long-lived GET endpoint discovery
    /// stream and cannot accept an initial POST to the configured URL.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(default)]
    pub connect_timeout: Option<u64>,
    #[serde(default)]
    pub execute_timeout: Option<u64>,
    #[serde(default)]
    pub read_timeout: Option<u64>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub enabled_tools: Vec<String>,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    /// Extra HTTP headers sent with every request to this MCP server.
    /// Only the HTTP transports (streamable HTTP today; SSE in a
    /// follow-up) honor this — `command`-based stdio servers ignore it.
    ///
    /// Mirrors the `headers` field that Claude Code, Codex, and
    /// OpenCode already accept in their MCP config formats. Use it to
    /// authenticate against gateways that require a Bearer token or
    /// API key, e.g.:
    ///
    /// ```jsonc
    /// "huggingface": {
    ///     "url": "https://huggingface.co/api/mcp",
    ///     "headers": { "Authorization": "Bearer ${HF_TOKEN}" }
    /// }
    /// ```
    ///
    /// Header keys and values are passed through as-is — we do not
    /// substitute environment variables in v0.8.31. If you store a
    /// real token here, the value lives in plain text in
    /// `~/.deepseek/mcp.json`; treat that file with the same care
    /// as any other secret-bearing config.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    /// HTTP headers whose values are read from environment variables at request
    /// time. This keeps common bearer/API-token integrations out of mcp.json.
    #[serde(default, alias = "env_http_headers")]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub env_headers: HashMap<String, String>,
    /// Environment variable containing a bearer token. When present and set,
    /// CodeWhale sends `Authorization: Bearer <value>` for URL-based servers.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<String>,
    /// OAuth scopes requested during `codewhale mcp login`.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    /// OAuth client override for MCP servers that require a pre-registered
    /// public client instead of dynamic registration.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpServerOAuthConfig>,
    /// Optional RFC 8707 resource parameter appended to the authorization URL.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_resource: Option<String>,
    /// In-memory provenance for MCP servers contributed by a reviewed plugin
    /// bundle. This is never deserialized from or serialized into user config:
    /// only the trusted plugin merge adapter may attach it.
    #[serde(skip)]
    pub(crate) reviewed_plugin: Option<ReviewedPluginMcpSource>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewedPluginMcpSource {
    authority: crate::plugins::types::PluginAuthority,
    approved_remote_endpoint: Option<String>,
    approved_remote_origin: Option<String>,
    host_environment: Arc<crate::plugins::HostEnvironment>,
}

impl ReviewedPluginMcpSource {
    fn from_authority(
        authority: crate::plugins::types::PluginAuthority,
        remote_endpoint: Option<&str>,
        host_environment: Arc<crate::plugins::HostEnvironment>,
    ) -> Result<Self> {
        let (approved_remote_endpoint, approved_remote_origin) = match remote_endpoint {
            Some(endpoint) => reviewed_remote_endpoint_identity(endpoint)
                .map(|(endpoint, origin)| (Some(endpoint), Some(origin)))?,
            None => (None, None),
        };
        Ok(Self {
            authority,
            approved_remote_endpoint,
            approved_remote_origin,
            host_environment,
        })
    }

    pub(crate) fn validate_before_stdio_spawn(&self, server_name: &str) -> Result<()> {
        self.validate_before_use(server_name, "spawn")
    }

    pub(crate) fn prepare_stdio_launch(
        &self,
        server_name: &str,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<ReviewedStdioLaunch> {
        self.validate_before_stdio_spawn(server_name)?;
        let staged_root = self
            .authority
            .staged_manifest
            .parent()
            .context("reviewed plugin stage manifest has no parent")?;
        let validated = crate::plugins::manifest::PluginManifest::validate_from_path(
            &self.authority.staged_manifest,
        )
        .map_err(|_| anyhow::anyhow!("reviewed plugin stage could not be opened for launch"))?;
        if validated.content_hash != self.authority.content_hash
            || validated.capability_hash != self.authority.capability_hash
        {
            anyhow::bail!("reviewed plugin stage changed before stdio launch");
        }

        let mut launch = ReviewedStdioLaunch {
            command: std::ffi::OsString::from(command),
            args: args.iter().map(std::ffi::OsString::from).collect(),
            cwd: cwd.map(Path::to_path_buf),
            opened_files: Vec::new(),
            #[cfg(unix)]
            cwd_fd: None,
        };
        if Path::new(command).is_absolute() {
            launch.bind_command(staged_root, Path::new(command), &validated.file_hashes)?;
        }
        for (index, argument) in args.iter().enumerate() {
            let path = Path::new(argument);
            if path.is_absolute() && path.starts_with(staged_root) && path.is_file() {
                launch.args[index] = launch.bind_file(staged_root, path, &validated.file_hashes)?;
            }
        }
        #[cfg(target_os = "macos")]
        if is_node_command(command) {
            let entry_index = args.iter().position(|argument| {
                let path = Path::new(argument);
                path.is_absolute()
                    && path.starts_with(staged_root)
                    && path.extension().is_some_and(|extension| extension == "mjs")
            });
            if let Some(entry_index) = entry_index {
                launch.args = node_esm_descriptor_args(&launch.args, entry_index);
            }
        }
        if let Some(cwd) = cwd {
            if !cwd.starts_with(staged_root) {
                anyhow::bail!("reviewed plugin stdio cwd escaped its staged root");
            }
            launch.bind_cwd(cwd)?;
        }
        // A final authority pass detects any non-executed companion/config
        // drift while handles were opened. Execution itself uses the handles.
        self.validate_before_stdio_spawn(server_name)?;
        Ok(launch)
    }

    fn required_capability(&self) -> crate::plugins::activation::PluginActivationCapability {
        if self.approved_remote_endpoint.is_some() {
            crate::plugins::activation::PluginActivationCapability::McpRemote
        } else {
            crate::plugins::activation::PluginActivationCapability::McpStdio
        }
    }

    fn validate_before_use(&self, server_name: &str, operation: &str) -> Result<()> {
        let remediation = format!(
            "Run `/plugin reload`, inspect `/plugin show {0}`, then repeat the displayed trust command and `/plugin enable {0}` before retrying",
            self.authority.plugin_name
        );
        crate::plugins::registry::verify_plugin_component_authority(
            &self.authority,
            self.required_capability(),
        )
        .map_err(|reason| {
            anyhow::anyhow!(
                "Refusing to {operation} MCP server '{server_name}' from plugin bundle `{}`: {reason}. {remediation}",
                self.authority.plugin_name
            )
        })
    }

    fn validate_remote_endpoint(&self, server_name: &str, endpoint: &str) -> Result<()> {
        let (endpoint, origin) = reviewed_remote_endpoint_identity(endpoint)?;
        if self.approved_remote_endpoint.as_deref() != Some(endpoint.as_str())
            || self.approved_remote_origin.as_deref() != Some(origin.as_str())
        {
            anyhow::bail!(
                "Refusing MCP server '{server_name}': its remote endpoint no longer matches the reviewed plugin origin"
            );
        }
        Ok(())
    }

    fn catalog_is_current(&self) -> bool {
        // Catalog exposure is an authority boundary too: stale tool, prompt,
        // or resource descriptions can steer the model even when the later
        // operation would be denied. Revalidate both the mutable reviewed
        // source and the Codewhale-owned stage before publishing any entry.
        crate::plugins::registry::verify_plugin_component_authority(
            &self.authority,
            self.required_capability(),
        )
        .is_ok()
    }
}

#[cfg(target_os = "macos")]
fn is_node_command(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "node" | "nodejs"))
}

/// Rewrite a Node launch so a reviewed `.mjs` entrypoint keeps ESM semantics
/// after Darwin's reviewed-launch binding replaced its staged path with an
/// inherited `/dev/fd/N` descriptor.
///
/// Node determines the entrypoint module type from its filename and a
/// descriptor path has no extension, so `node /dev/fd/N` exits without
/// evaluating the module. `--experimental-default-type=module` used to fix
/// that but was removed from current Node releases (Node 25 rejects it as a
/// bad option, which killed the child before the MCP handshake). `--import`
/// has loaded its specifier as an ES module on every supported release, so
/// the reviewed bytes are imported by descriptor once, `-e ""` supplies an
/// empty main, and the descriptor path is echoed after `--` so
/// `process.argv[1]` and the script's own arguments keep the file-mode shape.
/// Node options that preceded the entrypoint stay in front; anything after it
/// is passed through untouched. When the `.mjs` file is not the first
/// positional argument it is not the entrypoint and the launch is left alone.
#[cfg(target_os = "macos")]
fn node_esm_descriptor_args(
    args: &[std::ffi::OsString],
    entry_index: usize,
) -> Vec<std::ffi::OsString> {
    let leading_are_options = args[..entry_index]
        .iter()
        .all(|argument| argument.to_string_lossy().starts_with('-'));
    if !leading_are_options {
        return args.to_vec();
    }
    let bound_entry = args[entry_index].clone();
    let mut rewritten: Vec<std::ffi::OsString> = args[..entry_index].to_vec();
    rewritten.push(std::ffi::OsString::from("--import"));
    rewritten.push(bound_entry.clone());
    rewritten.push(std::ffi::OsString::from("-e"));
    rewritten.push(std::ffi::OsString::from(""));
    rewritten.push(std::ffi::OsString::from("--"));
    rewritten.push(bound_entry);
    rewritten.extend(args[entry_index + 1..].iter().cloned());
    rewritten
}

pub(crate) struct ReviewedStdioLaunch {
    pub(crate) command: std::ffi::OsString,
    pub(crate) args: Vec<std::ffi::OsString>,
    pub(crate) cwd: Option<PathBuf>,
    /// Kept for the child lifetime. Windows opens deny write/delete sharing;
    /// Unix children execute/read inherited descriptors rather than paths.
    pub(crate) opened_files: Vec<fs::File>,
    #[cfg(unix)]
    pub(crate) cwd_fd: Option<fs::File>,
}

impl ReviewedStdioLaunch {
    fn bind_command(
        &mut self,
        staged_root: &Path,
        path: &Path,
        expected_hashes: &std::collections::BTreeMap<PathBuf, String>,
    ) -> Result<()> {
        let bound_path = self.bind_file(staged_root, path, expected_hashes)?;
        #[cfg(not(target_os = "macos"))]
        {
            self.command = bound_path;
            Ok(())
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::FileExt as _;

            // Darwin devfs deliberately rejects execve("/dev/fd/N"). Bind
            // reviewed scripts by running the interpreter declared in their
            // exact hashed shebang and passing the inherited descriptor as
            // input. Native Mach-O bundle commands have no fexecve/execveat
            // equivalent on Darwin, so fail closed and require the manifest
            // to name a bare interpreter with the bundle file as an argument.
            let file = self
                .opened_files
                .last()
                .context("reviewed command handle disappeared")?;
            let mut prefix = [0_u8; 4_096];
            let read = file
                .read_at(&mut prefix, 0)
                .context("read reviewed command shebang")?;
            let prefix = &prefix[..read];
            let line_end = prefix
                .iter()
                .position(|byte| *byte == b'\n')
                .unwrap_or(prefix.len());
            let line = std::str::from_utf8(&prefix[..line_end])
                .context("reviewed script shebang is not UTF-8")?;
            let shebang = line.strip_prefix("#!").map(str::trim).filter(|s| !s.is_empty())
                .context(
                    "Darwin cannot execute a reviewed native bundle command by descriptor; use a shebang script or declare a bare interpreter command plus the script argument",
                )?;
            let mut words = shlex::split(shebang)
                .context("reviewed script shebang could not be parsed safely")?;
            let interpreter = words
                .first()
                .filter(|word| Path::new(word).is_absolute())
                .context("reviewed script shebang interpreter must be absolute")?
                .clone();
            words.remove(0);
            let mut args = words
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>();
            args.push(bound_path);
            args.append(&mut self.args);
            self.command = std::ffi::OsString::from(interpreter);
            self.args = args;
            Ok(())
        }
    }

    fn bind_file(
        &mut self,
        staged_root: &Path,
        path: &Path,
        expected_hashes: &std::collections::BTreeMap<PathBuf, String>,
    ) -> Result<std::ffi::OsString> {
        let relative = path
            .strip_prefix(staged_root)
            .context("reviewed plugin executable escaped its staged root")?;
        let expected = expected_hashes
            .get(relative)
            .context("reviewed plugin executable is absent from its byte inventory")?;
        let mut file = open_reviewed_launch_file(path)?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"codewhale-plugin-file-bytes-v1\0");
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .context("read reviewed launch file")?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let actual = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if &actual != expected {
            anyhow::bail!("reviewed plugin executable bytes changed before spawn");
        }
        file.seek(std::io::SeekFrom::Start(0))
            .context("rewind reviewed launch file after verification")?;

        #[cfg(unix)]
        let launch_path = {
            use std::os::fd::AsRawFd as _;
            let fd = file.as_raw_fd();
            // SAFETY: `fd` is owned by `file`; clearing only FD_CLOEXEC keeps
            // that same descriptor available across the imminent exec.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
            {
                anyhow::bail!("failed to inherit reviewed plugin executable descriptor");
            }
            #[cfg(target_os = "linux")]
            let prefix = "/proc/self/fd";
            #[cfg(not(target_os = "linux"))]
            let prefix = "/dev/fd";
            std::ffi::OsString::from(format!("{prefix}/{fd}"))
        };

        #[cfg(not(unix))]
        let launch_path = path.as_os_str().to_os_string();

        self.opened_files.push(file);
        Ok(launch_path)
    }

    fn bind_cwd(&mut self, cwd: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(cwd)
                .context("open reviewed plugin cwd without following links")?;
            self.cwd_fd = Some(file);
            self.cwd = None;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
            let file = fs::OpenOptions::new()
                .read(true)
                .share_mode(0x0000_0001) // FILE_SHARE_READ only
                .custom_flags(0x0220_0000) // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
                .open(cwd)
                .context("open reviewed plugin cwd without write/delete sharing")?;
            let metadata = file
                .metadata()
                .context("inspect reviewed plugin cwd handle")?;
            if !metadata.is_dir() || metadata.file_attributes() & 0x0000_0400 != 0 {
                anyhow::bail!("reviewed plugin cwd is a reparse point or non-directory");
            }
            self.opened_files.push(file);
        }
        Ok(())
    }
}

#[cfg(unix)]
fn open_reviewed_launch_file(path: &Path) -> Result<fs::File> {
    crate::plugins::manifest::open_bundle_file(path)
        .context("open reviewed launch file without following links")
}

#[cfg(windows)]
fn open_reviewed_launch_file(path: &Path) -> Result<fs::File> {
    crate::plugins::manifest::open_bundle_file(path)
        .context("open reviewed launch file without links, hard links, or write/delete sharing")
}

#[cfg(all(not(unix), not(windows)))]
fn open_reviewed_launch_file(path: &Path) -> Result<fs::File> {
    fs::File::open(path).context("open reviewed launch file")
}

fn reviewed_remote_endpoint_identity(endpoint: &str) -> Result<(String, String)> {
    let endpoint =
        reqwest::Url::parse(endpoint).context("reviewed plugin MCP endpoint is invalid")?;
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        anyhow::bail!("reviewed plugin MCP endpoint must not contain user information");
    }
    if endpoint.query().is_some() || endpoint.fragment().is_some() {
        anyhow::bail!("reviewed plugin MCP endpoint must not contain a query or fragment");
    }
    let origin = reviewed_remote_origin(&endpoint)
        .ok_or_else(|| anyhow::anyhow!("reviewed plugin MCP endpoint has an unsafe origin"))?;
    Ok((endpoint.to_string(), origin))
}

fn reviewed_remote_origin(endpoint: &reqwest::Url) -> Option<String> {
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        return None;
    }
    let host = endpoint.host_str()?;
    let allowed_scheme = endpoint.scheme() == "https"
        || (endpoint.scheme() == "http"
            && (host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())));
    allowed_scheme.then(|| endpoint.origin().ascii_serialization())
}

fn reviewed_redirect_matches_origin(endpoint: &reqwest::Url, approved_origin: &str) -> bool {
    reviewed_remote_origin(endpoint).as_deref() == Some(approved_origin)
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct McpServerOAuthConfig {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

fn default_enabled() -> bool {
    true
}

impl McpServerConfig {
    pub fn effective_connect_timeout(&self, global: &McpTimeouts) -> u64 {
        self.connect_timeout.unwrap_or(global.connect_timeout)
    }

    pub fn effective_execute_timeout(&self, global: &McpTimeouts) -> u64 {
        self.execute_timeout.unwrap_or(global.execute_timeout)
    }

    pub fn effective_read_timeout(&self, global: &McpTimeouts) -> u64 {
        self.read_timeout.unwrap_or(global.read_timeout)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled && !self.disabled
    }

    pub fn is_tool_enabled(&self, tool_name: &str) -> bool {
        let allowed = if self.enabled_tools.is_empty() {
            true
        } else {
            self.enabled_tools.iter().any(|t| t == tool_name)
        };
        if !allowed {
            return false;
        }
        !self.disabled_tools.iter().any(|t| t == tool_name)
    }
}

// === MCP Tool Definition ===

/// Tool discovered from an MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: serde_json::Value,
}

const MCP_TOOL_DESCRIPTION_MAX_CHARS: usize = 80;

/// Format an optional MCP tool description for terminal list surfaces.
///
/// CLI and TUI callers share this helper so both stay single-line and truncate
/// on Unicode scalar boundaries rather than slicing UTF-8 bytes.
pub(crate) fn format_mcp_tool_description(description: Option<&str>) -> String {
    let Some(first_line) = description
        .and_then(|description| description.split(['\r', '\n']).next())
        .map(str::trim)
        .filter(|description| !description.is_empty())
    else {
        return String::new();
    };

    let mut chars = first_line.chars();
    let summary: String = chars
        .by_ref()
        .take(MCP_TOOL_DESCRIPTION_MAX_CHARS)
        .collect();
    if chars.next().is_some() {
        format!(": {summary}...")
    } else {
        format!(": {summary}")
    }
}

/// Resource discovered from an MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
}

/// Resource template discovered from an MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpResourceTemplate {
    #[serde(rename = "uriTemplate")]
    pub uri_template: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
}

/// Fail-closed RFC 6570 subset used only as an authorization check. Literal,
/// simple (`{id}`), and reserved (`{+path}`) expansions cover the common MCP
/// resource templates. More elaborate operators remain listable but are not
/// callable until their expansion semantics are implemented exactly.
fn resource_uri_matches_template(uri: &str, template: &str) -> bool {
    let mut pattern = String::from("^");
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        pattern.push_str(&regex::escape(&rest[..start]));
        let Some(end) = rest[start + 1..].find('}') else {
            return false;
        };
        let expression = &rest[start + 1..start + 1 + end];
        let (reserved, variables) = match expression.strip_prefix('+') {
            Some(variables) => (true, variables),
            None => (false, expression),
        };
        if variables.is_empty()
            || variables.split(',').any(|variable| {
                variable.is_empty()
                    || !variable
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
            })
        {
            return false;
        }
        let atom = if reserved { ".+" } else { "[^/?#]+" };
        for (index, _) in variables.split(',').enumerate() {
            if index > 0 {
                pattern.push(',');
            }
            pattern.push_str(atom);
        }
        rest = &rest[start + end + 2..];
    }
    if rest.contains('}') {
        return false;
    }
    pattern.push_str(&regex::escape(rest));
    pattern.push('$');
    regex::Regex::new(&pattern).is_ok_and(|regex| regex.is_match(uri))
}

/// Prompt discovered from an MCP server
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

/// Argument for an MCP prompt
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpPromptArgument {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

// === Connection State ===

/// State of an MCP connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Ready,
    Disconnected,
}

/// MCP server capabilities advertised in the initialize response.
///
/// Each flag records presence of the corresponding MCP capability object. The
/// surrounding [`McpServerCapabilityMetadata`] preserves the important
/// distinction between an advertised empty set and a legacy server that did
/// not send capability metadata at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct McpServerCapabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
}

impl McpServerCapabilities {
    fn from_initialize_response(response: &serde_json::Value) -> Option<Self> {
        let capabilities = response.get("result")?.get("capabilities")?.as_object()?;
        Some(Self {
            tools: capabilities.contains_key("tools"),
            resources: capabilities.contains_key("resources"),
            prompts: capabilities.contains_key("prompts"),
        })
    }
}

/// Provenance-aware capability metadata for a manager snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum McpServerCapabilityMetadata {
    /// The server supplied a spec-shaped `capabilities` object at initialize.
    Advertised(McpServerCapabilities),
    /// A connected legacy server omitted metadata, so bounded discovery probes
    /// remain enabled for backward compatibility.
    LegacyFallback,
    /// The server has not completed initialization, so no truthful capability
    /// claim can be made yet.
    #[default]
    NotObserved,
}

fn response_result<'a>(
    response: &'a serde_json::Value,
    method: &str,
    suppress_server_details: bool,
) -> Result<Option<&'a serde_json::Value>> {
    if let Some(error) = response.get("error") {
        if suppress_server_details {
            anyhow::bail!(
                "Reviewed plugin MCP server returned an error in '{method}' (server details suppressed to protect environment-backed credentials)"
            );
        }
        anyhow::bail!("MCP error in '{method}': {error}");
    }
    Ok(response.get("result"))
}

async fn run_optional_discovery<F>(server: &str, method: &str, timeout: Duration, discovery: F)
where
    F: Future<Output = Result<()>>,
{
    match tokio::time::timeout(timeout, discovery).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(
                target: "mcp",
                server,
                method,
                error = %error,
                "optional MCP discovery failed; continuing with available capabilities"
            );
        }
        Err(error) => {
            tracing::warn!(
                target: "mcp",
                server,
                method,
                ?timeout,
                error = %error,
                "optional MCP discovery timed out; continuing with available capabilities"
            );
        }
    }
}

// === McpConnection - Async Connection Management ===

// === Transport Trait ===

#[async_trait::async_trait]
pub trait McpTransport: Send + Sync {
    async fn send(&mut self, msg: Vec<u8>) -> Result<()>;
    async fn recv(&mut self) -> Result<Vec<u8>>;

    /// Graceful shutdown — stdio transports send SIGTERM to the child and
    /// give it a brief window to exit before tokio's `kill_on_drop` fires
    /// SIGKILL as the backstop. Default is a no-op for non-stdio transports
    /// that have no child process. Whalescale#420.
    async fn shutdown(&mut self) {}
}

const MAX_MCP_CATALOG_PAGES: usize = 64;
const MAX_MCP_CATALOG_ITEMS: usize = 4_096;
const MAX_MCP_CATALOG_BYTES: usize = 32 * 1024 * 1024;

struct McpCatalogBudget {
    method: &'static str,
    pages: usize,
    items: usize,
    bytes: usize,
    seen_cursors: HashSet<String>,
}

impl McpCatalogBudget {
    fn new(method: &'static str) -> Self {
        Self {
            method,
            pages: 0,
            items: 0,
            bytes: 0,
            seen_cursors: HashSet::new(),
        }
    }

    fn observe_page(
        &mut self,
        result: &serde_json::Value,
        item_count: usize,
    ) -> Result<Option<String>> {
        self.pages = self.pages.saturating_add(1);
        self.items = self.items.saturating_add(item_count);
        self.bytes = self.bytes.saturating_add(serde_json::to_vec(result)?.len());
        if self.pages > MAX_MCP_CATALOG_PAGES {
            anyhow::bail!(
                "{} exceeded the {}-page catalogue limit",
                self.method,
                MAX_MCP_CATALOG_PAGES
            );
        }
        if self.items > MAX_MCP_CATALOG_ITEMS {
            anyhow::bail!(
                "{} exceeded the {}-item catalogue limit",
                self.method,
                MAX_MCP_CATALOG_ITEMS
            );
        }
        if self.bytes > MAX_MCP_CATALOG_BYTES {
            anyhow::bail!(
                "{} exceeded the {}-byte aggregate catalogue limit",
                self.method,
                MAX_MCP_CATALOG_BYTES
            );
        }
        let cursor = result
            .get("nextCursor")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        if let Some(cursor) = cursor.as_ref()
            && !self.seen_cursors.insert(cursor.clone())
        {
            anyhow::bail!("{} repeated pagination cursor; aborting", self.method);
        }
        Ok(cursor)
    }
}

fn is_legacy_sse_transport(config: &McpServerConfig) -> bool {
    config
        .transport
        .as_deref()
        .map(|transport| transport.trim().eq_ignore_ascii_case("sse"))
        .unwrap_or(false)
}

pub fn validate_mcp_transport(transport: Option<&str>) -> Result<()> {
    let Some(transport) = transport else {
        return Ok(());
    };
    if transport.trim().eq_ignore_ascii_case("sse") {
        return Ok(());
    }
    anyhow::bail!("Unsupported MCP transport '{transport}'. Supported values: sse");
}

fn response_id_matches(id: Option<&serde_json::Value>, expected_id: &str) -> bool {
    let Some(id) = id else {
        return false;
    };
    if id.as_str() == Some(expected_id) {
        return true;
    }
    id.as_u64()
        .map(|id| id.to_string() == expected_id)
        .unwrap_or(false)
}

// === McpConnection - Async Connection Management ===

/// Manages a single async connection to an MCP server
pub struct McpConnection {
    name: String,
    transport: Box<dyn McpTransport>,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    resource_templates: Vec<McpResourceTemplate>,
    prompts: Vec<McpPrompt>,
    request_id: AtomicU64,
    state: ConnectionState,
    config: McpServerConfig,
    server_capabilities: Option<McpServerCapabilities>,
    discovery_timeout: Duration,
    read_timeout_secs: u64,
    cancel_token: tokio_util::sync::CancellationToken,
    authority_revocation_reason: Arc<std::sync::Mutex<Option<String>>>,
    authority_watch: Option<tokio::task::JoinHandle<()>>,
    /// Pool catalog generation that created/last authorized this connection.
    /// Directly constructed test connections use zero until inserted.
    catalog_generation: u64,
}

struct PendingAuthorityWatch {
    handle: Option<tokio::task::JoinHandle<()>>,
    cancel: tokio_util::sync::CancellationToken,
    armed: bool,
}

impl PendingAuthorityWatch {
    fn start(
        source: ReviewedPluginMcpSource,
        cancel: tokio_util::sync::CancellationToken,
        reason_slot: Arc<std::sync::Mutex<Option<String>>>,
    ) -> Self {
        let task_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            loop {
                if let Err(reason) =
                    crate::plugins::registry::verify_plugin_state_authority(&source.authority)
                {
                    if let Ok(mut slot) = reason_slot.lock() {
                        *slot = Some(reason);
                    }
                    task_cancel.cancel();
                    break;
                }
                tokio::select! {
                    _ = task_cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            }
        });
        Self {
            handle: Some(handle),
            cancel,
            armed: true,
        }
    }

    fn disarm(mut self) -> tokio::task::JoinHandle<()> {
        self.armed = false;
        self.handle.take().expect("authority watch must exist")
    }
}

impl Drop for PendingAuthorityWatch {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.cancel();
            if let Some(handle) = self.handle.take() {
                handle.abort();
            }
        }
    }
}

impl McpConnection {
    /// Connect to an MCP server and initialize it.
    ///
    /// `network_policy` (added in v0.7.0 for #135) is consulted for HTTP/SSE
    /// transports only — STDIO transports are unaffected. Pass `None` to
    /// match pre-v0.7.0 permissive behavior.
    pub async fn connect_with_policy(
        name: String,
        config: McpServerConfig,
        global_timeouts: &McpTimeouts,
        network_policy: Option<&NetworkPolicyDecider>,
    ) -> Result<Self> {
        let connect_timeout_secs = config.effective_connect_timeout(global_timeouts);
        let read_timeout_secs = config.effective_read_timeout(global_timeouts);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let authority_revocation_reason = Arc::new(std::sync::Mutex::new(None));
        if let Some(source) = config.reviewed_plugin.as_ref() {
            source.validate_before_use(&name, "connect")?;
            if let Some(url) = config.url.as_deref() {
                source.validate_remote_endpoint(&name, url)?;
            }
        }
        // Start the cross-process generation watch before any network request
        // or child spawn. The guard cancels and aborts itself on every early
        // return; a successful connection transfers the task into `Self`.
        let authority_watch = config.reviewed_plugin.clone().map(|source| {
            PendingAuthorityWatch::start(
                source,
                cancel_token.clone(),
                Arc::clone(&authority_revocation_reason),
            )
        });
        let transport: Box<dyn McpTransport> = if let Some(url) = &config.url {
            // Per-domain network policy gate (#135). Only the HTTP/SSE transport
            // is gated; STDIO MCP servers run as local subprocesses and never
            // touch the network from this code path.
            if let Some(decider) = network_policy
                && let Some(host) = host_from_url(url)
            {
                match decider.evaluate(&host, "mcp") {
                    Decision::Allow => {}
                    Decision::Deny => {
                        anyhow::bail!(
                            "MCP server '{name}' connection to '{host}' blocked by network policy"
                        );
                    }
                    Decision::Prompt => {
                        anyhow::bail!(
                            "MCP server '{name}' connection to '{host}' requires approval; \
                             re-run after `/network allow {host}` or set network.default = \"allow\" in config"
                        );
                    }
                }
            }
            // Honor the standard `HTTP_PROXY` / `HTTPS_PROXY` (and their
            // lowercase equivalents) plus `NO_PROXY` env vars when
            // reaching MCP HTTP servers (#1408). Reqwest 0.13 does not
            // auto-detect these by default, so users behind corporate
            // proxies, on China-mainland connections routing through a
            // local Clash / Shadowsocks tunnel, etc. previously had MCP
            // HTTP traffic bypass the proxy entirely while every other
            // tool on the box (curl, npm, …) used it.
            // `connect_timeout` bounds only the connect phase; the total request
            // timeout is the read timeout (a sane backstop) so per-call
            // execute_timeout can actually govern request duration. Previously
            // this set reqwest's TOTAL `.timeout()` from connect_timeout (10s),
            // which silently capped every request at 10s and made the per-server
            // execute_timeout / read_timeout dead for HTTP transports.
            let mut client_builder = crate::tls::reqwest_client_builder()
                .connect_timeout(Duration::from_secs(connect_timeout_secs))
                .timeout(Duration::from_secs(read_timeout_secs));
            if let Some(approved_origin) = config
                .reviewed_plugin
                .as_ref()
                .and_then(|source| source.approved_remote_origin.clone())
            {
                client_builder =
                    client_builder.redirect(reqwest::redirect::Policy::custom(move |attempt| {
                        if attempt.previous().len() >= 5 {
                            return attempt.stop();
                        }
                        if reviewed_redirect_matches_origin(attempt.url(), &approved_origin) {
                            attempt.follow()
                        } else {
                            attempt.stop()
                        }
                    }));
            }
            client_builder =
                configure_mcp_proxy(client_builder, config.reviewed_plugin.is_some(), |name| {
                    std::env::var(name)
                });
            let client = client_builder.build()?;
            let oauth_runtime = if config.reviewed_plugin.is_some() {
                None
            } else {
                match oauth::build_default_headers(&config.headers, &config.env_headers) {
                    Ok(default_headers) => {
                        let prepared = tokio::select! {
                            biased;
                            _ = cancel_token.cancelled() => {
                                anyhow::bail!(
                                    "MCP OAuth setup cancelled after plugin authority changed"
                                )
                            }
                            prepared = oauth::McpOAuthRuntime::from_server_config(
                                &name,
                                &config,
                                default_headers,
                            ) => prepared,
                        };
                        match prepared {
                            Ok(runtime) => runtime,
                            Err(err) => {
                                if config.reviewed_plugin.is_some() {
                                    tracing::warn!(
                                        target: "mcp",
                                        server = %name,
                                        "failed to prepare reviewed plugin MCP OAuth runtime; provider details suppressed; continuing without stored OAuth token"
                                    );
                                } else {
                                    tracing::warn!(
                                        target: "mcp",
                                        server = %name,
                                        error = %err,
                                        "failed to prepare MCP OAuth runtime; continuing without stored OAuth token"
                                    );
                                }
                                None
                            }
                        }
                    }
                    Err(err) => {
                        if config.reviewed_plugin.is_some() {
                            tracing::warn!(
                                target: "mcp",
                                server = %name,
                                "failed to prepare reviewed plugin MCP OAuth headers; details suppressed; continuing without stored OAuth token"
                            );
                        } else {
                            tracing::warn!(
                                target: "mcp",
                                server = %name,
                                error = %err,
                                "failed to prepare MCP OAuth default headers; continuing without stored OAuth token"
                            );
                        }
                        None
                    }
                }
            };
            let http_auth = McpHttpAuth::from_config(&name, &config, oauth_runtime);
            if is_legacy_sse_transport(&config) {
                Box::new(
                    SseTransport::connect(
                        client,
                        url.clone(),
                        http_auth,
                        cancel_token.clone(),
                        Duration::from_secs(connect_timeout_secs),
                    )
                    .await?,
                )
            } else {
                let mut http = HttpTransport::new(
                    client,
                    url.clone(),
                    http_auth,
                    cancel_token.clone(),
                    Duration::from_secs(connect_timeout_secs),
                );
                // Best-effort session preflight for servers that require
                // a session ID on every POST including `initialize`
                // (e.g. Hindsight, #1629). Failures are non-fatal — the
                // `initialize` POST will proceed and may capture a session
                // ID from the response instead.
                if let Err(e) = http.try_establish_session().await {
                    tracing::debug!(
                        target: "mcp",
                        server = %name,
                        error = %e,
                        "session-establishment GET skipped; proceeding with POST initialize"
                    );
                }
                Box::new(http)
            }
        } else if let Some(command) = &config.command {
            Box::new(StdioTransport::spawn(
                &name,
                command,
                &config,
                cancel_token.clone(),
            )?)
        } else {
            anyhow::bail!("MCP server '{name}' config must have either 'command' or 'url'");
        };
        // Revalidate after transport construction as well: remote setup may
        // await DNS/TLS/SSE preflight, and a concurrent process can revoke the
        // receipt during that interval. Initialization and catalog discovery
        // never start under a stale generation.
        if let Some(source) = config.reviewed_plugin.as_ref() {
            source.validate_before_use(&name, "initialize")?;
        }
        let authority_watch = authority_watch.map(PendingAuthorityWatch::disarm);

        let mut conn = Self {
            name: name.clone(),
            transport,
            tools: Vec::new(),
            resources: Vec::new(),
            resource_templates: Vec::new(),
            prompts: Vec::new(),
            request_id: AtomicU64::new(1),
            state: ConnectionState::Connecting,
            config,
            server_capabilities: None,
            discovery_timeout: Duration::from_secs(connect_timeout_secs),
            read_timeout_secs,
            cancel_token,
            authority_revocation_reason,
            authority_watch,
            catalog_generation: 0,
        };

        // Initialize with timeout
        tokio::time::timeout(Duration::from_secs(connect_timeout_secs), conn.initialize())
            .await
            .with_context(|| format!("MCP server '{name}' initialization timed out"))??;

        conn.discover_all()
            .await
            .with_context(|| format!("MCP server '{name}' discovery failed"))?;

        conn.state = ConnectionState::Ready;
        Ok(conn)
    }

    /// Send initialize request and wait for response
    async fn initialize(&mut self) -> Result<()> {
        let init_id = self.next_id();
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": &init_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "clientInfo": {
                    "name": "codewhale-tui",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "tools": {},
                    "resources": {},
                    "prompts": {}
                }
            }
        }))
        .await?;

        let response = self.recv(init_id).await?;
        response_result(
            &response,
            "initialize",
            self.config.reviewed_plugin.is_some(),
        )?;
        self.server_capabilities = McpServerCapabilities::from_initialize_response(&response);

        // Send initialized notification (no id, no response expected)
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .await?;

        Ok(())
    }

    /// Discover tools, resources, and prompts
    async fn discover_all(&mut self) -> Result<()> {
        let capabilities = self.server_capabilities;
        let server = self.name.clone();
        let discovery_timeout = self.discovery_timeout;

        // Missing initialize metadata is treated as a legacy/unknown server:
        // retain tool discovery and bounded best-effort probes for compatibility.
        // When capabilities are advertised, do not call methods the server says
        // it does not implement (notably JetBrains tools-only MCP servers).
        if capabilities.is_none_or(|capabilities| capabilities.tools) {
            tokio::time::timeout(discovery_timeout, self.discover_tools())
                .await
                .with_context(|| {
                    format!(
                        "MCP server '{}' tool discovery timed out after {:?}",
                        server, discovery_timeout
                    )
                })??;
        }

        // Keep all three optional calls within one discovery-timeout budget in
        // the worst case while also respecting a tighter transport read timeout.
        let optional_timeout =
            (discovery_timeout / 3).min(Duration::from_secs(self.read_timeout_secs));
        if capabilities.is_none_or(|capabilities| capabilities.resources) {
            run_optional_discovery(
                &server,
                "resources/list",
                optional_timeout,
                self.discover_resources(),
            )
            .await;
            run_optional_discovery(
                &server,
                "resources/templates/list",
                optional_timeout,
                self.discover_resource_templates(),
            )
            .await;
        }
        if capabilities.is_none_or(|capabilities| capabilities.prompts) {
            run_optional_discovery(
                &server,
                "prompts/list",
                optional_timeout,
                self.discover_prompts(),
            )
            .await;
        }
        Ok(())
    }

    /// Discover available tools from the MCP server
    async fn discover_tools(&mut self) -> Result<()> {
        let mut cursor: Option<String> = None;
        let mut budget = McpCatalogBudget::new("tools/list");
        let mut discovered = Vec::new();
        loop {
            let list_id = self.next_id();
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            self.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": &list_id,
                "method": "tools/list",
                "params": params
            }))
            .await?;

            let response = self.recv(list_id).await?;
            let Some(result) = response_result(
                &response,
                "tools/list",
                self.config.reviewed_plugin.is_some(),
            )?
            else {
                break;
            };

            let items = result
                .get("tools")
                .and_then(|tools| tools.as_array())
                .map_or(0, Vec::len);
            if let Some(arr) = result.get("tools").and_then(|t| t.as_array()) {
                for item in arr {
                    match serde_json::from_value::<McpTool>(item.clone()) {
                        Ok(tool) => discovered.push(tool),
                        Err(err) => {
                            // Skip individual malformed entries instead of
                            // dropping the whole page (#1410). The old
                            // `unwrap_or_default()` would silently throw
                            // away every tool when one was misshapen.
                            tracing::debug!(target: "mcp", ?err, "skipping malformed tool item");
                        }
                    }
                }
            }

            cursor = budget.observe_page(result, items)?;
            if cursor.is_none() {
                break;
            }
        }
        // Sort by tool name so the order the model sees doesn't depend on
        // server-side pagination ordering — keeps the prompt prefix stable
        // for cache-hit purposes (#1319).
        discovered.sort_by(|a, b| a.name.cmp(&b.name));
        self.tools = discovered;
        Ok(())
    }

    /// Discover available resources from the MCP server
    async fn discover_resources(&mut self) -> Result<()> {
        let mut cursor: Option<String> = None;
        let mut budget = McpCatalogBudget::new("resources/list");
        let mut discovered = Vec::new();
        loop {
            let list_id = self.next_id();
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            self.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": &list_id,
                "method": "resources/list",
                "params": params
            }))
            .await?;

            let response = self.recv(list_id).await?;
            let Some(result) = response_result(
                &response,
                "resources/list",
                self.config.reviewed_plugin.is_some(),
            )?
            else {
                break;
            };

            let items = result
                .get("resources")
                .and_then(|resources| resources.as_array())
                .map_or(0, Vec::len);
            if let Some(arr) = result.get("resources").and_then(|r| r.as_array()) {
                for item in arr {
                    match serde_json::from_value::<McpResource>(item.clone()) {
                        Ok(resource) => discovered.push(resource),
                        Err(err) => {
                            tracing::debug!(target: "mcp", ?err, "skipping malformed resource item");
                        }
                    }
                }
            }

            cursor = budget.observe_page(result, items)?;
            if cursor.is_none() {
                break;
            }
        }
        self.resources = discovered;
        Ok(())
    }

    /// Discover available resource templates from the MCP server
    async fn discover_resource_templates(&mut self) -> Result<()> {
        let mut cursor: Option<String> = None;
        let mut budget = McpCatalogBudget::new("resources/templates/list");
        let mut discovered = Vec::new();
        loop {
            let list_id = self.next_id();
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            self.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": &list_id,
                "method": "resources/templates/list",
                "params": params
            }))
            .await?;

            let response = self.recv(list_id).await?;
            let Some(result) = response_result(
                &response,
                "resources/templates/list",
                self.config.reviewed_plugin.is_some(),
            )?
            else {
                break;
            };

            let templates = result
                .get("resourceTemplates")
                .or_else(|| result.get("templates"))
                .or_else(|| result.get("resource_templates"));
            let items = templates
                .and_then(|templates| templates.as_array())
                .map_or(0, Vec::len);
            if let Some(arr) = templates.and_then(|t| t.as_array()) {
                for item in arr {
                    match serde_json::from_value::<McpResourceTemplate>(item.clone()) {
                        Ok(tmpl) => discovered.push(tmpl),
                        Err(err) => {
                            tracing::debug!(target: "mcp", ?err, "skipping malformed resource_template item");
                        }
                    }
                }
            }

            cursor = budget.observe_page(result, items)?;
            if cursor.is_none() {
                break;
            }
        }
        self.resource_templates = discovered;
        Ok(())
    }

    /// Discover available prompts from the MCP server
    async fn discover_prompts(&mut self) -> Result<()> {
        let mut cursor: Option<String> = None;
        let mut budget = McpCatalogBudget::new("prompts/list");
        let mut discovered = Vec::new();
        loop {
            let list_id = self.next_id();
            let params = match &cursor {
                Some(c) => serde_json::json!({ "cursor": c }),
                None => serde_json::json!({}),
            };
            self.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": &list_id,
                "method": "prompts/list",
                "params": params
            }))
            .await?;

            let response = self.recv(list_id).await?;
            let Some(result) = response_result(
                &response,
                "prompts/list",
                self.config.reviewed_plugin.is_some(),
            )?
            else {
                break;
            };

            let items = result
                .get("prompts")
                .and_then(|prompts| prompts.as_array())
                .map_or(0, Vec::len);
            if let Some(arr) = result.get("prompts").and_then(|p| p.as_array()) {
                for item in arr {
                    match serde_json::from_value::<McpPrompt>(item.clone()) {
                        Ok(prompt) => discovered.push(prompt),
                        Err(err) => {
                            tracing::debug!(target: "mcp", ?err, "skipping malformed prompt item");
                        }
                    }
                }
            }

            cursor = budget.observe_page(result, items)?;
            if cursor.is_none() {
                break;
            }
        }
        self.prompts = discovered;
        Ok(())
    }

    /// Call a tool on this MCP server
    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        self.call_method(
            "tools/call",
            serde_json::json!({
                "name": tool_name,
                "arguments": arguments
            }),
            timeout_secs,
        )
        .await
    }

    /// Read a resource from this MCP server
    pub async fn read_resource(
        &mut self,
        uri: &str,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        self.call_method(
            "resources/read",
            serde_json::json!({
                "uri": uri
            }),
            timeout_secs,
        )
        .await
    }

    /// Get a prompt from this MCP server
    pub async fn get_prompt(
        &mut self,
        prompt_name: &str,
        arguments: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        self.call_method(
            "prompts/get",
            serde_json::json!({
                "name": prompt_name,
                "arguments": arguments
            }),
            timeout_secs,
        )
        .await
    }

    /// Generic method to call an MCP method
    async fn call_method(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value> {
        if self.state != ConnectionState::Ready {
            anyhow::bail!(
                "Failed to call MCP method '{}': connection '{}' is not ready",
                method,
                self.name
            );
        }
        if let Some(source) = self.config.reviewed_plugin.as_ref() {
            source.validate_before_use(&self.name, method)?;
        }

        let call_id = self.next_id();
        if let Err(error) = self
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": &call_id,
                "method": method,
                "params": params
            }))
            .await
        {
            return self.finish_guarded_error(error).await;
        }

        let response =
            match tokio::time::timeout(Duration::from_secs(timeout_secs), self.recv(call_id))
                .await
                .with_context(|| {
                    format!(
                        "MCP method '{}' on server '{}' timed out after {}s",
                        method, self.name, timeout_secs
                    )
                }) {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => return self.finish_guarded_error(error).await,
                Err(error) => return self.finish_guarded_error(error).await,
            };

        if let Some(error) = response.get("error") {
            if self.config.reviewed_plugin.is_some() {
                anyhow::bail!(
                    "Reviewed plugin MCP server returned an error in '{method}' (server details suppressed to protect environment-backed credentials)"
                );
            }
            return Err(anyhow::anyhow!(
                "MCP error in '{}': {}",
                method,
                serde_json::to_string_pretty(error)?
            ));
        }

        // JSON-RPC requires exactly one of `result` / `error`. Treating a
        // response carrying neither as an empty success handed the model a
        // `null` tool result that is indistinguishable from a tool that
        // genuinely returned nothing. An explicit `"result": null` is still a
        // valid empty success and passes through unchanged.
        response.get("result").cloned().with_context(|| {
            format!(
                "MCP response from server '{}' for '{method}' contained neither a result nor an error",
                self.name
            )
        })
    }

    /// Get discovered tools
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Get discovered resources
    pub fn resources(&self) -> &[McpResource] {
        &self.resources
    }

    /// Get discovered resource templates
    pub fn resource_templates(&self) -> &[McpResourceTemplate] {
        &self.resource_templates
    }

    /// Get discovered prompts
    pub fn prompts(&self) -> &[McpPrompt] {
        &self.prompts
    }

    /// Get server name
    #[allow(dead_code)] // Public API for MCP consumers
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Check if connection is ready
    pub fn is_ready(&self) -> bool {
        self.state == ConnectionState::Ready && self.catalog_authorized()
    }

    /// Get server config
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Get connection state
    #[allow(dead_code)] // Public API for MCP consumers
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    fn next_id(&self) -> String {
        self.request_id.fetch_add(1, Ordering::SeqCst).to_string()
    }

    async fn send(&mut self, msg: serde_json::Value) -> Result<()> {
        let bytes = serde_json::to_vec(&msg).context("Failed to serialize MCP JSON-RPC message")?;
        let cancel_token = self.cancel_token.clone();
        let name = self.name.clone();
        let result = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                Err(anyhow::anyhow!("MCP connection '{name}' was cancelled"))
            }
            result = self.transport.send(bytes) => result,
        };
        if result.is_err() {
            // A dead write side is as fatal as a dead read side: the pool
            // reuses any connection whose `is_ready()` is true, so leaving
            // this one in `Ready` would hand the same broken transport back
            // on every later call instead of rebuilding it.
            self.state = ConnectionState::Disconnected;
        }
        result
    }

    async fn recv(&mut self, expected_id: String) -> Result<serde_json::Value> {
        loop {
            let bytes = match tokio::time::timeout(
                Duration::from_secs(self.read_timeout_secs),
                async {
                    tokio::select! {
                        biased;
                        _ = self.cancel_token.cancelled() => {
                            anyhow::bail!("MCP connection '{}' was cancelled", self.name)
                        }
                        result = self.transport.recv() => result,
                    }
                },
            )
            .await
            {
                Ok(result) => result.inspect_err(|_e| {
                    self.state = ConnectionState::Disconnected;
                })?,
                Err(_) => {
                    self.state = ConnectionState::Disconnected;
                    anyhow::bail!(
                        "Timed out waiting for MCP JSON-RPC response from server '{}' after {}s",
                        self.name,
                        self.read_timeout_secs
                    );
                }
            };
            let value: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(err) => {
                    self.state = ConnectionState::Disconnected;
                    let preview = if self.config.reviewed_plugin.is_some() {
                        "<server details suppressed for reviewed plugin>".to_string()
                    } else {
                        invalid_json_preview(&bytes)
                    };
                    return Err(err).with_context(|| {
                        format!(
                            "Invalid MCP JSON-RPC message from server '{}': {}",
                            self.name, preview
                        )
                    });
                }
            };

            // Check if this is a response with the expected id. We emit
            // string IDs because some MCP gateways reject numeric JSON-RPC
            // IDs, but accept numeric echoes for compatibility with older
            // servers and tests.
            if response_id_matches(value.get("id"), &expected_id) {
                if let Some(error) = value.get("error")
                    && is_mcp_stale_session_body(&error.to_string())
                {
                    anyhow::bail!("MCP session expired: {error}");
                }
                return Ok(value);
            }
            // Skip notifications (no id) and responses with different ids
        }
    }

    /// Gracefully close the connection
    #[allow(dead_code)] // Public API for MCP consumers
    pub fn close(&mut self) {
        self.cancel_token.cancel();
        self.state = ConnectionState::Disconnected;
    }

    fn catalog_authorized(&self) -> bool {
        self.config
            .reviewed_plugin
            .as_ref()
            .is_none_or(ReviewedPluginMcpSource::catalog_is_current)
    }

    async fn finish_guarded_error<T>(&mut self, error: anyhow::Error) -> Result<T> {
        let reason = self
            .authority_revocation_reason
            .lock()
            .ok()
            .and_then(|reason| reason.clone());
        if let Some(reason) = reason {
            self.transport.shutdown().await;
            self.state = ConnectionState::Disconnected;
            anyhow::bail!(
                "MCP operation on plugin server '{}' was cancelled after authority changed: {reason}",
                self.name
            );
        }
        Err(error)
    }
}

/// Apply the ambient proxy policy for MCP HTTP transports.
///
/// User-authored MCP configuration keeps the long-standing corporate-proxy
/// behavior. Reviewed plugin bundles deliberately do not: proxy URLs can carry
/// credentials and proxy processes can observe request metadata, neither of
/// which is part of the v1 reviewed remote authority. Return before consulting
/// the environment so even reading ambient proxy credentials is impossible on
/// that path, and call `no_proxy` explicitly to keep this invariant stable if
/// reqwest's defaults change.
fn configure_mcp_proxy<F>(
    mut client_builder: reqwest::ClientBuilder,
    reviewed_plugin: bool,
    mut read_environment: F,
) -> reqwest::ClientBuilder
where
    F: FnMut(&str) -> std::result::Result<String, std::env::VarError>,
{
    if reviewed_plugin {
        return client_builder.no_proxy();
    }

    let env_proxy_url = read_environment("HTTPS_PROXY")
        .or_else(|_| read_environment("https_proxy"))
        .or_else(|_| read_environment("HTTP_PROXY"))
        .or_else(|_| read_environment("http_proxy"))
        .ok()
        .filter(|s| !s.trim().is_empty());
    if let Some(proxy_url) = env_proxy_url {
        match reqwest::Proxy::all(&proxy_url) {
            Ok(proxy) => {
                let no_proxy = read_environment("NO_PROXY")
                    .or_else(|_| read_environment("no_proxy"))
                    .ok()
                    .and_then(|value| reqwest::NoProxy::from_string(&value));
                let proxy = proxy.no_proxy(no_proxy);
                client_builder = client_builder.proxy(proxy);
            }
            Err(err) => {
                // Redact userinfo (the `username[:password]@…`
                // portion of the URL) before logging so an
                // HTTPS_PROXY that embeds credentials
                // (common in corporate setups) doesn't leak the
                // password to the on-disk `~/.deepseek/logs/`.
                let proxy_redacted = redact_proxy_userinfo(&proxy_url);
                tracing::warn!(
                    target: "mcp",
                    ?err,
                    proxy = %proxy_redacted,
                    "ignoring malformed HTTP(S)_PROXY env var; MCP connection will bypass proxy"
                );
            }
        }
    }
    client_builder
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        if let Some(watch) = self.authority_watch.take() {
            watch.abort();
        }
    }
}

// === McpPool - Connection Pool Management ===

#[derive(Debug, Clone)]
struct McpToolRoute {
    server_name: String,
    tool_name: String,
    catalog_generation: u64,
    plugin_authority: Option<crate::plugins::types::PluginAuthority>,
}

/// Model-facing name suffix of the synthetic self-serve OAuth login tool
/// (`mcp_<server>_authenticate`). Registered by [`McpPool::to_api_tools`] for
/// servers whose last connect failed auth-required; executed by
/// [`McpPool::call_tool`] through the same flow `/mcp login` uses.
pub(crate) const AUTHENTICATE_TOOL_NAME: &str = "authenticate";

/// Result of [`McpPool::begin_authenticate_tool`]: either the shared token
/// store already holds a usable credential (a login completed elsewhere since
/// the catalog was built) or a browser login has been started and must be
/// finished outside the pool lock.
pub(crate) enum AuthenticateToolStart {
    AlreadyAuthorized,
    Login(Box<oauth::McpOAuthToolLogin>),
}

/// How the login phase of the synthetic authenticate tool concluded, fed to
/// [`McpPool::finish_authenticate_tool`].
pub(crate) enum AuthenticateToolOutcome {
    AlreadyAuthorized,
    Authenticated { authorization_url: String },
}

/// Execute the synthetic `mcp_<server>_authenticate` tool against a shared
/// pool without holding the pool lock during the browser wait: lock to start
/// the flow, release, wait for the loopback callback, then lock again to
/// reconnect. `on_authorization_url` fires as soon as the URL exists — before
/// the wait — so the runtime can show it to the user while the call blocks;
/// the model only sees the URL in the result, after the flow has already
/// finished, so this hook is the user's real path to the sign-in page when
/// the browser did not open.
pub(crate) async fn authenticate_tool_via_pool(
    pool: &Arc<tokio::sync::Mutex<McpPool>>,
    server_name: &str,
    on_authorization_url: impl FnOnce(&str),
) -> Result<serde_json::Value> {
    let start = pool
        .lock()
        .await
        .begin_authenticate_tool(server_name)
        .await?;
    let outcome = match start {
        AuthenticateToolStart::AlreadyAuthorized => AuthenticateToolOutcome::AlreadyAuthorized,
        AuthenticateToolStart::Login(login) => {
            let authorization_url = login.authorization_url().to_string();
            on_authorization_url(&authorization_url);
            login.finish().await?;
            AuthenticateToolOutcome::Authenticated { authorization_url }
        }
    };
    pool.lock()
        .await
        .finish_authenticate_tool(server_name, outcome)
        .await
}

/// Pool of MCP connections for reuse
pub struct McpPool {
    connections: HashMap<String, McpConnection>,
    config: McpConfig,
    network_policy: Option<NetworkPolicyDecider>,
    /// Source paths the config was loaded from. Empty for pools constructed
    /// directly via `new` (tests, ad-hoc snapshots). Workspace-aware pools
    /// track both global and project-level MCP config paths so lazy reload sees
    /// either file appear or change.
    config_sources: Vec<PathBuf>,
    workspace: Option<PathBuf>,
    plugin_registry: Option<Arc<crate::plugins::PluginRegistry>>,
    /// 64-bit content hash of the active config (`hash_mcp_config`). Compared
    /// against the freshly-loaded config after an mtime change to skip
    /// reloading when the file was merely touched.
    config_hash: u64,
    /// Monotonic identity for the exact config/plugin catalog generation that
    /// advertised a callable MCP item. Resolution captures this value and the
    /// call boundary rejects any intervening lazy reload or dynamic mutation.
    catalog_generation: AtomicU64,
    /// Most recently observed mtime for `config_sources`.
    last_mtimes: Vec<Option<std::time::SystemTime>>,
    /// Dynamically added MCP servers (from tool calls at runtime).
    /// These are not persisted to disk and live for the process lifetime.
    pub(crate) dynamic_servers: Arc<RwLock<HashMap<String, McpServerConfig>>>,
    /// Servers whose most recent connect attempt failed auth-required (401 /
    /// OAuth not logged in). Each gets a synthetic `mcp_<server>_authenticate`
    /// tool in the model catalog so the model can self-serve the OAuth login
    /// instead of dead-ending on the error item. BTreeSet keeps catalog
    /// construction deterministic.
    needs_auth_servers: BTreeSet<String>,
    /// Configured OAuth callback overrides, so the synthetic self-serve
    /// login tool honors the same pre-registered redirect URI `/mcp login`
    /// uses (`mcp_oauth_callback_port` / `mcp_oauth_callback_url`).
    oauth_callback_port: Option<u16>,
    oauth_callback_url: Option<String>,
    /// Bumped on every `needs_auth_servers` mutation. The engine reads it
    /// around a tool call so a live 401 that flips the auth surface can
    /// flag `mcp_catalog_changed` on the failed result and the turn loop
    /// replaces the pool's catalog slice before the next model request.
    needs_auth_generation: u64,
}

type McpPendingConnect = (String, McpServerConfig);
type McpConnectError = (String, anyhow::Error);

impl McpPool {
    /// Create a new pool with the given configuration
    pub fn new(config: McpConfig) -> Self {
        let config_hash = hash_mcp_config(&config);
        Self {
            connections: HashMap::new(),
            config,
            network_policy: None,
            oauth_callback_port: None,
            oauth_callback_url: None,
            config_sources: Vec::new(),
            workspace: None,
            plugin_registry: None,
            config_hash,
            catalog_generation: AtomicU64::new(1),
            last_mtimes: Vec::new(),
            dynamic_servers: Arc::new(RwLock::new(HashMap::new())),
            needs_auth_servers: BTreeSet::new(),
            needs_auth_generation: 0,
        }
    }

    /// Create a pool from a configuration file path.
    #[cfg(test)]
    pub fn from_config_path(path: &std::path::Path) -> Result<Self> {
        let config = load_config(path)?;
        let mut pool = Self::new(config);
        pool.config_sources = vec![path.to_path_buf()];
        pool.last_mtimes = vec![mcp_config_mtime(path)];
        Ok(pool)
    }

    /// Create a pool from global MCP config plus workspace-local
    /// `.codewhale/mcp.json`. Project servers override same-name global
    /// servers and default stdio `cwd` to the workspace root.
    #[cfg(test)]
    pub fn from_config_path_with_workspace(
        path: &std::path::Path,
        workspace: &Path,
    ) -> Result<Self> {
        let plugins = Arc::new(crate::plugins::PluginRegistry::empty(workspace));
        Self::from_config_path_with_workspace_and_plugins(path, workspace, plugins)
    }

    pub fn from_config_path_with_workspace_and_plugins(
        path: &std::path::Path,
        workspace: &Path,
        plugins: Arc<crate::plugins::PluginRegistry>,
    ) -> Result<Self> {
        if plugins.workspace() != workspace {
            anyhow::bail!("plugin registry workspace does not match MCP pool workspace");
        }
        let config = load_config_with_workspace_and_plugins(path, workspace, plugins.as_ref())?;
        let workspace = checked_workspace_path(workspace)?;
        let mut pool = Self::new(config);
        pool.config_sources = vec![
            path.to_path_buf(),
            checked_workspace_mcp_config_path(&workspace)?,
        ];
        pool.config_sources
            .extend(crate::config::workspace_trust_config_candidate_paths());
        pool.last_mtimes = pool
            .config_sources
            .iter()
            .map(|source| mcp_config_mtime(source))
            .collect();
        pool.workspace = Some(workspace);
        pool.plugin_registry = Some(plugins);
        Ok(pool)
    }

    /// Construct a source-aware empty pool after the initial config load
    /// failed. Keeping the source paths means a later edit or explicit
    /// `/mcp reload` can recover in-process instead of pinning the session to
    /// an ad-hoc pool that has no files to re-read.
    pub(crate) fn empty_with_workspace_config_sources(
        path: &std::path::Path,
        workspace: &Path,
        plugins: Arc<crate::plugins::PluginRegistry>,
    ) -> Result<Self> {
        validate_mcp_config_path(path)?;
        if plugins.workspace() != workspace {
            anyhow::bail!("plugin registry workspace does not match MCP pool workspace");
        }
        let workspace = checked_workspace_path(workspace)?;
        let mut pool = Self::new(McpConfig::default());
        pool.config_sources = vec![
            path.to_path_buf(),
            checked_workspace_mcp_config_path(&workspace)?,
        ];
        pool.config_sources
            .extend(crate::config::workspace_trust_config_candidate_paths());
        pool.last_mtimes = pool
            .config_sources
            .iter()
            .map(|source| mcp_config_mtime(source))
            .collect();
        pool.workspace = Some(workspace);
        pool.plugin_registry = Some(plugins);
        Ok(pool)
    }

    /// Attach a per-domain network policy (#135). When set, HTTP/SSE
    /// transports are gated through it; STDIO transports are unaffected.
    pub fn with_network_policy(mut self, policy: NetworkPolicyDecider) -> Self {
        self.network_policy = Some(policy);
        self
    }

    /// Configure the OAuth callback overrides (`mcp_oauth_callback_port` /
    /// `mcp_oauth_callback_url`) for installations whose OAuth client has a
    /// pre-registered redirect URI. The synthetic self-serve login tool must
    /// use the same overrides as `/mcp login`, or the provider rejects its
    /// ephemeral loopback redirect.
    pub fn with_oauth_callback(mut self, port: Option<u16>, url: Option<String>) -> Self {
        self.oauth_callback_port = port;
        self.oauth_callback_url = url;
        self
    }

    pub(crate) fn connect_timeouts(&self) -> McpTimeouts {
        self.config.timeouts
    }

    pub(crate) fn cloned_network_policy(&self) -> Option<NetworkPolicyDecider> {
        self.network_policy.clone()
    }

    pub(crate) fn current_catalog_generation(&self) -> u64 {
        self.catalog_generation.load(Ordering::SeqCst)
    }

    fn drop_connection(&mut self, server_name: &str, reason: &str) {
        if self.connections.remove(server_name).is_some() {
            tracing::debug!(
                target: "mcp",
                server = %server_name,
                reason = %reason,
                "dropped MCP connection"
            );
        }
    }

    fn drop_all_connections(&mut self, reason: &str) {
        // Auth state is only known from a live connect attempt; once every
        // connection is dropped (config reload, source switch, shutdown) the
        // next attempt re-derives it.
        self.needs_auth_servers.clear();
        self.needs_auth_generation = self.needs_auth_generation.wrapping_add(1);
        if self.connections.is_empty() {
            return;
        }
        let count = self.connections.len();
        tracing::debug!(
            target: "mcp",
            count,
            reason = %reason,
            "dropping MCP connections"
        );
        self.connections.clear();
    }

    /// If the source config file's mtime has changed since the last check,
    /// re-read it and (only when the content hash also changed) drop all
    /// existing connections so the next `get_or_connect` reattaches under
    /// the new config. No-op when the pool was constructed via [`McpPool::new`]
    /// (no source path), when stat fails, or when the file content is
    /// byte-identical to what we last loaded. Returns `Ok(true)` if any
    /// connections were dropped, `Ok(false)` otherwise.
    ///
    /// This is the lazy half of the auto-reload story for #1267: instead of a
    /// long-lived file watcher, the next tool invocation pays a single `stat`
    /// call (and only re-reads the file when the mtime moved). On networked
    /// or remote filesystems where mtime granularity is poor, the hash
    /// compare keeps us from churning connections on every check.
    fn reload_from_config_sources(&mut self, force: bool) -> Result<bool> {
        if self.config_sources.is_empty() {
            if force {
                anyhow::bail!("MCP pool has no configuration source to reload");
            }
            return Ok(false);
        }
        let current_mtimes: Vec<_> = self
            .config_sources
            .iter()
            .map(|path| mcp_config_mtime(path))
            .collect();
        if !force && current_mtimes == self.last_mtimes {
            return Ok(false);
        }
        // An mtime moved, or the user explicitly requested a reload: re-read
        // the complete global + workspace + plugin-backed config.
        let primary = self
            .config_sources
            .first()
            .context("MCP config source list unexpectedly empty")?;
        let new_config = if let Some(workspace) = self.workspace.as_deref() {
            match self.plugin_registry.as_deref() {
                Some(plugins) => {
                    load_config_with_workspace_and_plugins(primary, workspace, plugins)?
                }
                None => load_config_with_workspace(primary, workspace)?,
            }
        } else {
            load_config(primary)?
        };
        let new_hash = hash_mcp_config(&new_config);
        // Always advance mtimes so a touched-but-unchanged file doesn't
        // make us re-read on every subsequent call.
        self.last_mtimes = current_mtimes;
        if !force && new_hash == self.config_hash {
            return Ok(false);
        }
        // A real content change, or an explicit reload, invalidates every
        // advertised route and live transport. The latter matters when OAuth
        // credentials changed without changing the config bytes.
        self.drop_all_connections(if force {
            "explicit config reload"
        } else {
            "config reload"
        });
        self.config = new_config;
        self.config_hash = new_hash;
        self.catalog_generation.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }

    pub async fn reload_if_config_changed(&mut self) -> Result<bool> {
        self.reload_from_config_sources(false)
    }

    /// Force a source re-read, invalidate all advertised routes, reconnect
    /// enabled servers, and return per-server connection errors. Dynamic
    /// in-memory servers remain registered because this mutates the existing
    /// pool rather than replacing it.
    pub async fn reload_and_connect_all(&mut self) -> Result<Vec<(String, anyhow::Error)>> {
        self.reload_from_config_sources(true)?;
        Ok(self.connect_all().await)
    }

    /// Switch the global config source transactionally, preserving this
    /// shared pool (and its dynamic runtime servers) for parent and sub-agent
    /// holders. A malformed replacement leaves the current config,
    /// connections, and source paths unchanged.
    pub(crate) async fn switch_workspace_config_source_and_connect_all(
        &mut self,
        path: &Path,
        workspace: &Path,
        plugins: Arc<crate::plugins::PluginRegistry>,
    ) -> Result<Vec<(String, anyhow::Error)>> {
        validate_mcp_config_path(path)?;
        if plugins.workspace() != workspace {
            anyhow::bail!("plugin registry workspace does not match MCP pool workspace");
        }
        let workspace = checked_workspace_path(workspace)?;
        let new_config =
            load_config_with_workspace_and_plugins(path, &workspace, plugins.as_ref())?;
        let mut new_sources = vec![
            path.to_path_buf(),
            checked_workspace_mcp_config_path(&workspace)?,
        ];
        new_sources.extend(crate::config::workspace_trust_config_candidate_paths());
        let new_mtimes = new_sources
            .iter()
            .map(|source| mcp_config_mtime(source))
            .collect();

        self.drop_all_connections("config source switch");
        self.config_hash = hash_mcp_config(&new_config);
        self.config = new_config;
        self.config_sources = new_sources;
        self.last_mtimes = new_mtimes;
        self.workspace = Some(workspace);
        self.plugin_registry = Some(plugins);
        self.catalog_generation.fetch_add(1, Ordering::SeqCst);
        Ok(self.connect_all().await)
    }

    /// Get or create a connection to a server
    pub async fn get_or_connect(&mut self, server_name: &str) -> Result<&mut McpConnection> {
        // Lazy auto-reload (#1267 part 2): cheap mtime-then-hash check before
        // each connection lookup. Transient FS errors are logged but not
        // propagated so a brief hiccup can't take down the whole tool dispatch.
        if let Err(e) = self.reload_if_config_changed().await {
            tracing::warn!("MCP config reload check failed: {e:#}");
        }

        let plugin_source = self
            .connections
            .get(server_name)
            .and_then(|connection| connection.config().reviewed_plugin.clone())
            .or_else(|| {
                self.config
                    .servers
                    .get(server_name)
                    .and_then(|config| config.reviewed_plugin.clone())
            });
        if let Some(source) = plugin_source
            && let Err(error) = source.validate_before_use(server_name, "use")
        {
            self.drop_connection(server_name, "plugin authority revoked or changed");
            return Err(error);
        }

        let is_ready = self
            .connections
            .get(server_name)
            .map(|conn| conn.is_ready())
            .unwrap_or(false);
        if is_ready {
            return self
                .connections
                .get_mut(server_name)
                .ok_or_else(|| anyhow::anyhow!("MCP connection disappeared for {server_name}"));
        }

        self.drop_connection(server_name, "reconnect");

        // Check static config first, then dynamic servers
        let server_config = self
            .config
            .servers
            .get(server_name)
            .cloned()
            .or_else(|| self.dynamic_servers.read().get(server_name).cloned())
            .ok_or_else(|| anyhow::anyhow!("Failed to find MCP server: {server_name}"))?;

        if !server_config.is_enabled() {
            anyhow::bail!("Failed to connect MCP server '{server_name}': server is disabled");
        }

        let mut connection = match McpConnection::connect_with_policy(
            server_name.to_string(),
            server_config,
            &self.config.timeouts,
            self.network_policy.as_ref(),
        )
        .await
        {
            Ok(connection) => connection,
            Err(error) => {
                self.note_connect_failure(server_name, &error);
                return Err(error);
            }
        };
        connection.catalog_generation = self.catalog_generation.load(Ordering::SeqCst);

        self.store_ready_connection(server_name.to_string(), connection);
        self.connections
            .get_mut(server_name)
            .ok_or_else(|| anyhow::anyhow!("Failed to store MCP connection for {server_name}"))
    }

    /// Retry exactly one server against the configuration already owned by
    /// this pool.
    ///
    /// Unlike normal lazy tool dispatch, an explicit row retry must not notice
    /// a concurrent config mtime and invalidate healthy siblings. Config edits
    /// remain owned by the explicit reload path; this operation only replaces
    /// the named transport.
    pub async fn retry_connection(&mut self, server_name: &str) -> Result<&mut McpConnection> {
        let plugin_source = self
            .connections
            .get(server_name)
            .and_then(|connection| connection.config().reviewed_plugin.clone())
            .or_else(|| {
                self.config
                    .servers
                    .get(server_name)
                    .and_then(|config| config.reviewed_plugin.clone())
            });
        if let Some(source) = plugin_source
            && let Err(error) = source.validate_before_use(server_name, "use")
        {
            self.drop_connection(server_name, "plugin authority revoked or changed");
            return Err(error);
        }

        self.drop_connection(server_name, "retry");

        let server_config = self
            .config
            .servers
            .get(server_name)
            .cloned()
            .or_else(|| self.dynamic_servers.read().get(server_name).cloned())
            .ok_or_else(|| anyhow::anyhow!("Failed to find MCP server: {server_name}"))?;

        if !server_config.is_enabled() {
            anyhow::bail!("Failed to connect MCP server '{server_name}': server is disabled");
        }

        let connection = match McpConnection::connect_with_policy(
            server_name.to_string(),
            server_config,
            &self.config.timeouts,
            self.network_policy.as_ref(),
        )
        .await
        {
            Ok(connection) => connection,
            Err(error) => {
                self.note_connect_failure(server_name, &error);
                return Err(error);
            }
        };
        self.store_ready_connection(server_name.to_string(), connection);
        self.connections
            .get_mut(server_name)
            .ok_or_else(|| anyhow::anyhow!("Failed to store MCP connection for {server_name}"))
    }

    pub(crate) fn store_ready_connection(&mut self, name: String, mut connection: McpConnection) {
        connection.catalog_generation = self.catalog_generation.load(Ordering::SeqCst);
        // A successful connect settles the auth question for this server.
        if self.needs_auth_servers.remove(&name) {
            self.needs_auth_generation = self.needs_auth_generation.wrapping_add(1);
        }
        self.connections.insert(name, connection);
    }

    /// Record a connect failure's auth classification. When the failure looks
    /// like a missing/expired OAuth login, the next model catalog offers the
    /// synthetic `mcp_<server>_authenticate` tool so the model can self-serve
    /// the login instead of dead-ending on the error item. A non-auth
    /// failure replaces the verdict — the state is "the most recent connect
    /// failed auth-required", not "some connect once did".
    pub(crate) fn note_connect_failure(&mut self, name: &str, error: &anyhow::Error) {
        let changed = if oauth::error_looks_auth_required(error) {
            self.needs_auth_servers.insert(name.to_string())
        } else {
            self.needs_auth_servers.remove(name)
        };
        if changed {
            self.needs_auth_generation = self.needs_auth_generation.wrapping_add(1);
        }
    }

    /// Current needs-auth surface generation. Compare across a tool call to
    /// learn whether the call flipped a server into or out of the
    /// `◆ auth required` state (a live 401, or a login that landed).
    #[must_use]
    pub fn needs_auth_generation(&self) -> u64 {
        self.needs_auth_generation
    }

    /// Whether the server's most recent connect attempt failed auth-required
    /// (the typed `◆ auth required` state). Cleared by a successful connect
    /// and by any full connection drop (reload, source switch, shutdown).
    #[must_use]
    pub fn server_needs_auth(&self, name: &str) -> bool {
        self.needs_auth_servers.contains(name)
    }

    /// The needs-auth server that owns a model tool name (`mcp_<server>_…`),
    /// if any: the server the model is trying to reach with a real tool name
    /// from a catalog built before its login lapsed. Longest configured name
    /// wins so `mcp_a_b_tool` routes to server `a_b` over `a`.
    fn needs_auth_server_for_tool_name(&self, prefixed_name: &str) -> Option<String> {
        let rest = prefixed_name.strip_prefix("mcp_")?;
        self.needs_auth_servers
            .iter()
            .filter(|server| {
                rest.strip_prefix(server.as_str())
                    .is_some_and(|suffix| suffix.starts_with('_'))
            })
            .max_by_key(|server| server.len())
            .cloned()
    }

    /// Peak concurrent spawn+handshake attempts. Uncapped, a config full of
    /// `npx` servers would start one node runtime per server at the same
    /// instant — a memory spike on low-end machines the sequential loop never
    /// produced. Eight keeps wall-clock wins (the connect timeout dominates)
    /// while bounding peak memory.
    const CONNECT_CONCURRENCY: usize = 8;

    /// Decide which enabled configured servers still need a handshake.
    /// Dynamic runtime servers stay registered and connect via
    /// [`Self::get_or_connect`]; `connect_all` has never spawned them.
    pub(crate) fn collect_pending_connects(
        &mut self,
    ) -> (Vec<McpPendingConnect>, Vec<McpConnectError>) {
        let names: Vec<String> = self
            .config
            .servers
            .iter()
            .filter(|(_, server)| server.is_enabled())
            .map(|(name, _)| name.clone())
            .collect();
        let mut pending = Vec::new();
        let mut errors = Vec::new();
        for name in names {
            let Some(server_config) = self.config.servers.get(&name).cloned() else {
                continue;
            };

            let plugin_source = self
                .connections
                .get(&name)
                .and_then(|connection| connection.config().reviewed_plugin.clone())
                .or_else(|| server_config.reviewed_plugin.clone());
            if let Some(source) = plugin_source
                && let Err(error) = source.validate_before_use(&name, "use")
            {
                self.drop_connection(&name, "plugin authority revoked or changed");
                errors.push((name, error));
                continue;
            }

            if self
                .connections
                .get(&name)
                .is_some_and(McpConnection::is_ready)
            {
                continue;
            }
            self.drop_connection(&name, "reconnect");
            pending.push((name, server_config));
        }
        (pending, errors)
    }

    pub(crate) fn push_required_server_errors(&self, errors: &mut Vec<McpConnectError>) {
        for (name, server_cfg) in &self.config.servers {
            // Only stand in for a missing diagnosis. When the connect attempt
            // above already reported why this server failed, appending a
            // second, contentless entry for the same name buries it: callers
            // fold these pairs into a `HashMap<name, message>`, so the later
            // generic string silently replaced the real cause.
            if server_cfg.required
                && server_cfg.is_enabled()
                && !self
                    .connections
                    .get(name)
                    .is_some_and(McpConnection::is_ready)
                && !errors.iter().any(|(failed, _)| failed == name)
            {
                errors.push((
                    name.clone(),
                    anyhow::anyhow!("required MCP server failed to initialize"),
                ));
            }
        }
    }

    /// Handshake the pending servers concurrently without holding the pool
    /// lock. Callers insert results under a short lock so a live turn can
    /// snapshot ready tools while optional servers are still connecting.
    pub(crate) async fn connect_pending_concurrently(
        pending: Vec<McpPendingConnect>,
        timeouts: McpTimeouts,
        network_policy: Option<NetworkPolicyDecider>,
        catalog_generation: u64,
    ) -> Vec<(String, Result<McpConnection, anyhow::Error>)> {
        if pending.is_empty() {
            return Vec::new();
        }
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(Self::CONNECT_CONCURRENCY));
        let mut joins: tokio::task::JoinSet<(String, Result<McpConnection, anyhow::Error>)> =
            tokio::task::JoinSet::new();
        for (name, config) in pending {
            let permit = semaphore.clone();
            let network_policy = network_policy.clone();
            joins.spawn(async move {
                let _permit = permit.acquire_owned().await;
                let connection = McpConnection::connect_with_policy(
                    name.clone(),
                    config,
                    &timeouts,
                    network_policy.as_ref(),
                )
                .await
                .map(|mut connection| {
                    connection.catalog_generation = catalog_generation;
                    connection
                });
                (name, connection)
            });
        }

        let mut results = Vec::new();
        while let Some(joined) = joins.join_next().await {
            match joined {
                Ok(result) => results.push(result),
                // A panicked connect task loses its server name in the
                // JoinError; attribute generically. The sequential loop
                // would have propagated the panic and taken the whole
                // pool down with it, so this is strictly better.
                Err(join_error) => {
                    results.push(("connection task".to_string(), Err(join_error.into())));
                }
            }
        }
        results
    }

    /// Connect to all enabled servers, returning errors for failed connections.
    ///
    /// Servers connect **concurrently** (bounded by [`Self::CONNECT_CONCURRENCY`]).
    /// This used to be a sequential loop over `get_or_connect`, so every
    /// server paid the slowest server's spawn+handshake from its own budget:
    /// with the default 10s connect timeout, N servers meant a worst case of
    /// N×10s before the pool was usable. Each connection still gets its own
    /// configured connect timeout; one wedged server can no longer serialize
    /// the rest.
    ///
    /// Semantics preserved from the sequential loop: only configured servers
    /// are connected (dynamic runtime entries stay registered), the config is
    /// reloaded before the name snapshot (so a server added mid-session
    /// connects on this call, not the next), plugin-authority revocation
    /// drops the connection instead of silently reconnecting, and the
    /// required-server sweep reports at most one error per name. Config edits
    /// that land while the batch is in flight are reconciled by one retry
    /// pass: a content change drops every connection the previous pass
    /// inserted.
    pub async fn connect_all(&mut self) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        // Reload before taking the configured-name snapshot. Previously the
        // first call after adding a server captured the old names, then only
        // noticed the config change inside `get_or_connect`, delaying the new
        // server until a second turn.
        if let Err(err) = self.reload_if_config_changed().await {
            errors.push(("configuration".to_string(), err));
            return errors;
        }

        // Drop needs-auth markers for servers that are no longer connectable
        // (removed from config or disabled); the loop below re-marks any
        // server whose fresh connect attempt still fails auth-required.
        {
            let dynamic = self.dynamic_servers.read();
            let before = self.needs_auth_servers.len();
            self.needs_auth_servers.retain(|name| {
                self.config
                    .servers
                    .get(name)
                    .is_some_and(|server| server.is_enabled())
                    || dynamic.get(name).is_some_and(|server| server.is_enabled())
            });
            if self.needs_auth_servers.len() != before {
                self.needs_auth_generation = self.needs_auth_generation.wrapping_add(1);
            }
        }

        for _pass in 0..2 {
            let (pending, auth_errors) = self.collect_pending_connects();
            errors.extend(auth_errors);
            if pending.is_empty() {
                break;
            }

            let results = Self::connect_pending_concurrently(
                pending,
                self.config.timeouts,
                self.network_policy.clone(),
                self.catalog_generation.load(Ordering::SeqCst),
            )
            .await;
            for (name, result) in results {
                match result {
                    Ok(connection) => self.store_ready_connection(name, connection),
                    Err(error) => {
                        self.note_connect_failure(&name, &error);
                        errors.push((name, error));
                    }
                }
            }

            // Reconcile a config edit that landed mid-batch: a content
            // change dropped every connection this pass inserted, so run one
            // more pass against the new config and drop the stale pass's
            // errors with it.
            match self.reload_if_config_changed().await {
                Ok(true) => {
                    errors.clear();
                    continue;
                }
                Ok(false) => break,
                Err(error) => {
                    errors.push(("configuration".to_string(), error));
                    break;
                }
            }
        }

        self.push_required_server_errors(&mut errors);
        errors
    }

    /// The single definition of an MCP tool's model-facing name.
    ///
    /// [`Self::all_tools`] (which builds the model catalog) and
    /// [`Self::resolved_tool_servers`] (which tells tool inspection which
    /// server owns a name) both call this, so a human-facing server
    /// attribution can never drift from the name the model actually received.
    #[must_use]
    pub fn mcp_model_tool_name(server: &str, tool: &str) -> String {
        format!("mcp_{server}_{tool}")
    }

    /// Map an exact `mcp_<server>_authenticate` model tool name to its server
    /// when the synthetic self-serve OAuth login tool should answer for it:
    /// the server's last connect failed auth-required, or the name matches a
    /// configured OAuth-capable server whose catalog entry is stale (a login
    /// completed since the catalog was built). Never fires when a ready
    /// connection advertises a real tool under the same model name — the
    /// server's own `authenticate` tool always wins.
    pub(crate) fn authenticate_tool_target(&self, prefixed_name: &str) -> Option<String> {
        let target = self
            .needs_auth_servers
            .iter()
            .find(|server| {
                Self::mcp_model_tool_name(server, AUTHENTICATE_TOOL_NAME) == prefixed_name
            })
            .cloned()
            .or_else(|| {
                let dynamic = self.dynamic_servers.read();
                self.config
                    .servers
                    .iter()
                    .chain(dynamic.iter())
                    .find(|(name, _)| {
                        Self::mcp_model_tool_name(name, AUTHENTICATE_TOOL_NAME) == prefixed_name
                    })
                    .map(|(name, _)| name.clone())
            })?;
        let capable = {
            let dynamic = self.dynamic_servers.read();
            self.config
                .servers
                .get(&target)
                .or_else(|| dynamic.get(&target))
                .is_some_and(oauth::server_supports_oauth_login)
        };
        if !capable {
            return None;
        }
        if self.parse_prefixed_name(prefixed_name).is_ok() {
            return None;
        }
        Some(target)
    }

    /// The configured server by name, static config first, then the
    /// session's dynamically added servers.
    fn server_config(&self, server_name: &str) -> Option<McpServerConfig> {
        self.config
            .servers
            .get(server_name)
            .cloned()
            .or_else(|| self.dynamic_servers.read().get(server_name).cloned())
    }

    /// Phase one of the synthetic `mcp_<server>_authenticate` tool: decide
    /// whether a browser login is needed and, if so, start it. Only touches
    /// config, the connection map, and the token store — it returns as soon
    /// as the authorization URL exists, so a caller holding the pool lock can
    /// release it before the (up to five minute) browser wait in
    /// [`oauth::McpOAuthToolLogin::finish`]. Holding the lock across that wait
    /// would freeze every other MCP call, the `/mcp` manager, and the
    /// Extensions view for the whole sign-in.
    pub(crate) async fn begin_authenticate_tool(
        &self,
        server_name: &str,
    ) -> Result<AuthenticateToolStart> {
        let server = self
            .server_config(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server '{server_name}' is no longer configured"))?;
        if !server.is_enabled() {
            anyhow::bail!("MCP server '{server_name}' is disabled");
        }

        // Already-authorized branch: a login that completed since the catalog
        // was built (e.g. `codewhale mcp login` in another window) must not
        // restart the browser flow — adopt the stored tokens by reconnecting.
        let ready = self
            .connections
            .get(server_name)
            .is_some_and(McpConnection::is_ready);
        if ready || oauth::has_usable_stored_tokens(server_name, &server) {
            return Ok(AuthenticateToolStart::AlreadyAuthorized);
        }
        let login = oauth::begin_oauth_login_for_server_tool(
            server_name,
            &server,
            self.oauth_callback_port,
            self.oauth_callback_url.as_deref(),
        )
        .await?;
        Ok(AuthenticateToolStart::Login(Box::new(login)))
    }

    /// Phase two of the synthetic authenticate tool: reconnect the server so
    /// its real tools resolve in this session, and describe the outcome for
    /// the model. `get_or_connect` drops any non-ready connection itself, so
    /// both the fresh-login and already-authorized outcomes fall through to
    /// it. Errors are never swallowed — a reconnect that still fails
    /// auth-required re-marks the server (via `get_or_connect`) and surfaces
    /// truthfully for the model to relay.
    pub(crate) async fn finish_authenticate_tool(
        &mut self,
        server_name: &str,
        outcome: AuthenticateToolOutcome,
    ) -> Result<serde_json::Value> {
        match self.get_or_connect(server_name).await {
            Ok(conn) => {
                let tools: Vec<String> = conn
                    .tools()
                    .iter()
                    .filter(|tool| conn.config().is_tool_enabled(&tool.name))
                    .map(|tool| Self::mcp_model_tool_name(server_name, &tool.name))
                    .collect();
                let (status, detail) = match &outcome {
                    AuthenticateToolOutcome::Authenticated { .. } => (
                        "authenticated",
                        "authenticated successfully and is now connected",
                    ),
                    AuthenticateToolOutcome::AlreadyAuthorized => (
                        "already_authorized",
                        "already had valid OAuth credentials and is now connected",
                    ),
                };
                let mut result = serde_json::json!({
                    "status": status,
                    "server": server_name,
                    "tools": tools,
                    "message": format!(
                        "MCP server '{server_name}' {detail}. Its real MCP tools (listed in 'tools') replaced the synthetic authenticate tool and are callable from the next model request in this session."
                    ),
                });
                if let AuthenticateToolOutcome::Authenticated { authorization_url } = outcome {
                    result["authorization_url"] = serde_json::Value::String(authorization_url);
                }
                Ok(result)
            }
            Err(error) => {
                // A stored token the server just rejected must not feed the
                // loop again: `begin_authenticate_tool` short-circuits to
                // AlreadyAuthorized whenever usable-looking tokens exist, so
                // keeping them means the model re-calls the synthetic tool
                // forever while every reconnect fails auth-required. Drop
                // the durable copy so the next begin starts a real login.
                if matches!(outcome, AuthenticateToolOutcome::AlreadyAuthorized)
                    && oauth::error_looks_auth_required(&error)
                    && let Some(server) = self.server_config(server_name)
                {
                    match oauth::delete_oauth_tokens_for_server(server_name, &server) {
                        Ok(true) => tracing::info!(
                            target: "mcp",
                            server = %server_name,
                            "rejected stored OAuth token removed after failed AlreadyAuthorized reconnect"
                        ),
                        Ok(false) => {}
                        Err(delete_err) => tracing::warn!(
                            target: "mcp",
                            server = %server_name,
                            error = %delete_err,
                            "could not remove rejected stored OAuth token"
                        ),
                    }
                }
                Err(error).with_context(|| {
                    format!(
                        "MCP server '{server_name}' completed OAuth login but the reconnect failed"
                    )
                })
            }
        }
    }

    /// Execute the synthetic `mcp_<server>_authenticate` tool in place,
    /// holding `&mut self` (and therefore any enclosing pool lock) for the
    /// whole flow. Engine tool execution uses
    /// [`authenticate_tool_via_pool`] instead, which releases the shared
    /// pool between the two phases.
    async fn run_authenticate_tool(&mut self, server_name: &str) -> Result<serde_json::Value> {
        let outcome = match self.begin_authenticate_tool(server_name).await? {
            AuthenticateToolStart::AlreadyAuthorized => AuthenticateToolOutcome::AlreadyAuthorized,
            AuthenticateToolStart::Login(login) => {
                let authorization_url = login.authorization_url().to_string();
                login.finish().await?;
                AuthenticateToolOutcome::Authenticated { authorization_url }
            }
        };
        self.finish_authenticate_tool(server_name, outcome).await
    }

    /// The model-facing recovery for a server in the `◆ auth required`
    /// state: the one call that recovers it. Names the synthetic
    /// `mcp_<server>_authenticate` tool when the server is OAuth-servable, and
    /// otherwise the credential source (plugin environment header, manual
    /// bearer) the server is allowed to authenticate with.
    fn auth_required_hint(&self, server_name: &str) -> String {
        let tool_name = Self::mcp_model_tool_name(server_name, AUTHENTICATE_TOOL_NAME);
        if self.authenticate_tool_target(&tool_name).is_some() {
            return format!(
                "MCP server '{server_name}' requires OAuth login (◆ auth required); call the `{tool_name}` tool to authenticate, or run `/mcp login {server_name}`"
            );
        }
        let recovery = match self.server_config(server_name) {
            Some(server) => oauth::auth_required_recovery_hint(server_name, &server),
            None => oauth::auth_required_login_hint(server_name),
        };
        format!("MCP server '{server_name}' requires authentication (◆ auth required); {recovery}")
    }

    /// Route an auth-required failure from a live tool call into the same
    /// typed state a failed connect produces: drop the connection (its
    /// credential is no longer accepted), mark the server needs-auth so the
    /// next catalog offers the synthetic login tool, and name the recovery on
    /// the error. Any other error passes through untouched.
    fn note_live_call_failure(&mut self, server_name: &str, error: anyhow::Error) -> anyhow::Error {
        if !oauth::error_looks_auth_required(&error) {
            return error;
        }
        self.drop_connection(server_name, "auth required on live call");
        self.note_connect_failure(server_name, &error);
        error.context(self.auth_required_hint(server_name))
    }

    /// Fold `(server, tool)` pairs into `model name -> owning server`.
    ///
    /// Mirrors [`Self::all_tools`]' ambiguity rule: when two servers produce
    /// the same model name, the name is dropped entirely rather than
    /// attributed to an arbitrary winner. Callers then report it as unknown.
    #[must_use]
    pub fn resolve_tool_server_map<'a>(
        pairs: impl Iterator<Item = (&'a str, &'a str)>,
    ) -> std::collections::BTreeMap<String, String> {
        let mut resolved: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        for (server, tool) in pairs {
            match resolved.entry(Self::mcp_model_tool_name(server, tool)) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Some(server.to_string()));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.insert(None);
                }
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, server)| server.map(|server| (name, server)))
            .collect()
    }

    /// Model tool name -> owning server name, for the tools this pool actually
    /// resolved. Names the pool did not resolve are simply absent, so callers
    /// report them as unknown instead of parsing `mcp_{server}_{tool}` (a
    /// server name may itself contain `_`, so that split is a guess).
    ///
    /// Read-only projection used by tool inspection; it never connects or
    /// executes.
    #[must_use]
    pub fn resolved_tool_servers(&self) -> std::collections::BTreeMap<String, String> {
        Self::resolve_tool_server_map(self.connections.iter().flat_map(|(server, conn)| {
            let authorized = conn.catalog_authorized();
            conn.tools().iter().filter_map(move |tool| {
                (authorized && conn.config().is_tool_enabled(&tool.name))
                    .then_some((server.as_str(), tool.name.as_str()))
            })
        }))
    }

    /// Get all discovered tools with server-prefixed names
    pub fn all_tools(&self) -> Vec<(String, &McpTool)> {
        let mut by_name: std::collections::BTreeMap<String, Option<&McpTool>> =
            std::collections::BTreeMap::new();
        for (server, conn) in &self.connections {
            if !conn.catalog_authorized() {
                continue;
            }
            for tool in conn.tools() {
                if !conn.config().is_tool_enabled(&tool.name) {
                    continue;
                }
                let name = Self::mcp_model_tool_name(server, &tool.name);
                match by_name.entry(name.clone()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(Some(tool));
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        tracing::warn!(
                            target: "mcp",
                            model_tool = %name,
                            "hiding ambiguous MCP model tool name"
                        );
                        entry.insert(None);
                    }
                }
            }
        }
        by_name
            .into_iter()
            .filter_map(|(name, tool)| tool.map(|tool| (name, tool)))
            .collect()
    }

    /// Get all discovered resources with server-prefixed names
    pub fn all_resources(&self) -> Vec<(String, &McpResource)> {
        let mut resources = Vec::new();
        for (server, conn) in &self.connections {
            if !conn.catalog_authorized() {
                continue;
            }
            for resource in conn.resources() {
                // Format: mcp_{server}_{resource_name}
                // Note: resource names might contain spaces, we should probably slugify them
                let safe_name = resource.name.replace(' ', "_").to_lowercase();
                resources.push((format!("mcp_{server}_{safe_name}"), resource));
            }
        }
        resources
    }

    /// Get all discovered resource templates with server-prefixed names
    #[allow(dead_code)] // Public API for MCP resource discovery
    pub fn all_resource_templates(&self) -> Vec<(String, &McpResourceTemplate)> {
        let mut templates = Vec::new();
        for (server, conn) in &self.connections {
            if !conn.catalog_authorized() {
                continue;
            }
            for template in conn.resource_templates() {
                let safe_name = template.name.replace(' ', "_").to_lowercase();
                templates.push((format!("mcp_{server}_{safe_name}"), template));
            }
        }
        templates
    }

    async fn list_resources(&mut self, server: Option<String>) -> Result<Vec<serde_json::Value>> {
        if let Some(server_name) = server {
            let conn = self.get_or_connect(&server_name).await?;
            let resources = conn
                .resources()
                .iter()
                .map(|resource| {
                    serde_json::json!({
                        "server": server_name.clone(),
                        "uri": resource.uri,
                        "name": resource.name,
                        "description": resource.description,
                        "mime_type": resource.mime_type,
                    })
                })
                .collect();
            return Ok(resources);
        }

        let mut items = Vec::new();
        let errors = self.connect_all().await;
        for (server, err) in errors {
            tracing::warn!("Failed to connect MCP server '{server}' for resources: {err:#}");
            if oauth::error_looks_auth_required(&err) {
                items.push(self.mcp_auth_required_error_item(&server));
            }
        }
        for (server, conn) in &self.connections {
            if !conn.catalog_authorized() {
                continue;
            }
            for resource in conn.resources() {
                items.push(serde_json::json!({
                    "server": server,
                    "uri": resource.uri,
                    "name": resource.name,
                    "description": resource.description,
                    "mime_type": resource.mime_type,
                }));
            }
        }
        Ok(items)
    }

    async fn list_resource_templates(
        &mut self,
        server: Option<String>,
    ) -> Result<Vec<serde_json::Value>> {
        if let Some(server_name) = server {
            let conn = self.get_or_connect(&server_name).await?;
            let templates = conn
                .resource_templates()
                .iter()
                .map(|template| {
                    serde_json::json!({
                        "server": server_name.clone(),
                        "uri_template": template.uri_template,
                        "name": template.name,
                        "description": template.description,
                        "mime_type": template.mime_type,
                    })
                })
                .collect();
            return Ok(templates);
        }

        let mut items = Vec::new();
        let errors = self.connect_all().await;
        for (server, err) in errors {
            tracing::warn!(
                "Failed to connect MCP server '{server}' for resource templates: {err:#}"
            );
            if oauth::error_looks_auth_required(&err) {
                items.push(self.mcp_auth_required_error_item(&server));
            }
        }
        for (server, conn) in &self.connections {
            if !conn.catalog_authorized() {
                continue;
            }
            for template in conn.resource_templates() {
                items.push(serde_json::json!({
                    "server": server,
                    "uri_template": template.uri_template,
                    "name": template.name,
                    "description": template.description,
                    "mime_type": template.mime_type,
                }));
            }
        }
        Ok(items)
    }

    /// Listing-time error item for a needs-auth server. Carries the same
    /// recovery the tool-call path names (the synthetic authenticate tool
    /// when OAuth-servable) so a resource or prompt listing never dead-ends
    /// on a login the model could have self-served.
    fn mcp_auth_required_error_item(&self, server: &str) -> serde_json::Value {
        let mut item = serde_json::json!({
            "error": "authentication_required",
            "server": server,
            "message": self.auth_required_hint(server),
        });
        let tool_name = Self::mcp_model_tool_name(server, AUTHENTICATE_TOOL_NAME);
        if self.authenticate_tool_target(&tool_name).is_some() {
            item["authenticate_tool"] = serde_json::Value::String(tool_name);
        }
        item
    }

    /// Get all discovered prompts with server-prefixed names
    pub fn all_prompts(&self) -> Vec<(String, &McpPrompt)> {
        let mut prompts = Vec::new();
        for (server, conn) in &self.connections {
            if !conn.catalog_authorized() {
                continue;
            }
            for prompt in conn.prompts() {
                // Format: mcp_{server}_{prompt}
                prompts.push((format!("mcp_{}_{}", server, prompt.name), prompt));
            }
        }
        prompts
    }

    /// Read a resource from a specific server
    pub async fn read_resource(
        &mut self,
        server_name: &str,
        uri: &str,
    ) -> Result<serde_json::Value> {
        let global_timeouts = self.config.timeouts;
        let conn = self.get_or_connect(server_name).await?;
        let advertised_literal = conn.resources().iter().any(|resource| resource.uri == uri);
        let advertised_template = conn
            .resource_templates()
            .iter()
            .any(|template| resource_uri_matches_template(uri, &template.uri_template));
        if !advertised_literal && !advertised_template {
            anyhow::bail!("MCP resource URI '{uri}' was not advertised by server '{server_name}'");
        }
        let timeout = conn.config().effective_read_timeout(&global_timeouts);
        conn.read_resource(uri, timeout).await
    }

    /// Get a prompt from a specific server
    pub async fn get_prompt(
        &mut self,
        server_name: &str,
        prompt_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let global_timeouts = self.config.timeouts;
        let conn = self.get_or_connect(server_name).await?;
        if !conn
            .prompts()
            .iter()
            .any(|prompt| prompt.name == prompt_name)
        {
            anyhow::bail!(
                "MCP prompt '{prompt_name}' was not advertised by server '{server_name}'"
            );
        }
        let timeout = conn.config().effective_execute_timeout(&global_timeouts);
        conn.get_prompt(prompt_name, arguments, timeout).await
    }

    /// Parse a prefixed name into (server_name, tool_name)
    pub(crate) fn parse_prefixed_name(&self, prefixed_name: &str) -> Result<(String, String)> {
        let Some(rest) = prefixed_name.strip_prefix("mcp_") else {
            anyhow::bail!("Invalid MCP tool name: {prefixed_name}");
        };

        let mut matched: Option<(String, String)> = None;
        for (server, connection) in &self.connections {
            if !connection.catalog_authorized() {
                continue;
            }
            for tool in connection.tools() {
                if !connection.config().is_tool_enabled(&tool.name)
                    || format!("{server}_{}", tool.name) != rest
                {
                    continue;
                }
                if matched.is_some() {
                    anyhow::bail!(
                        "Ambiguous MCP tool name '{prefixed_name}' matches more than one server/tool authority"
                    );
                }
                matched = Some((server.clone(), tool.name.clone()));
            }
        }
        if let Some(matched) = matched {
            return Ok(matched);
        }

        Err(anyhow::anyhow!("Unknown MCP tool name: {prefixed_name}"))
    }

    /// Resolve an MCP tool through an exact advertised catalog. A configured
    /// but lazy server may be connected and asked for `tools/list`; the
    /// requested suffix is never treated as authority on its own.
    async fn resolve_advertised_tool(&mut self, prefixed_name: &str) -> Result<McpToolRoute> {
        if let Ok((server_name, tool_name)) = self.parse_prefixed_name(prefixed_name) {
            return self.capture_tool_route(server_name, tool_name);
        }
        let Some(rest) = prefixed_name.strip_prefix("mcp_") else {
            anyhow::bail!("Invalid MCP tool name: {prefixed_name}");
        };
        let mut candidates = {
            let dynamic = self.dynamic_servers.read();
            self.config
                .servers
                .iter()
                .filter_map(|(name, config)| {
                    (config.is_enabled()
                        && rest
                            .strip_prefix(name)
                            .is_some_and(|suffix| suffix.starts_with('_')))
                    .then_some(name.clone())
                })
                .chain(dynamic.iter().filter_map(|(name, config)| {
                    (config.is_enabled()
                        && rest
                            .strip_prefix(name)
                            .is_some_and(|suffix| suffix.starts_with('_')))
                    .then_some(name.clone())
                }))
                .collect::<Vec<_>>()
        };
        candidates.sort();
        candidates.dedup();
        for server in candidates {
            // Connecting and catalog discovery are the only lazy side effects.
            // A guessed method is never sent to the transport.
            let _ = self.get_or_connect(&server).await?;
        }
        let (server_name, tool_name) = self.parse_prefixed_name(prefixed_name)?;
        self.capture_tool_route(server_name, tool_name)
    }

    fn capture_tool_route(&self, server_name: String, tool_name: String) -> Result<McpToolRoute> {
        let connection = self
            .connections
            .get(&server_name)
            .context("advertised MCP connection disappeared during resolution")?;
        let plugin_authority = connection
            .config()
            .reviewed_plugin
            .as_ref()
            .map(|source| source.authority.clone());
        Ok(McpToolRoute {
            server_name,
            tool_name,
            catalog_generation: connection.catalog_generation,
            plugin_authority,
        })
    }

    /// Every model-facing tool name the runtime MCP pool can own right now:
    /// the current `to_api_tools` output plus the synthetic
    /// `mcp_<server>_authenticate` name for every enabled OAuth-servable
    /// server, whether or not it is currently needs-auth. The turn loop
    /// uses this universe to REPLACE the pool's slice of the tool catalog
    /// instead of additively merging it — the synthetic entry must leave
    /// after a login, and dead real tools must leave after a live 401.
    pub fn model_tool_names(&self) -> std::collections::HashSet<String> {
        let mut names: std::collections::HashSet<String> = self
            .to_api_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        let dynamic = self.dynamic_servers.read();
        for (server, config) in self.config.servers.iter().chain(dynamic.iter()) {
            if config.is_enabled() && oauth::server_supports_oauth_login(config) {
                names.insert(Self::mcp_model_tool_name(server, AUTHENTICATE_TOOL_NAME));
            }
        }
        names
    }

    /// Convert discovered tools to API Tool format
    pub fn to_api_tools(&self) -> Vec<crate::models::Tool> {
        let mut api_tools = Vec::new();
        // Add regular tools
        for (name, tool) in self.all_tools() {
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name,
                description: tool.description.clone().unwrap_or_default(),
                input_schema: tool.input_schema.clone(),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
        }

        // A server whose last connect failed auth-required has no real tools
        // to advertise. In their place offer exactly one synthetic
        // `mcp_<server>_authenticate` tool so the model can self-serve the
        // OAuth login instead of dead-ending on the listing's error item.
        // Never shadow a real tool that owns the same model name.
        {
            let dynamic = self.dynamic_servers.read();
            for server in &self.needs_auth_servers {
                let Some(config) = self
                    .config
                    .servers
                    .get(server)
                    .or_else(|| dynamic.get(server))
                else {
                    continue;
                };
                if !config.is_enabled() || !oauth::server_supports_oauth_login(config) {
                    continue;
                }
                let name = Self::mcp_model_tool_name(server, AUTHENTICATE_TOOL_NAME);
                if api_tools.iter().any(|tool| tool.name == name) {
                    continue;
                }
                api_tools.push(crate::models::Tool {
                    tool_type: None,
                    name,
                    description: oauth::authenticate_tool_description(server),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {}
                    }),
                    allowed_callers: Some(vec!["direct".to_string()]),
                    defer_loading: Some(false),
                    input_examples: None,
                    strict: None,
                    cache_control: None,
                });
            }
        }

        // Only advertise each resource-listing meta-tool when the servers actually
        // expose the corresponding kind. Previously both were injected whenever any
        // MCP server was configured, so tools-only servers left the model with
        // meta-tools that can only ever return empty results — a wasted tool slot
        // and prompt tokens. Gate each on its own non-empty collection, mirroring
        // the `mcp_read_resource` guard below (`!resources.is_empty()`).
        if !self.all_resources().is_empty() {
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "list_mcp_resources".to_string(),
                description: "List available MCP resources across servers (optionally filtered by server).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "Optional MCP server name to filter by" }
                    }
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
        }
        if !self.all_resource_templates().is_empty() {
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "list_mcp_resource_templates".to_string(),
                description: "List available MCP resource templates across servers (optionally filtered by server).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "Optional MCP server name to filter by" }
                    }
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
        }

        // Add resource reading tools if resources exist
        let resources = self.all_resources();
        if !resources.is_empty() {
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "mcp_read_resource".to_string(),
                description: "Read a resource from an MCP server using its URI".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "The name of the MCP server" },
                        "uri": { "type": "string", "description": "The URI of the resource to read" }
                    },
                    "required": ["server", "uri"]
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "read_mcp_resource".to_string(),
                description: "Alias for mcp_read_resource.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "The name of the MCP server" },
                        "uri": { "type": "string", "description": "The URI of the resource to read" }
                    },
                    "required": ["server", "uri"]
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
        }

        // Add prompt getting tools if prompts exist
        let prompts = self.all_prompts();
        if !prompts.is_empty() {
            api_tools.push(crate::models::Tool {
                tool_type: None,
                name: "mcp_get_prompt".to_string(),
                description: "Get a prompt from an MCP server".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string", "description": "The name of the MCP server" },
                        "name": { "type": "string", "description": "The name of the prompt" },
                        "arguments": {
                            "type": "object",
                            "description": "Optional arguments for the prompt",
                            "additionalProperties": { "type": "string" }
                        }
                    },
                    "required": ["server", "name"]
                }),
                allowed_callers: Some(vec!["direct".to_string()]),
                defer_loading: Some(false),
                input_examples: None,
                strict: None,
                cache_control: None,
            });
        }

        // Sort by name for prefix-cache stability — the tool block sent to
        // the model needs to be deterministic across runs (#1319).
        api_tools.sort_by(|a, b| a.name.cmp(&b.name));
        api_tools
    }

    /// Call a tool by its prefixed name (mcp_{server}_{tool})
    pub async fn call_tool(
        &mut self,
        prefixed_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if prefixed_name == "list_mcp_resources" {
            let server = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let resources = self.list_resources(server).await?;
            return Ok(serde_json::json!({ "resources": resources }));
        }

        if prefixed_name == "list_mcp_resource_templates" {
            let server = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let templates = self.list_resource_templates(server).await?;
            return Ok(serde_json::json!({ "templates": templates }));
        }

        if prefixed_name == "mcp_read_resource" {
            let server_name = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .context("Missing 'server' argument")?;
            let uri = arguments
                .get("uri")
                .and_then(|v| v.as_str())
                .context("Missing 'uri' argument")?;
            return self.read_resource(server_name, uri).await;
        }

        if prefixed_name == "read_mcp_resource" {
            let server_name = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .context("Missing 'server' argument")?;
            let uri = arguments
                .get("uri")
                .and_then(|v| v.as_str())
                .context("Missing 'uri' argument")?;
            return self.read_resource(server_name, uri).await;
        }

        if prefixed_name == "mcp_get_prompt" {
            let server_name = arguments
                .get("server")
                .and_then(|v| v.as_str())
                .context("Missing 'server' argument")?;
            let name = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .context("Missing 'name' argument")?;
            let args = arguments
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            return self.get_prompt(server_name, name, args).await;
        }

        // Synthetic self-serve OAuth login: `mcp_<server>_authenticate` runs
        // the same flow `/mcp login` uses and reconnects the server on
        // success, so the real tools resolve in this session.
        if let Some(server_name) = self.authenticate_tool_target(prefixed_name) {
            return self.run_authenticate_tool(&server_name).await;
        }

        let route = match self.resolve_advertised_tool(prefixed_name).await {
            Ok(route) => route,
            Err(error) => {
                // A real tool name reached a server whose login has lapsed
                // (stale catalog, or a mid-session 401). Name the one call
                // that recovers it instead of leaving a dead error.
                if let Some(server) = self.needs_auth_server_for_tool_name(prefixed_name) {
                    return Err(error.context(self.auth_required_hint(&server)));
                }
                return Err(error);
            }
        };
        let server_name = route.server_name.clone();
        let tool_name = route.tool_name.clone();
        // Copy the global timeouts to avoid borrow conflict
        let global_timeouts = self.config.timeouts;
        let conn = self.get_or_connect(&server_name).await?;
        if conn.catalog_generation != route.catalog_generation {
            anyhow::bail!("MCP catalog changed after tool resolution; retry the call");
        }
        if conn
            .config()
            .reviewed_plugin
            .as_ref()
            .map(|source| &source.authority)
            != route.plugin_authority.as_ref()
            || !conn.config().is_tool_enabled(&tool_name)
            || !conn.tools().iter().any(|tool| tool.name == tool_name)
        {
            anyhow::bail!("MCP tool '{tool_name}' is disabled for server '{server_name}'");
        }
        let timeout = conn.config().effective_execute_timeout(&global_timeouts);
        let result = match conn.call_tool(&tool_name, arguments.clone(), timeout).await {
            Ok(result) => Ok(result),
            // A rejected credential is not a stale session: reconnecting
            // replays the same rejection, so it takes the auth-required
            // path below instead of the transparent retry.
            Err(err)
                if is_mcp_stale_session_error(&err) && !oauth::error_looks_auth_required(&err) =>
            {
                tracing::debug!(
                    target: "mcp",
                    server = server_name,
                    tool = tool_name,
                    error = %err,
                    "retrying MCP tool call after stale session"
                );
                self.drop_connection(&server_name, "stale session retry");
                // No `?` here: a reconnect that fails auth-required must
                // still reach the classification below.
                match self.get_or_connect(&server_name).await {
                    Ok(conn) => {
                        if conn.catalog_generation != route.catalog_generation
                            || conn
                                .config()
                                .reviewed_plugin
                                .as_ref()
                                .map(|source| &source.authority)
                                != route.plugin_authority.as_ref()
                            || !conn.config().is_tool_enabled(&tool_name)
                            || !conn.tools().iter().any(|tool| tool.name == tool_name)
                        {
                            Err(anyhow::anyhow!(
                                "MCP tool '{tool_name}' is disabled for server '{server_name}'"
                            ))
                        } else {
                            let timeout = conn.config().effective_execute_timeout(&global_timeouts);
                            conn.call_tool(&tool_name, arguments, timeout).await
                        }
                    }
                    Err(err) => Err(err),
                }
            }
            Err(err) => Err(err),
        };
        // A credential the server stopped accepting mid-session (revoked or
        // rotated elsewhere, refresh rejected) is the same `◆ auth required`
        // class as a failed connect: land it in the same typed state so the
        // next catalog offers the login tool instead of a dead error.
        result.map_err(|err| self.note_live_call_failure(&server_name, err))
    }

    /// Get list of configured server names (static + dynamic)
    #[allow(dead_code)] // Public API for MCP consumers
    pub fn server_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.config.servers.keys().cloned().collect();
        let dynamic = self.dynamic_servers.read();
        for name in dynamic.keys() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    /// Add a runtime server configuration (in-memory only, not persisted).
    ///
    /// This is used for dynamically started MCP servers from chat context.
    /// Stored in `dynamic_servers` so it doesn't interfere with file-based config reload.
    ///
    /// Returns `Err` if a server with the same name already exists as a static config
    /// or a dynamic config. The caller should surface the error to the LLM/user.
    pub fn add_runtime_server_config(
        &self,
        name: String,
        config: McpServerConfig,
    ) -> Result<(), String> {
        if self.config.servers.contains_key(&name) {
            return Err(format!(
                "MCP server '{}' already exists in the config file. \
                 Remove it from the config first, or choose a different name.",
                name
            ));
        }
        let mut dynamic = self.dynamic_servers.write();
        if dynamic.contains_key(&name) {
            return Err(format!(
                "MCP server '{}' was already started earlier in this session. \
                 Choose a different name.",
                name
            ));
        }
        dynamic.insert(name, config);
        self.catalog_generation.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Remove an in-memory runtime server after a failed start attempt.
    /// This makes dynamic registration transactional: callers may retry the
    /// same deterministic name after correcting an argument or install issue.
    pub fn remove_runtime_server_config(&mut self, name: &str) {
        self.drop_connection(name, "runtime server start rolled back");
        if self.dynamic_servers.write().remove(name).is_some() {
            self.catalog_generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Get list of connected server names
    #[allow(dead_code)] // Public API; the HTTP list endpoint no longer spawns a pool to call it (#3532)
    pub fn connected_servers(&self) -> Vec<&str> {
        self.connections
            .iter()
            .filter(|(_, c)| c.is_ready())
            .map(|(n, _)| n.as_str())
            .collect()
    }

    /// Names of every *enabled* server this pool would connect on the next
    /// turn (static config + dynamic runtime entries).
    ///
    /// Read-only: unlike [`Self::connect_all`], it neither reloads the config
    /// sources nor starts a process. `/preview-request` uses it, together
    /// with [`Self::connected_servers`] and
    /// [`Self::config_sources_unchanged`], to decide whether the currently
    /// connected tool set is *exactly* what the next turn would send — and to
    /// report the tool surface as unavailable when it is not (#1004).
    pub fn enabled_server_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .config
            .servers
            .iter()
            .filter(|(_, server)| server.is_enabled())
            .map(|(name, _)| name.clone())
            .collect();
        let dynamic = self.dynamic_servers.read();
        for (name, server) in dynamic.iter() {
            if server.is_enabled() && !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    /// Whether every configured MCP source still has the mtime this pool last
    /// read, i.e. whether `connect_all` would find anything new.
    ///
    /// Stats files; never reads, parses, reloads, or drops a connection. A
    /// pool with no configured source is trivially unchanged.
    pub fn config_sources_unchanged(&self) -> bool {
        if self.config_sources.is_empty() {
            return true;
        }
        let current: Vec<_> = self
            .config_sources
            .iter()
            .map(|path| mcp_config_mtime(path))
            .collect();
        current == self.last_mtimes
    }

    /// Graceful shutdown of every connection in the pool: send SIGTERM to
    /// each stdio child and give them a short grace period before drop
    /// fires SIGKILL. Whalescale#420.
    ///
    /// Call from the TUI exit path *before* dropping the pool to give
    /// MCP servers a chance to flush state. The fallback Drop on
    /// `StdioTransport` still sends SIGTERM if this never runs, so even
    /// abnormal exits avoid leaking PIDs without a signal.
    pub async fn shutdown_all(&mut self) {
        let names: Vec<String> = self.connections.keys().cloned().collect();
        for name in names {
            if let Some(conn) = self.connections.get_mut(&name) {
                conn.transport.shutdown().await;
            }
        }
        self.connections.clear();
    }

    /// Check if a tool name is an MCP tool
    pub fn is_mcp_tool(name: &str) -> bool {
        name.starts_with("mcp_")
            || matches!(
                name,
                "list_mcp_resources" | "list_mcp_resource_templates" | "read_mcp_resource"
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpWriteStatus {
    Created,
    Overwritten,
    SkippedExists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDiscoveredItem {
    pub name: String,
    pub model_name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSnapshot {
    pub name: String,
    pub enabled: bool,
    pub required: bool,
    pub transport: String,
    pub command_or_url: String,
    pub connect_timeout: u64,
    pub execute_timeout: u64,
    pub read_timeout: u64,
    pub connected: bool,
    pub error: Option<String>,
    /// Typed `◆ auth required` state: the server's most recent connect
    /// attempt failed because a login is missing, expired, or revoked
    /// (401 / OAuth not logged in / `invalid_grant`). Derived from the pool's
    /// needs-auth set — the same source that decides whether the model gets
    /// the synthetic `mcp_<server>_authenticate` tool — so every surface
    /// (session boot row, `/mcp` manager, Extensions, model catalog) agrees.
    pub auth_required: bool,
    pub capability_metadata: McpServerCapabilityMetadata,
    pub tools: Vec<McpDiscoveredItem>,
    pub resources: Vec<McpDiscoveredItem>,
    pub prompts: Vec<McpDiscoveredItem>,
}

impl McpServerSnapshot {
    /// Recovery for this observed server. The typed auth-required state wins
    /// over error-text sniffing so a needs-auth server always routes to
    /// `/mcp login <name>`.
    #[must_use]
    pub fn recovery_kind(&self, oauth_capable: bool) -> McpRecoveryKind {
        if self.enabled && !self.connected && self.auth_required {
            return McpRecoveryKind::Reauth;
        }
        mcp_recovery_kind(
            self.enabled,
            true,
            self.connected,
            self.error.as_deref(),
            oauth_capable,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpManagerSnapshot {
    pub config_path: std::path::PathBuf,
    pub config_exists: bool,
    pub reload_required: bool,
    pub servers: Vec<McpServerSnapshot>,
}

/// First-class recovery for a configured MCP server. Commands named here exist:
/// `/mcp login`, `/mcp reload`, `/mcp validate`, `/mcp enable`. There is no
/// `/mcp auth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRecoveryKind {
    Enable,
    Connect,
    Reconnect,
    Reauth,
    Diagnose,
}

impl McpRecoveryKind {
    #[must_use]
    pub fn slash_command(self, name: &str) -> String {
        match self {
            Self::Enable => format!("/mcp enable {name}"),
            Self::Connect | Self::Reconnect => "/mcp reload".to_string(),
            Self::Reauth => format!("/mcp login {name}"),
            Self::Diagnose => "/mcp validate".to_string(),
        }
    }

    #[must_use]
    pub fn label_key(self) -> crate::localization::MessageId {
        match self {
            Self::Enable => crate::localization::MessageId::ExtensionsActionEnable,
            Self::Connect => crate::localization::MessageId::ExtensionsActionConnect,
            Self::Reconnect => crate::localization::MessageId::ExtensionsActionReconnect,
            Self::Reauth => crate::localization::MessageId::ExtensionsActionReauth,
            Self::Diagnose => crate::localization::MessageId::ExtensionsActionDiagnose,
        }
    }
}

#[must_use]
pub fn mcp_name_is_command_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

/// Display target for an MCP server row: stdio servers show the command
/// name only — no `./…` relative-path prefix, no directories, no args —
/// while URL servers keep their full URL, which is the identity. The
/// snapshot's `command_or_url` keeps full fidelity for the engine and the
/// wire; this is presentation only.
#[must_use]
pub fn mcp_display_target(transport: &str, command_or_url: &str) -> String {
    if transport != "stdio" {
        return command_or_url.to_string();
    }
    let command = command_or_url
        .split_whitespace()
        .next()
        .unwrap_or(command_or_url);
    let name = command.rsplit(['/', '\\']).next().unwrap_or(command);
    if name.is_empty() {
        command_or_url.to_string()
    } else {
        name.to_string()
    }
}

#[must_use]
pub fn mcp_server_oauth_capable(config: &McpServerConfig) -> bool {
    config.url.is_some()
        && (config.oauth.is_some() || !config.scopes.is_empty() || config.oauth_resource.is_some())
}

#[must_use]
pub fn mcp_recovery_kind(
    enabled: bool,
    inspected: bool,
    connected: bool,
    error: Option<&str>,
    oauth_capable: bool,
) -> McpRecoveryKind {
    if !enabled {
        return McpRecoveryKind::Enable;
    }
    if let Some(error) = error {
        if oauth::error_text_looks_auth_required(error) {
            return McpRecoveryKind::Reauth;
        }
        return McpRecoveryKind::Diagnose;
    }
    if !inspected {
        return McpRecoveryKind::Connect;
    }
    if connected {
        return McpRecoveryKind::Diagnose;
    }
    if oauth_capable {
        return McpRecoveryKind::Reauth;
    }
    McpRecoveryKind::Reconnect
}

/// Session-start / manager line: name the server, name the failure, one command.
/// Matches the Codex shape (`The X MCP server requires OAuth reauthentication. Run …`
/// / `MCP startup incomplete (failed: X)`).
#[must_use]
pub fn mcp_startup_warning(name: &str, kind: McpRecoveryKind, failed: bool) -> String {
    let command = kind.slash_command(name);
    match kind {
        McpRecoveryKind::Reauth => {
            format!("The {name} MCP server requires OAuth reauthentication. Run `{command}`.")
        }
        McpRecoveryKind::Enable => {
            format!("The {name} MCP server is disabled. Run `{command}`.")
        }
        _ if failed => format!("MCP startup incomplete (failed: {name}). Run `{command}`."),
        McpRecoveryKind::Connect => {
            format!("The {name} MCP server is not connected yet. Run `{command}`.")
        }
        _ => format!("The {name} MCP server needs attention. Run `{command}`."),
    }
}

pub fn load_config(path: &Path) -> Result<McpConfig> {
    validate_mcp_config_path(path)?;
    let Some(contents) = read_mcp_config_file(path)? else {
        return Ok(McpConfig::default());
    };
    serde_json::from_str(&contents).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse MCP config {}; file contents were omitted",
            codewhale_config::quote_os_path(path)
        )
    })
}

fn read_mcp_config_file(path: &Path) -> Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Failed to inspect MCP config {}", path.display()));
        }
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_file() {
        anyhow::bail!("MCP config path must be a regular file: {}", path.display());
    }

    let mut file = open_mcp_config_file(path)
        .with_context(|| format!("Failed to read MCP config {}", path.display()))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .with_context(|| format!("Failed to read MCP config {}", path.display()))?;
    Ok(Some(contents))
}

#[cfg(unix)]
fn open_mcp_config_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_mcp_config_file(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

pub fn workspace_mcp_config_path(workspace: &Path) -> PathBuf {
    normalize_workspace_path(workspace)
        .join(".codewhale")
        .join("mcp.json")
}

pub fn load_config_with_workspace(global_path: &Path, workspace: &Path) -> Result<McpConfig> {
    let plugins = crate::plugins::PluginRegistry::empty(workspace);
    load_config_with_workspace_and_plugins(global_path, workspace, &plugins)
}

pub fn load_config_with_workspace_and_plugins(
    global_path: &Path,
    workspace: &Path,
    plugins: &crate::plugins::PluginRegistry,
) -> Result<McpConfig> {
    let mut merged = load_config(global_path)?;
    let workspace = checked_workspace_path(workspace)?;
    let project_path = checked_workspace_mcp_config_path(&workspace)?;
    if !project_path.exists() || paths_refer_to_same_config(global_path, &project_path) {
        return merge_plugin_mcp_servers(merged, plugins);
    }
    // Workspace-local MCP can spawn stdio servers, so it is only honored after
    // the user has trusted this workspace in user-owned config. Do not accept
    // project-local legacy trust markers here: a repository could carry those
    // files itself and silently reintroduce the project-scope `mcp_config_path`
    // risk denied in #417.
    if !workspace_allows_project_mcp_config(&workspace) {
        return merge_plugin_mcp_servers(merged, plugins);
    }

    let mut project = load_config(&project_path)?;
    for server in project.servers.values_mut() {
        if server.command.is_some() && server.url.is_none() {
            server.cwd = Some(resolve_project_mcp_cwd(&workspace, server.cwd.as_deref())?);
        }
    }
    merged.servers.extend(project.servers);

    merge_plugin_mcp_servers(merged, plugins)
}

fn merge_plugin_mcp_servers(
    config: McpConfig,
    registry: &crate::plugins::PluginRegistry,
) -> Result<McpConfig> {
    let Some(state_path) = registry.state_path().map(Path::to_path_buf) else {
        return Ok(config);
    };
    let plugins = registry
        .active_plugins()
        .into_iter()
        .filter_map(|plugin| {
            plugin
                .authority(state_path.clone(), registry.workspace().to_path_buf())
                .map(|authority| (plugin.name().to_string(), plugin.clone(), authority))
        })
        .collect::<Vec<_>>();

    let host_environment = registry.host_environment().ok_or_else(|| {
        anyhow::anyhow!("active plugin registry is missing its pre-dotenv environment snapshot")
    })?;
    merge_plugin_mcp_servers_from_plugins_with_environment(config, plugins, host_environment)
}

fn merge_plugin_mcp_servers_from_plugins_with_environment(
    mut config: McpConfig,
    plugins: impl IntoIterator<
        Item = (
            String,
            crate::plugins::types::LoadedPlugin,
            crate::plugins::types::PluginAuthority,
        ),
    >,
    host_environment: Arc<crate::plugins::HostEnvironment>,
) -> Result<McpConfig> {
    for (plugin_name, plugin, authority) in plugins {
        // Adapter-level denial keeps headless paths fail-closed even if a
        // future caller accidentally passes the full inventory instead of the
        // registry's active-only view.
        if !plugin.active() {
            continue;
        }
        if let Some(mcp_servers) = &plugin.manifest.mcp_servers {
            let mut mcp_servers = mcp_servers.iter().collect::<Vec<_>>();
            mcp_servers.sort_by_key(|(name, _)| *name);
            for (server_name, server_config) in mcp_servers {
                let required_capability = if server_config.command.is_some()
                    && server_config.url.is_none()
                {
                    crate::plugins::activation::PluginActivationCapability::McpStdio
                } else if server_config.url.is_some() && server_config.command.is_none() {
                    crate::plugins::activation::PluginActivationCapability::McpRemote
                } else {
                    tracing::warn!(
                        target: "mcp",
                        plugin = %plugin_name,
                        server = %server_name,
                        "plugin MCP server is neither a reviewed stdio nor remote transport; denying it"
                    );
                    continue;
                };
                if !plugin.component_active(required_capability)
                    || crate::plugins::registry::verify_plugin_component_authority(
                        &authority,
                        required_capability,
                    )
                    .is_err()
                {
                    tracing::warn!(
                        target: "mcp",
                        plugin = %plugin_name,
                        server = %server_name,
                        capability = required_capability.as_str(),
                        "plugin bundle changed after review or this transport is inactive; denying the MCP server until reload and re-review"
                    );
                    continue;
                }
                let qualified_name = qualified_plugin_server_name(&plugin_name, server_name);
                if config.servers.contains_key(&qualified_name) {
                    tracing::warn!(
                        target: "mcp",
                        plugin = %plugin_name,
                        server = %server_name,
                        qualified_name = %qualified_name,
                        "explicit MCP configuration keeps precedence over a colliding plugin server"
                    );
                    continue;
                }
                let mut server_config = server_config.clone();

                if server_config.command.is_some() && server_config.url.is_none() {
                    let staged_root = plugin
                        .staged_root
                        .as_deref()
                        .context("active plugin is missing its runtime snapshot")?;
                    server_config.cwd = Some(resolve_plugin_mcp_cwd(
                        staged_root,
                        server_config.cwd.as_deref(),
                    )?);
                    freeze_plugin_stdio_paths(&mut server_config, staged_root)?;
                }
                server_config.reviewed_plugin = Some(ReviewedPluginMcpSource::from_authority(
                    authority.clone(),
                    server_config.url.as_deref(),
                    Arc::clone(&host_environment),
                )?);

                config.servers.insert(qualified_name, server_config);
            }
        }
    }

    Ok(config)
}

#[cfg(test)]
fn merge_plugin_mcp_servers_from_plugins(
    config: McpConfig,
    plugins: impl IntoIterator<
        Item = (
            String,
            crate::plugins::types::LoadedPlugin,
            crate::plugins::types::PluginAuthority,
        ),
    >,
) -> Result<McpConfig> {
    merge_plugin_mcp_servers_from_plugins_with_environment(
        config,
        plugins,
        Arc::new(crate::plugins::HostEnvironment::capture()),
    )
}

fn qualified_plugin_server_name(plugin_name: &str, server_name: &str) -> String {
    format!(
        "plugin-{}-{}-{}",
        plugin_name.len(),
        plugin_name,
        server_name
    )
}

fn freeze_plugin_stdio_paths(config: &mut McpServerConfig, staged_root: &Path) -> Result<()> {
    if let Some(command) = config.command.as_mut()
        && (command.contains('/') || command.contains('\\'))
    {
        let frozen = resolve_plugin_mcp_cwd(staged_root, Some(Path::new(command)))?;
        *command = frozen.display().to_string();
    }
    let runtime_cwd = config.cwd.as_deref().unwrap_or(staged_root).to_path_buf();
    for argument in &mut config.args {
        if argument.starts_with('-') || Path::new(argument).is_absolute() {
            continue;
        }
        let candidate = normalize_path_components(&runtime_cwd.join(argument.as_str()));
        if candidate.exists() {
            let frozen = candidate
                .canonicalize()
                .context("failed to freeze reviewed plugin MCP argument path")?;
            if !frozen.starts_with(staged_root) {
                anyhow::bail!("reviewed plugin MCP argument path escaped its staged root");
            }
            *argument = frozen.display().to_string();
        }
    }
    Ok(())
}

fn resolve_plugin_mcp_cwd(plugin_path: &Path, cwd: Option<&Path>) -> Result<PathBuf> {
    let cwd = match cwd {
        Some(cwd) if cwd.is_relative() => normalize_path_components(&plugin_path.join(cwd)),
        Some(cwd) => normalize_path_components(cwd),
        None => plugin_path.to_path_buf(),
    };
    let resolved = cwd
        .canonicalize()
        .unwrap_or_else(|_| normalize_path_components(&cwd));
    if !resolved.starts_with(plugin_path) {
        anyhow::bail!("reviewed plugin MCP path escaped its staged root");
    }
    Ok(resolved)
}

fn workspace_allows_project_mcp_config(workspace: &Path) -> bool {
    crate::config::is_workspace_trusted(workspace)
}

fn checked_workspace_mcp_config_path(workspace: &Path) -> Result<PathBuf> {
    Ok(checked_workspace_path(workspace)?
        .join(".codewhale")
        .join("mcp.json"))
}

fn checked_workspace_path(workspace: &Path) -> Result<PathBuf> {
    if workspace.as_os_str().is_empty() {
        anyhow::bail!("workspace path cannot be empty");
    }
    if workspace
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("workspace path cannot contain '..' components");
    }
    let absolute = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to resolve current directory for workspace")?
            .join(workspace)
    };
    match absolute.canonicalize() {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(normalize_path_components(&absolute))
        }
        Err(err) => {
            Err(err).with_context(|| format!("failed to resolve workspace {}", workspace.display()))
        }
    }
}

fn normalize_workspace_path(workspace: &Path) -> PathBuf {
    if let Ok(canonical) = workspace.canonicalize() {
        return canonical;
    }
    let absolute = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(workspace)
    };
    normalize_path_components(&absolute)
}

fn resolve_project_mcp_cwd(workspace: &Path, cwd: Option<&Path>) -> Result<PathBuf> {
    let cwd = match cwd {
        Some(cwd) if cwd.is_relative() => normalize_path_components(&workspace.join(cwd)),
        Some(cwd) => normalize_path_components(cwd),
        None => workspace.to_path_buf(),
    };
    let resolved = cwd
        .canonicalize()
        .unwrap_or_else(|_| normalize_path_components(&cwd));
    if !resolved.starts_with(workspace) {
        anyhow::bail!(
            "Project MCP server cwd must stay within workspace: {}",
            resolved.display()
        );
    }
    Ok(resolved)
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn paths_refer_to_same_config(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => normalize_workspace_path(left) == normalize_workspace_path(right),
    }
}

/// Rebuild a JSON value with every object's keys in sorted order.
///
/// [`McpConfig`] is full of `HashMap`s (`servers`, and per-server `env`,
/// `headers`, `env_headers`), and `serde_json` is built here with
/// `preserve_order`, so a serialization inherits whatever order the source
/// `HashMap` happened to iterate in. Two `HashMap`s built separately in one
/// process do *not* share an iteration order — `RandomState` re-seeds per map
/// — so the raw serialization of two structurally identical configs differs.
/// Sorting first makes the byte form depend only on content. The value tree
/// here is the fixed `McpConfig` shape, so the recursion depth is bounded by
/// that struct, not by untrusted input.
fn canonicalize_json_keys(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map.into_iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json_keys(value)))
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonicalize_json_keys).collect())
        }
        other => other,
    }
}

/// 64-bit content hash of an [`McpConfig`]. Used by [`McpPool`] to decide
/// whether a freshly-read config differs from the one currently driving the
/// live connections. Hashing the JSON serialization avoids forcing every
/// nested config type to derive `Hash` (the timeouts struct, network policy
/// stubs, etc.); the serialization is key-sorted first so the hash depends on
/// content alone and not on per-`HashMap` iteration order. The hash is stable
/// within a process for structurally identical configs, and across runs of the
/// same Rust toolchain for byte-identical input.
fn hash_mcp_config(config: &McpConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let canonical = serde_json::to_value(config)
        .map(canonicalize_json_keys)
        .unwrap_or(serde_json::Value::Null);
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Best-effort fetch of the MCP config file's last-modified time. Returns
/// `None` when the file is missing, when stat fails, when the platform
/// doesn't expose mtime, or when the path fails the same allow-list check
/// that `load_config` / `save_config` apply. The lazy-reload check in
/// `McpPool::get_or_connect` treats `None` as "skip the check this turn",
/// so a rejected path simply degrades to "no auto-reload" rather than an
/// error path. Callers already validate via `validate_mcp_config_path` at
/// construction time; the redundant validation here keeps this helper
/// safe-by-construction for any future caller and ties the validation to
/// the call site rather than relying on cross-function reasoning.
fn mcp_config_mtime(path: &Path) -> Option<std::time::SystemTime> {
    validate_mcp_config_path(path).ok()?;
    fs::metadata(path).ok()?.modified().ok()
}

pub fn save_config(path: &Path, cfg: &McpConfig) -> Result<()> {
    validate_mcp_config_path(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create MCP config directory {}", parent.display())
        })?;
    }
    let rendered = serde_json::to_string_pretty(cfg).context("Failed to serialize MCP config")?;
    write_atomic(path, rendered.as_bytes())
        .with_context(|| format!("Failed to write MCP config {}", path.display()))?;
    Ok(())
}

fn mcp_template_json() -> Result<String> {
    let mut cfg = McpConfig::default();
    cfg.servers.insert(
        "example".to_string(),
        McpServerConfig {
            command: Some("node".to_string()),
            args: vec!["./path/to/your-mcp-server.js".to_string()],
            env: HashMap::new(),
            cwd: None,
            url: None,
            transport: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: true,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: HashMap::new(),
            env_headers: HashMap::new(),
            bearer_token_env_var: None,
            scopes: Vec::new(),
            oauth: None,
            oauth_resource: None,
            reviewed_plugin: None,
        },
    );
    serde_json::to_string_pretty(&cfg).context("Failed to render MCP template JSON")
}

pub fn init_config(path: &Path, force: bool) -> Result<McpWriteStatus> {
    validate_mcp_config_path(path)?;
    if path.exists() && !force {
        return Ok(McpWriteStatus::SkippedExists);
    }
    let status = if path.exists() {
        McpWriteStatus::Overwritten
    } else {
        McpWriteStatus::Created
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create MCP config directory {}", parent.display())
        })?;
    }
    let template = mcp_template_json()?;
    write_atomic(path, template.as_bytes())
        .with_context(|| format!("Failed to write MCP config {}", path.display()))?;
    Ok(status)
}

pub fn add_server_config(
    path: &Path,
    name: String,
    command: Option<String>,
    url: Option<String>,
    args: Vec<String>,
    transport: Option<String>,
) -> Result<()> {
    if command.is_none() && url.is_none() {
        anyhow::bail!("Provide either a command or URL for MCP server '{name}'.");
    }
    validate_mcp_transport(transport.as_deref())?;
    let mut cfg = load_config(path)?;
    cfg.servers.insert(
        name,
        McpServerConfig {
            command,
            args,
            env: HashMap::new(),
            cwd: None,
            url,
            transport,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: HashMap::new(),
            env_headers: HashMap::new(),
            bearer_token_env_var: None,
            scopes: Vec::new(),
            oauth: None,
            oauth_resource: None,
            reviewed_plugin: None,
        },
    );
    save_config(path, &cfg)
}

pub fn remove_server_config(path: &Path, name: &str) -> Result<()> {
    let mut cfg = load_config(path)?;
    if cfg.servers.remove(name).is_none() {
        anyhow::bail!("MCP server '{name}' not found");
    }
    save_config(path, &cfg)
}

pub fn set_server_enabled(path: &Path, name: &str, enabled: bool) -> Result<()> {
    let mut cfg = load_config(path)?;
    let server = cfg
        .servers
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("MCP server '{name}' not found"))?;
    server.enabled = enabled;
    server.disabled = !enabled;
    save_config(path, &cfg)
}

#[cfg(test)]
pub fn manager_snapshot_from_config(
    path: &Path,
    reload_required: bool,
) -> Result<McpManagerSnapshot> {
    let cfg = load_config(path)?;
    Ok(snapshot_from_config(
        path,
        path.exists(),
        reload_required,
        &cfg,
        None,
    ))
}

#[cfg(test)]
pub fn manager_snapshot_from_config_with_workspace(
    path: &Path,
    workspace: &Path,
    reload_required: bool,
) -> Result<McpManagerSnapshot> {
    let plugins = crate::plugins::PluginRegistry::empty(workspace);
    manager_snapshot_from_config_with_workspace_and_plugins(
        path,
        workspace,
        reload_required,
        &plugins,
    )
}

pub fn manager_snapshot_from_config_with_workspace_and_plugins(
    path: &Path,
    workspace: &Path,
    reload_required: bool,
    plugins: &crate::plugins::PluginRegistry,
) -> Result<McpManagerSnapshot> {
    let cfg = load_config_with_workspace_and_plugins(path, workspace, plugins)?;
    Ok(snapshot_from_config(
        path,
        path.exists(),
        reload_required,
        &cfg,
        None,
    ))
}

#[cfg(test)]
pub async fn discover_manager_snapshot(
    path: &Path,
    network_policy: Option<NetworkPolicyDecider>,
    reload_required: bool,
) -> Result<McpManagerSnapshot> {
    let cfg = load_config(path)?;
    let mut pool = McpPool::new(cfg.clone());
    if let Some(policy) = network_policy {
        pool = pool.with_network_policy(policy);
    }
    let errors = pool
        .connect_all()
        .await
        .into_iter()
        .map(|(name, err)| (name, format_mcp_error_for_display(&err)))
        .collect::<HashMap<_, _>>();
    Ok(snapshot_from_config(
        path,
        path.exists(),
        reload_required,
        &cfg,
        Some((&pool, &errors)),
    ))
}

pub async fn discover_manager_snapshot_with_workspace_and_plugins(
    path: &Path,
    workspace: &Path,
    network_policy: Option<NetworkPolicyDecider>,
    reload_required: bool,
    plugins: Arc<crate::plugins::PluginRegistry>,
) -> Result<McpManagerSnapshot> {
    let cfg = load_config_with_workspace_and_plugins(path, workspace, plugins.as_ref())?;
    let mut pool = McpPool::new(cfg.clone());
    pool.workspace = Some(checked_workspace_path(workspace)?);
    pool.plugin_registry = Some(plugins);
    if let Some(policy) = network_policy {
        pool = pool.with_network_policy(policy);
    }
    let errors = pool
        .connect_all()
        .await
        .into_iter()
        .map(|(name, err)| (name, format_mcp_error_for_display(&err)))
        .collect::<HashMap<_, _>>();
    Ok(snapshot_from_config(
        path,
        path.exists(),
        reload_required,
        &cfg,
        Some((&pool, &errors)),
    ))
}

pub(crate) fn format_mcp_error_for_display(error: &anyhow::Error) -> String {
    codewhale_config::persistence::redact_secrets(&format!("{error:#}"))
}

impl McpPool {
    /// Snapshot the live pool rather than starting a second discovery pool.
    /// This keeps the manager, hotbar, and next model turn aligned on one
    /// exact config/catalog generation.
    pub(crate) fn manager_snapshot(
        &self,
        path: &Path,
        reload_required: bool,
        errors: &HashMap<String, String>,
    ) -> McpManagerSnapshot {
        snapshot_from_config(
            path,
            path.exists(),
            reload_required,
            &self.config,
            Some((self, errors)),
        )
    }
}

fn snapshot_from_config(
    path: &Path,
    config_exists: bool,
    reload_required: bool,
    cfg: &McpConfig,
    discovery: Option<(&McpPool, &HashMap<String, String>)>,
) -> McpManagerSnapshot {
    let mut servers = cfg
        .servers
        .iter()
        .map(|(name, server)| {
            let transport = if server.url.is_some() {
                if is_legacy_sse_transport(server) {
                    "sse"
                } else {
                    "http/sse"
                }
            } else {
                "stdio"
            };
            let command_or_url = server.url.clone().unwrap_or_else(|| {
                let mut command = server
                    .command
                    .clone()
                    .unwrap_or_else(|| "(missing)".to_string());
                if !server.args.is_empty() {
                    command.push(' ');
                    command.push_str(&server.args.join(" "));
                }
                command
            });
            let mut snapshot = McpServerSnapshot {
                name: name.clone(),
                enabled: server.is_enabled(),
                required: server.required,
                transport: transport.to_string(),
                command_or_url,
                connect_timeout: server.effective_connect_timeout(&cfg.timeouts),
                execute_timeout: server.effective_execute_timeout(&cfg.timeouts),
                read_timeout: server.effective_read_timeout(&cfg.timeouts),
                connected: false,
                error: if server.is_enabled() {
                    None
                } else {
                    Some("disabled".to_string())
                },
                auth_required: false,
                capability_metadata: McpServerCapabilityMetadata::NotObserved,
                tools: Vec::new(),
                resources: Vec::new(),
                prompts: Vec::new(),
            };

            if let Some((pool, errors)) = discovery {
                if let Some(error) = errors.get(name) {
                    snapshot.error = Some(error.clone());
                }
                // The pool's needs-auth set is the authority; the error text
                // fallback keeps a boot-time error map (held by the engine
                // after the pool's live state was rebuilt) on the same
                // classification instead of downgrading to a plain failure.
                snapshot.auth_required = server.is_enabled()
                    && (pool.server_needs_auth(name)
                        || snapshot
                            .error
                            .as_deref()
                            .is_some_and(oauth::error_text_looks_auth_required));
                if let Some(conn) = pool.connections.get(name) {
                    snapshot.connected = conn.is_ready();
                    snapshot.capability_metadata = conn.server_capabilities.map_or(
                        McpServerCapabilityMetadata::LegacyFallback,
                        McpServerCapabilityMetadata::Advertised,
                    );
                    if snapshot.connected {
                        // A count of connected servers and nothing else. The
                        // name, the command or URL, and the error string are
                        // user-chosen and routinely name internal infra.
                        codewhale_telemetry::session_counters()
                            .bump(codewhale_telemetry::Counter::McpServerConnected);
                    }
                    snapshot.tools = conn
                        .tools()
                        .iter()
                        .filter(|tool| conn.config().is_tool_enabled(&tool.name))
                        .map(|tool| McpDiscoveredItem {
                            name: tool.name.clone(),
                            model_name: format!("mcp_{}_{}", name, tool.name),
                            description: tool.description.clone(),
                        })
                        .collect();
                    snapshot.resources =
                        conn.resources()
                            .iter()
                            .map(|resource| McpDiscoveredItem {
                                name: resource.name.clone(),
                                model_name: format!(
                                    "mcp_{}_{}",
                                    name,
                                    resource.name.replace(' ', "_").to_lowercase()
                                ),
                                description: resource.description.clone(),
                            })
                            .chain(conn.resource_templates().iter().map(|template| {
                                McpDiscoveredItem {
                                    name: template.name.clone(),
                                    model_name: format!(
                                        "mcp_{}_{}",
                                        name,
                                        template.name.replace(' ', "_").to_lowercase()
                                    ),
                                    description: template.description.clone(),
                                }
                            }))
                            .collect();
                    snapshot.prompts = conn
                        .prompts()
                        .iter()
                        .map(|prompt| McpDiscoveredItem {
                            name: prompt.name.clone(),
                            model_name: format!("mcp_{}_{}", name, prompt.name),
                            description: prompt.description.clone(),
                        })
                        .collect();
                }
            }

            snapshot
        })
        .collect::<Vec<_>>();
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    McpManagerSnapshot {
        config_path: path.to_path_buf(),
        config_exists,
        reload_required,
        servers,
    }
}

// === Unit Tests ===

#[cfg(test)]
mod tests;
