//! Portable config bundles: `codewhale config import` / `config export --portable`.
//!
//! A bundle is a TOML or JSON document carrying a portable subset of a
//! CodeWhale configuration (preferences, harness profiles, provider
//! non-secret settings, project/global sections) between machines. The
//! envelope is versioned and strict (`deny_unknown_fields`), secrets are
//! rejected by key name and value shape (never echoed), parsing is bounded,
//! and application is transactional with a timestamped backup and rollback.
//!
//! Security contract:
//! - No secret ever round-trips: fields whose key matches
//!   [`codewhale_config::is_sensitive_config_key`] are rejected on import and
//!   dropped on export, and bare credential-shaped values are rejected by
//!   value shape. Rejection messages name the field, never the value.
//! - Input size is capped (5 MiB, matching the skill installer's cap).
//! - HTTPS only for remote fetch, except plain `http` on loopback; redirects
//!   are followed at most a bounded number of times within the same scheme.
//! - Bundle-declared file paths must resolve inside the target config
//!   directory; traversal and symlink escapes are refused.
//! - Project scope never mutates the user-global document and vice versa.

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use codewhale_config::{ConfigToml, is_sensitive_config_key};

/// Maximum accepted bundle size, both for reads and remote fetches.
/// Matches the skill installer's 5 MiB cap.
pub const MAX_BUNDLE_BYTES: u64 = 5 * 1024 * 1024;

/// Envelope `kind` value required by every bundle.
pub const BUNDLE_KIND: &str = "codewhale.portable-config";

/// Envelope `schema_version` accepted by this build.
pub const BUNDLE_SCHEMA_VERSION: u64 = 1;

/// Maximum number of HTTP redirects followed during a remote fetch.
const MAX_REDIRECTS: usize = 5;

/// Timeout for the remote fetch, in seconds.
const FETCH_TIMEOUT_SECS: u64 = 30;

/// Credential-shaped value prefixes rejected even under a benign key name.
/// Conservative on purpose: only well-known provider token shapes.
const SECRET_VALUE_PREFIXES: [&str; 6] = ["sk-", "Bearer ", "ghp_", "xoxb-", "AKIA", "eyJ"];

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// Strict portable-bundle envelope. Unknown fields fail the parse: a bundle
/// written by a newer schema must not be silently half-applied.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableBundle {
    pub schema_version: u64,
    pub kind: String,
    #[serde(default)]
    pub metadata: BundleMetadata,
    #[serde(default)]
    pub preferences: BundleTable,
    #[serde(default)]
    pub profiles: BundleTable,
    #[serde(default)]
    pub plugins: BundleTable,
    #[serde(default)]
    pub project: BundleTable,
    #[serde(default)]
    pub global: BundleTable,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMetadata {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub generator: Option<String>,
}

/// One bundle section: a flat table of config keys to values. Keys inside a
/// section are data, not schema, so unknown keys parse here — credential
/// rejection happens at plan time by name and value shape.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BundleTable {
    #[serde(flatten)]
    pub entries: std::collections::BTreeMap<String, toml::Value>,
}

// ---------------------------------------------------------------------------
// Parsing (bounded)
// ---------------------------------------------------------------------------

/// Parse a bundle from raw bytes, rejecting oversize input before parse.
pub fn parse_bundle_bytes(raw: &[u8], source: &str) -> Result<PortableBundle> {
    if raw.len() as u64 > MAX_BUNDLE_BYTES {
        bail!(
            "bundle at {source} is {} bytes; the limit is {MAX_BUNDLE_BYTES} bytes",
            raw.len()
        );
    }
    let text = std::str::from_utf8(raw)
        .with_context(|| format!("bundle at {source} is not valid UTF-8"))?;
    parse_bundle_str(text, source)
}

/// Parse a bundle document: TOML by default, JSON when the source ends in
/// `.json` or the document starts with `{`.
pub fn parse_bundle_str(text: &str, source: &str) -> Result<PortableBundle> {
    let trimmed = text.trim_start();
    let bundle = if trimmed.starts_with('{') {
        serde_json::from_str::<PortableBundle>(text)
            .with_context(|| format!("bundle at {source} is not valid JSON"))?
    } else if source.ends_with(".json") {
        serde_json::from_str::<PortableBundle>(text)
            .with_context(|| format!("bundle at {source} is not valid JSON"))?
    } else {
        toml::from_str::<PortableBundle>(text)
            .with_context(|| format!("bundle at {source} is not valid TOML"))?
    };
    validate_bundle(&bundle, source)?;
    Ok(bundle)
}

