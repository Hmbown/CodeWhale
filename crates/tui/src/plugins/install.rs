//! Plugin install on-ramp (#5182).
//!
//! Fetches a plugin bundle from a local directory, a `github:owner/repo`
//! archive, or a direct tarball URL, and places it under the user plugins
//! root (`~/.codewhale/plugins/<name>/`). This module deliberately mirrors
//! [`crate::skills::install`]: the download, network-gating, traversal
//! rejection, and marker machinery is *reused* from there (`fetch_tarball`,
//! `is_safe_path`, `write_installed_from_v2`, `INSTALLED_FROM_MARKER`), while
//! the scan/extract step is plugin-shaped (a bundle is rooted at the single
//! `plugin.toml` in the tree, not at a `SKILL.md`).
//!
//! # Hard rules
//!
//! * Everything is staged in a private `.staging-*` sibling first. The
//!   destination is only created (via atomic rename) once the bundle clears
//!   every check — half-installed plugins never appear on disk.
//! * The fetched tree must contain **exactly one** `plugin.toml`; that file's
//!   directory becomes the bundle root. Zero (not a plugin) or more than one
//!   (ambiguous mono-repo) are both rejected.
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

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use thiserror::Error;

use crate::network_policy::NetworkPolicy;
use crate::skills::install::{
    self as skill_install, FetchOutcome, InstallSource, InstalledFromMarker, fetch_tarball,
    is_safe_path, sha256_hex, source_spec_string, validate_skill_name_segment,
};

use super::manifest::PluginManifest;

/// Marker file shared with the skill installer. Its presence means "this
/// bundle was placed by `/plugin install`" and enables update/uninstall.
pub use crate::skills::install::INSTALLED_FROM_MARKER;

/// Default per-bundle size cap. Mirrors the skill installer; the runtime
/// staging budget in `registry.rs` stays the outer bound.
pub const DEFAULT_MAX_SIZE_BYTES: u64 = skill_install::DEFAULT_MAX_SIZE_BYTES;

/// File count cap for local copies, mirroring the registry staging budget.
const MAX_BUNDLE_FILES: usize = 4_096;

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
        "archive must contain exactly one plugin.toml root; found {0} (install a single plugin bundle, not a mono-repo)"
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
/// size rules and the single-`plugin.toml` requirement → validate the staged
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
    name_conflict: &dyn Fn(&str) -> Option<String>,
) -> Result<PluginInstallOutcome> {
    match &source {
        PluginInstallSource::LocalPath(path) => {
            let staged = stage_local_copy(path, user_plugins_dir, max_size)?;
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
            )
        }
    }
}

/// Stage and finalize an already-downloaded remote tarball. Kept separate
/// from [`install`] so [`update`] can compare the checksum of the bytes it
/// already fetched instead of downloading twice.
fn install_remote_bytes(
    remote: &InstallSource,
    bytes: &[u8],
    url: &str,
    user_plugins_dir: &Path,
    max_size: u64,
    update: bool,
    name_conflict: &dyn Fn(&str) -> Option<String>,
) -> Result<PluginInstallOutcome> {
    let checksum = sha256_hex(bytes);
    let staged = stage_tarball(bytes, user_plugins_dir, max_size)?;
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

// ─────────────────────────────────────────────────────────────────────────────
// Staging
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct StagedPlugin {
    name: String,
    staged_path: PathBuf,
    content_hash: String,
}

fn fresh_staging_dir(user_plugins_dir: &Path) -> Result<PathBuf> {
    ensure_plugins_dir(user_plugins_dir)?;
    // A crashed stage can leave residue that discovery will surface as an
    // untrusted, disabled bundle; the next install attempt cleans it up by
    // using a fresh uuid path and never reuses the stale one.
    let staged_path = user_plugins_dir.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&staged_path)
        .with_context(|| format!("failed to create staging dir {}", staged_path.display()))?;
    Ok(staged_path)
}

/// Create the user plugins root when missing. The persisted plugin state
/// (`state.json`) lives in this same directory, so it must satisfy the
/// registry's owner-only contract the first time `/plugin install` brings it
/// into existence — a pre-existing directory is left untouched (trust reports
/// unsafe permissions fail-closed rather than silently repairing them).
#[cfg(unix)]
fn ensure_plugins_dir(user_plugins_dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(user_plugins_dir).with_context(|| {
        format!(
            "failed to create user plugins directory {}",
            user_plugins_dir.display()
        )
    })
}

