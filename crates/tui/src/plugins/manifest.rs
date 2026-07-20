use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::path_identity::metadata_is_link_or_reparse;
#[cfg(windows)]
use super::path_identity::windows_file_identity;
use crate::mcp::{McpServerConfig, is_relative_stdio_path_arg};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
const MAX_PLUGIN_NAME_CHARS: usize = 64;
const MAX_COMPONENT_PATHS: usize = 64;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_HASHED_FILES: usize = 4_096;
const MAX_HASHED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MCP_ARGS: usize = 64;
const MAX_MCP_ENV: usize = 64;
const MAX_MCP_HEADERS: usize = 64;
const MAX_MCP_SCOPES: usize = 64;
const MAX_MCP_TOOL_FILTERS: usize = 256;
const MAX_MCP_TIMEOUT_SECS: u64 = 3_600;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// Missing means the legacy, pre-versioned Codewhale manifest. Legacy
    /// manifests remain readable, but `/plugin validate` reports the migration.
    #[serde(default)]
    pub schema_version: u32,
    pub plugin: PluginMeta,
    #[serde(default)]
    pub skills: Option<PluginPathSpec>,
    #[serde(default)]
    pub commands: Option<PluginPathSpec>,
    #[serde(default, alias = "profiles")]
    pub agents: Option<PluginPathSpec>,
    #[serde(default)]
    pub hooks: Option<PluginPathSpec>,
    #[serde(default, alias = "lsp_servers")]
    pub lsp: Option<PluginPathSpec>,
    #[serde(default, alias = "native_extension")]
    pub native: Option<PluginPathSpec>,
    #[serde(default)]
    pub mcp_servers: Option<HashMap<String, McpServerConfig>>,
    #[serde(default)]
    pub capabilities: PluginCapabilities,
    #[serde(default)]
    pub when: Option<PluginWhen>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginMeta {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: Option<String>,
}

/// A declarative component location. `path` preserves the original manifest
/// shape; `paths` lets a bundle split one component kind across directories.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPathSpec {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