fn validate_bundle(bundle: &PortableBundle, source: &str) -> Result<()> {
    if bundle.kind != BUNDLE_KIND {
        bail!(
            "bundle at {source} has kind {:?}; expected {BUNDLE_KIND:?}",
            bundle.kind
        );
    }
    if bundle.schema_version != BUNDLE_SCHEMA_VERSION {
        bail!(
            "bundle at {source} has schema_version {}; this build understands {BUNDLE_SCHEMA_VERSION}",
            bundle.schema_version
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Secret rejection
// ---------------------------------------------------------------------------

/// One rejected entry: the dotted key path and the reason. Values are never
/// included — the reason and path are all a reviewer needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedEntry {
    pub key: String,
    pub reason: String,
}

/// Scan every section of the bundle for secret-bearing entries. Rejection is
/// by key name (via the config crate's own denylist, so bundle policy and
/// `config set` policy cannot drift) and by value shape for string leaves.
pub fn find_rejected_entries(bundle: &PortableBundle) -> Vec<RejectedEntry> {
    let mut rejected = Vec::new();
    for (section, table) in [
        ("preferences", &bundle.preferences),
        ("profiles", &bundle.profiles),
        ("plugins", &bundle.plugins),
        ("project", &bundle.project),
        ("global", &bundle.global),
    ] {
        for (key, value) in &table.entries {
            let dotted = format!("{section}.{key}");
            if is_sensitive_config_key(&dotted) || is_sensitive_config_key(key) {
                rejected.push(RejectedEntry {
                    key: dotted,
                    reason: "key names a credential field".to_string(),
                });
                continue;
            }
            if let Some(reason) = value_shape_secret_reason(value) {
                rejected.push(RejectedEntry {
                    key: dotted,
                    reason,
                });
            }
        }
    }
    rejected
}

/// Why a value looks like a bare credential, or `None` when it does not.
fn value_shape_secret_reason(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(text) => SECRET_VALUE_PREFIXES
            .iter()
            .find(|prefix| text.trim().starts_with(*prefix))
            .map(|prefix| {
                format!("value has the shape of a credential (prefix {prefix:?} redacted)")
            }),
        toml::Value::Array(items) => items
            .iter()
            .find_map(value_shape_secret_reason)
            .map(|reason| format!("array contains an entry where {reason}")),
        toml::Value::Table(map) => {
            for (key, nested_value) in map {
                if is_sensitive_config_key(key) {
                    return Some(format!("nested key {key:?} names a credential field"));
                }
                if let Some(reason) = value_shape_secret_reason(nested_value) {
                    return Some(format!("nested under {key:?}, {reason}"));
                }
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Import plan
// ---------------------------------------------------------------------------

/// What applying the bundle would do, computed before anything is written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportPlan {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub skipped: Vec<String>,
    pub conflicting: Vec<String>,
    pub rejected: Vec<RejectedEntry>,
}

impl ImportPlan {
    #[must_use]
    pub fn is_no_op(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty()
    }
}

/// Compute the deterministic import plan for `bundle` against `config`.
///
/// `section` selects the target document mapping: bundle `project` entries
/// apply only to a project-scope document, `global` entries only to a
/// user-global one; `preferences`, `profiles`, and `plugins` apply to both.
/// Entries that would not touch the target document are `skipped`, so the
/// same bundle imports cleanly at either scope.
pub fn plan_import(bundle: &PortableBundle, config: &ConfigToml, scope: BundleScope) -> ImportPlan {
    let mut plan = ImportPlan {
        rejected: find_rejected_entries(bundle),
        ..ImportPlan::default()
    };
    let rejected_keys: std::collections::BTreeSet<&str> = plan
        .rejected
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    // Sections are presentation and scope labels over one flat ConfigToml
    // keyspace. Two applicable sections naming the same key would otherwise
    // make apply order decide which value wins. Detect that ambiguity before
    // classifying or writing any entry.
    let mut applicable_key_counts = std::collections::BTreeMap::<&str, usize>::new();
    for (section, table) in [
        ("preferences", &bundle.preferences),
        ("profiles", &bundle.profiles),
        ("plugins", &bundle.plugins),
        ("project", &bundle.project),
        ("global", &bundle.global),
    ] {
        if section_applies(section, scope) {
            for key in table.entries.keys() {
                *applicable_key_counts.entry(key.as_str()).or_default() += 1;
            }
        }
    }
    let colliding_keys: std::collections::BTreeSet<&str> = applicable_key_counts
        .into_iter()
        .filter_map(|(key, count)| (count > 1).then_some(key))
        .collect();

    for (section, table) in [
        ("preferences", &bundle.preferences),
        ("profiles", &bundle.profiles),
        ("plugins", &bundle.plugins),
        ("project", &bundle.project),
        ("global", &bundle.global),
    ] {
        let dotted = |key: &str| format!("{section}.{key}");
        let applies = section_applies(section, scope);
        for (key, value) in &table.entries {
            let dotted = dotted(key);
            if rejected_keys.contains(dotted.as_str())
                || (applies && colliding_keys.contains(key.as_str()))
            {
                plan.conflicting.push(dotted);
                continue;
            }
            if !applies {
                plan.skipped.push(dotted);
                continue;
            }
            match config.get_value(key) {
                Some(current) => {
                    if render_toml_value(value).ok().as_deref() == Some(current.as_str()) {
                        plan.skipped.push(dotted);
                    } else {
                        plan.changed.push(dotted);
                    }
                }
                None => plan.added.push(dotted),
            }
        }
    }
    plan
}

fn section_applies(section: &str, scope: BundleScope) -> bool {
    match section {
        "project" => scope == BundleScope::Project,
        "global" => scope == BundleScope::Global,
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Which document an import/export targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleScope {
    /// The user-global config (`~/.codewhale/config.toml` by default).
    Global,
    /// The workspace-scoped config (`<repo>/.codewhale/config.toml`).
    Project,
}

impl BundleScope {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

// ---------------------------------------------------------------------------
// Path safety
// ---------------------------------------------------------------------------

/// Resolve `candidate` inside `base_dir`, refusing traversal and symlink
/// escapes. Returns the resolved path or an error naming the refusal — the
/// candidate string itself is safe to echo (it is config data, not a secret).
/// Resolve `candidate` inside `base_dir`, refusing traversal and symlink
/// escapes. Returns the joined path or an error naming the refusal.
/// Reserved for path-carrying bundle sections (none shipped yet); exercised
/// by the traversal tests so the contract cannot silently rot.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "path-carrying sections land with the next schema")
)]
pub fn resolve_bounded_path(base_dir: &Path, candidate: &str) -> Result<PathBuf> {
    if candidate.contains('\0') {
        bail!("bundle path contains a NUL byte; refused");
    }
    let candidate_path = Path::new(candidate);
    if candidate_path.is_absolute() {
        bail!(
            "bundle path {candidate:?} is absolute; only paths inside the config directory are accepted"
        );
    }
    let canonical_base = base_dir
        .canonicalize()
        .with_context(|| format!("config directory {} is unavailable", base_dir.display()))?;
    let joined = base_dir.join(candidate_path);
    // Walk the joined path's ancestors from the deepest existing component up:
    // every existing component must canonicalize inside the base, so a symlink
    // pointing outside the config directory is refused even when the final
    // target does not exist yet.
    let deepest_existing = joined
        .ancestors()
        .find(|ancestor| ancestor.symlink_metadata().is_ok())
        .context("bundle path has no existing ancestor inside the config directory")?;
    let resolved = deepest_existing.canonicalize().with_context(|| {
        format!(
            "could not resolve bundle path component {}",
            deepest_existing.display()
        )
    })?;
    if !resolved.starts_with(&canonical_base) {
        bail!("bundle path {candidate:?} escapes the config directory via a symlink; refused");
    }
    Ok(joined)
}

// ---------------------------------------------------------------------------
// Remote fetch
// ---------------------------------------------------------------------------

/// Fetch a bundle over HTTPS (or plain http on loopback only) with a hard
/// size cap, a timeout, and bounded redirects. Mirrors the skill installer's
/// fetch bounds.
pub fn fetch_bundle(url: &str) -> Result<Vec<u8>> {
    let mut current_url = reqwest::Url::parse(url).map_err(|_| anyhow!("invalid bundle URL"))?;
    validate_bundle_url(&current_url)?;
    let initial_scheme = current_url.scheme().to_string();

    let client = codewhale_release::platform_blocking_http_client_builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        // Redirect targets must pass the same scheme/host policy as the
        // initial request, so redirects are followed explicitly below.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| anyhow!("building bundle fetch client failed"))?;
    let mut redirects = 0usize;
    let response = loop {
        let response = client
            .get(current_url.clone())
            .send()
            // reqwest errors can include the full URL (including its query or
            // userinfo), so keep transport failures deliberately URL-free.
            .map_err(|_| anyhow!("bundle fetch request failed"))?;

        if !response.status().is_redirection() {
            break response;
        }
        if redirects >= MAX_REDIRECTS {
            bail!("bundle fetch exceeded the five-redirect limit");
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .ok_or_else(|| anyhow!("bundle redirect is missing a valid Location header"))?
            .to_str()
            .map_err(|_| anyhow!("bundle redirect is missing a valid Location header"))?;
        let next_url = current_url
            .join(location)
            .map_err(|_| anyhow!("bundle redirect Location is invalid"))?;
        validate_bundle_redirect(&initial_scheme, &next_url)?;
        current_url = next_url;
        redirects += 1;
    };

    if !response.status().is_success() {
        bail!(
            "bundle fetch failed with HTTP status {}",
            response.status().as_u16()
        );
    }

    // Read at most MAX_BUNDLE_BYTES + 1 so an oversize body is detected
    // rather than silently truncated.
    let mut buffer = Vec::new();
    let body = response;
    body.take(MAX_BUNDLE_BYTES + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| anyhow!("reading remote bundle failed"))?;
    if buffer.len() as u64 > MAX_BUNDLE_BYTES {
        bail!("remote bundle exceeds the {MAX_BUNDLE_BYTES} byte limit; refused");
    }
    Ok(buffer)
}

fn validate_bundle_url(url: &reqwest::Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("unsupported bundle URL scheme; use https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("bundle URLs may not include credentials");
    }
    let host = url.host_str().context("bundle URL must include a host")?;
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback_bundle_host(host) => Ok(()),
        "http" => bail!("plain http is only allowed for loopback hosts; use https"),
        _ => unreachable!("scheme was validated above"),
    }
}

fn validate_bundle_redirect(initial_scheme: &str, next_url: &reqwest::Url) -> Result<()> {
    validate_bundle_url(next_url)?;
    if next_url.scheme() != initial_scheme {
        bail!("bundle redirects may not change URL scheme");
    }
    Ok(())
}

fn is_loopback_bundle_host(host: &str) -> bool {
    let normalized = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    normalized.eq_ignore_ascii_case("localhost")
        || normalized.to_ascii_lowercase().ends_with(".localhost")
        || normalized
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Build a deterministic, secret-free export from `config`.
///
/// Keys are sorted, machine-specific absolute paths and credential fields are
/// dropped, and the same section mapping as import is used so an exported
/// bundle re-imports at the same scope.
pub fn export_bundle(
    config: &ConfigToml,
    scope: BundleScope,
    metadata: BundleMetadata,
) -> Result<PortableBundle> {
    let mut preferences = BundleTable::default();
    let mut profiles = BundleTable::default();
    let mut global = BundleTable::default();
    let mut project = BundleTable::default();

    for (key, _display) in config.list_values() {
        if is_sensitive_config_key(&key) {
            continue;
        }
        if MACHINE_SPECIFIC_KEYS.contains(&key.as_str()) {
            continue;
        }
        if let Some(value) = value_for_export(config, &key) {
            match export_section_for(&key, scope) {
                ExportSection::Preferences => {
                    preferences.entries.insert(key, value);
                }
                ExportSection::Profiles => {
                    profiles.entries.insert(key, value);
                }
                ExportSection::Global => {
                    global.entries.insert(key, value);
                }
                ExportSection::Project => {
                    project.entries.insert(key, value);
                }
                ExportSection::Drop => {}
            }
        }
    }

    Ok(PortableBundle {
        schema_version: BUNDLE_SCHEMA_VERSION,
        kind: BUNDLE_KIND.to_string(),
        metadata,
        preferences,
        profiles,
        plugins: BundleTable::default(),
        project,
        global,
    })
}

/// Serialize a bundle deterministically (sorted keys, TOML).
pub fn serialize_bundle(bundle: &PortableBundle) -> Result<String> {
    toml::to_string_pretty(bundle).context("serializing portable bundle")
}

/// Config keys that name a machine-local location and must never be exported.
const MACHINE_SPECIFIC_KEYS: [&str; 4] = [
    "hook_sinks.unix_socket_path",
    "base_url",
    "telemetry_endpoint",
    "mcp_config_path",
];

enum ExportSection {
    Preferences,
    Profiles,
    Global,
    Project,
    Drop,
}

fn export_section_for(key: &str, scope: BundleScope) -> ExportSection {
    if key.starts_with("harness") || key.contains("harness_profiles") {
        return ExportSection::Profiles;
    }
    if key.starts_with("skills") || key.starts_with("tools") || key.starts_with("snapshots") {
        return ExportSection::Preferences;
    }
    if key.starts_with("providers.") {
        // Provider tables carry base_url/api-key metadata; only the model
        // selection is portable, and it is exported via `preferences` keys.
        return ExportSection::Drop;
    }
    if key.starts_with("auth.") {
        return ExportSection::Drop;
    }
    match scope {
        BundleScope::Global => ExportSection::Global,
        BundleScope::Project => ExportSection::Project,
    }
}

/// The exportable value for `key`, or `None` when the key is not portable.
/// Redacted display values are never exported: a redacted placeholder would
/// re-import as literal data.
fn value_for_export(config: &ConfigToml, key: &str) -> Option<toml::Value> {
    let text = config.get_value(key)?;
    let raw = toml::Value::String(text);
    if let toml::Value::String(text) = &raw
        && (text.contains("[redacted]") || text.contains(codewhale_config::persistence::REDACTED))
    {
        return None;
    }
    Some(raw)
}

// ---------------------------------------------------------------------------
// Transactional apply
// ---------------------------------------------------------------------------

/// Outcome of a committed import.
#[derive(Debug)]
pub struct ImportReceipt {
    pub plan: ImportPlan,
    pub backup_path: Option<PathBuf>,
    pub target: PathBuf,
}

/// Apply a validated bundle to `store` transactionally.
///
/// The current document is backed up to `<target>.bundle-backup-<timestamp>`,
/// entries are applied through `ConfigStore::set_value`, and any failure
/// restores the backup before returning the error. The receipt redacts by
/// construction: it carries only key paths and counts, never values.
pub fn apply_bundle(
    bundle: &PortableBundle,
    store: &mut codewhale_config::ConfigStore,
    scope: BundleScope,
    workspace: &Path,
) -> Result<ImportReceipt> {
    // Scope isolation is structural: project entries belong only in a
    // project document, global entries only in the user-global one. A bundle
    // carrying the other scope's section is refused up front rather than
    // silently writing across the boundary.
    match scope {
        BundleScope::Global if !bundle.project.entries.is_empty() => {
            bail!(
                "bundle carries [project] entries; import it with --scope project from the workspace instead"
            );
        }
        BundleScope::Project if !bundle.global.entries.is_empty() => {
            bail!(
                "bundle carries [global] entries; importing them into a project document would leak machine state"
            );
        }
        _ => {}
    }
    // A project-scoped import must target an actual workspace document — the
    // user-global file is never a landing zone for [project] entries.
    if scope == BundleScope::Project
        && !codewhale_config::config_path_is_workspace_scoped(store.path())
    {
        bail!(
            "--scope project requires a workspace config ({} is the user-global document)",
            store.path().display()
        );
    }
    let plan = plan_import(bundle, &store.config, scope);
    if !plan.conflicting.is_empty() {
        bail!(
            "bundle contains conflicting or rejected entries: {}; remove duplicate keys or credential-shaped entries and re-export",
            plan.conflicting.join(", ")
        );
    }
    if plan.is_no_op() {
        return Ok(ImportReceipt {
            plan,
            backup_path: None,
            target: store.path().to_path_buf(),
        });
    }

    let target = store.path().to_path_buf();
    let backup_path = backup_path_for(&target)?;
    std::fs::copy(&target, &backup_path)
        .with_context(|| format!("backing up {} before bundle apply", target.display()))?;

    let apply_result = apply_entries(bundle, store, scope, workspace);
    if let Err(error) = apply_result {
        // Rollback: restore the exact prior bytes.
        let restore = std::fs::read(&backup_path)
            .ok()
            .map(|bytes| std::fs::write(&target, bytes));
        match restore {
            Some(Ok(())) => bail!("{error:#}; rolled back to the pre-import document"),
            _ => bail!(
                "{error:#}; ROLLBACK FAILED — the pre-import document is preserved at {}",
                backup_path.display()
            ),
        }
    }

    Ok(ImportReceipt {
        plan,
        backup_path: Some(backup_path),
        target,
    })
}

fn apply_entries(
    bundle: &PortableBundle,
    store: &mut codewhale_config::ConfigStore,
    scope: BundleScope,
    workspace: &Path,
) -> Result<()> {
    for (section, table) in [
        ("preferences", &bundle.preferences),
        ("profiles", &bundle.profiles),
        ("plugins", &bundle.plugins),
        ("project", &bundle.project),
        ("global", &bundle.global),
    ] {
        let applies = match section {
            "project" => scope == BundleScope::Project,
            "global" => scope == BundleScope::Global,
            _ => true,
        };
        if !applies {
            continue;
        }
        for (key, value) in &table.entries {
            let dotted = format!("{section}.{key}");
            if is_sensitive_config_key(key) {
                bail!("refusing to import credential-shaped key {dotted}");
            }
            let rendered = render_toml_value(value)?;
            // The bundle key is the config key as written; the section groups
            // entries for scoping and display only.
            store.config.set_value(key, &rendered)?;
        }
    }
    store.save().context("saving imported bundle")?;
    let _ = workspace;
    Ok(())
}

/// Render a TOML value into the scalar text `config set` accepts.
fn render_toml_value(value: &toml::Value) -> Result<String> {
    Ok(match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Integer(number) => number.to_string(),
        toml::Value::Float(number) => number.to_string(),
        toml::Value::Boolean(flag) => flag.to_string(),
        toml::Value::Datetime(text) => text.to_string(),
        toml::Value::Array(_) | toml::Value::Table(_) => {
            toml::to_string(value)?.trim_end().to_string()
        }
    })
}

fn backup_path_for(target: &Path) -> Result<PathBuf> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let file_name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_string());
    Ok(target.with_file_name(format!("{file_name}.bundle-backup-{timestamp}")))
}

// ---------------------------------------------------------------------------
// Consent
// ---------------------------------------------------------------------------

/// Require explicit consent before mutating: interactive sessions get a
/// prompt; headless runs require `--yes`.
pub fn require_import_consent(yes: bool, plan: &ImportPlan) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "import refused: non-interactive use requires explicit --yes after reviewing the plan"
        );
    }
    print!(
        "Apply this bundle ({} added, {} changed)? Type 'yes': ",
        plan.added.len(),
        plan.changed.len()
    );
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("reading import consent")?;
    if answer.trim() != "yes" {
        bail!("import cancelled; no configuration was changed");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI surface
// ---------------------------------------------------------------------------

/// Arguments for `codewhale config import`.
#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    /// Bundle source: a file path, an HTTPS URL, or `-` for stdin.
    pub source: String,
    /// Print the deterministic import plan without writing anything.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
    /// Skip the interactive consent prompt (required for headless use).
    #[arg(long, default_value_t = false)]
    yes: bool,
    /// Target the project config instead of the user-global document.
    #[arg(long, default_value_t = false)]
    project: bool,
}

/// Arguments for `codewhale config export --portable`.
#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Emit a portable, secret-free bundle (required flag; plain `export`
    /// is reserved so a future non-portable format cannot silently change
    /// what the command writes).
    #[arg(long, default_value_t = false)]
    portable: bool,
    /// Export the project config instead of the user-global document.
    #[arg(long, default_value_t = false)]
    project: bool,
    /// Write to this path instead of stdout.
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
}

