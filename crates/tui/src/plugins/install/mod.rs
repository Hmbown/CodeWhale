//! Plugin install on-ramp (#5182).
//!
//! Fetches a plugin bundle from a local directory, a `github:owner/repo`
//! archive, or a direct tarball URL, and places it under the user plugins
//! root (`~/.codewhale/plugins/<name>/`). This module deliberately mirrors
//! [`crate::skills::install`]: the download, network-gating, traversal
//! rejection, and marker machinery is *reused* from there (`fetch_tarball`,
//! `is_safe_path`, `write_installed_from_v2`, `INSTALLED_FROM_MARKER`), while
//! the scan/extract step is plugin-shaped (a bundle is rooted at the single
//! supported plugin manifest in the tree, not at a `SKILL.md`).
//!
//! # Hard rules
//!
//! * Everything is staged in a private `.staging-*` sibling first. The
//!   destination is only created (via atomic rename) once the bundle clears
//!   every check — half-installed plugins never appear on disk.
//! * The fetched tree must contain **exactly one** plugin bundle root holding
//!   `plugin.json`, `kimi.plugin.json`, or `plugin.toml`. Zero (not a plugin)
//!   or more than one (ambiguous mono-repo) are both rejected.
//! * Path traversal (`..`, absolute paths) and symlinks/hard links inside the
//!   selected bundle subtree are rejected. Entries outside the subtree are
//!   never extracted.
//! * The manifest `[plugin].name` must be a single path-safe segment; it
//!   becomes the destination directory name.
//! * Overwriting a bundle that lacks the `.installed-from` marker is refused
//!   — hand-placed bundles are never clobbered. `update` swaps atomically
//!   only when the upstream bytes changed; a changed bundle automatically
//!   invalidates the hash-bound trust receipt at the next discovery.
//! * Installed bits land **disabled and untrusted**; trust/enablement is the
//!   existing registry flow, not this module's concern.

//!
//! # Module map
//!
//! This module owns the source spec, the result types, and the three
//! verbs. The pipeline stages each live next door:
//!
//! * [`stage`] — copy a local bundle into a private `.staging-*` sibling
//!   (symlink, file-count, and size rejection; manifest validation).
//! * [`tarball`] — the two-pass archive reader: scan for the single
//!   supported manifest under the size cap, then extract just that subtree.
//! * [`place`] — atomic rename into `<name>/`, marker write, and the
//!   containment guards shared with discovery.
//!
//! Fetching is not ours: remote bytes come from
//! [`crate::skills::install::fetch_tarball`], network gating included.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use thiserror::Error;

use crate::network_policy::NetworkPolicy;
use crate::skills::install::{
    self as skill_install, FetchOutcome, InstallSource, InstalledFromMarker, fetch_tarball,
    sha256_hex, source_spec_string,
};

mod place;
mod stage;
mod tarball;

#[cfg(test)]
mod tests;

use place::{ensure_target_within_plugins_dir, finalize_install, plugin_target_path};
use stage::stage_local_copy;
use tarball::stage_tarball;

/// Marker file shared with the skill installer. Its presence means "this
/// bundle was placed by `/plugin install`" and enables update/uninstall.
pub use crate::skills::install::INSTALLED_FROM_MARKER;

/// Default per-bundle size cap. Mirrors the skill installer; the runtime
/// staging budget in `registry.rs` stays the outer bound.
pub const DEFAULT_MAX_SIZE_BYTES: u64 = skill_install::DEFAULT_MAX_SIZE_BYTES;

// ─────────────────────────────────────────────────────────────────────────────
// Source parsing
// ─────────────────────────────────────────────────────────────────────────────

/// Where a plugin bundle is installed from. See [`PluginInstallSource::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginInstallSource {
    /// Local bundle directory (copied, never executed). Parsed from a plain
    /// path or an explicit `path:<dir>` spec (the marker round-trip form).
    LocalPath(PathBuf),
    /// `github:owner/repo` or a direct `http(s)://…` tarball URL, downloaded
    /// through the shared skill-install machinery. There is no registry
    /// index in v1.
    Remote(InstallSource),
}