impl PluginPathSpec {
    fn declared_paths(&self, default: Option<&str>) -> Result<Vec<String>, String> {
        let mut paths = Vec::new();
        if let Some(path) = self.path.as_deref() {
            paths.push(path.to_string());
        }
        paths.extend(self.paths.iter().cloned());
        if paths.is_empty()
            && let Some(default) = default
        {
            paths.push(default.to_string());
        }
        if paths.is_empty() {
            return Err("component table must declare `path` or `paths`".to_string());
        }
        if paths.len() > MAX_COMPONENT_PATHS {
            return Err(format!(
                "component declares {} paths; maximum is {MAX_COMPONENT_PATHS}",
                paths.len()
            ));
        }
        let mut seen = BTreeSet::new();
        for path in &paths {
            if !seen.insert(path.clone()) {
                return Err(format!(
                    "component path `{path}` is declared more than once"
                ));
            }
        }
        Ok(paths)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginCapabilities {
    /// Requested filesystem roots are inventory-only in v0.9.1. Declaring any
    /// keeps the bundle inactive until a later permission adapter exists.
    #[serde(default)]
    pub filesystem_roots: Vec<String>,
    /// Requested hosts are inventory-only. MCP URL hosts are added to the
    /// effective capability inventory automatically.
    #[serde(default)]
    pub network_hosts: Vec<String>,
    /// Lifecycle mutation is inventoried but unsupported in v0.9.1.
    #[serde(default)]
    pub lifecycle_mutation: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginWhen {
    #[serde(default)]
    pub os: Option<Vec<String>>,
    #[serde(default)]
    pub binaries: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedPluginComponents {
    pub skills: Vec<PathBuf>,
    pub commands: Vec<PathBuf>,
    pub agents: Vec<PathBuf>,
    pub hooks: Vec<PathBuf>,
    pub lsp: Vec<PathBuf>,
    pub native: Vec<PathBuf>,
}

impl ResolvedPluginComponents {
    pub fn all_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.skills
            .iter()
            .chain(&self.commands)
            .chain(&self.agents)
            .chain(&self.hooks)
            .chain(&self.lsp)
            .chain(&self.native)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginInventory {
    pub skills: usize,
    pub mcp_servers: usize,
    /// MCP servers that launch a child process under the Codewhale user's
    /// host permissions. Kept separate from remote MCP so the review screen
    /// cannot imply that an empty declared filesystem/network list is a
    /// sandbox boundary.
    #[serde(default)]
    pub stdio_mcp_servers: usize,
    /// MCP servers contacted over HTTP(S) without launching a local child.
    #[serde(default)]
    pub remote_mcp_servers: usize,
    pub commands: usize,
    pub agents: usize,
    pub hooks: usize,
    pub lsp: usize,
    pub native: usize,
    pub filesystem_roots: Vec<String>,
    pub network_hosts: Vec<String>,
    pub lifecycle_mutation: bool,
}

impl PluginInventory {
    #[must_use]
    pub fn unsupported_labels(&self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.commands > 0 {
            labels.push("commands");
        }
        if self.agents > 0 {
            labels.push("agents");
        }
        if self.hooks > 0 {
            labels.push("hooks");
        }
        if self.lsp > 0 {
            labels.push("lsp");
        }
        if self.native > 0 {
            labels.push("native");
        }
        if !self.filesystem_roots.is_empty() {
            labels.push("filesystem-roots");
        }
        if self.lifecycle_mutation {
            labels.push("lifecycle-mutation");
        }
        labels
    }

    #[must_use]
    pub fn has_unsupported_capabilities(&self) -> bool {
        !self.unsupported_labels().is_empty()
    }

    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "skills={} mcp={} (stdio={} remote={}) commands={} agents={} hooks={} lsp={} native={}",
            self.skills,
            self.mcp_servers,
            self.stdio_mcp_servers,
            self.remote_mcp_servers,
            self.commands,
            self.agents,
            self.hooks,
            self.lsp,
            self.native
        )
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedManifest {
    pub manifest: PluginManifest,
    pub canonical_root: PathBuf,
    pub components: ResolvedPluginComponents,
    pub inventory: PluginInventory,
    pub content_hash: String,
    /// Digest of the exact bytes read for every regular bundle file, keyed by
    /// its lossless relative OS path. Runtime adapters use this to bind parsed
    /// representations to the same bytes that produced `content_hash`.
    pub(crate) file_hashes: BTreeMap<PathBuf, String>,
    pub capability_hash: String,
    pub applicable: bool,
    pub warnings: Vec<String>,
}

impl PluginManifest {
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|e| format!("failed to inspect plugin.toml: {e}"))?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err("plugin.toml may not be a symbolic link".to_string());
        }
        let bytes = read_manifest_bytes(path)?;
        let content = std::str::from_utf8(&bytes)
            .map_err(|_| "plugin.toml must be valid UTF-8".to_string())?;
        validate_nested_mcp_schema(content)?;
        toml::from_str(content).map_err(|error| safe_toml_parse_error(&error))
    }

    pub fn validate_from_path(path: &Path) -> Result<ValidatedManifest, String> {
        let manifest_metadata = fs::symlink_metadata(path)
            .map_err(|e| format!("failed to inspect plugin.toml: {e}"))?;
        if metadata_is_link_or_reparse(&manifest_metadata) || !manifest_metadata.is_file() {
            return Err("plugin.toml must be a regular file, not a symbolic link".to_string());
        }
        let root = path
            .parent()
            .ok_or_else(|| "plugin.toml has no parent directory".to_string())?;
        let root_metadata = fs::symlink_metadata(root)
            .map_err(|e| format!("failed to inspect plugin root: {e}"))?;
        if metadata_is_link_or_reparse(&root_metadata) || !root_metadata.is_dir() {
            return Err("plugin root must be a directory, not a symbolic link".to_string());
        }
        let canonical_root = root
            .canonicalize()
            .map_err(|e| format!("failed to canonicalize plugin root: {e}"))?;
        if !canonical_root.is_dir() {
            return Err("plugin root is not a directory".to_string());
        }

        let manifest_bytes = read_manifest_bytes(path)?;
        let manifest_text = std::str::from_utf8(&manifest_bytes)
            .map_err(|_| "plugin.toml must be valid UTF-8".to_string())?;
        validate_nested_mcp_schema(manifest_text)?;
        let mut manifest: Self =
            toml::from_str(manifest_text).map_err(|error| safe_toml_parse_error(&error))?;
        let warnings = if manifest.schema_version == 0 {
            let mut warnings = vec![format!(
                "legacy manifest: add `schema_version = {CURRENT_SCHEMA_VERSION}`"
            )];
            if manifest.plugin.version.trim().is_empty() {
                manifest.plugin.version = "0.0.0".to_string();
                warnings.push(
                    "legacy manifest: add a semantic `[plugin].version`; displaying `0.0.0`"
                        .to_string(),
                );
            }
            warnings
        } else {
            Vec::new()
        };
        manifest.validate_metadata()?;

        let components = manifest.resolve_components(&canonical_root)?;
        manifest.validate_mcp_servers(&canonical_root)?;
        let inventory = manifest.inventory(&components)?;
        let (content_hash, file_hashes) = hash_bundle(&canonical_root, &manifest_bytes)?;
        let capability_hash = hash_inventory(&inventory);
        let applicable = manifest.check_when();
        if read_manifest_bytes(path)? != manifest_bytes {
            return Err(
                "plugin.toml changed while it was being validated; retry discovery".to_string(),
            );
        }

        Ok(ValidatedManifest {
            manifest,
            canonical_root,
            components,
            inventory,
            content_hash,
            file_hashes,
            capability_hash,
            applicable,
            warnings,
        })
    }

    fn validate_metadata(&self) -> Result<(), String> {
        if self.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version {}; maximum is {CURRENT_SCHEMA_VERSION}",
                self.schema_version
            ));
        }
        validate_plugin_name(&self.plugin.name)?;
        Version::parse(self.plugin.version.trim()).map_err(|e| {
            format!(
                "plugin version `{}` is not valid semantic versioning: {e}",
                self.plugin.version
            )
        })?;
        validate_optional_text("description", self.plugin.description.as_deref(), 1_024)?;
        validate_optional_text("author", self.plugin.author.as_deref(), 256)?;
        validate_unique_texts("filesystem root", &self.capabilities.filesystem_roots, 512)?;
        validate_unique_texts("network host", &self.capabilities.network_hosts, 253)?;
        let declared_network_hosts = self
            .capabilities
            .network_hosts
            .iter()
            .map(|host| normalize_network_host(host))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let remote_network_hosts = self
            .mcp_servers
            .as_ref()
            .into_iter()
            .flat_map(|servers| servers.values())
            .filter_map(|server| server.url.as_deref())
            .map(|url| {
                reqwest::Url::parse(url)
                    .map_err(|_| "remote MCP URL is invalid".to_string())?
                    .host_str()
                    .map(str::to_string)
                    .ok_or_else(|| "remote MCP URL is missing a host".to_string())
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if declared_network_hosts != remote_network_hosts {
            return Err(
                "capabilities.network_hosts must exactly match the normalized host set of all remote MCP endpoints"
                    .to_string(),
            );
        }
        if let Some(when) = &self.when {
            if let Some(os_values) = &when.os {
                validate_unique_texts("OS", os_values, 32)?;
                const SUPPORTED: &[&str] = &[
                    "windows", "linux", "macos", "freebsd", "openbsd", "netbsd", "android", "ios",
                ];
                for os in os_values {
                    if !SUPPORTED.contains(&os.to_ascii_lowercase().as_str()) {
                        return Err(format!("unsupported OS selector `{os}`"));
                    }
                }
            }
            if let Some(binaries) = &when.binaries {
                validate_unique_texts("binary", binaries, 128)?;
                for binary in binaries {
                    if binary.contains('/')
                        || binary.contains('\\')
                        || looks_windows_absolute(binary)
                    {
                        return Err(format!(
                            "binary condition `{binary}` must be a bare executable name"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve_components(&self, root: &Path) -> Result<ResolvedPluginComponents, String> {
        Ok(ResolvedPluginComponents {
            skills: resolve_spec(root, "skills", self.skills.as_ref(), Some("skills"))?,
            commands: resolve_spec(root, "commands", self.commands.as_ref(), None)?,
            agents: resolve_spec(root, "agents", self.agents.as_ref(), None)?,
            hooks: resolve_spec(root, "hooks", self.hooks.as_ref(), None)?,
            lsp: resolve_spec(root, "lsp", self.lsp.as_ref(), None)?,
            native: resolve_spec(root, "native", self.native.as_ref(), None)?,
        })
    }

    fn validate_mcp_servers(&self, root: &Path) -> Result<(), String> {
        let Some(servers) = &self.mcp_servers else {
            return Ok(());
        };
        if servers.len() > MAX_COMPONENT_PATHS {
            return Err(format!(
                "manifest declares {} MCP servers; maximum is {MAX_COMPONENT_PATHS}",
                servers.len()
            ));
        }
        for (name, server) in servers {
            validate_component_name("MCP server", name)?;
            if server.args.len() > MAX_MCP_ARGS {
                return Err(format!(
                    "MCP server `{name}` declares too many arguments; maximum is {MAX_MCP_ARGS}"
                ));
            }
            if server.env.len() > MAX_MCP_ENV {
                return Err(format!(
                    "MCP server `{name}` declares too many environment mappings; maximum is {MAX_MCP_ENV}"
                ));
            }
            if server.env_headers.len() > MAX_MCP_HEADERS {
                return Err(format!(
                    "MCP server `{name}` declares too many environment-backed headers; maximum is {MAX_MCP_HEADERS}"
                ));
            }
            if server.scopes.len() > MAX_MCP_SCOPES {
                return Err(format!(
                    "MCP server `{name}` declares too many OAuth scopes; maximum is {MAX_MCP_SCOPES}"
                ));
            }
            if server.enabled_tools.len() > MAX_MCP_TOOL_FILTERS
                || server.disabled_tools.len() > MAX_MCP_TOOL_FILTERS
            {
                return Err(format!(
                    "MCP server `{name}` declares too many tool filters; maximum is {MAX_MCP_TOOL_FILTERS} per list"
                ));
            }
            for (label, timeout) in [
                ("connect_timeout", server.connect_timeout),
                ("execute_timeout", server.execute_timeout),
                ("read_timeout", server.read_timeout),
            ] {
                if timeout.is_some_and(|seconds| !(1..=MAX_MCP_TIMEOUT_SECS).contains(&seconds)) {
                    return Err(format!(
                        "MCP server `{name}` {label} must be 1-{MAX_MCP_TIMEOUT_SECS} seconds"
                    ));
                }
            }
            if server.required && !server.enabled {
                return Err(format!(
                    "MCP server `{name}` cannot be required while disabled"
                ));
            }
            for arg in &server.args {
                validate_text("MCP argument", arg, 4_096)?;
            }
            validate_unique_texts("enabled MCP tool", &server.enabled_tools, 256)?;
            validate_unique_texts("disabled MCP tool", &server.disabled_tools, 256)?;
            if server.enabled_tools.iter().any(|tool| {
                server
                    .disabled_tools
                    .iter()
                    .any(|disabled| disabled == tool)
            }) {
                return Err(format!(
                    "MCP server `{name}` declares a tool in both enabled_tools and disabled_tools"
                ));
            }
            match (server.command.as_deref(), server.url.as_deref()) {
                (Some(command), None) => {
                    validate_text("MCP command", command, 512)?;
                    if server.transport.is_some()
                        || !server.headers.is_empty()
                        || !server.env_headers.is_empty()
                        || server.bearer_token_env_var.is_some()
                        || !server.scopes.is_empty()
                        || server.oauth.is_some()
                        || server.oauth_resource.is_some()
                    {
                        return Err(format!(
                            "stdio MCP server `{name}` may not declare remote transport or authentication fields"
                        ));
                    }
                    if command.contains('/') || command.contains('\\') {
                        let resolved = resolve_contained_path(root, command, "MCP command")?;
                        if !resolved.is_file() {
                            return Err(format!(
                                "MCP server `{name}` command is not a regular file"
                            ));
                        }
                    } else {
                        validate_bare_executable(command)?;
                    }
                    if let Some(cwd) = server.cwd.as_deref() {
                        let raw = cwd.to_string_lossy();
                        let resolved = resolve_contained_path(root, &raw, "MCP cwd")?;
                        if !resolved.is_dir() {
                            return Err(format!(
                                "MCP server `{name}` cwd is not a directory: {}",
                                resolved.display()
                            ));
                        }
                    }
                    validate_mcp_argv_has_no_literal_credentials(name, &server.args)?;
                    for (index, arg) in server.args.iter().enumerate() {
                        if Path::new(arg).is_absolute() || looks_windows_absolute(arg) {
                            return Err(format!(
                                "MCP server `{name}` argument #{} must not use an absolute path",
                                index + 1
                            ));
                        }
                        if is_relative_stdio_path_arg(arg)
                            && Path::new(arg)
                                .components()
                                .any(|part| matches!(part, Component::ParentDir))
                        {
                            return Err(format!(
                                "MCP server `{name}` argument #{} escapes the plugin root",
                                index + 1
                            ));
                        }
                    }
                    for (destination, source) in &server.env {
                        validate_environment_name("MCP environment destination", destination)?;
                        let source = exact_environment_placeholder(source).ok_or_else(|| {
                            format!(
                                "MCP server `{name}` environment values must be exact `${{SOURCE_ENV}}` references"
                            )
                        })?;
                        validate_environment_name("MCP environment source", source)?;
                    }
                }
                (None, Some(url)) => {
                    if !server.scopes.is_empty()
                        || server.oauth.is_some()
                        || server.oauth_resource.is_some()
                    {
                        return Err(format!(
                            "remote MCP server `{name}` may not declare OAuth fields because plugin OAuth is disabled in v0.9.1; use env_headers or bearer_token_env_var"
                        ));
                    }
                    let parsed = reqwest::Url::parse(url)
                        .map_err(|e| format!("MCP server `{name}` URL is invalid: {e}"))?;
                    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                        return Err(format!(
                            "MCP server `{name}` URL must use http or https and include a host"
                        ));
                    }
                    if parsed.scheme() == "http"
                        && !parsed.host_str().is_some_and(|host| {
                            host.eq_ignore_ascii_case("localhost")
                                || host
                                    .parse::<std::net::IpAddr>()
                                    .is_ok_and(|address| address.is_loopback())
                        })
                    {
                        return Err(format!(
                            "MCP server `{name}` URL must use HTTPS unless it targets loopback"
                        ));
                    }
                    if !parsed.username().is_empty() || parsed.password().is_some() {
                        return Err(format!(
                            "MCP server `{name}` URL must not embed credentials; use environment-backed authentication"
                        ));
                    }
                    if parsed.query().is_some() || parsed.fragment().is_some() {
                        return Err(format!(
                            "MCP server `{name}` URL may not contain a query or fragment"
                        ));
                    }
                    if server.cwd.is_some() || !server.args.is_empty() || !server.env.is_empty() {
                        return Err(format!(
                            "remote MCP server `{name}` may not declare stdio cwd, args, or env"
                        ));
                    }
                    if !server.headers.is_empty() {
                        return Err(format!(
                            "remote MCP server `{name}` may not contain literal headers; use env_headers or bearer_token_env_var"
                        ));
                    }
                    if let Some(transport) = server.transport.as_deref() {
                        validate_text("MCP transport", transport, 32)?;
                        if !transport.eq_ignore_ascii_case("sse") {
                            return Err(format!(
                                "MCP server `{name}` transport must be `sse` when explicitly set"
                            ));
                        }
                    }
                    for (header, env_var) in &server.env_headers {
                        validate_http_header_name(header)?;
                        validate_environment_name("MCP header environment source", env_var)?;
                    }
                    if let Some(env_var) = server.bearer_token_env_var.as_deref() {
                        validate_environment_name("MCP bearer environment source", env_var)?;
                    }
                    validate_unique_texts("OAuth scope", &server.scopes, 256)?;
                    if let Some(oauth) = &server.oauth
                        && let Some(client_id) = oauth.client_id.as_deref()
                    {
                        validate_text("OAuth client id", client_id, 512)?;
                    }
                    if let Some(resource) = server.oauth_resource.as_deref() {
                        validate_safe_oauth_resource(resource)?;
                    }
                }
                (Some(_), Some(_)) => {
                    return Err(format!(
                        "MCP server `{name}` must declare exactly one of command or url"
                    ));
                }
                (None, None) => {
                    return Err(format!(
                        "MCP server `{name}` must declare exactly one of command or url"
                    ));
                }
            }
        }
        Ok(())
    }

    fn inventory(&self, components: &ResolvedPluginComponents) -> Result<PluginInventory, String> {
        let stdio_mcp_servers = self.mcp_servers.as_ref().map_or(0, |servers| {
            servers
                .values()
                .filter(|server| server.command.is_some() && server.url.is_none())
                .count()
        });
        let remote_mcp_servers = self.mcp_servers.as_ref().map_or(0, |servers| {
            servers
                .values()
                .filter(|server| server.url.is_some() && server.command.is_none())
                .count()
        });
        let mut network_hosts = self
            .capabilities
            .network_hosts
            .iter()
            .map(|host| host.to_ascii_lowercase())
            .collect::<Vec<_>>();
        if let Some(servers) = &self.mcp_servers {
            for server in servers.values() {
                if let Some(url) = server.url.as_deref()
                    && let Ok(url) = reqwest::Url::parse(url)
                    && let Some(host) = url.host_str()
                {
                    network_hosts.push(host.to_ascii_lowercase());
                }
            }
        }
        network_hosts.sort();
        network_hosts.dedup();

        let mut filesystem_roots = self.capabilities.filesystem_roots.clone();
        filesystem_roots.sort();
        filesystem_roots.dedup();

        Ok(PluginInventory {
            skills: components.skills.len(),
            mcp_servers: self.mcp_servers.as_ref().map_or(0, HashMap::len),
            stdio_mcp_servers,
            remote_mcp_servers,
            commands: components.commands.len(),
            agents: components.agents.len(),
            hooks: components.hooks.len(),
            lsp: components.lsp.len(),
            native: components.native.len(),
            filesystem_roots,
            network_hosts,
            lifecycle_mutation: self.capabilities.lifecycle_mutation,
        })
    }

    #[must_use]
    pub fn check_when(&self) -> bool {
        let Some(when) = &self.when else {
            return true;
        };
        if let Some(os_list) = &when.os {
            let os = std::env::consts::OS;
            if !os_list
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(os))
            {
                return false;
            }
        }
        if let Some(binaries) = &when.binaries {
            for binary in binaries {
                if !Self::has_binary(binary) {
                    return false;
                }
            }
        }
        true
    }

    fn has_binary(name: &str) -> bool {
        let paths = std::env::var_os("PATH").unwrap_or_default();
        for path in std::env::split_paths(&paths) {
            let candidate = path.join(name);
            if candidate.is_file() {
                return true;
            }
            #[cfg(windows)]
            if candidate.with_extension("exe").is_file() {
                return true;
            }
        }
        false
    }
}

fn safe_toml_parse_error(error: &toml::de::Error) -> String {
    // `Display` includes source excerpts and can echo a malformed literal
    // secret. Byte location is enough to repair the file without copying
    // manifest values into logs, diagnostics, or transcripts.
    error.span().map_or_else(
        || "failed to parse plugin.toml; check the v1 schema and field types".to_string(),
        |span| {
            format!(
                "failed to parse plugin.toml near bytes {}..{}; check the v1 schema and field types",
                span.start, span.end
            )
        },
    )
}

fn read_manifest_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let file = open_bundle_file(path)
        .map_err(|e| format!("failed to open plugin.toml without following links: {e}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("failed to read plugin.toml: {e}"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!(
            "plugin.toml exceeds the {MAX_MANIFEST_BYTES}-byte review limit"
        ));
    }
    Ok(bytes)
}

pub fn validate_plugin_name(name: &str) -> Result<(), String> {
    let count = name.chars().count();
    let valid = count > 0
        && count <= MAX_PLUGIN_NAME_CHARS
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && name
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(format!(
            "plugin name `{name}` must be 1-{MAX_PLUGIN_NAME_CHARS} lowercase ASCII letters, digits, or internal hyphens"
        ))
    }
}

fn validate_component_name(kind: &str, name: &str) -> Result<(), String> {
    let count = name.chars().count();
    let valid = count > 0
        && count <= MAX_PLUGIN_NAME_CHARS
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        && !name.starts_with(['-', '_'])
        && !name.ends_with(['-', '_']);
    if valid {
        Ok(())
    } else {
        Err(format!("{kind} name `{name}` is invalid"))
    }
}

fn validate_optional_text(
    field: &str,
    value: Option<&str>,
    max_chars: usize,
) -> Result<(), String> {
    if let Some(value) = value {
        validate_text(field, value, max_chars)?;
    }
    Ok(())
}

fn validate_mcp_argv_has_no_literal_credentials(
    server_name: &str,
    arguments: &[String],
) -> Result<(), String> {
    for (index, argument) in arguments.iter().enumerate() {
        let (key, assigned_value) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(key, value)| (key, Some(value)));
        if credential_argument_key(key)
            && (assigned_value.is_some_and(|value| !value.is_empty())
                || (assigned_value.is_none() && arguments.get(index + 1).is_some()))
        {
            return Err(format!(
                "MCP server `{server_name}` argument #{} embeds a credential-bearing value; pass credentials through a reviewed environment mapping instead",
                index + 1
            ));
        }
        if looks_like_literal_credential(argument) {
            return Err(format!(
                "MCP server `{server_name}` argument #{} looks like a literal credential; pass credentials through a reviewed environment mapping instead",
                index + 1
            ));
        }
    }
    Ok(())
}

fn credential_argument_key(value: &str) -> bool {
    let key = value
        .trim_start_matches('-')
        .replace('_', "-")
        .to_ascii_lowercase();
    [
        "token",
        "api-key",
        "apikey",
        "password",
        "passwd",
        "secret",
        "client-secret",
        "authorization",
        "auth-token",
        "access-key",
        "private-key",
        "credential",
        "credentials",
    ]
    .iter()
    .any(|sensitive| key == *sensitive || key.ends_with(&format!("-{sensitive}")))
}

fn looks_like_literal_credential(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("sk-")
        || trimmed.starts_with("ghp_")
        || trimmed.starts_with("github_pat_")
        || trimmed.starts_with("xoxb-")
        || trimmed.starts_with("xoxp-")
        || (trimmed.starts_with("AKIA") && trimmed.len() >= 16)
}

fn validate_text(field: &str, value: &str, max_chars: usize) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max_chars {
        return Err(format!(
            "{field} must contain 1-{max_chars} non-whitespace characters"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field} may not contain control characters"));
    }
    if value.chars().any(is_bidi_control) {
        return Err(format!(
            "{field} may not contain bidirectional formatting characters"
        ));
    }
    Ok(())
}

fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

fn validate_unique_texts(field: &str, values: &[String], max_chars: usize) -> Result<(), String> {
    if values.len() > MAX_COMPONENT_PATHS {
        return Err(format!(
            "too many {field} values; maximum is {MAX_COMPONENT_PATHS}"
        ));
    }
    let mut seen = BTreeSet::new();
    for value in values {
        validate_text(field, value, max_chars)?;
        let normalized = value.to_ascii_lowercase();
        if !seen.insert(normalized) {
            return Err(format!("duplicate {field} value `{value}`"));
        }
    }
    Ok(())
}

fn normalize_network_host(host: &str) -> Result<String, String> {
    if host.contains("://") || host.contains('/') || host.contains('\\') {
        return Err(format!(
            "network host `{host}` must be a host name, not a URL or path"
        ));
    }
    let parsed = reqwest::Url::parse(&format!("https://{host}"))
        .map_err(|e| format!("network host `{host}` is invalid: {e}"))?;
    if parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(format!(
            "network host `{host}` must contain only a normalized host name"
        ));
    }
    parsed
        .host_str()
        .map(|host| host.to_ascii_lowercase())
        .ok_or_else(|| format!("network host `{host}` is invalid"))
}

fn validate_bare_executable(command: &str) -> Result<(), String> {
    if command.len() <= 128
        && command
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '+'))
        && !matches!(command, "." | "..")
    {
        Ok(())
    } else {
        Err("MCP command must be a bare executable name or a contained plugin path".to_string())
    }
}