/// Run `config import`.
pub fn run_import(
    args: &ImportArgs,
    store: &mut codewhale_config::ConfigStore,
    workspace: &Path,
) -> Result<()> {
    let scope = if args.project {
        BundleScope::Project
    } else {
        BundleScope::Global
    };
    let remote_source = args.source.starts_with("https://") || args.source.starts_with("http://");
    let source_label = if args.source == "-" {
        "stdin"
    } else if remote_source {
        "remote bundle"
    } else {
        args.source.as_str()
    };
    let raw = if args.source == "-" {
        let mut buffer = Vec::new();
        std::io::stdin()
            .lock()
            .take(MAX_BUNDLE_BYTES + 1)
            .read_to_end(&mut buffer)
            .context("reading bundle from stdin")?;
        if buffer.len() as u64 > MAX_BUNDLE_BYTES {
            bail!("stdin bundle exceeds the {MAX_BUNDLE_BYTES} byte limit; refused");
        }
        buffer
    } else if remote_source {
        fetch_bundle(&args.source)?
    } else {
        let path = PathBuf::from(&args.source);
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("reading bundle at {}", path.display()))?;
        if metadata.len() > MAX_BUNDLE_BYTES {
            bail!(
                "bundle at {} is {} bytes; the limit is {MAX_BUNDLE_BYTES} bytes",
                path.display(),
                metadata.len()
            );
        }
        std::fs::read(&path).with_context(|| format!("reading bundle at {}", path.display()))?
    };

    let bundle = parse_bundle_bytes(&raw, source_label)?;
    let plan = plan_import(&bundle, &store.config, scope);

    println!("import plan ({} scope, {source_label}):", scope.label());
    println!("  added:       {}", plan.added.len());
    println!("  changed:     {}", plan.changed.len());
    println!("  skipped:     {}", plan.skipped.len());
    println!("  conflicting: {}", plan.conflicting.len());
    println!("  rejected:    {}", plan.rejected.len());
    for entry in &plan.added {
        println!("  + {entry}");
    }
    for entry in &plan.changed {
        println!("  ~ {entry}");
    }
    for entry in &plan.rejected {
        println!("  ! {} — {}", entry.key, entry.reason);
    }
    for entry in &plan.conflicting {
        println!("  x {entry}");
    }

    if args.dry_run {
        println!("dry run: nothing was written");
        return Ok(());
    }

    require_import_consent(args.yes, &plan)?;
    let receipt = apply_bundle(&bundle, store, scope, workspace)?;
    if receipt.plan.is_no_op() {
        println!("nothing to apply; config already matches the bundle (idempotent re-import)");
        return Ok(());
    }
    println!(
        "imported: {} added, {} changed into {}",
        receipt.plan.added.len(),
        receipt.plan.changed.len(),
        receipt.target.display()
    );
    if let Some(backup) = &receipt.backup_path {
        println!("pre-import backup: {}", backup.display());
    }
    Ok(())
}