#[cfg(not(unix))]
fn ensure_plugins_dir(user_plugins_dir: &Path) -> Result<()> {
    fs::create_dir_all(user_plugins_dir).with_context(|| {
        format!(
            "failed to create user plugins directory {}",
            user_plugins_dir.display()
        )
    })
}

/// Validate the staged tree and return the manifest name + content hash.
fn validate_staged(staged_path: &Path) -> Result<(String, String)> {
    let validated = PluginManifest::validate_from_path(&staged_path.join("plugin.toml"))
        .map_err(|error| anyhow::anyhow!("staged plugin.toml failed validation: {error}"))?;
    let name = validated.manifest.plugin.name.clone();
    validate_skill_name_segment(&name).map_err(|error| {
        anyhow::anyhow!("[plugin].name is not a safe directory name: {error:#}")
    })?;
    Ok((name, validated.content_hash))
}

/// Copy a local bundle directory into staging. Symlinks anywhere in the
/// source are rejected; a stale `.installed-from` marker is never copied so
/// provenance always reflects *this* install.
fn stage_local_copy(source: &Path, user_plugins_dir: &Path, max_size: u64) -> Result<StagedPlugin> {
    // Validate the source first; this also rejects symlinked roots/manifests.
    PluginManifest::validate_from_path(&source.join("plugin.toml"))
        .map_err(|error| anyhow::anyhow!("source is not a valid plugin bundle: {error}"))?;
    let canonical_source = source
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", source.display()))?;
    if let Ok(canonical_plugins) = user_plugins_dir.canonicalize()
        && (canonical_source == canonical_plugins
            || canonical_source.starts_with(&canonical_plugins))
    {
        bail!(
            "cannot install a bundle from inside the user plugins directory {}; \
             it is already in place",
            canonical_plugins.display()
        );
    }

    let staged_path = fresh_staging_dir(user_plugins_dir)?;
    let result = (|| -> Result<StagedPlugin> {
        let mut budget = CopyBudget::default();
        copy_bundle_regular_files(&canonical_source, &staged_path, max_size, &mut budget)?;
        let (name, content_hash) = validate_staged(&staged_path)?;
        Ok(StagedPlugin {
            name,
            staged_path: staged_path.clone(),
            content_hash,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staged_path);
    }
    result
}

#[derive(Default)]
struct CopyBudget {
    files: usize,
    bytes: u64,
}

fn copy_bundle_regular_files(
    source: &Path,
    dest: &Path,
    max_size: u64,
    budget: &mut CopyBudget,
) -> Result<()> {
    for entry in fs::read_dir(source)
        .with_context(|| format!("failed to read bundle dir {}", source.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(PluginInstallError::SymlinkRejected.into());
        }
        let name = entry.file_name();
        if name == std::ffi::OsStr::new(INSTALLED_FROM_MARKER) {
            continue;
        }
        let target = dest.join(&name);
        if metadata.is_dir() {
            fs::create_dir(&target)
                .with_context(|| format!("failed to create {}", target.display()))?;
            copy_bundle_regular_files(&path, &target, max_size, budget)?;
        } else if metadata.is_file() {
            budget.files = budget.files.saturating_add(1);
            if budget.files > MAX_BUNDLE_FILES {
                bail!("bundle exceeds the {MAX_BUNDLE_FILES} file limit");
            }
            budget.bytes = budget.bytes.saturating_add(metadata.len());
            if budget.bytes > max_size {
                return Err(PluginInstallError::OversizedBundle { limit: max_size }.into());
            }
            fs::copy(&path, &target).with_context(|| {
                format!("failed to copy {} to {}", path.display(), target.display())
            })?;
        }
    }
    Ok(())
}

/// Validate a tarball and extract the `plugin.toml`-rooted subtree into a
/// `.staging-*` sibling of the destination.
fn stage_tarball(bytes: &[u8], user_plugins_dir: &Path, max_size: u64) -> Result<StagedPlugin> {
    let scan = scan_tarball(bytes, max_size)?;
    let staged_path = fresh_staging_dir(user_plugins_dir)?;
    let result = extract_into(&scan, bytes, &staged_path, max_size)
        .and_then(|()| validate_staged(&staged_path));
    match result {
        Ok((name, content_hash)) => Ok(StagedPlugin {
            name,
            staged_path,
            content_hash,
        }),
        Err(error) => {
            let _ = fs::remove_dir_all(&staged_path);
            Err(error)
        }
    }
}

#[derive(Debug)]
struct TarballScan {
    /// Archive-relative directory containing the single `plugin.toml`
    /// (`""` when the manifest sits at the archive root).
    plugin_root: String,
}

/// First pass: validate entry paths, enforce the uncompressed size cap, and
/// locate the single `plugin.toml`. Nothing is written in this pass.
fn scan_tarball(bytes: &[u8], max_size: u64) -> Result<TarballScan> {
    let cursor = std::io::Cursor::new(bytes);
    let gz = GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);

    let mut total_size: u64 = 0;
    let mut manifest_paths: Vec<String> = Vec::new();

    for entry in archive
        .entries()
        .context("failed to read tar entries (corrupt archive?)")?
    {
        let entry = entry.context("failed to read tar entry")?;
        let header = entry.header().clone();
        let path = entry
            .path()
            .context("tar entry has invalid path")?
            .to_path_buf();
        let path_str = path.to_string_lossy().into_owned();
        if !is_safe_path(&path) {
            return Err(PluginInstallError::PathTraversal(path_str).into());
        }
        if let Ok(size) = header.size() {
            total_size = total_size.saturating_add(size);
            if total_size > max_size {
                return Err(PluginInstallError::OversizedBundle { limit: max_size }.into());
            }
        }
        if header.entry_type().is_file()
            && path
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("plugin.toml"))
        {
            manifest_paths.push(path_str);
        }
    }

    if manifest_paths.len() != 1 {
        return Err(PluginInstallError::PluginTomlRoots(manifest_paths.len()).into());
    }
    let manifest = &manifest_paths[0];
    let plugin_root = manifest
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default();
    Ok(TarballScan { plugin_root })
}