fn validate_environment_name(field: &str, value: &str) -> Result<(), String> {
    validate_text(field, value, 128)?;
    if value
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        Ok(())
    } else {
        Err(format!(
            "{field} must be an ASCII environment variable name"
        ))
    }
}

fn exact_environment_placeholder(value: &str) -> Option<&str> {
    value.strip_prefix("${")?.strip_suffix('}')
}

fn validate_http_header_name(name: &str) -> Result<(), String> {
    validate_text("MCP HTTP header name", name, 128)?;
    reqwest::header::HeaderName::from_bytes(name.as_bytes())
        .map(|_| ())
        .map_err(|_| "MCP HTTP header name is invalid".to_string())
}

fn validate_safe_oauth_resource(resource: &str) -> Result<(), String> {
    validate_text("OAuth resource", resource, 2_048)?;
    let parsed = reqwest::Url::parse(resource)
        .map_err(|_| "OAuth resource must be an absolute HTTPS URL".to_string())?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "OAuth resource must be an HTTPS URL without credentials, query, or fragment"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_nested_mcp_schema(content: &str) -> Result<(), String> {
    const SERVER_FIELDS: &[&str] = &[
        "command",
        "args",
        "env",
        "cwd",
        "url",
        "transport",
        "connect_timeout",
        "execute_timeout",
        "read_timeout",
        "enabled",
        "required",
        "enabled_tools",
        "disabled_tools",
        "headers",
        "env_headers",
        "env_http_headers",
        "bearer_token_env_var",
        "scopes",
        "oauth",
        "oauth_resource",
    ];
    let value: toml::Value =
        toml::from_str(content).map_err(|error| safe_toml_parse_error(&error))?;
    let Some(servers) = value.get("mcp_servers") else {
        return Ok(());
    };
    let servers = servers
        .as_table()
        .ok_or_else(|| "mcp_servers must be a table".to_string())?;
    for server in servers.values() {
        let server = server
            .as_table()
            .ok_or_else(|| "each MCP server must be a table".to_string())?;
        if server
            .keys()
            .any(|field| !SERVER_FIELDS.contains(&field.as_str()))
        {
            return Err("plugin MCP server contains an unsupported field".to_string());
        }
        if let Some(oauth) = server.get("oauth") {
            let oauth = oauth
                .as_table()
                .ok_or_else(|| "plugin MCP oauth must be a table".to_string())?;
            if oauth.keys().any(|field| field != "client_id") {
                return Err("plugin MCP oauth contains an unsupported field".to_string());
            }
        }
    }
    Ok(())
}

fn resolve_spec(
    root: &Path,
    kind: &str,
    spec: Option<&PluginPathSpec>,
    default: Option<&str>,
) -> Result<Vec<PathBuf>, String> {
    let Some(spec) = spec else {
        return Ok(Vec::new());
    };
    spec.declared_paths(default)?
        .iter()
        .map(|path| resolve_contained_path(root, path, kind))
        .collect()
}

fn resolve_contained_path(root: &Path, raw: &str, kind: &str) -> Result<PathBuf, String> {
    validate_text(&format!("{kind} path"), raw, 1_024)?;
    if Path::new(raw).is_absolute() || looks_windows_absolute(raw) {
        return Err(format!("{kind} path must be relative: `{raw}`"));
    }
    if Path::new(raw).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!("{kind} path escapes the plugin root: `{raw}`"));
    }
    let joined = root.join(raw);
    reject_symlink_components(root, &joined, kind)?;
    let canonical = joined
        .canonicalize()
        .map_err(|e| format!("{kind} path `{raw}` cannot be resolved: {e}"))?;
    if !canonical.starts_with(root) {
        return Err(format!("{kind} path escapes the plugin root: `{raw}`"));
    }
    Ok(canonical)
}