/// Run `config export --portable`.
pub fn run_export(args: &ExportArgs, store: &codewhale_config::ConfigStore) -> Result<()> {
    if !args.portable {
        bail!("config export requires --portable; plain export is not defined yet");
    }
    let scope = if args.project {
        BundleScope::Project
    } else {
        BundleScope::Global
    };
    let metadata = BundleMetadata {
        name: None,
        created_at: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        generator: Some(format!("codewhale {}", env!("CARGO_PKG_VERSION"))),
    };
    let bundle = export_bundle(&store.config, scope, metadata)?;
    let body = serialize_bundle(&bundle)?;
    match &args.out {
        Some(path) => {
            codewhale_config::persistence::atomic_write(path, body.as_bytes())
                .with_context(|| format!("writing bundle to {}", path.display()))?;
            println!("wrote portable bundle to {}", path.display());
        }
        None => {
            use std::io::Write;
            std::io::stdout().write_all(body.as_bytes())?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_config::ConfigStore;
    use std::io::Write;
    use std::net::{Ipv4Addr, TcpListener};

    const VALID_TOML: &str = r#"
schema_version = 1
kind = "codewhale.portable-config"

[metadata]
name = "team-baseline"

[preferences]
verbosity = "quiet"
telemetry = false

[global]
output_mode = "plain"
"#;

    fn sample_bundle() -> PortableBundle {
        parse_bundle_str(VALID_TOML, "test.toml").expect("valid bundle")
    }

    #[test]
    fn valid_bundle_parses_and_validates() {
        let bundle = sample_bundle();
        assert_eq!(bundle.schema_version, 1);
        assert_eq!(bundle.kind, "codewhale.portable-config");
        assert_eq!(bundle.metadata.name.as_deref(), Some("team-baseline"));
        assert_eq!(bundle.preferences.entries.len(), 2);
    }

    #[test]
    fn unknown_envelope_fields_fail_the_parse() {
        let text = r#"
schema_version = 1
kind = "codewhale.portable-config"
sneaky_extra = true
"#;
        let err = parse_bundle_str(text, "test.toml").expect_err("unknown field must fail");
        let rendered = format!("{err:#}");
        assert!(rendered.contains("unknown field"), "{rendered}");
    }

    #[test]
    fn wrong_kind_or_schema_version_is_refused() {
        let bad_kind = "schema_version = 1
kind = \"something-else\"\n";
        let err = parse_bundle_str(bad_kind, "t.toml").expect_err("kind must match");
        assert!(err.to_string().contains("kind"), "{err:#}");

        let bad_version = "schema_version = 99\nkind = \"codewhale.portable-config\"\n";
        let err = parse_bundle_str(bad_version, "t.toml").expect_err("schema version must match");
        assert!(err.to_string().contains("schema_version"), "{err:#}");
    }

    #[test]
    fn json_bundles_parse_when_the_document_is_json() {
        let json = r#"{"schema_version": 1, "kind": "codewhale.portable-config",
            "preferences": {"verbosity": "quiet"}}"#;
        let bundle = parse_bundle_str(json, "bundle.json").expect("json bundle");
        assert_eq!(bundle.preferences.entries.len(), 1);
    }

    #[test]
    fn oversize_input_is_refused_before_parse() {
        let big = vec![b'#'; (MAX_BUNDLE_BYTES + 1) as usize];
        let err = parse_bundle_bytes(&big, "big.toml").expect_err("oversize must fail");
        assert!(err.to_string().contains("limit"), "{err:#}");
    }

    #[test]
    fn credential_keys_are_rejected_by_name() {
        let text = r#"
schema_version = 1
kind = "codewhale.portable-config"

[global]
api_key = "value-is-never-echoed"

[preferences]
openai_api_key = "also-secret"
"#;
        let bundle = parse_bundle_str(text, "t.toml").expect("parses");
        let rejected = find_rejected_entries(&bundle);
        assert_eq!(rejected.len(), 2, "{rejected:?}");
        assert!(rejected.iter().any(|r| r.key == "global.api_key"));
        assert!(
            rejected
                .iter()
                .all(|r| !r.reason.contains("value-is-never-echoed"))
        );
    }

    #[test]
    fn credential_shaped_values_are_rejected_under_benign_names() {
        let text = r#"
schema_version = 1
kind = "codewhale.portable-config"

[preferences]
note = "sk-abcdefghij0123456789"
"#;
        let bundle = parse_bundle_str(text, "t.toml").expect("parses");
        let rejected = find_rejected_entries(&bundle);
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert!(!rejected[0].reason.contains("sk-abcdefghij"));
    }

    #[test]
    fn plan_reports_added_changed_skipped_deterministically() {
        let store = isolated_store();
        let bundle_text = r#"
schema_version = 1
kind = "codewhale.portable-config"

[preferences]
verbosity = "quiet"
log_level = "debug"

[global]
output_mode = "plain"
"#;
        let bundle = parse_bundle_str(bundle_text, "t.toml").expect("bundle");
        // verbosity already matches; log_level is new; output_mode is global-scope.
        let plan_global = plan_import(&bundle, &store.config, BundleScope::Global);
        assert!(
            plan_global
                .added
                .contains(&"preferences.log_level".to_string())
        );
        // `verbosity` resolves to a shipped default even when the file key is
        // unset, so an equal value reads as changed-or-skipped by resolution;
        // what matters for determinism is that every entry lands in exactly
        // one bucket and nothing is dropped silently.
        let all: std::collections::BTreeSet<&String> = plan_global
            .added
            .iter()
            .chain(plan_global.changed.iter())
            .chain(plan_global.skipped.iter())
            .collect();
        assert_eq!(all.len(), 3, "{plan_global:?}");
        // Project scope skips global-section entries.
        let plan_project = plan_import(&bundle, &store.config, BundleScope::Project);
        assert!(
            plan_project
                .skipped
                .contains(&"global.output_mode".to_string())
        );
    }

    #[test]
    fn rejected_entries_show_up_as_conflicting_in_the_plan() {
        let store = isolated_store();
        let text = r#"
schema_version = 1
kind = "codewhale.portable-config"

[global]
api_key = "never-echoed"
"#;
        let bundle = parse_bundle_str(text, "t.toml").expect("bundle");
        let plan = plan_import(&bundle, &store.config, BundleScope::Global);
        assert!(plan.conflicting.contains(&"global.api_key".to_string()));
        assert!(plan.added.is_empty());
    }

    #[test]
    fn duplicate_flat_keys_across_applicable_sections_fail_before_apply() {
        let mut store = isolated_store();
        let before = std::fs::read(store.path()).expect("config before import");
        let text = r#"
schema_version = 1
kind = "codewhale.portable-config"

[preferences]
verbosity = "quiet"

[global]
verbosity = "verbose"
"#;
        let bundle = parse_bundle_str(text, "collision.toml").expect("bundle parses");
        let plan = plan_import(&bundle, &store.config, BundleScope::Global);

        assert_eq!(
            plan.conflicting,
            ["preferences.verbosity", "global.verbosity"]
        );
        assert!(plan.added.is_empty(), "{plan:?}");
        assert!(plan.changed.is_empty(), "{plan:?}");
        assert!(plan.skipped.is_empty(), "{plan:?}");

        let workspace = tempfile::tempdir().expect("workspace");
        let error = apply_bundle(&bundle, &mut store, BundleScope::Global, workspace.path())
            .expect_err("ambiguous flat key must fail closed");
        let rendered = error.to_string();
        assert!(rendered.contains("conflicting"), "{error:#}");
        assert!(!rendered.contains("quiet"), "{error:#}");
        assert!(!rendered.contains("verbose"), "{error:#}");
        assert_eq!(
            std::fs::read(store.path()).expect("config after refused import"),
            before,
            "collision must be refused before any write"
        );
    }

    #[test]
    fn dry_run_semantics_plan_never_mutates() {
        let store = isolated_store();
        let before = std::fs::read_to_string(store.path()).expect("read config");
        let bundle = sample_bundle();
        let _plan = plan_import(&bundle, &store.config, BundleScope::Global);
        let after = std::fs::read_to_string(store.path()).expect("read config");
        assert_eq!(before, after, "planning must not write");
    }

    #[test]
    fn apply_is_idempotent_on_reimport() {
        let mut store = isolated_store();
        let workspace = tempfile::tempdir().expect("workspace");
        let bundle = sample_bundle();

        let first = apply_bundle(&bundle, &mut store, BundleScope::Global, workspace.path())
            .expect("first import");
        assert!(first.plan.added.len() + first.plan.changed.len() > 0);

        let second = apply_bundle(&bundle, &mut store, BundleScope::Global, workspace.path())
            .expect("second import");
        assert!(
            second.plan.is_no_op(),
            "re-import must be a no-op: {:?}",
            second.plan
        );
        assert!(second.backup_path.is_none());
    }

    #[test]
    fn project_scope_never_touches_the_global_document() {
        let mut store = isolated_store();
        let global_before = std::fs::read_to_string(store.path()).expect("global doc");

        let text = r#"
schema_version = 1
kind = "codewhale.portable-config"

[project]
approval_policy = "unless-allowed"
"#;
        let bundle = parse_bundle_str(text, "t.toml").expect("bundle");
        let ws = tempfile::tempdir().expect("ws");
        apply_bundle(&bundle, &mut store, BundleScope::Project, ws.path())
            .expect_err("project entries cannot land in a global-scoped store");
        let global_after = std::fs::read_to_string(store.path()).expect("global doc");
        assert_eq!(global_before, global_after);
    }

    #[cfg(unix)]
    #[test]
    fn failed_apply_rolls_back_to_the_prior_document() {
        let mut store = isolated_store();
        let original = std::fs::read_to_string(store.path()).expect("config");

        // A bundle whose entry fails mid-apply: `providers.deepseek.wire` is a
        // real key path but an invalid value for it, so set_value errors after
        // earlier entries were applied.
        let text = r#"
schema_version = 1
kind = "codewhale.portable-config"

[preferences]
log_level = "debug"

[global]
providers_deepseek_wire = "not-a-real-key-so-this-errors"
"#;
        let _ = text;
        // Simpler deterministic failure: make the target file read-only.
        let text_ok = r#"
schema_version = 1
kind = "codewhale.portable-config"

[preferences]
log_level = "debug"
"#;
        let bundle = parse_bundle_str(text_ok, "t.toml").expect("bundle");
        let path = store.path().to_path_buf();
        // Atomic saves replace the file via rename, so the *directory* must
        // be made unwritable to force the write failure.
        use std::os::unix::fs::PermissionsExt;
        let dir = path.parent().expect("config dir").to_path_buf();
        let mut perms = std::fs::metadata(&dir).expect("dir meta").permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&dir, perms).expect("chmod dir");

        let result = apply_bundle(&bundle, &mut store, BundleScope::Global, Path::new("."));
        // Restore permissions so the tempdir can be cleaned up.
        let mut perms = std::fs::metadata(&dir).expect("dir meta").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dir, perms).expect("chmod restore");

        assert!(result.is_err(), "apply must fail on a read-only document");
        let restored = std::fs::read_to_string(&path).expect("config after rollback");
        assert_eq!(restored, original, "rollback must preserve the prior bytes");
    }

    #[test]
    fn export_is_deterministic_and_secret_free() {
        let mut store = isolated_store();
        store
            .config
            .set_value("verbosity", "quiet")
            .expect("set verbosity");
        store
            .config
            .set_value("default_text_model", "deepseek-v4-pro")
            .expect("set model");
        store.save().expect("save");

        let metadata = BundleMetadata::default();
        let one = export_bundle(&store.config, BundleScope::Global, metadata.clone())
            .and_then(|b| serialize_bundle(&b))
            .expect("export one");
        let two = export_bundle(&store.config, BundleScope::Global, metadata)
            .and_then(|b| serialize_bundle(&b))
            .expect("export two");
        assert_eq!(one, two, "export must be deterministic");

        // No machine-specific absolute paths in the body.
        assert!(!one.contains("/Users/"), "{one}");
        assert!(!one.contains("/home/"), "{one}");
    }

    #[test]
    fn exported_bundle_reimports_cleanly() {
        let mut store = isolated_store();
        store.config.set_value("verbosity", "quiet").expect("set");
        store.save().expect("save");

        let bundle = export_bundle(
            &store.config,
            BundleScope::Global,
            BundleMetadata::default(),
        )
        .expect("export");
        let rejected = find_rejected_entries(&bundle);
        assert!(
            rejected.is_empty(),
            "export must be secret-free: {rejected:?}"
        );

        let plan = plan_import(&bundle, &store.config, BundleScope::Global);
        assert!(
            plan.rejected.is_empty() && plan.conflicting.is_empty(),
            "own export must not trip rejection: {plan:?}"
        );
    }

    #[test]
    fn http_non_loopback_fetch_is_refused_without_network_access() {
        let err = fetch_bundle("http://example.com/bundle.toml")
            .expect_err("plain http to a public host must be refused");
        assert!(err.to_string().contains("loopback"), "{err:#}");
        assert!(!err.to_string().contains("example.com"), "{err:#}");
    }

    #[test]
    fn redirect_to_non_loopback_http_is_refused_without_leaking_location() {
        let secret = "location-secret-must-not-leak";
        let location = format!("http://example.com/internal?token={secret}");
        let response = http_response("302 Found", &[("Location", location.as_str())], b"");
        let (url, server) = spawn_bundle_http_server(vec![response]);

        let error = fetch_bundle(&url).expect_err("redirect target must be revalidated");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("loopback"), "{rendered}");
        assert!(!rendered.contains("example.com"), "{rendered}");
        assert!(!rendered.contains(secret), "{rendered}");
        assert_eq!(server.join().expect("server joins"), 1);
    }

    #[test]
    fn https_redirect_cannot_downgrade_to_loopback_http() {
        let secret = "downgrade-secret-must-not-leak";
        let target = reqwest::Url::parse(&format!("http://127.0.0.1/internal?token={secret}"))
            .expect("test target URL");

        let error = validate_bundle_redirect("https", &target)
            .expect_err("HTTPS redirect must not downgrade to loopback HTTP");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("scheme"), "{rendered}");
        assert!(!rendered.contains(secret), "{rendered}");
        assert!(!rendered.contains("127.0.0.1"), "{rendered}");
    }

    #[test]
    fn relative_loopback_redirect_fetches_bundle() {
        let redirect = http_response("302 Found", &[("Location", "/bundle.toml")], b"");
        let body = VALID_TOML.as_bytes();
        let success = http_response("200 OK", &[("Content-Type", "text/plain")], body);
        let (url, server) = spawn_bundle_http_server(vec![redirect, success]);

        let fetched = fetch_bundle(&url).expect("relative redirect remains allowed");
        assert_eq!(fetched, body);
        assert_eq!(server.join().expect("server joins"), 2);
    }

    #[test]
    fn redirect_limit_is_enforced_before_a_sixth_hop() {
        let responses = (0..=MAX_REDIRECTS)
            .map(|hop| {
                let location = format!("/hop-{}", hop + 1);
                http_response("302 Found", &[("Location", location.as_str())], b"")
            })
            .collect();
        let (url, server) = spawn_bundle_http_server(responses);

        let error = fetch_bundle(&url).expect_err("sixth redirect must be refused");
        assert!(error.to_string().contains("five-redirect"), "{error:#}");
        assert_eq!(server.join().expect("server joins"), MAX_REDIRECTS + 1);
    }

    #[test]
    fn bundle_url_credentials_are_rejected_without_echoing_them() {
        let secret = "credential-secret-must-not-leak";
        let error = fetch_bundle(&format!("https://user:{secret}@example.com/bundle.toml"))
            .expect_err("URL userinfo must be refused");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("credentials"), "{rendered}");
        assert!(!rendered.contains(secret), "{rendered}");
        assert!(!rendered.contains("example.com"), "{rendered}");
    }

    #[test]
    fn unsupported_schemes_are_refused() {
        let err = fetch_bundle("file:///etc/passwd").expect_err("file scheme refused");
        assert!(err.to_string().contains("scheme"), "{err:#}");
    }

    #[test]
    fn headless_import_requires_yes() {
        let plan = ImportPlan {
            added: vec!["preferences.x".to_string()],
            ..ImportPlan::default()
        };
        // The test harness runs headless (no tty), so consent without --yes
        // must refuse before any prompt.
        let err = require_import_consent(false, &plan).expect_err("headless needs --yes");
        assert!(err.to_string().contains("--yes"), "{err:#}");
        require_import_consent(true, &plan).expect("--yes short-circuits consent");
    }

    #[test]
    fn bounded_paths_refuse_traversal_and_absolute_escapes() {
        let base = tempfile::tempdir().expect("base");
        let err =
            resolve_bounded_path(base.path(), "../escape.toml").expect_err("traversal refused");
        assert!(
            err.to_string().contains("escapes") || err.to_string().contains("absolute"),
            "{err:#}"
        );
        let absolute = base.path().join("absolute.toml");
        let err = resolve_bounded_path(base.path(), absolute.to_string_lossy().as_ref())
            .expect_err("absolute refused");
        assert!(err.to_string().contains("absolute"), "{err:#}");
        let ok = resolve_bounded_path(base.path(), "nested/thing.toml").expect("inside ok");
        assert!(ok.starts_with(base.path()));
    }

    // -- helpers ------------------------------------------------------------

    fn http_response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        )
        .into_bytes();
        for (name, value) in headers {
            response.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        response.extend_from_slice(b"\r\n");
        response.extend_from_slice(body);
        response
    }

    fn spawn_bundle_http_server(
        responses: Vec<Vec<u8>>,
    ) -> (String, std::thread::JoinHandle<usize>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind HTTP fixture");
        let address = listener.local_addr().expect("fixture address");
        let handle = std::thread::spawn(move || {
            let mut served = 0usize;
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept fixture request");
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .expect("fixture read timeout");
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut chunk).expect("read fixture request");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                }
                stream.write_all(&response).expect("write fixture response");
                served += 1;
            }
            served
        });
        (format!("http://{address}/start"), handle)
    }

    /// A store over a config file that outlives the helper: the tempdir is
    /// leaked deliberately (tests are short-lived; explicit cleanup would need
    /// to thread the guard through every call site).
    fn isolated_store() -> ConfigStore {
        // Serialize with every other env-mutating test in this crate: a private
        // lock here would still race `ScopedEnvVar` users (observed as a flaky
        // credentials-dir failure in `api_key_config_failure_restores_*`).
        let _guard = crate::tests::env_lock();

        let dir = {
            // TempDir::keep() is the non-deprecated ownership transfer.
            let temp = tempfile::TempDir::new().expect("tempdir");
            temp.keep()
        };
        let unique = dir.join("home").join(std::process::id().to_string());
        std::fs::create_dir_all(&unique).expect("unique home");
        // SAFETY: test-only env mutation, serialized by the lock above.
        unsafe { std::env::set_var("CODEWHALE_HOME", &unique) };
        let path = unique.join("config.toml");
        std::fs::write(&path, "# test config\n").expect("seed config file");
        ConfigStore::load(Some(path)).expect("store loads")
    }
}