/// Second pass: extract only entries under the scanned bundle root.
fn extract_into(scan: &TarballScan, bytes: &[u8], dest: &Path, max_size: u64) -> Result<()> {
    let cursor = std::io::Cursor::new(bytes);
    let gz = GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);
    let mut total_size: u64 = 0;

    for entry in archive
        .entries()
        .context("failed to read tar entries (corrupt archive?)")?
    {
        let mut entry = entry.context("failed to read tar entry")?;
        let header = entry.header().clone();
        let entry_type = header.entry_type();
        let path = entry
            .path()
            .context("tar entry has invalid path")?
            .to_path_buf();
        let path_str = path.to_string_lossy().into_owned();
        if !is_safe_path(&path) {
            return Err(PluginInstallError::PathTraversal(path_str).into());
        }

        // Keep only the bundle subtree. Entries outside it (including any
        // symlinks a mono-repo ships elsewhere) are ignored, never extracted.
        let stripped = if scan.plugin_root.is_empty() {
            path_str.clone()
        } else if path_str == scan.plugin_root {
            String::new()
        } else if let Some(rest) = path_str.strip_prefix(&format!("{}/", scan.plugin_root)) {
            rest.to_string()
        } else {
            continue;
        };
        if stripped.is_empty() {
            // The bundle root directory itself — the staging dir already exists.
            continue;
        }
        // Defense-in-depth: re-validate the stripped path.
        let stripped_path = Path::new(&stripped);
        if !is_safe_path(stripped_path) {
            return Err(PluginInstallError::PathTraversal(stripped).into());
        }
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(PluginInstallError::SymlinkRejected.into());
        }

        let target = dest.join(stripped_path);
        // Final paranoia check: the composed target must stay under dest.
        let target_components: Vec<_> = target.components().collect();
        let dest_components: Vec<_> = dest.components().collect();
        if !target_components.starts_with(dest_components.as_slice()) {
            return Err(PluginInstallError::PathTraversal(stripped).into());
        }

        if entry_type.is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("failed to create dir {}", target.display()))?;
            continue;
        }
        if entry_type.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create dir {}", parent.display()))?;
            }
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .with_context(|| format!("failed to read {}", path.display()))?;
            total_size = total_size.saturating_add(buf.len() as u64);
            if total_size > max_size {
                return Err(PluginInstallError::OversizedBundle { limit: max_size }.into());
            }
            let mut out = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&target)
                .with_context(|| format!("failed to create {}", target.display()))?;
            out.write_all(&buf)
                .with_context(|| format!("failed to write {}", target.display()))?;
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Finalize (atomic rename + marker)
// ─────────────────────────────────────────────────────────────────────────────