fn reject_symlink_components(root: &Path, target: &Path, kind: &str) -> Result<(), String> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| format!("{kind} path is outside the plugin root"))?;
    let mut cursor = root.to_path_buf();
    for component in relative.components() {
        cursor.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&cursor)
            .map_err(|e| format!("failed to inspect {kind} path {}: {e}", cursor.display()))?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err(format!(
                "{kind} path may not traverse symbolic link {}",
                cursor.display()
            ));
        }
    }
    Ok(())
}

fn looks_windows_absolute(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes
        .first()
        .is_some_and(|byte| matches!(*byte, b'\\' | b'/'))
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
}

fn hash_bundle(
    root: &Path,
    manifest_bytes: &[u8],
) -> Result<(String, BTreeMap<PathBuf, String>), String> {
    let mut hasher = Sha256::new();
    // v2 length-frames every variable-length field. The v1 delimiter-only
    // stream was structurally ambiguous across file-record boundaries.
    // Changing the domain invalidates every ambiguous v1 receipt.
    hasher.update(b"codewhale-plugin-content-v2\0plugin.toml\0");
    hasher.update((manifest_bytes.len() as u64).to_le_bytes());
    hasher.update(manifest_bytes);
    let mut budget = HashBudget::default();
    // Hash the complete bundle, not only declared component roots. Local MCP
    // entrypoints and companion assets are security-relevant even when they do
    // not have a separate component table.
    hash_path(root, root, &mut hasher, &mut budget)?;
    Ok((hex_digest(hasher.finalize()), budget.file_hashes))
}