impl PluginInstallSource {
    /// Parse a user-supplied spec.
    ///
    /// * `github:owner/repo`, `https://…` → [`PluginInstallSource::Remote`]
    ///   (via [`InstallSource::parse`]; registry names are unreachable here)
    /// * `path:<dir>` or any other value → [`PluginInstallSource::LocalPath`]
    pub fn parse(spec: &str) -> Result<Self> {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            bail!("install source must not be empty");
        }
        if let Some(path) = trimmed.strip_prefix("path:") {
            return Self::local(path);
        }
        if trimmed.starts_with("github:")
            || trimmed.starts_with("https://")
            || trimmed.starts_with("http://")
        {
            let source = InstallSource::parse(trimmed)?;
            return match source {
                InstallSource::GitHubRepo(_) | InstallSource::DirectUrl(_) => {
                    Ok(Self::Remote(source))
                }
                InstallSource::Registry(_) => {
                    unreachable!("prefixed specs never parse as a registry name")
                }
            };
        }
        Self::local(trimmed)
    }

    fn local(spec: &str) -> Result<Self> {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            bail!("local install path must not be empty");
        }
        Ok(Self::LocalPath(PathBuf::from(trimmed)))
    }
}

/// Serialize a source for the `.installed-from` marker. Must round-trip
/// through [`PluginInstallSource::parse`].
fn plugin_spec_string(source: &PluginInstallSource, canonical_source: Option<&Path>) -> String {
    match source {
        PluginInstallSource::LocalPath(_) => {
            let path = canonical_source.expect("local installs record the canonical source");
            format!("path:{}", path.display())
        }
        PluginInstallSource::Remote(remote) => source_spec_string(remote),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Outcome / result types
// ─────────────────────────────────────────────────────────────────────────────

/// Outcome of an install attempt. Same shape as the skill installer's so the
/// caller can drop `NeedsApproval`/`NetworkDenied` into its approval flow.
#[derive(Debug)]
pub enum PluginInstallOutcome {
    /// The bundle was installed (atomic rename + marker write succeeded).
    Installed(InstalledPlugin),
    /// The download host requires user approval; nothing touched disk.
    NeedsApproval(String),
    /// The download host is denied by network policy.
    NetworkDenied(String),
}

/// Metadata for a successfully installed plugin bundle.
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    /// Plugin name from `[plugin].name`; also the destination directory name.
    pub name: String,
    /// Final on-disk path: `<user_plugins_dir>/<name>/`.
    pub path: PathBuf,
    /// Whole-bundle content hash of the staged tree (pre-marker). Informational;
    /// trust receipts always bind to the discovery-time hash.
    pub content_hash: String,
    /// Whole-bundle hash after the provenance marker is written. Callers can
    /// compare this with immediate rediscovery before reporting success.
    pub installed_content_hash: String,
    /// SHA-256 over the downloaded tarball bytes (empty for local copies).
    /// Used by [`update`] to detect upstream changes without re-extracting.
    pub source_checksum: String,
}

/// Result of an [`update`] call.
#[derive(Debug)]
pub enum PluginUpdateResult {
    /// Upstream tarball is byte-identical to the recorded checksum; no action.
    NoChange,
    /// Upstream changed and the on-disk bundle was atomically replaced.
    Updated(InstalledPlugin),
    /// Network policy requires approval for the download host.
    NeedsApproval(String),
    /// Network policy denied the download host.
    NetworkDenied(String),
}

/// Install-time errors, kept as an enum so tests can pattern-match without
/// parsing strings.
#[derive(Debug, Error)]
pub enum PluginInstallError {
    #[error("entry escapes destination directory: {0}")]
    PathTraversal(String),
    #[error("bundle is too large; uncompressed total would exceed {limit} bytes")]
    OversizedBundle { limit: u64 },
    #[error(
        "archive must contain exactly one plugin bundle root (a directory holding plugin.json, kimi.plugin.json, or plugin.toml); found {0} (install a single plugin bundle, not a mono-repo)"
    )]
    PluginTomlRoots(usize),
    #[error("symlinks and hard links are not allowed in plugin bundles")]
    SymlinkRejected,
    #[error("plugin '{0}' is already installed; use /plugin update or uninstall it first")]
    AlreadyInstalled(String),
    #[error(
        "plugin '{0}' was not installed via /plugin install (no .installed-from marker); refusing to touch the hand-placed bundle"
    )]
    NotInstalledHere(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Install a plugin bundle into `user_plugins_dir`.
///
/// Steps: resolve source → (remote only) network-gate and download under the
/// size cap → stage into a `.staging-*` sibling, enforcing traversal/symlink/
/// size rules and the single-manifest-root requirement → validate the staged
/// manifest → `name_conflict` check → atomic rename into `<name>/` → write
/// `.installed-from` last.
///
/// `update = false` rejects an existing destination. `update = true` (only
/// called from [`update`]) requires the marker and replaces atomically with a
/// backup-restore on failure.
///
/// `name_conflict` is consulted with the validated manifest name before the
/// rename; returning `Some(message)` aborts the install. It lets the caller
/// reject names already claimed by builtin/workspace bundles.
pub async fn install(
    source: PluginInstallSource,
    user_plugins_dir: &Path,
    max_size: u64,
    network: &NetworkPolicy,
    update: bool,
    name_conflict: &(dyn Fn(&str) -> Option<String> + Send + Sync),
) -> Result<PluginInstallOutcome> {
    install_inner(
        source,
        user_plugins_dir,
        max_size,
        network,
        update,
        name_conflict,
        None,
    )
    .await
}

/// Install only when the exact bytes copied into staging match a prior
/// review hash. The comparison happens before atomic placement, so a source
/// that changes between inspection and copying leaves no installed bundle.
pub async fn install_with_expected_content_hash(
    source: PluginInstallSource,
    user_plugins_dir: &Path,
    max_size: u64,
    network: &NetworkPolicy,
    name_conflict: &(dyn Fn(&str) -> Option<String> + Send + Sync),
    expected_content_hash: &str,
) -> Result<PluginInstallOutcome> {
    install_inner(
        source,
        user_plugins_dir,
        max_size,
        network,
        false,
        name_conflict,
        Some(expected_content_hash),
    )
    .await
}

async fn install_inner(
    source: PluginInstallSource,
    user_plugins_dir: &Path,
    max_size: u64,
    network: &NetworkPolicy,
    update: bool,
    name_conflict: &(dyn Fn(&str) -> Option<String> + Send + Sync),
    expected_content_hash: Option<&str>,
) -> Result<PluginInstallOutcome> {
    match &source {
        PluginInstallSource::LocalPath(path) => {
            let staged = stage_local_copy(path, user_plugins_dir, max_size)?;
            verify_expected_content_hash(&staged, expected_content_hash)?;
            if let Some(conflict) = name_conflict(&staged.name) {
                let _ = fs::remove_dir_all(&staged.staged_path);
                bail!(conflict);
            }
            let canonical = path
                .canonicalize()
                .with_context(|| format!("failed to resolve {}", path.display()))?;
            finalize_install(
                staged,
                &plugin_spec_string(&source, Some(&canonical)),
                None,
                "",
                user_plugins_dir,
                update,
            )
        }
        PluginInstallSource::Remote(remote) => {
            let (bytes, url) = match fetch_tarball(remote, network, max_size).await? {
                FetchOutcome::Bytes { bytes, url } => (bytes, url),
                FetchOutcome::NeedsApproval(host) => {
                    return Ok(PluginInstallOutcome::NeedsApproval(host));
                }
                FetchOutcome::Denied(host) => {
                    return Ok(PluginInstallOutcome::NetworkDenied(host));
                }
            };
            install_remote_bytes(
                remote,
                &bytes,
                &url,
                user_plugins_dir,
                max_size,
                update,
                name_conflict,
                expected_content_hash,
            )
        }
    }
}

fn verify_expected_content_hash(
    staged: &stage::StagedPlugin,
    expected_content_hash: Option<&str>,
) -> Result<()> {
    let Some(expected) = expected_content_hash else {
        return Ok(());
    };
    if staged.content_hash == expected {
        return Ok(());
    }
    let actual = staged.content_hash.clone();
    let _ = fs::remove_dir_all(&staged.staged_path);
    bail!(
        "plugin source changed after review: expected content hash {expected}, copied bytes hash is {actual}; nothing was installed"
    )
}

/// Stage and finalize an already-downloaded remote tarball. Kept separate
/// from [`install`] so [`update`] can compare the checksum of the bytes it
/// already fetched instead of downloading twice.
#[allow(clippy::too_many_arguments)]
fn install_remote_bytes(
    remote: &InstallSource,
    bytes: &[u8],
    url: &str,
    user_plugins_dir: &Path,
    max_size: u64,
    update: bool,
    name_conflict: &(dyn Fn(&str) -> Option<String> + Send + Sync),
    expected_content_hash: Option<&str>,
) -> Result<PluginInstallOutcome> {
    let checksum = sha256_hex(bytes);
    let staged = stage_tarball(bytes, user_plugins_dir, max_size)?;
    verify_expected_content_hash(&staged, expected_content_hash)?;
    if let Some(conflict) = name_conflict(&staged.name) {
        let _ = fs::remove_dir_all(&staged.staged_path);
        bail!(conflict);
    }
    finalize_install(
        staged,
        &source_spec_string(remote),
        Some(url),
        &checksum,
        user_plugins_dir,
        update,
    )
}

/// Re-fetch a previously installed plugin and atomically replace it if the
/// upstream tarball changed. The replaced bundle carries new content, so the
/// existing hash-bound trust receipt stops matching at the next discovery —
/// re-review is forced by the registry, not by this function.
///
/// Bundles installed from a local path cannot be re-downloaded; reinstall
/// them with `/plugin install <path>` instead.
pub async fn update(
    name: &str,
    user_plugins_dir: &Path,
    max_size: u64,
    network: &NetworkPolicy,
) -> Result<PluginUpdateResult> {
    let target = plugin_target_path(name, user_plugins_dir)?;
    if target.exists() {
        ensure_target_within_plugins_dir(&target, user_plugins_dir)?;
    }
    let marker_path = target.join(INSTALLED_FROM_MARKER);
    if !marker_path.exists() {
        return Err(PluginInstallError::NotInstalledHere(name.to_string()).into());
    }
    let marker_body = fs::read_to_string(&marker_path)
        .with_context(|| format!("failed to read {}", marker_path.display()))?;
    let marker: InstalledFromMarker = serde_json::from_str(&marker_body)
        .with_context(|| format!("malformed {INSTALLED_FROM_MARKER} for {name}"))?;
    let source = PluginInstallSource::parse(&marker.spec)?;
    let PluginInstallSource::Remote(remote) = source else {
        bail!(
            "plugin '{name}' was installed from a local path ({}) and cannot be updated from the network; \
             reinstall it with /plugin install <path>",
            marker.spec
        );
    };

    let (bytes, url) = match fetch_tarball(&remote, network, max_size).await? {
        FetchOutcome::Bytes { bytes, url } => (bytes, url),
        FetchOutcome::NeedsApproval(host) => {
            return Ok(PluginUpdateResult::NeedsApproval(host));
        }
        FetchOutcome::Denied(host) => return Ok(PluginUpdateResult::NetworkDenied(host)),
    };
    if sha256_hex(&bytes) == marker.source_checksum() {
        return Ok(PluginUpdateResult::NoChange);
    }

    let outcome = install_remote_bytes(
        &remote,
        &bytes,
        &url,
        user_plugins_dir,
        max_size,
        true,
        &|_| None,
        None,
    )?;
    match outcome {
        PluginInstallOutcome::Installed(installed) => Ok(PluginUpdateResult::Updated(installed)),
        PluginInstallOutcome::NeedsApproval(host) => Ok(PluginUpdateResult::NeedsApproval(host)),
        PluginInstallOutcome::NetworkDenied(host) => Ok(PluginUpdateResult::NetworkDenied(host)),
    }
}

/// Remove a plugin installed via `/plugin install`.
///
/// Refuses to touch any directory that doesn't carry the `.installed-from`
/// marker — that's our cue that it's hand-placed and not ours to delete.
/// Callers must require the bundle to be disabled first (the mutation
/// controller does) and prune the registry state entry afterwards.
pub fn uninstall(name: &str, user_plugins_dir: &Path) -> Result<()> {
    let target = plugin_target_path(name, user_plugins_dir)?;
    if !target.exists() {
        bail!("plugin '{name}' is not installed at {}", target.display());
    }
    ensure_target_within_plugins_dir(&target, user_plugins_dir)?;
    if !target.join(INSTALLED_FROM_MARKER).exists() {
        return Err(PluginInstallError::NotInstalledHere(name.to_string()).into());
    }
    fs::remove_dir_all(&target)
        .with_context(|| format!("failed to remove {}", target.display()))?;
    Ok(())
}