fn finalize_install(
    staged: StagedPlugin,
    spec: &str,
    url: Option<&str>,
    source_checksum: &str,
    user_plugins_dir: &Path,
    update: bool,
) -> Result<PluginInstallOutcome> {
    let final_path = user_plugins_dir.join(&staged.name);
    let mut backup_path: Option<PathBuf> = None;
    if final_path.exists() {
        if !update {
            let has_marker = final_path.join(INSTALLED_FROM_MARKER).exists();
            let _ = fs::remove_dir_all(&staged.staged_path);
            if has_marker {
                return Err(PluginInstallError::AlreadyInstalled(staged.name).into());
            }
            return Err(PluginInstallError::NotInstalledHere(staged.name).into());
        }
        if !final_path.join(INSTALLED_FROM_MARKER).exists() {
            let _ = fs::remove_dir_all(&staged.staged_path);
            return Err(PluginInstallError::NotInstalledHere(staged.name).into());
        }
        let backup = user_plugins_dir.join(format!("{}.bak", staged.name));
        if backup.exists() {
            fs::remove_dir_all(&backup).ok();
        }
        fs::rename(&final_path, &backup).with_context(|| {
            format!(
                "failed to backup existing plugin at {}",
                final_path.display()
            )
        })?;
        if let Err(error) = fs::rename(&staged.staged_path, &final_path) {
            fs::rename(&backup, &final_path).ok();
            return Err(error).context("failed to install staged plugin");
        }
        backup_path = Some(backup);
    } else if let Err(error) = fs::rename(&staged.staged_path, &final_path) {
        let _ = fs::remove_dir_all(&staged.staged_path);
        return Err(error).context("failed to install staged plugin");
    }

    // Discovery fail-closed rule: the installed bundle must canonicalize to a
    // direct child of the user plugins root.
    if let Err(error) = ensure_target_within_plugins_dir(&final_path, user_plugins_dir) {
        let _ = fs::remove_dir_all(&final_path);
        if let Some(backup) = backup_path.take() {
            let _ = fs::rename(&backup, &final_path);
        }
        return Err(error);
    }

    // Write the marker last so a partial install never leaves a stale
    // `.installed-from` on disk.
    if let Err(error) = skill_install::write_installed_from_v2(
        &final_path,
        spec,
        url,
        source_checksum,
        &staged.content_hash,
        &staged.name,
    ) {
        let _ = fs::remove_dir_all(&final_path);
        if let Some(backup) = backup_path.take() {
            let _ = fs::rename(&backup, &final_path);
        }
        return Err(error);
    }
    if let Some(backup) = backup_path {
        fs::remove_dir_all(&backup).ok();
    }

    Ok(PluginInstallOutcome::Installed(InstalledPlugin {
        name: staged.name,
        path: final_path,
        content_hash: staged.content_hash,
        source_checksum: source_checksum.to_string(),
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Path guards
// ─────────────────────────────────────────────────────────────────────────────

fn plugin_target_path(name: &str, user_plugins_dir: &Path) -> Result<PathBuf> {
    let name = validate_skill_name_segment(name)
        .map_err(|error| anyhow::anyhow!("plugin name is not a safe directory name: {error:#}"))?;
    Ok(user_plugins_dir.join(name))
}

/// The resolved bundle must be a direct child of the resolved plugins root,
/// matching discovery's fail-closed containment rule.
fn ensure_target_within_plugins_dir(target: &Path, user_plugins_dir: &Path) -> Result<()> {
    let root = fs::canonicalize(user_plugins_dir).with_context(|| {
        format!(
            "failed to resolve plugins directory {}",
            user_plugins_dir.display()
        )
    })?;
    let target = fs::canonicalize(target)
        .with_context(|| format!("failed to resolve {}", target.display()))?;
    if target.parent() != Some(root.as_path()) {
        bail!(
            "plugin path {} escapes plugins directory {}",
            target.display(),
            root.display()
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn write_bundle(root: &Path, dir: &str, name: &str) -> PathBuf {
        let bundle = root.join(dir);
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join("plugin.toml"),
            format!("schema_version = 1\n[plugin]\nname = {name:?}\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
        bundle
    }

    fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (path, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *body).unwrap();
        }
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    fn symlink_tarball(link_path: &str, target: &str, manifest: &str) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let body = b"schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, manifest, &body[..])
            .unwrap();
        let mut link_header = tar::Header::new_gnu();
        link_header.set_entry_type(tar::EntryType::Symlink);
        link_header.set_size(0);
        link_header.set_mode(0o777);
        link_header.set_cksum();
        builder
            .append_link(&mut link_header, link_path, target)
            .unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    /// Emit one raw ustar file entry with an arbitrary (possibly hostile) name.
    /// `tar::Builder` refuses `..` and absolute paths on write, so adversarial
    /// archives have to be assembled byte-by-byte.
    fn raw_tar_file_entry(name: &[u8], body: &[u8]) -> Vec<u8> {
        let mut header = [0_u8; 512];
        header[..name.len()].copy_from_slice(name);
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        let size = format!("{:011o}\0", body.len());
        header[124..136].copy_from_slice(size.as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[148..156].copy_from_slice(b"        ");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        let checksum = format!("{checksum:06o}\0 ");
        header[148..156].copy_from_slice(checksum.as_bytes());
        let mut out = header.to_vec();
        out.extend_from_slice(body);
        let padding = (512 - body.len() % 512) % 512;
        out.extend(std::iter::repeat_n(0, padding));
        out
    }

    fn raw_tarball(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
        use std::io::Write as _;

        let mut tar_bytes = Vec::new();
        for (name, body) in entries {
            tar_bytes.extend(raw_tar_file_entry(name, body));
        }
        tar_bytes.extend(std::iter::repeat_n(0, 1024));
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn allow_all() -> NetworkPolicy {
        NetworkPolicy {
            default: crate::network_policy::DecisionToml::Allow,
            ..Default::default()
        }
    }

    fn no_conflict() -> impl Fn(&str) -> Option<String> {
        |_| None
    }

    // ── scan/extract rules ────────────────────────────────────────────────

    #[test]
    fn scan_rejects_path_traversal() {
        let bytes = raw_tarball(&[(
            b"repo-main/../evil/plugin.toml",
            b"schema_version = 1\n[plugin]\nname = \"evil\"\n",
        )]);
        let err = scan_tarball(&bytes, DEFAULT_MAX_SIZE_BYTES).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<PluginInstallError>(),
                Some(PluginInstallError::PathTraversal(_))
            ),
            "got: {err:#}"
        );
    }

    #[test]
    fn scan_rejects_absolute_paths() {
        let bytes = raw_tarball(&[(
            b"/tmp/evil/plugin.toml",
            b"schema_version = 1\n[plugin]\nname = \"evil\"\n",
        )]);
        assert!(scan_tarball(&bytes, DEFAULT_MAX_SIZE_BYTES).is_err());
    }

    #[test]
    fn scan_enforces_size_cap() {
        let body = vec![b'x'; 1024];
        let bytes = tarball(&[
            (
                "repo-main/plugin.toml",
                b"schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n",
            ),
            ("repo-main/blob.bin", &body),
        ]);
        let err = scan_tarball(&bytes, 512).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<PluginInstallError>(),
                Some(PluginInstallError::OversizedBundle { .. })
            ),
            "got: {err:#}"
        );
    }

    #[test]
    fn scan_requires_exactly_one_plugin_toml_root() {
        let zero = tarball(&[("repo-main/README.md", b"no manifest here")]);
        let err = scan_tarball(&zero, DEFAULT_MAX_SIZE_BYTES).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<PluginInstallError>(),
                Some(PluginInstallError::PluginTomlRoots(0))
            ),
            "got: {err:#}"
        );

        let manifest = b"schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n";
        let two = tarball(&[
            ("repo-main/plugin.toml", manifest),
            ("repo-main/examples/other/plugin.toml", manifest),
        ]);
        let err = scan_tarball(&two, DEFAULT_MAX_SIZE_BYTES).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<PluginInstallError>(),
                Some(PluginInstallError::PluginTomlRoots(2))
            ),
            "got: {err:#}"
        );
    }

    #[test]
    fn extract_rejects_symlinks_inside_the_bundle_subtree() {
        let bytes = symlink_tarball(
            "repo-main/evil-link",
            "/etc/passwd",
            "repo-main/plugin.toml",
        );
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let err = stage_tarball(&bytes, &plugins, DEFAULT_MAX_SIZE_BYTES).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<PluginInstallError>(),
                Some(PluginInstallError::SymlinkRejected)
            ),
            "got: {err:#}"
        );
        assert!(fs::read_dir(&plugins).unwrap().next().is_none());
    }

    #[test]
    fn extract_ignores_entries_outside_the_bundle_subtree() {
        let manifest = b"schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n";
        let bytes = tarball(&[
            ("repo-main/bundles/demo/plugin.toml", manifest),
            (
                "repo-main/bundles/demo/skills/a/SKILL.md",
                b"---\nname: a\ndescription: a\n---\n",
            ),
            ("repo-main/other/plugin.toml.bak", b"ignored"),
            ("repo-main/README.md", b"repo docs stay behind"),
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let staged = stage_tarball(&bytes, &plugins, DEFAULT_MAX_SIZE_BYTES).unwrap();
        assert_eq!(staged.name, "demo");
        assert!(staged.staged_path.join("plugin.toml").exists());
        assert!(staged.staged_path.join("skills/a/SKILL.md").exists());
        assert!(!staged.staged_path.join("README.md").exists());
        assert!(!staged.staged_path.join("other").exists());
        fs::remove_dir_all(&staged.staged_path).unwrap();
    }

    // ── local copy rules ──────────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_from_local_path_copies_and_marks_the_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let source = write_bundle(tmp.path(), "src/demo", "demo");
        fs::create_dir_all(source.join("skills/hello")).unwrap();
        fs::write(
            source.join("skills/hello/SKILL.md"),
            "---\nname: hello\ndescription: hi\n---\nbody\n",
        )
        .unwrap();

        let outcome = install(
            PluginInstallSource::parse(source.to_str().unwrap()).unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &allow_all(),
            false,
            &no_conflict(),
        )
        .await
        .unwrap();
        let PluginInstallOutcome::Installed(installed) = outcome else {
            panic!("expected install to succeed");
        };
        assert_eq!(installed.name, "demo");
        assert_eq!(installed.path, plugins.join("demo"));
        assert!(installed.path.join("plugin.toml").exists());
        assert!(installed.path.join("skills/hello/SKILL.md").exists());
        let marker: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(installed.path.join(INSTALLED_FROM_MARKER)).unwrap(),
        )
        .unwrap();
        assert!(marker["spec"].as_str().unwrap().starts_with("path:"));
        // Local copies must not inherit a stale provenance marker.
        assert_ne!(marker["spec"].as_str().unwrap(), "path:");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_refuses_to_overwrite_a_hand_placed_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        write_bundle(&plugins, "demo", "demo");
        let source = write_bundle(tmp.path(), "src/demo", "demo");

        let err = install(
            PluginInstallSource::parse(source.to_str().unwrap()).unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &allow_all(),
            false,
            &no_conflict(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<PluginInstallError>(),
                Some(PluginInstallError::NotInstalledHere(_))
            ),
            "hand-placed bundle must be protected, got: {err:#}"
        );
        assert!(
            !plugins.join("demo/skills").exists(),
            "no partial overwrite"
        );

        // A bundle that *was* installed here gets the AlreadyInstalled hint.
        fs::write(plugins.join("demo").join(INSTALLED_FROM_MARKER), "{}").unwrap();
        let err = install(
            PluginInstallSource::parse(source.to_str().unwrap()).unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &allow_all(),
            false,
            &no_conflict(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<PluginInstallError>(),
                Some(PluginInstallError::AlreadyInstalled(_))
            ),
            "got: {err:#}"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_install_rejects_symlinks_in_the_source() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let source = write_bundle(tmp.path(), "src/demo", "demo");
        std::os::unix::fs::symlink("/etc/passwd", source.join("linked")).unwrap();

        let err = install(
            PluginInstallSource::parse(source.to_str().unwrap()).unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &allow_all(),
            false,
            &no_conflict(),
        )
        .await
        .unwrap_err();
        // The bundle validator rejects symlinked content before any copy runs.
        assert!(format!("{err:#}").contains("symbolic link"), "got: {err:#}");
        assert!(!plugins.join("demo").exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_refuses_sources_inside_the_plugins_root() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let nested = write_bundle(&plugins, "demo", "demo");
        let err = install(
            PluginInstallSource::parse(nested.to_str().unwrap()).unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &allow_all(),
            false,
            &no_conflict(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("inside the user plugins directory"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_enforces_the_name_conflict_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let source = write_bundle(tmp.path(), "src/demo", "demo");
        let err = install(
            PluginInstallSource::parse(source.to_str().unwrap()).unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &allow_all(),
            false,
            &|name| Some(format!("name '{name}' is shadowed by a builtin bundle")),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("shadowed by a builtin bundle"));
        assert!(!plugins.join("demo").exists());
        // The staging dir must be cleaned up on the conflict path.
        assert!(
            !fs::read_dir(&plugins)
                .map(|mut entries| entries.any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".staging-")))
                .unwrap_or(false)
        );
    }

    // ── update / uninstall ────────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_refuses_local_installs_and_missing_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let source = write_bundle(tmp.path(), "src/demo", "demo");
        install(
            PluginInstallSource::parse(source.to_str().unwrap()).unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &allow_all(),
            false,
            &no_conflict(),
        )
        .await
        .unwrap();

        let err = update("demo", &plugins, DEFAULT_MAX_SIZE_BYTES, &allow_all())
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("local path"), "got: {err:#}");

        write_bundle(&plugins, "hand", "hand");
        let err = update("hand", &plugins, DEFAULT_MAX_SIZE_BYTES, &allow_all())
            .await
            .unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<PluginInstallError>(),
                Some(PluginInstallError::NotInstalledHere(_))
            ),
            "got: {err:#}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn uninstall_requires_the_marker_and_removes_the_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let source = write_bundle(tmp.path(), "src/demo", "demo");
        install(
            PluginInstallSource::parse(source.to_str().unwrap()).unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &allow_all(),
            false,
            &no_conflict(),
        )
        .await
        .unwrap();

        uninstall("demo", &plugins).unwrap();
        assert!(!plugins.join("demo").exists());

        write_bundle(&plugins, "hand", "hand");
        let err = uninstall("hand", &plugins).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<PluginInstallError>(),
                Some(PluginInstallError::NotInstalledHere(_))
            ),
            "got: {err:#}"
        );
        assert!(plugins.join("hand").exists(), "hand-placed bundle survives");
        assert!(uninstall("missing", &plugins).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_rejects_symlink_targets_escaping_the_plugins_root() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&plugins).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join(INSTALLED_FROM_MARKER), "{}").unwrap();
        std::os::unix::fs::symlink(&outside, plugins.join("linked")).unwrap();

        let err = uninstall("linked", &plugins).unwrap_err();
        assert!(format!("{err:#}").contains("escapes plugins directory"));
        assert!(outside.exists());
    }

    // ── source parsing ────────────────────────────────────────────────────

    #[test]
    fn parse_routes_remote_and_local_specs() {
        assert_eq!(
            PluginInstallSource::parse("github:owner/repo").unwrap(),
            PluginInstallSource::Remote(InstallSource::GitHubRepo("owner/repo".into()))
        );
        assert_eq!(
            PluginInstallSource::parse("https://example.com/p.tar.gz").unwrap(),
            PluginInstallSource::Remote(InstallSource::DirectUrl(
                "https://example.com/p.tar.gz".into()
            ))
        );
        assert_eq!(
            PluginInstallSource::parse("./bundles/demo").unwrap(),
            PluginInstallSource::LocalPath(PathBuf::from("./bundles/demo"))
        );
        assert_eq!(
            PluginInstallSource::parse("path:/opt/demo").unwrap(),
            PluginInstallSource::LocalPath(PathBuf::from("/opt/demo"))
        );
        assert!(PluginInstallSource::parse("").is_err());
        assert!(PluginInstallSource::parse("   ").is_err());
        assert!(PluginInstallSource::parse("path:").is_err());
    }

    // ── remote fetch against a loopback server ────────────────────────────

    /// Serve each body once, in order, over plain loopback HTTP.
    fn serve_bodies(bodies: Vec<Vec<u8>>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for body in bodies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                // Consume the request headers before responding.
                let mut request = Vec::new();
                let mut buf = [0_u8; 1024];
                loop {
                    use std::io::Read as _;
                    let read = stream.read(&mut buf).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                use std::io::Write as _;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        format!("http://127.0.0.1:{port}/plugin.tar.gz")
    }

    fn loopback_policy() -> NetworkPolicy {
        NetworkPolicy {
            allow: vec!["127.0.0.1".to_string()],
            ..Default::default()
        }
    }

    fn remote_bundle_bytes(name: &str, extra: &[u8]) -> Vec<u8> {
        let manifest =
            format!("schema_version = 1\n[plugin]\nname = {name:?}\nversion = \"1.0.0\"\n");
        tarball(&[
            ("repo-main/plugin.toml", manifest.as_bytes()),
            ("repo-main/data.txt", extra),
        ])
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_is_a_digest_noop_until_the_upstream_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        let v1 = remote_bundle_bytes("demo", b"v1");
        let v2 = remote_bundle_bytes("demo", b"v2-changed");
        // install, update (same bytes → no-op), update (new bytes → swap).
        let url = serve_bodies(vec![v1.clone(), v1.clone(), v2.clone()]);

        let outcome = install(
            PluginInstallSource::parse(&url).unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &loopback_policy(),
            false,
            &no_conflict(),
        )
        .await
        .unwrap();
        let PluginInstallOutcome::Installed(installed) = outcome else {
            panic!("expected install to succeed");
        };
        assert_eq!(installed.name, "demo");
        assert_eq!(
            fs::read(plugins.join("demo/data.txt")).unwrap(),
            b"v1".to_vec()
        );

        let no_change = update("demo", &plugins, DEFAULT_MAX_SIZE_BYTES, &loopback_policy())
            .await
            .unwrap();
        assert!(
            matches!(no_change, PluginUpdateResult::NoChange),
            "identical upstream bytes must be a digest no-op"
        );
        assert_eq!(
            fs::read(plugins.join("demo/data.txt")).unwrap(),
            b"v1".to_vec()
        );

        let changed = update("demo", &plugins, DEFAULT_MAX_SIZE_BYTES, &loopback_policy())
            .await
            .unwrap();
        let PluginUpdateResult::Updated(updated) = changed else {
            panic!("changed upstream bytes must swap the bundle");
        };
        assert_eq!(
            fs::read(updated.path.join("data.txt")).unwrap(),
            b"v2-changed".to_vec()
        );
        // The marker records the new checksum, so a following update against
        // the same bytes would be a no-op again.
        let marker: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(updated.path.join(INSTALLED_FROM_MARKER)).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["source_checksum"].as_str().unwrap(), sha256_hex(&v2));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_install_surfaces_policy_gates_without_touching_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");

        // Default policy prompts for unknown hosts.
        let outcome = install(
            PluginInstallSource::parse("https://plugin.example.invalid/x.tar.gz").unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &NetworkPolicy::default(),
            false,
            &no_conflict(),
        )
        .await
        .unwrap();
        assert!(
            matches!(
                outcome,
                PluginInstallOutcome::NeedsApproval(ref host) if host == "plugin.example.invalid"
            ),
            "got: {outcome:?}"
        );

        let denied = NetworkPolicy {
            deny: vec!["plugin.example.invalid".to_string()],
            ..Default::default()
        };
        let outcome = install(
            PluginInstallSource::parse("https://plugin.example.invalid/x.tar.gz").unwrap(),
            &plugins,
            DEFAULT_MAX_SIZE_BYTES,
            &denied,
            false,
            &no_conflict(),
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, PluginInstallOutcome::NetworkDenied(_)),
            "got: {outcome:?}"
        );
        assert!(!plugins.join("demo").exists());
    }
}