#[derive(Default)]
struct HashBudget {
    files: usize,
    bytes: u64,
    file_hashes: BTreeMap<PathBuf, String>,
}

fn hash_path(
    root: &Path,
    path: &Path,
    hasher: &mut Sha256,
    budget: &mut HashBudget,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("failed to inspect component {}: {e}", path.display()))?;
    if metadata_is_link_or_reparse(&metadata) {
        return Err(format!(
            "component trees may not contain symbolic link {}",
            path.display()
        ));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("component {} is outside the plugin root", path.display()))?;
    hash_permissions(&metadata, hasher);
    if metadata.is_dir() {
        #[cfg(windows)]
        let directory_guard = open_bundle_directory(path)
            .map_err(|e| format!("failed to open component directory safely: {e}"))?;
        #[cfg(windows)]
        ensure_windows_path_still_opened(path, &directory_guard)?;
        hasher.update(b"D\0");
        super::path_identity::hash_os_path(hasher, b"bundle-relative-directory", relative);
        let mut entries = fs::read_dir(path)
            .map_err(|e| format!("failed to read component directory {}: {e}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to read component directory {}: {e}", path.display()))?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            hash_path(root, &entry.path(), hasher, budget)?;
        }
        hasher.update(b"E\0");
        #[cfg(windows)]
        ensure_windows_path_still_opened(path, &directory_guard)?;
    } else if metadata.is_file() {
        budget.files += 1;
        if budget.files > MAX_HASHED_FILES {
            return Err(format!(
                "plugin bundle content exceeds the v0.9.1 review limit ({MAX_HASHED_FILES} files / {MAX_HASHED_BYTES} bytes)"
            ));
        }
        hasher.update(b"F\0");
        super::path_identity::hash_os_path(hasher, b"bundle-relative-file", relative);
        let mut file = open_bundle_file(path)
            .map_err(|e| format!("failed to read component file {}: {e}", path.display()))?;
        #[cfg(windows)]
        ensure_windows_path_still_opened(path, &file)?;
        let expected_len = file
            .metadata()
            .map_err(|e| format!("failed to inspect component file {}: {e}", path.display()))?
            .len();
        hasher.update(expected_len.to_le_bytes());
        let mut file_hasher = Sha256::new();
        file_hasher.update(b"codewhale-plugin-file-bytes-v1\0");
        // Keep the read buffer off the stack. `hash_path` is recursive and the
        // fixed-size array inflated every directory frame, which could exhaust
        // a Tokio worker stack while revalidating a nested plugin bundle.
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut actual_len = 0_u64;
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|e| format!("failed to read component file {}: {e}", path.display()))?;
            if read == 0 {
                break;
            }
            actual_len = actual_len.saturating_add(read as u64);
            budget.bytes = budget.bytes.saturating_add(read as u64);
            if budget.bytes > MAX_HASHED_BYTES {
                return Err(format!(
                    "plugin bundle content exceeds the v0.9.1 review limit ({MAX_HASHED_FILES} files / {MAX_HASHED_BYTES} bytes)"
                ));
            }
            hasher.update(&buffer[..read]);
            file_hasher.update(&buffer[..read]);
        }
        if actual_len != expected_len {
            return Err(format!(
                "component file {} changed length while being reviewed",
                path.display()
            ));
        }
        budget
            .file_hashes
            .insert(relative.to_path_buf(), hex_digest(file_hasher.finalize()));
        #[cfg(windows)]
        ensure_windows_path_still_opened(path, &file)?;
    } else {
        return Err(format!(
            "component {} is neither a regular file nor directory",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn hash_permissions(metadata: &fs::Metadata, hasher: &mut Sha256) {
    use std::os::unix::fs::PermissionsExt;

    // Runtime snapshots deliberately remove group/other access and write bits.
    // Bind identity only to whether a regular file is executable, so the
    // owner-only staged representation has the same reviewed content hash.
    hasher.update(b"unix-executable\0");
    hasher.update([u8::from(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
    )]);
}

#[cfg(not(unix))]
fn hash_permissions(metadata: &fs::Metadata, hasher: &mut Sha256) {
    let _ = metadata;
    // Windows staging marks files read-only as a defense-in-depth hardening
    // step; that representation change is not plugin content identity.
    hasher.update(b"portable-mode\0");
}

#[cfg(unix)]
pub(crate) fn open_bundle_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(windows)]
pub(crate) fn open_bundle_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001) // deny concurrent writes and replacement
        .custom_flags(0x0020_0000) // FILE_FLAG_OPEN_REPARSE_POINT
        .open(path)?;
    let metadata = file.metadata()?;
    let identity = windows_file_identity(&file)?;
    if !metadata.is_file()
        || identity.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || identity.links != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "plugin file is a reparse point, hard link, or non-regular file",
        ));
    }
    Ok(file)
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) fn open_bundle_file(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

#[cfg(windows)]
fn open_bundle_directory(path: &Path) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001)
        .custom_flags(0x0220_0000) // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
        .open(path)?;
    let metadata = file.metadata()?;
    let identity = windows_file_identity(&file)?;
    if !metadata.is_dir() || identity.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "plugin directory is a reparse point or non-directory",
        ));
    }
    Ok(file)
}

/// Reopen a reviewed path only to compare its current handle identity with a
/// retained authority handle. Desired access is zero and sharing is permissive
/// because the retained handle remains responsible for denying writes and
/// replacement throughout the comparison.
#[cfg(windows)]
pub(crate) fn open_bundle_identity_probe(
    path: &Path,
    expect_directory: bool,
) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ_WRITE_DELETE: u32 = 0x0000_0007;
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if expect_directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    let file = fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ_WRITE_DELETE)
        .custom_flags(flags)
        .open(path)?;
    let metadata = file.metadata()?;
    let identity = windows_file_identity(&file)?;
    let expected_kind = if expect_directory {
        metadata.is_dir()
    } else {
        metadata.is_file() && identity.links == 1
    };
    if !expected_kind || identity.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "plugin identity probe found a reparse point, hard link, or unexpected object",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn ensure_windows_path_still_opened(path: &Path, opened: &fs::File) -> Result<(), String> {
    let after = fs::symlink_metadata(path)
        .map_err(|e| format!("failed to re-inspect plugin path after handle open: {e}"))?;
    if metadata_is_link_or_reparse(&after) {
        return Err("plugin path changed into a reparse point during validation".to_string());
    }
    let expect_directory = if after.is_dir() {
        true
    } else if after.is_file() {
        false
    } else {
        return Err("plugin path changed into an unsupported object during validation".to_string());
    };
    let current = open_bundle_identity_probe(path, expect_directory)
        .map_err(|e| format!("failed to reopen plugin path for identity validation: {e}"))?;
    let opened = windows_file_identity(opened)
        .map_err(|e| format!("failed to identify retained plugin handle: {e}"))?;
    let current = windows_file_identity(&current)
        .map_err(|e| format!("failed to identify current plugin path: {e}"))?;
    if opened.volume != current.volume || opened.index != current.index {
        return Err("plugin path identity changed between handle open and validation".to_string());
    }
    Ok(())
}

fn hash_inventory(inventory: &PluginInventory) -> String {
    let mut normalized = BTreeMap::new();
    normalized.insert("skills", inventory.skills.to_string());
    normalized.insert("mcp", inventory.mcp_servers.to_string());
    normalized.insert("mcp-stdio", inventory.stdio_mcp_servers.to_string());
    normalized.insert("mcp-remote", inventory.remote_mcp_servers.to_string());
    normalized.insert("commands", inventory.commands.to_string());
    normalized.insert("agents", inventory.agents.to_string());
    normalized.insert("hooks", inventory.hooks.to_string());
    normalized.insert("lsp", inventory.lsp.to_string());
    normalized.insert("native", inventory.native.to_string());
    normalized.insert("filesystem", inventory.filesystem_roots.join("\n"));
    normalized.insert("network", inventory.network_hosts.join("\n"));
    normalized.insert("lifecycle", inventory.lifecycle_mutation.to_string());
    let mut hasher = Sha256::new();
    hasher.update(b"codewhale-plugin-capabilities-v1\0");
    for (key, value) in normalized {
        hasher.update(key.as_bytes());
        hasher.update(b"\0");
        hasher.update(value.as_bytes());
        hasher.update(b"\0");
    }
    hex_digest(hasher.finalize())
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(root: &Path, extra: &str) -> PathBuf {
        fs::create_dir_all(root.join("skills/example")).unwrap();
        fs::write(
            root.join("skills/example/SKILL.md"),
            "---\nname: example\ndescription: example\n---\nbody\n",
        )
        .unwrap();
        let path = root.join("plugin.toml");
        fs::write(
            &path,
            format!(
                "schema_version = 1\n[plugin]\nname = \"example-plugin\"\nversion = \"1.2.3\"\n[skills]\npath = \"skills\"\n{extra}"
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn validates_versioned_manifest_and_hashes_declared_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(tmp.path(), "");
        let first = PluginManifest::validate_from_path(&path).unwrap();
        assert_eq!(first.inventory.skills, 1);
        assert!(first.warnings.is_empty());

        fs::write(
            tmp.path().join("skills/example/SKILL.md"),
            "---\nname: example\ndescription: changed\n---\nbody\n",
        )
        .unwrap();
        let second = PluginManifest::validate_from_path(&path).unwrap();
        assert_ne!(first.content_hash, second.content_hash);
        assert_eq!(first.capability_hash, second.capability_hash);
    }

    #[test]
    fn validates_deep_bundle_on_small_stack() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = write_manifest(tmp.path(), "");
        let mut nested = tmp.path().join("nested");
        for level in 0..8 {
            nested = nested.join(format!("level-{level}"));
        }
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("payload.txt"), "nested bundle payload").unwrap();

        let worker = std::thread::Builder::new()
            .name("plugin-small-stack-validation".to_string())
            .stack_size(512 * 1024)
            .spawn(move || PluginManifest::validate_from_path(&manifest))
            .unwrap();
        let validated = worker
            .join()
            .expect("nested plugin validation must not overflow a small stack")
            .expect("nested plugin bundle must validate");
        assert_eq!(validated.inventory.skills, 1);
    }

    #[test]
    fn bundle_hash_is_deterministic_and_covers_undeclared_companion_files() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let left_manifest = write_manifest(left.path(), "");
        let right_manifest = write_manifest(right.path(), "");
        fs::write(left.path().join("z.txt"), "z").unwrap();
        fs::write(left.path().join("a.txt"), "a").unwrap();
        fs::write(right.path().join("a.txt"), "a").unwrap();
        fs::write(right.path().join("z.txt"), "z").unwrap();

        let left_hash = PluginManifest::validate_from_path(&left_manifest).unwrap();
        let right_hash = PluginManifest::validate_from_path(&right_manifest).unwrap();
        assert_eq!(left_hash.content_hash, right_hash.content_hash);
        assert_eq!(left_hash.capability_hash, right_hash.capability_hash);

        fs::write(right.path().join("z.txt"), "changed").unwrap();
        let changed = PluginManifest::validate_from_path(&right_manifest).unwrap();
        assert_ne!(right_hash.content_hash, changed.content_hash);
        assert_eq!(right_hash.capability_hash, changed.capability_hash);
    }

    #[cfg(unix)]
    #[test]
    fn bundle_hash_frames_adversarial_binary_records() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let manifest = b"schema_version = 1\n[plugin]\nname = \"framing\"\nversion = \"1.0.0\"\n";
        for root in [left.path(), right.path()] {
            fs::write(root.join("plugin.toml"), manifest).unwrap();
        }

        fs::write(left.path().join("a.bin"), b"alpha").unwrap();
        fs::write(left.path().join("b.bin"), b"omega").unwrap();

        let mut adversarial = b"alpha\0unix-executable\0\0F\0codewhale-os-path-v1\0".to_vec();
        adversarial.extend_from_slice(&(b"bundle-relative-file".len() as u64).to_le_bytes());
        adversarial.extend_from_slice(b"bundle-relative-file");
        adversarial.extend_from_slice(b"unix-bytes\0");
        adversarial.extend_from_slice(&(b"b.bin".len() as u64).to_le_bytes());
        adversarial.extend_from_slice(b"b.bin");
        adversarial.extend_from_slice(b"omega");
        fs::write(right.path().join("a.bin"), adversarial).unwrap();

        let left = PluginManifest::validate_from_path(&left.path().join("plugin.toml")).unwrap();
        let right = PluginManifest::validate_from_path(&right.path().join("plugin.toml")).unwrap();
        assert_ne!(left.content_hash, right.content_hash);
        assert_eq!(left.capability_hash, right.capability_hash);
    }

    // Darwin rejects these malformed bytes at the filesystem boundary. The
    // platform-independent native-path framing is covered in path_identity;
    // run this full bundle-walk regression where Unix permits the entries.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn bundle_hash_distinguishes_lossy_colliding_native_file_names() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let left_manifest = write_manifest(left.path(), "");
        let right_manifest = write_manifest(right.path(), "");
        let left_name = OsString::from_vec(vec![b'a', 0xff]);
        let right_name = OsString::from_vec(vec![b'a', 0xfe]);
        assert_eq!(left_name.to_string_lossy(), right_name.to_string_lossy());
        fs::write(left.path().join(left_name), "same bytes").unwrap();
        fs::write(right.path().join(right_name), "same bytes").unwrap();

        let left = PluginManifest::validate_from_path(&left_manifest).unwrap();
        let right = PluginManifest::validate_from_path(&right_manifest).unwrap();
        assert_ne!(left.content_hash, right.content_hash);
    }

    #[test]
    fn legacy_manifest_is_accepted_with_migration_warning() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("plugin.toml"),
            "[plugin]\nname = \"legacy\"\n",
        )
        .unwrap();
        let validated =
            PluginManifest::validate_from_path(&tmp.path().join("plugin.toml")).unwrap();
        assert_eq!(validated.manifest.schema_version, 0);
        assert_eq!(validated.manifest.plugin.version, "0.0.0");
        assert_eq!(validated.warnings.len(), 2);
    }

    #[test]
    fn rejects_unknown_fields_invalid_names_and_versions() {
        let invalid = [
            "schema_version = 1\nunknown = true\n[plugin]\nname = \"ok\"\nversion = \"1.0.0\"\n",
            "schema_version = 1\n[plugin]\nname = \"Bad_Name\"\nversion = \"1.0.0\"\n",
            "schema_version = 1\n[plugin]\nname = \"ok\"\nversion = \"latest\"\n",
            "schema_version = 1\n[plugin]\nname = \"ok\"\n",
        ];
        for source in invalid {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("plugin.toml");
            fs::write(&path, source).unwrap();
            assert!(PluginManifest::validate_from_path(&path).is_err());
        }
    }

    #[test]
    fn parse_diagnostics_do_not_echo_manifest_values() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plugin.toml");
        fs::write(
            &path,
            "schema_version = 1\n[plugin]\nname = \"safe\"\nversion = \"1.0.0\"\ndescription = [\"sk-sensitive-value\"]\n",
        )
        .unwrap();
        let error = PluginManifest::validate_from_path(&path).unwrap_err();
        assert!(!error.contains("sk-sensitive-value"));
    }

    #[test]
    fn rejects_parent_absolute_and_windows_absolute_component_paths() {
        for bad in [
            "../escape",
            "/tmp/escape",
            r"C:\\escape",
            r"\\\\server\\share",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let path = write_manifest(tmp.path(), &format!("\n[commands]\npath = {bad:?}\n"));
            assert!(
                PluginManifest::validate_from_path(&path).is_err(),
                "accepted {bad}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_component_and_nested_symlink() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("SKILL.md"), "# outside").unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(tmp.path(), "");
        fs::remove_dir_all(tmp.path().join("skills")).unwrap();
        symlink(outside.path(), tmp.path().join("skills")).unwrap();
        assert!(PluginManifest::validate_from_path(&path).is_err());

        fs::remove_file(tmp.path().join("skills")).unwrap();
        fs::create_dir_all(tmp.path().join("skills/example")).unwrap();
        fs::write(tmp.path().join("skills/example/SKILL.md"), "# safe").unwrap();
        symlink(
            outside.path().join("SKILL.md"),
            tmp.path().join("skills/example/linked.md"),
        )
        .unwrap();
        assert!(PluginManifest::validate_from_path(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_manifest() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.toml");
        fs::write(
            &real,
            "schema_version = 1\n[plugin]\nname = \"linked\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let linked = tmp.path().join("plugin.toml");
        symlink(&real, &linked).unwrap();

        assert!(PluginManifest::validate_from_path(&linked).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_bundle_root() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let real_root = tmp.path().join("real");
        fs::create_dir(&real_root).unwrap();
        fs::write(
            real_root.join("plugin.toml"),
            "schema_version = 1\n[plugin]\nname = \"linked-root\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let linked_root = tmp.path().join("linked");
        symlink(&real_root, &linked_root).unwrap();

        assert!(PluginManifest::validate_from_path(&linked_root.join("plugin.toml")).is_err());
    }

    #[test]
    fn rejects_absolute_mcp_arguments_and_embedded_url_credentials() {
        let absolute = tempfile::tempdir().unwrap();
        let absolute_path = write_manifest(
            absolute.path(),
            "\n[mcp_servers.local]\ncommand = \"node\"\nargs = [\"/tmp/server.js\"]\n",
        );
        assert!(PluginManifest::validate_from_path(&absolute_path).is_err());

        let credentialed = tempfile::tempdir().unwrap();
        let credentialed_path = write_manifest(
            credentialed.path(),
            "\n[mcp_servers.remote]\nurl = \"https://user:secret@example.invalid/mcp\"\n",
        );
        assert!(PluginManifest::validate_from_path(&credentialed_path).is_err());
    }

    #[test]
    fn windows_rooted_path_detection_is_host_independent() {
        assert!(looks_windows_absolute(r"C:\plugins\server.js"));
        assert!(looks_windows_absolute(r"C:/plugins/server.js"));
        assert!(looks_windows_absolute(r"\plugins\server.js"));
        assert!(looks_windows_absolute("/plugins/server.js"));
        assert!(!looks_windows_absolute("plugins/server.js"));
        assert!(!looks_windows_absolute("server.js"));
    }

    #[cfg(windows)]
    #[test]
    fn retained_bundle_handle_allows_identity_revalidation_but_denies_writes() {
        use std::os::windows::fs::OpenOptionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reviewed.bin");
        fs::write(&path, b"reviewed").unwrap();
        let retained = open_bundle_file(&path).unwrap();

        ensure_windows_path_still_opened(&path, &retained).unwrap();
        let denied = fs::OpenOptions::new()
            .write(true)
            .share_mode(0x0000_0007)
            .open(&path);
        assert!(denied.is_err(), "retained authority must deny mutation");

        drop(retained);
        fs::OpenOptions::new().write(true).open(&path).unwrap();
    }

    #[test]
    fn unsupported_capabilities_are_inventoried() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("hooks")).unwrap();
        let path = write_manifest(
            tmp.path(),
            "\n[hooks]\npath = \"hooks\"\n[capabilities]\nfilesystem_roots = [\"workspace\"]\nlifecycle_mutation = true\n",
        );
        let validated = PluginManifest::validate_from_path(&path).unwrap();
        assert!(validated.inventory.has_unsupported_capabilities());
        assert!(validated.inventory.unsupported_labels().contains(&"hooks"));
        assert!(
            validated
                .inventory
                .unsupported_labels()
                .contains(&"filesystem-roots")
        );
    }

    #[test]
    fn plugin_mcp_schema_and_transport_combinations_fail_closed() {
        let invalid = [
            "\n[mcp_servers.remote]\nurl = \"https://example.invalid/mcp\"\nunknown_nested = true\n[capabilities]\nnetwork_hosts = [\"example.invalid\"]\n",
            "\n[mcp_servers.remote]\nurl = \"https://example.invalid/mcp\"\nargs = [\"secret\"]\n[capabilities]\nnetwork_hosts = [\"example.invalid\"]\n",
            "\n[mcp_servers.remote]\nurl = \"https://example.invalid/mcp?token=secret\"\n[capabilities]\nnetwork_hosts = [\"example.invalid\"]\n",
            "\n[mcp_servers.remote]\nurl = \"http://example.invalid/mcp\"\n[capabilities]\nnetwork_hosts = [\"example.invalid\"]\n",
            "\n[mcp_servers.remote]\nurl = \"https://example.invalid/mcp\"\n[capabilities]\nnetwork_hosts = [\"other.invalid\"]\n",
            "\n[mcp_servers.local]\ncommand = \"node\"\ntransport = \"sse\"\n",
            "\n[mcp_servers.local]\ncommand = \"node\"\nconnect_timeout = 0\n",
            "\n[mcp_servers.local]\ncommand = \"node\"\nenabled_tools = [\"same\"]\ndisabled_tools = [\"same\"]\n",
            "\n[mcp_servers.local]\ncommand = \"node\"\n[mcp_servers.local.env]\nTOKEN = \"literal-secret\"\n",
            "\n[mcp_servers.remote]\nurl = \"https://example.invalid/mcp\"\n[mcp_servers.remote.headers]\nAuthorization = \"literal-secret\"\n[capabilities]\nnetwork_hosts = [\"example.invalid\"]\n",
            "\n[mcp_servers.remote]\nurl = \"https://example.invalid/mcp\"\n[mcp_servers.remote.oauth]\nclient_id = \"public\"\nsecret = \"must-not-parse\"\n[capabilities]\nnetwork_hosts = [\"example.invalid\"]\n",
        ];
        for extra in invalid {
            let tmp = tempfile::tempdir().unwrap();
            let path = write_manifest(tmp.path(), extra);
            assert!(
                PluginManifest::validate_from_path(&path).is_err(),
                "accepted invalid plugin MCP manifest: {extra}"
            );
        }
    }

    #[test]
    fn plugin_mcp_remote_allowlist_and_env_provenance_are_exact() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"
[mcp_servers.remote]
url = "https://Example.Invalid:8443/mcp/v1"
transport = "sse"
connect_timeout = 30
execute_timeout = 120
read_timeout = 180
required = true
enabled_tools = ["read"]
disabled_tools = ["write"]
bearer_token_env_var = "PLUGIN_BEARER"

[mcp_servers.remote.env_headers]
X_Api_Key = "PLUGIN_API_KEY"

[capabilities]
network_hosts = ["example.invalid"]
"#,
        );
        let validated = PluginManifest::validate_from_path(&path).unwrap();
        assert_eq!(
            validated.inventory.network_hosts,
            vec!["example.invalid".to_string()]
        );
        assert_eq!(validated.inventory.remote_mcp_servers, 1);
    }

    #[test]
    fn plugin_mcp_oauth_fields_are_rejected_for_v091() {
        for oauth_fields in [
            "scopes = [\"tools.read\"]\n",
            "oauth_resource = \"https://resource.invalid/mcp\"\n",
            "[mcp_servers.remote.oauth]\nclient_id = \"public-client-id\"\n",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let path = write_manifest(
                tmp.path(),
                &format!(
                    "\n[mcp_servers.remote]\nurl = \"https://example.invalid/mcp\"\n{oauth_fields}[capabilities]\nnetwork_hosts = [\"example.invalid\"]\n"
                ),
            );
            let error = PluginManifest::validate_from_path(&path)
                .expect_err("plugin OAuth authority must remain disabled in v0.9.1");
            assert!(error.contains("plugin OAuth is disabled in v0.9.1"));
        }
    }

    #[test]
    fn reviewed_stdio_argv_rejects_literal_credentials_but_accepts_exact_safe_values() {
        for args in [
            r#"["server.js", "--token", "literal-secret"]"#,
            r#"["server.js", "--api-key=literal-secret"]"#,
            r#"["server.js", "sk-live-literal"]"#,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            fs::write(tmp.path().join("server.js"), "// entrypoint\n").unwrap();
            let path = write_manifest(
                tmp.path(),
                &format!("\n[mcp_servers.local]\ncommand = \"node\"\nargs = {args}\n"),
            );
            let error = PluginManifest::validate_from_path(&path)
                .expect_err("credential-bearing argv must fail closed");
            assert!(error.contains("credential"), "{error}");
        }

        let safe = tempfile::tempdir().unwrap();
        fs::write(safe.path().join("server.js"), "// entrypoint\n").unwrap();
        let path = write_manifest(
            safe.path(),
            r#"
[mcp_servers.local]
command = "node"
args = ["server.js", "--mode=worker", "-e", "console.log('ready')"]
"#,
        );
        PluginManifest::validate_from_path(&path)
            .expect("safe interpreter argv should remain reviewable exactly");
    }

    #[test]
    fn manifest_text_rejects_controls_and_bidirectional_spoofing() {
        for unsafe_text in ["line\nbreak", "safe\u{202e}lmot.nigulp"] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("plugin.toml");
            fs::write(
                &path,
                format!(
                    "schema_version = 1\n[plugin]\nname = \"safe\"\nversion = \"1.0.0\"\nauthor = {unsafe_text:?}\n"
                ),
            )
            .unwrap();
            assert!(PluginManifest::validate_from_path(&path).is_err());
        }
    }
}
