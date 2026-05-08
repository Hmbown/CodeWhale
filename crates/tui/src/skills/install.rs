//! Community-skill installer (#140).
//!
//! Pulls user-authored skills from GitHub or direct tarball URLs, validates them
//! against a path-traversal- and size-bounded extractor, and writes them into
//! `<skills_dir>/<name>/`. No backend service, no auto-execution: every install
//! is gated by the per-domain [`crate::network_policy::NetworkPolicy`] and
//! validation rejects any tarball entry that escapes the destination directory.
//!
//! Public surface:
//!
//! * [`InstallSource`] — `github:owner/repo`, raw URL, or curated registry
//!   name. Parsed from a single string with [`InstallSource::parse`].
//! * [`install`] / [`update`] / [`uninstall`] — async install, atomic update,
//!   and clean uninstall. All three preserve a `.installed-from` marker so the
//!   bundled `skill-creator` (which lacks the marker) is never touched.
//! * [`InstallOutcome`] — `Installed` / `NeedsApproval(host)` /
//!   `NetworkDenied(host)`. The `NeedsApproval` variant is returned without
//!   side effects so the caller (slash-command, runtime API, etc.) can route
//!   through its own approval flow.
//!
//! # Hard rules
//!
//! * Validation extracts to a temp directory first. The destination path is
//!   only created (via atomic rename) once the tarball clears every check.
//!   Half-installed skills can never appear on disk.
//! * Path traversal rejection covers both `..` segments and absolute paths.
//!   Symlinks inside the selected skill subtree are rejected — there's no use
//!   case for them in a SKILL.md bundle and they're a notorious foothold for
//!   escape. Multi-skill repository archives may contain unrelated symlinks
//!   outside that selected subtree; those entries are ignored and never
//!   extracted.
//! * No `+x` is granted on extracted files. The optional `/skill trust <name>`
//!   command writes a `.trusted` marker; tool-execution gating is a separate
//!   concern that lives next to the tool registry.

use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::network_policy::{Decision, NetworkPolicy, host_from_url};

/// Cache directory for registry-synced skills.
///
/// Lives at `~/.deepseek/cache/skills/` so it's separate from user-installed
/// skills and can be blown away without losing anything irreplaceable.
pub fn default_cache_skills_dir() -> PathBuf {
    dirs::home_dir().map_or_else(
        || PathBuf::from("/tmp/deepseek/cache/skills"),
        |p| p.join(".deepseek").join("cache").join("skills"),
    )
}

/// Default registry. Falls back to a community-curated `index.json` hosted on
/// GitHub raw; users can override via `[skills] registry_url` in config.toml.
pub const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/Hmbown/deepseek-skills/main/index.json";

/// Default per-skill size cap (5 MiB). Honored at unpack time so a malicious
/// gzip bomb can't blow up RAM.
pub const DEFAULT_MAX_SIZE_BYTES: u64 = 5 * 1024 * 1024;

/// File written under each installed skill so [`update`] / [`uninstall`] can
/// recover the original [`InstallSource`] without re-parsing user input.
pub const INSTALLED_FROM_MARKER: &str = ".installed-from";

/// File written under each trusted skill. Currently advisory (the install path
/// never auto-runs anything) — the runtime tool-invocation gate consults this
/// marker before executing scripts that ship with the skill.
pub const TRUSTED_MARKER: &str = ".trusted";

// ─────────────────────────────────────────────────────────────────────────────
// Source parsing
// ─────────────────────────────────────────────────────────────────────────────

/// Where a skill is being installed from. See [`InstallSource::parse`] for the
/// accepted spec syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallSource {
    /// `github:owner/repo`. Resolved to
    /// `https://github.com/<owner>/<repo>/archive/refs/heads/main.tar.gz`
    /// with a `master.tar.gz` fallback on 404.
    GitHubRepo(String),
    /// Raw `http(s)://…` tarball URL. Used as-is.
    DirectUrl(String),
    /// Curated registry lookup key. Looked up via the configured `registry_url`.
    Registry(String),
}

impl InstallSource {
    /// Parse a user-supplied spec. Empty / whitespace-only input is rejected.
    ///
    /// * `github:owner/repo` → [`InstallSource::GitHubRepo`]
    /// * `https://github.com/owner/repo[.git]` (no path past the repo) →
    ///   [`InstallSource::GitHubRepo`]
    /// * any other `http://` or `https://` prefix → [`InstallSource::DirectUrl`]
    /// * anything else → [`InstallSource::Registry`]
    pub fn parse(spec: &str) -> Result<Self> {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            bail!("install source must not be empty");
        }
        if let Some(rest) = trimmed.strip_prefix("github:") {
            let rest = rest.trim();
            // Reject obviously bogus values up front. We intentionally accept
            // case-insensitive owner/repo so `github:Hmbown/Foo` works.
            let (owner, repo) = rest.split_once('/').with_context(|| {
                format!("github source must be 'github:owner/repo' (got {spec})")
            })?;
            let owner = owner.trim();
            let repo = repo.trim().trim_end_matches('/');
            if owner.is_empty() || repo.is_empty() {
                bail!("github source must be 'github:owner/repo' (got {spec})");
            }
            if owner.contains('/') || repo.contains('/') {
                bail!("github source must be 'github:owner/repo' (got {spec})");
            }
            return Ok(Self::GitHubRepo(format!("{owner}/{repo}")));
        }
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            if let Some(repo) = parse_github_browser_url(trimmed) {
                return Ok(Self::GitHubRepo(repo));
            }
            return Ok(Self::DirectUrl(trimmed.to_string()));
        }
        Ok(Self::Registry(trimmed.to_string()))
    }
}

/// Detect bare `https://github.com/<owner>/<repo>` URLs (with or without a
/// trailing `.git`) and return `owner/repo`. Returns `None` for any URL that
/// already points at a specific archive / blob / tree path — those are real
/// direct URLs and the caller fetches them as-is.
fn parse_github_browser_url(url: &str) -> Option<String> {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, rest) = after_scheme.split_once('/')?;
    if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
        return None;
    }
    let trimmed = rest.trim_end_matches('/');
    let mut parts = trimmed.splitn(3, '/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim().trim_end_matches(".git");
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    // If there is a third segment, the URL points at a sub-resource
    // (`/archive/...`, `/blob/...`, `/tree/...`). Treat that as a real direct
    // URL — the user explicitly wants whatever lives at that path.
    if parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Outcome / result types
// ─────────────────────────────────────────────────────────────────────────────

/// Outcome of an install attempt.
#[derive(Debug)]
pub enum InstallOutcome {
    /// The skill was installed (or already present and idempotent).
    Installed(InstalledSkill),
    /// The host requires user approval before the install can proceed. The
    /// caller should surface this through whatever approval pathway it has and
    /// retry once approved (typically by adding the host to the policy's
    /// allow list).
    NeedsApproval(String),
    /// The host is denied by network policy. The install is aborted.
    NetworkDenied(String),
}

/// Metadata for a successfully installed skill.
#[derive(Debug, Clone)]
pub struct InstalledSkill {
    /// Skill name (taken from SKILL.md frontmatter).
    pub name: String,
    /// Final on-disk path: `<skills_dir>/<name>/`.
    pub path: PathBuf,
    /// SHA-256 over the downloaded tarball bytes. Used by [`update`] to detect
    /// upstream changes without re-extracting; also surfaced for telemetry /
    /// future signature-verification work.
    #[allow(dead_code)]
    pub source_checksum: String,
}

/// Result of an [`update`] call.
#[derive(Debug)]
pub enum UpdateResult {
    /// Upstream tarball is byte-identical to the on-disk checksum; no action.
    NoChange,
    /// Upstream changed and the on-disk install was atomically replaced.
    Updated(InstalledSkill),
    /// Network policy short-circuited the update. Same semantics as
    /// [`InstallOutcome::NeedsApproval`].
    NeedsApproval(String),
    /// Network policy denied the update.
    NetworkDenied(String),
}

/// Errors that can happen during install. Most variants are flattened into
/// `anyhow::Error` at the public boundary; this enum is used internally so
/// tests can pattern-match without parsing strings.
#[derive(Debug, Error)]
pub enum InstallError {
    #[error("entry escapes destination directory: {0}")]
    PathTraversal(String),
    #[error("entry is too large; uncompressed total would exceed {limit} bytes")]
    OversizedTarball { limit: u64 },
    #[error("missing SKILL.md in archive")]
    MissingSkillMd,
    #[error("SKILL.md frontmatter missing required field: {0}")]
    MissingFrontmatterField(&'static str),
    #[error("symlinks are not allowed in skill tarballs")]
    SymlinkRejected,
    #[error("skill '{0}' is already installed; use update or remove it first")]
    AlreadyInstalled(String),
    #[error("skill '{0}' was not installed via /skill install (no .installed-from marker)")]
    NotInstalledHere(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Install a community skill into `skills_dir`.
///
/// Steps:
///
/// 1. Resolve `source` to one or more candidate URLs (GitHub adds a
///    `master` fallback after `main`).
/// 2. Consult `network` for the host. `Allow` proceeds; `Deny` returns
///    [`InstallOutcome::NetworkDenied`]; `Prompt` returns
///    [`InstallOutcome::NeedsApproval`] without touching disk.
/// 3. Stream the tarball into a tempfile (capped at `max_size`).
/// 4. Validate the archive (path-traversal, size, no symlinks in the selected
///    skill subtree, SKILL.md present with required frontmatter fields) into a
///    sibling `<name>.tmp/` directory.
/// 5. Atomic-rename `<name>.tmp/` → `<name>/`.
/// 6. Write `.installed-from` and return [`InstalledSkill`].
///
/// `update = false` rejects an existing destination. Pass `update = true`
/// from [`update`] to allow replacement.
///
/// Convenience wrapper over [`install_with_registry`] that uses the bundled
/// [`DEFAULT_REGISTRY_URL`]. Public for downstream consumers (tests, runtime
/// API) even though the slash-command path always goes through
/// [`install_with_registry`] so the user's configured registry wins.
#[allow(dead_code)]
pub async fn install(
    source: InstallSource,
    skills_dir: &Path,
    max_size: u64,
    network: &NetworkPolicy,
    update: bool,
) -> Result<InstallOutcome> {
    install_with_registry(
        source,
        skills_dir,
        max_size,
        network,
        update,
        DEFAULT_REGISTRY_URL,
    )
    .await
}

/// Same as [`install`] but lets the caller override the registry URL. Useful
/// for tests; the slash-command path always uses the configured registry.
pub async fn install_with_registry(
    source: InstallSource,
    skills_dir: &Path,
    max_size: u64,
    network: &NetworkPolicy,
    update: bool,
    registry_url: &str,
) -> Result<InstallOutcome> {
    let urls = candidate_urls(&source, network, registry_url).await?;
    let urls = match urls {
        UrlResolution::Resolved(urls) => urls,
        UrlResolution::NeedsApproval(host) => return Ok(InstallOutcome::NeedsApproval(host)),
        UrlResolution::Denied(host) => return Ok(InstallOutcome::NetworkDenied(host)),
    };

    // Try each URL in order — GitHub returns 404 for `main` on master-only
    // repos, and we don't want to fail the install on that.
    let (bytes, source_url) = match download_first_success(&urls, network, max_size).await? {
        DownloadOutcome::Bytes { bytes, url } => (bytes, url),
        DownloadOutcome::NeedsApproval(host) => return Ok(InstallOutcome::NeedsApproval(host)),
        DownloadOutcome::Denied(host) => return Ok(InstallOutcome::NetworkDenied(host)),
    };

    // Compute a checksum before unpacking so [`update`] can detect upstream
    // no-op changes without redoing the extract.
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let checksum = format!("{:x}", hasher.finalize());

    let staged = stage_tarball(&bytes, skills_dir, max_size)?;

    // Move the staged dir into its final location. If `update` is set and the
    // destination exists, replace it; otherwise reject.
    let final_path = skills_dir.join(&staged.skill_name);
    if final_path.exists() {
        if !update {
            // Clean up the staging dir before returning the error.
            let _ = fs::remove_dir_all(&staged.staged_path);
            return Err(InstallError::AlreadyInstalled(staged.skill_name).into());
        }
        // Best-effort backup-then-replace; on failure we restore the original.
        let backup = skills_dir.join(format!("{}.bak", staged.skill_name));
        // If a previous failed update left a stale `.bak/`, drop it.
        if backup.exists() {
            fs::remove_dir_all(&backup).ok();
        }
        fs::rename(&final_path, &backup).with_context(|| {
            format!(
                "failed to backup existing skill at {}",
                final_path.display()
            )
        })?;
        if let Err(err) = fs::rename(&staged.staged_path, &final_path) {
            // Roll back: restore the backup so the user isn't left with an
            // empty skill directory.
            fs::rename(&backup, &final_path).ok();
            return Err(err).context("failed to install staged skill");
        }
        fs::remove_dir_all(&backup).ok();
    } else {
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create skills directory {}", parent.display())
            })?;
        }
        fs::rename(&staged.staged_path, &final_path).context("failed to install staged skill")?;
    }

    // Write the marker last so a partial install never leaves a stale
    // .installed-from on disk.
    let marker_body = serde_json::json!({
        "spec": source_spec_string(&source),
        "url": source_url,
        "checksum": checksum,
    })
    .to_string();
    fs::write(final_path.join(INSTALLED_FROM_MARKER), marker_body).with_context(|| {
        format!(
            "failed to write {} marker for skill {}",
            INSTALLED_FROM_MARKER, staged.skill_name
        )
    })?;

    Ok(InstallOutcome::Installed(InstalledSkill {
        name: staged.skill_name,
        path: final_path,
        source_checksum: checksum,
    }))
}

/// Re-fetch a previously installed skill and replace it on disk if the
/// upstream tarball changed.
///
/// Reads `.installed-from` to recover the original [`InstallSource`], so
/// a skill installed via `/skill install github:foo/bar` can be updated via
/// `/skill update bar` without the user re-typing the spec.
///
/// Convenience wrapper over [`update_with_registry`].
#[allow(dead_code)]
pub async fn update(
    name: &str,
    skills_dir: &Path,
    max_size: u64,
    network: &NetworkPolicy,
) -> Result<UpdateResult> {
    update_with_registry(name, skills_dir, max_size, network, DEFAULT_REGISTRY_URL).await
}

/// Same as [`update`] but lets the caller override the registry URL.
pub async fn update_with_registry(
    name: &str,
    skills_dir: &Path,
    max_size: u64,
    network: &NetworkPolicy,
    registry_url: &str,
) -> Result<UpdateResult> {
    let target = skills_dir.join(name);
    let marker_path = target.join(INSTALLED_FROM_MARKER);
    if !marker_path.exists() {
        return Err(InstallError::NotInstalledHere(name.to_string()).into());
    }
    let marker_body = fs::read_to_string(&marker_path)
        .with_context(|| format!("failed to read {}", marker_path.display()))?;
    let marker: InstalledFromMarker = serde_json::from_str(&marker_body)
        .with_context(|| format!("malformed {} for {name}", INSTALLED_FROM_MARKER))?;

    // Re-resolve the URL, taking the existing checksum as a short-circuit hint:
    // we still hit the network so the user gets a useful "no upstream change"
    // signal, but we skip the unpack step if the bytes match.
    let source = InstallSource::parse(&marker.spec)?;
    let urls = match candidate_urls(&source, network, registry_url).await? {
        UrlResolution::Resolved(urls) => urls,
        UrlResolution::NeedsApproval(host) => return Ok(UpdateResult::NeedsApproval(host)),
        UrlResolution::Denied(host) => return Ok(UpdateResult::NetworkDenied(host)),
    };
    let (bytes, _url) = match download_first_success(&urls, network, max_size).await? {
        DownloadOutcome::Bytes { bytes, url } => (bytes, url),
        DownloadOutcome::NeedsApproval(host) => return Ok(UpdateResult::NeedsApproval(host)),
        DownloadOutcome::Denied(host) => return Ok(UpdateResult::NetworkDenied(host)),
    };

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let checksum = format!("{:x}", hasher.finalize());
    if checksum == marker.checksum {
        return Ok(UpdateResult::NoChange);
    }

    // Bytes changed — fall back to the regular install path with `update = true`
    // so we get the same atomic-replace semantics.
    let outcome =
        install_with_registry(source, skills_dir, max_size, network, true, registry_url).await?;
    match outcome {
        InstallOutcome::Installed(installed) => Ok(UpdateResult::Updated(installed)),
        InstallOutcome::NeedsApproval(host) => Ok(UpdateResult::NeedsApproval(host)),
        InstallOutcome::NetworkDenied(host) => Ok(UpdateResult::NetworkDenied(host)),
    }
}

/// Remove a community-installed skill.
///
/// Refuses to touch any directory that doesn't carry the `.installed-from`
/// marker — that's our cue that it's user-owned and not a system skill.
pub fn uninstall(name: &str, skills_dir: &Path) -> Result<()> {
    let target = skills_dir.join(name);
    if !target.exists() {
        bail!("skill '{name}' is not installed at {}", target.display());
    }
    if !target.join(INSTALLED_FROM_MARKER).exists() {
        return Err(InstallError::NotInstalledHere(name.to_string()).into());
    }
    fs::remove_dir_all(&target)
        .with_context(|| format!("failed to remove {}", target.display()))?;
    Ok(())
}

/// Mark a community-installed skill as trusted. Currently a marker file only;
/// callers that wire tool execution against `<name>/scripts/` consult the file
/// before invoking anything. No-op if already trusted.
///
/// Refuses to mark system skills (no `.installed-from`) so the bundled
/// `skill-creator` doesn't accidentally inherit elevated tool privileges.
pub fn trust(name: &str, skills_dir: &Path) -> Result<()> {
    let target = skills_dir.join(name);
    if !target.exists() {
        bail!("skill '{name}' is not installed at {}", target.display());
    }
    if !target.join(INSTALLED_FROM_MARKER).exists() {
        return Err(InstallError::NotInstalledHere(name.to_string()).into());
    }
    let marker = target.join(TRUSTED_MARKER);
    if !marker.exists() {
        fs::write(
            &marker,
            "Skill scripts/ are user-trusted. Delete this file to revoke.\n",
        )
        .with_context(|| format!("failed to write {}", marker.display()))?;
    }
    Ok(())
}

/// Fetch the curated registry and return the parsed entries.
///
/// Honours `network` (skipping the call entirely on Deny / Prompt).
pub async fn fetch_registry(
    network: &NetworkPolicy,
    registry_url: &str,
) -> Result<RegistryFetchResult> {
    let host = match host_from_url(registry_url) {
        Some(host) => host,
        None => bail!("invalid registry url: {registry_url}"),
    };
    match network.decide(&host) {
        Decision::Allow => {}
        Decision::Deny => return Ok(RegistryFetchResult::Denied(host)),
        Decision::Prompt => return Ok(RegistryFetchResult::NeedsApproval(host)),
    }
    let body = reqwest::get(registry_url)
        .await
        .with_context(|| format!("failed to fetch registry {registry_url}"))?
        .error_for_status()
        .with_context(|| format!("registry {registry_url} returned an error status"))?
        .text()
        .await
        .with_context(|| format!("failed to read registry body from {registry_url}"))?;
    let parsed: RegistryDocument = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse registry json from {registry_url}"))?;
    Ok(RegistryFetchResult::Loaded(parsed))
}

// ─────────────────────────────────────────────────────────────────────────────
// Registry sync (issue #433)
// ─────────────────────────────────────────────────────────────────────────────

/// Outcome of a single skill entry during [`sync_registry`].
#[derive(Debug, Clone)]
pub enum SkillSyncOutcome {
    /// Skill downloaded and written to the cache directory.
    Downloaded { name: String, path: PathBuf },
    /// Cached bytes match the upstream ETag / SHA-256; nothing written.
    Fresh { name: String },
    /// Skill download failed; the error is non-fatal so the sync continues.
    Failed { name: String, reason: String },
    /// Network policy blocked the download host.
    Denied { name: String, host: String },
    /// Network policy requires user approval for the download host.
    NeedsApproval { name: String, host: String },
}

/// Overall result of [`sync_registry`].
#[derive(Debug)]
pub enum SyncResult {
    /// Sync completed. `outcomes` contains one entry per skill in the index.
    Done { outcomes: Vec<SkillSyncOutcome> },
    /// The registry fetch was blocked by network policy.
    RegistryDenied(String),
    /// The registry fetch requires user approval.
    RegistryNeedsApproval(String),
}

/// Freshness metadata written alongside each cached skill so subsequent syncs
/// can skip unchanged content.
#[derive(Debug, Serialize, Deserialize)]
struct CacheMeta {
    /// ETag returned by the server for the primary asset, if any.
    #[serde(default)]
    etag: Option<String>,
    /// SHA-256 hex digest of the downloaded bytes.
    sha256: String,
    /// Source URL the asset was fetched from.
    url: String,
}

/// Sync the remote registry to the local cache.
///
/// For every skill listed in `index.json` this function:
///
/// 1. Resolves the download URL (same logic as `install`).
/// 2. Checks the cached [`CacheMeta`] (etag + sha256) for freshness; skips
///    the download if unchanged.
/// 3. Downloads SKILL.md (and any companion files if the source is a tarball)
///    into `<cache_dir>/<name>/`.
/// 4. Writes updated [`CacheMeta`] so the next sync is fast.
///
/// Failures per-skill are non-fatal: [`SkillSyncOutcome::Failed`] is recorded
/// and the sync continues. The caller decides how to surface per-skill errors.
pub async fn sync_registry(
    network: &NetworkPolicy,
    registry_url: &str,
    cache_dir: &Path,
    max_size: u64,
) -> Result<SyncResult> {
    let doc = match fetch_registry(network, registry_url).await? {
        RegistryFetchResult::Loaded(doc) => doc,
        RegistryFetchResult::Denied(host) => return Ok(SyncResult::RegistryDenied(host)),
        RegistryFetchResult::NeedsApproval(host) => {
            return Ok(SyncResult::RegistryNeedsApproval(host));
        }
    };

    let mut outcomes = Vec::new();

    for (name, entry) in &doc.skills {
        let outcome = sync_one_skill(name, entry, network, cache_dir, max_size).await;
        outcomes.push(outcome);
    }

    Ok(SyncResult::Done { outcomes })
}

/// Sync a single skill entry from the registry into the cache directory.
async fn sync_one_skill(
    name: &str,
    entry: &RegistryEntry,
    network: &NetworkPolicy,
    cache_dir: &Path,
    max_size: u64,
) -> SkillSyncOutcome {
    // Resolve the source to a concrete URL list.
    let source = match InstallSource::parse(&entry.source) {
        Ok(s) => s,
        Err(err) => {
            return SkillSyncOutcome::Failed {
                name: name.to_string(),
                reason: format!("invalid source spec '{}': {err:#}", entry.source),
            };
        }
    };

    // Registry sources in index.json must not point back at another registry.
    if matches!(source, InstallSource::Registry(_)) {
        return SkillSyncOutcome::Failed {
            name: name.to_string(),
            reason: format!("registry entry for '{name}' must not point to another registry entry"),
        };
    }

    let urls = match &source {
        InstallSource::GitHubRepo(repo) => vec![
            format!("https://github.com/{repo}/archive/refs/heads/main.tar.gz"),
            format!("https://github.com/{repo}/archive/refs/heads/master.tar.gz"),
        ],
        InstallSource::DirectUrl(url) => vec![url.clone()],
        InstallSource::Registry(_) => unreachable!("guarded above"),
    };

    // Check the first downloadable URL against any cached meta.
    let skill_cache_dir = cache_dir.join(name);
    let meta_path = skill_cache_dir.join(".cache-meta.json");

    // Try each candidate URL in order.
    for url in &urls {
        let host = match host_from_url(url) {
            Some(h) => h,
            None => continue,
        };
        match network.decide(&host) {
            Decision::Allow => {}
            Decision::Deny => {
                return SkillSyncOutcome::Denied {
                    name: name.to_string(),
                    host,
                };
            }
            Decision::Prompt => {
                return SkillSyncOutcome::NeedsApproval {
                    name: name.to_string(),
                    host,
                };
            }
        }

        // Perform a HEAD request (or conditional GET) for freshness. We use a
        // simple GET with If-None-Match when we have an ETag, falling back to
        // an unconditional GET for servers that don't support ETags.
        let existing_meta: Option<CacheMeta> = meta_path
            .exists()
            .then(|| {
                fs::read_to_string(&meta_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
            })
            .flatten();

        // Build the request — add If-None-Match if we have a cached ETag.
        let client = reqwest::Client::new();
        let mut req = client.get(url);
        if let Some(ref meta) = existing_meta
            && let Some(ref etag) = meta.etag
        {
            req = req.header("If-None-Match", etag);
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(err) => {
                // Network error — try the next candidate URL.
                let _ = err;
                continue;
            }
        };

        let status = resp.status();

        // 304 Not Modified: cached copy is still fresh.
        if status == reqwest::StatusCode::NOT_MODIFIED {
            return SkillSyncOutcome::Fresh {
                name: name.to_string(),
            };
        }

        if status == reqwest::StatusCode::NOT_FOUND {
            // Try next URL (main → master fallback).
            continue;
        }

        if !status.is_success() {
            return SkillSyncOutcome::Failed {
                name: name.to_string(),
                reason: format!("GET {url} returned HTTP {status}"),
            };
        }

        // Capture ETag before consuming the response body.
        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let compressed_cap = max_size.saturating_mul(4);
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(err) => {
                return SkillSyncOutcome::Failed {
                    name: name.to_string(),
                    reason: format!("failed to read body from {url}: {err:#}"),
                };
            }
        };
        if bytes.len() as u64 > compressed_cap {
            return SkillSyncOutcome::Failed {
                name: name.to_string(),
                reason: format!(
                    "download from {url} exceeds compressed size cap ({} bytes)",
                    compressed_cap
                ),
            };
        }

        // Compute SHA-256 of the downloaded bytes.
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let sha256 = format!("{:x}", hasher.finalize());

        // Short-circuit: if the hash matches the cached one, we're fresh even
        // without a 304 (some CDNs strip ETags on redirects).
        if let Some(ref meta) = existing_meta
            && meta.sha256 == sha256
            && meta.url == *url
        {
            return SkillSyncOutcome::Fresh {
                name: name.to_string(),
            };
        }

        // Determine whether this is a tarball or a plain SKILL.md.
        // Heuristic: the URL ends with `.tar.gz` or `.tgz`, or the content
        // starts with the gzip magic bytes (0x1f 0x8b).
        let is_tarball =
            url.ends_with(".tar.gz") || url.ends_with(".tgz") || bytes.starts_with(&[0x1f, 0x8b]);

        let final_path: PathBuf = if is_tarball {
            // Extract into a temp staging dir, then rename atomically.
            let staged = match stage_tarball(&bytes, cache_dir, max_size) {
                Ok(s) => s,
                Err(err) => {
                    return SkillSyncOutcome::Failed {
                        name: name.to_string(),
                        reason: format!("tarball extraction failed: {err:#}"),
                    };
                }
            };
            // Move staged dir into its final location, replacing any prior cache.
            let dest = cache_dir.join(name);
            if dest.exists() {
                let _ = fs::remove_dir_all(&dest);
            }
            if let Err(err) = fs::rename(&staged.staged_path, &dest) {
                let _ = fs::remove_dir_all(&staged.staged_path);
                return SkillSyncOutcome::Failed {
                    name: name.to_string(),
                    reason: format!("failed to move staged skill into cache: {err:#}"),
                };
            }
            dest
        } else {
            // Plain SKILL.md (or other companion text file). Write directly.
            if let Err(err) = fs::create_dir_all(&skill_cache_dir) {
                return SkillSyncOutcome::Failed {
                    name: name.to_string(),
                    reason: format!("failed to create cache dir: {err:#}"),
                };
            }
            let skill_md_path = skill_cache_dir.join("SKILL.md");
            if let Err(err) = fs::write(&skill_md_path, &bytes) {
                return SkillSyncOutcome::Failed {
                    name: name.to_string(),
                    reason: format!("failed to write SKILL.md to cache: {err:#}"),
                };
            }
            skill_cache_dir.clone()
        };

        // Write the updated freshness metadata.
        let meta = CacheMeta {
            etag,
            sha256,
            url: url.clone(),
        };
        let meta_json = serde_json::to_string(&meta).unwrap_or_default();
        let _ = fs::write(final_path.join(".cache-meta.json"), meta_json);

        return SkillSyncOutcome::Downloaded {
            name: name.to_string(),
            path: final_path,
        };
    }

    // All candidate URLs exhausted without a successful response.
    SkillSyncOutcome::Failed {
        name: name.to_string(),
        reason: format!(
            "all candidate URLs for '{}' failed or were not found",
            entry.source
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct InstalledFromMarker {
    spec: String,
    #[serde(default)]
    checksum: String,
}

/// Curated-registry document. The shape is intentionally minimal so adding
/// optional metadata later (homepage, version, signature) is forward-compatible.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryDocument {
    /// Map of skill name → entry.
    #[serde(default)]
    pub skills: std::collections::BTreeMap<String, RegistryEntry>,
}

/// One row in the curated registry. `description` is optional so old indices
/// keep parsing.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryEntry {
    /// Source spec (e.g. `github:owner/repo`).
    pub source: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Successful registry fetch result. Same shape as [`InstallOutcome`] for the
/// network-policy outcomes so the caller can drop directly into approval flow.
#[derive(Debug)]
pub enum RegistryFetchResult {
    Loaded(RegistryDocument),
    NeedsApproval(String),
    Denied(String),
}

enum UrlResolution {
    Resolved(Vec<String>),
    NeedsApproval(String),
    Denied(String),
}

enum DownloadOutcome {
    Bytes { bytes: Vec<u8>, url: String },
    NeedsApproval(String),
    Denied(String),
}

/// Resolve the source spec into one or more candidate URLs to try in order.
async fn candidate_urls(
    source: &InstallSource,
    network: &NetworkPolicy,
    registry_url: &str,
) -> Result<UrlResolution> {
    match source {
        InstallSource::GitHubRepo(repo) => {
            // GitHub's archive endpoint lives on `codeload.github.com` after
            // the redirect, but the public URL we hit is `github.com`. Both
            // typically appear in user allow lists; we check the canonical
            // host.
            Ok(UrlResolution::Resolved(vec![
                format!("https://github.com/{repo}/archive/refs/heads/main.tar.gz"),
                format!("https://github.com/{repo}/archive/refs/heads/master.tar.gz"),
            ]))
        }
        InstallSource::DirectUrl(url) => Ok(UrlResolution::Resolved(vec![url.clone()])),
        InstallSource::Registry(name) => {
            match fetch_registry(network, registry_url).await? {
                RegistryFetchResult::Loaded(doc) => {
                    let entry = doc
                        .skills
                        .get(name)
                        .with_context(|| format!("skill '{name}' not found in registry"))?
                        .clone();
                    let inner = InstallSource::parse(&entry.source).with_context(|| {
                        format!(
                            "registry entry for '{name}' has invalid source: {}",
                            entry.source
                        )
                    })?;
                    // Recurse only one level — registry pointing at registry is
                    // disallowed to avoid cycles.
                    if matches!(inner, InstallSource::Registry(_)) {
                        bail!("registry entry for '{name}' must not point to another registry");
                    }
                    // Reuse this function for the inner source so GitHub fallback
                    // still applies.
                    Box::pin(candidate_urls(&inner, network, registry_url)).await
                }
                RegistryFetchResult::NeedsApproval(host) => Ok(UrlResolution::NeedsApproval(host)),
                RegistryFetchResult::Denied(host) => Ok(UrlResolution::Denied(host)),
            }
        }
    }
}

/// Download the first URL whose host the policy allows and which returns 2xx.
/// Returns `NeedsApproval` if every candidate hit `Prompt`, or `Denied` if every
/// candidate was denied.
async fn download_first_success(
    urls: &[String],
    network: &NetworkPolicy,
    max_size: u64,
) -> Result<DownloadOutcome> {
    let mut last_status: Option<reqwest::StatusCode> = None;
    let mut prompt_host: Option<String> = None;
    let mut denied_host: Option<String> = None;
    for url in urls {
        let host = match host_from_url(url) {
            Some(h) => h,
            None => bail!("invalid download url: {url}"),
        };
        match network.decide(&host) {
            Decision::Allow => {}
            Decision::Deny => {
                denied_host.get_or_insert(host);
                continue;
            }
            Decision::Prompt => {
                prompt_host.get_or_insert(host);
                continue;
            }
        }
        match download_with_cap(url, max_size).await? {
            DownloadAttempt::Bytes(bytes) => {
                return Ok(DownloadOutcome::Bytes {
                    bytes,
                    url: url.clone(),
                });
            }
            DownloadAttempt::NotFound(status) => {
                last_status = Some(status);
                continue;
            }
        }
    }
    if let Some(host) = denied_host {
        return Ok(DownloadOutcome::Denied(host));
    }
    if let Some(host) = prompt_host {
        return Ok(DownloadOutcome::NeedsApproval(host));
    }
    bail!(
        "failed to download skill (last status: {})",
        last_status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
}

enum DownloadAttempt {
    Bytes(Vec<u8>),
    NotFound(reqwest::StatusCode),
}

/// Stream a URL into memory with a size cap. Aborts on the first read that
/// would push the buffer over `max_size * 4` (the *4 accounts for compression;
/// the unpack step still enforces `max_size` on the *uncompressed* bytes).
async fn download_with_cap(url: &str, max_size: u64) -> Result<DownloadAttempt> {
    let resp = reqwest::get(url)
        .await
        .with_context(|| format!("failed to GET {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(DownloadAttempt::NotFound(status));
        }
        bail!("download {url} returned {status}");
    }
    // Soft cap on the *compressed* download — well above max_size to allow
    // for highly compressible payloads but still bounded.
    let compressed_cap = max_size.saturating_mul(4);
    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("failed to read body of {url}"))?;
    if (bytes.len() as u64) > compressed_cap {
        bail!("download {url} exceeds compressed size cap of {compressed_cap} bytes");
    }
    Ok(DownloadAttempt::Bytes(bytes.to_vec()))
}

struct StagedSkill {
    skill_name: String,
    staged_path: PathBuf,
}

/// Validate a tarball and extract it into `<skills_dir>/<name>.tmp/`.
fn stage_tarball(bytes: &[u8], skills_dir: &Path, max_size: u64) -> Result<StagedSkill> {
    fs::create_dir_all(skills_dir)
        .with_context(|| format!("failed to create skills directory {}", skills_dir.display()))?;

    // Two passes: first determine the skill name (and therefore the staged
    // dir) by finding the SKILL.md, then extract under that staged dir.
    // Both passes share the same archive bytes; we reset by wrapping fresh
    // decoders.

    let scan = scan_tarball(bytes, max_size)?;

    // Prepare staged directory. Use a `.tmp` suffix so a crashed install
    // never collides with a real name; remove any leftover from a prior
    // failed attempt.
    let staged_path = skills_dir.join(format!("{}.tmp", scan.skill_name));
    if staged_path.exists() {
        fs::remove_dir_all(&staged_path).with_context(|| {
            format!(
                "failed to clean stale staging dir {}",
                staged_path.display()
            )
        })?;
    }
    fs::create_dir_all(&staged_path)
        .with_context(|| format!("failed to create staging dir {}", staged_path.display()))?;

    // Second pass — extract.
    let result = extract_into(&scan, bytes, &staged_path, max_size);
    if let Err(err) = result {
        // Cleanup on failure so a half-staged directory doesn't survive.
        let _ = fs::remove_dir_all(&staged_path);
        return Err(err);
    }

    Ok(StagedSkill {
        skill_name: scan.skill_name,
        staged_path,
    })
}

struct TarballScan {
    /// Skill name from SKILL.md frontmatter.
    skill_name: String,
    /// Archive prefix to strip from each entry (e.g. `repo-main/`). May be empty.
    prefix: String,
    /// Sub-directory inside `prefix` that the SKILL.md lives in (`""` if root,
    /// or `skills/<name>` for repos that bundle multiple skills).
    skill_root: String,
}

/// First pass: locate SKILL.md, validate frontmatter, compute total size,
/// reject path-traversal entries and symlinks inside the selected install
/// subtree. We do not write anything in this pass; that's the second pass's job.
fn scan_tarball(bytes: &[u8], max_size: u64) -> Result<TarballScan> {
    let cursor = std::io::Cursor::new(bytes);
    let gz = GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);

    let mut total_size: u64 = 0;
    let mut prefix: Option<String> = None;
    let mut skill_md_relative: Option<(SkillMdCandidate, Vec<u8>)> = None;
    let mut link_paths: Vec<String> = Vec::new();

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
            return Err(InstallError::PathTraversal(path_str).into());
        }

        // Track total size against `max_size` (uncompressed). We honor `header
        // .size` rather than streaming-read every file; tar archives are
        // self-describing so this is reliable for non-malicious inputs and
        // catches the gzip-bomb case.
        if let Ok(size) = header.size() {
            total_size = total_size.saturating_add(size);
            if total_size > max_size {
                return Err(InstallError::OversizedTarball { limit: max_size }.into());
            }
        }

        // Detect prefix from the first entry. GitHub archives wrap everything
        // in `<repo>-<branch>/`; direct tarballs may have no prefix. We treat
        // the first path component as the prefix iff the archive has more than
        // one entry under it, but for SKILL.md detection we just strip the
        // first component if every entry shares it.
        if prefix.is_none() {
            if let Some(Component::Normal(first)) = path.components().next() {
                let candidate = first.to_string_lossy().into_owned();
                // Only treat the first component as a prefix if it's a
                // directory-like (no extension and the path has more
                // components). Otherwise leave prefix empty.
                if path.components().count() > 1 {
                    prefix = Some(candidate);
                } else {
                    prefix = Some(String::new());
                }
            } else {
                prefix = Some(String::new());
            }
        }

        if entry_type.is_symlink() || entry_type.is_hard_link() {
            link_paths.push(path_str);
            continue;
        }

        // SKILL.md detection. Match the same workflow layouts that runtime
        // discovery understands:
        //   * `<prefix>/SKILL.md`
        //   * `<prefix>/*/skills/<name>/SKILL.md`
        //   * `<prefix>/<name>/SKILL.md`
        if entry_type.is_file() {
            let stripped = strip_prefix(&path_str, prefix.as_deref().unwrap_or(""));
            if let Some(candidate) = skill_md_candidate(&stripped) {
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .context("failed to read SKILL.md from archive")?;
                // Prefer the most explicit match: repo-root SKILL.md first,
                // then known skill-directory layouts, then a single nested
                // `<name>/SKILL.md` repository.
                let replace = skill_md_relative
                    .as_ref()
                    .is_none_or(|(current, _)| candidate.rank < current.rank);
                if replace {
                    skill_md_relative = Some((candidate, buf));
                }
            }
        }
    }

    let prefix = prefix.unwrap_or_default();
    let (skill_md, skill_md_bytes) = skill_md_relative
        .ok_or(InstallError::MissingSkillMd)
        .map_err(anyhow::Error::from)?;

    for link_path in link_paths {
        if is_within_selected_root(&link_path, &prefix, &skill_md.skill_root) {
            return Err(InstallError::SymlinkRejected.into());
        }
    }

    // Parse frontmatter to extract the skill name. We reuse the same parser
    // shape as `SkillRegistry::parse_skill` but inline it here so we don't
    // depend on the discovery module's private function.
    let name = parse_frontmatter_name(&skill_md_bytes)?;

    Ok(TarballScan {
        skill_name: name,
        prefix,
        skill_root: skill_md.skill_root,
    })
}

struct SkillMdCandidate {
    rank: u8,
    skill_root: String,
}

fn skill_md_candidate(stripped_path: &str) -> Option<SkillMdCandidate> {
    if stripped_path.eq_ignore_ascii_case("SKILL.md") {
        return Some(SkillMdCandidate {
            rank: 0,
            skill_root: String::new(),
        });
    }

    let parts: Vec<&str> = stripped_path.split('/').collect();
    if parts
        .last()
        .is_none_or(|last| !last.eq_ignore_ascii_case("SKILL.md"))
    {
        return None;
    }

    // Common workflow-pack layouts:
    // `skills/<name>/SKILL.md`, `.agents/skills/<name>/SKILL.md`,
    // `.claude/skills/<name>/SKILL.md`, and nested package layouts such as
    // `packages/foo/skills/<name>/SKILL.md`.
    if parts.len() >= 3 {
        let container = parts[parts.len() - 3];
        let name = parts[parts.len() - 2];
        if container.eq_ignore_ascii_case("skills") && !name.is_empty() {
            return Some(SkillMdCandidate {
                rank: 1,
                skill_root: parts[..parts.len() - 1].join("/"),
            });
        }
    }

    // Single-skill repos sometimes keep their root tidy with
    // `<skill-name>/SKILL.md` plus sibling docs at repo root.
    if parts.len() == 2 && !parts[0].is_empty() {
        return Some(SkillMdCandidate {
            rank: 2,
            skill_root: parts[0].to_string(),
        });
    }

    None
}

fn extract_into(scan: &TarballScan, bytes: &[u8], dest: &Path, max_size: u64) -> Result<()> {
    let cursor = std::io::Cursor::new(bytes);
    let gz = GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);

    let mut total_size: u64 = 0;
    let prefix_with_root = if scan.skill_root.is_empty() {
        scan.prefix.clone()
    } else if scan.prefix.is_empty() {
        scan.skill_root.clone()
    } else {
        format!("{}/{}", scan.prefix, scan.skill_root)
    };

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
            return Err(InstallError::PathTraversal(path_str).into());
        }

        // Only extract entries that live under our skill root. For simple
        // tarballs (`SKILL.md` at root) that's everything; for multi-skill
        // repos it's the `skills/<name>/` slice.
        let stripped = strip_prefix(&path_str, &prefix_with_root).into_owned();
        if stripped.is_empty() && entry_type.is_dir() {
            // The root directory itself — already created.
            continue;
        }
        if stripped == path_str && !prefix_with_root.is_empty() {
            // Nothing to strip => entry is outside our subtree, skip.
            continue;
        }
        // Defense-in-depth: re-validate the stripped path.
        let stripped_path = Path::new(&stripped);
        if !is_safe_path(stripped_path) {
            return Err(InstallError::PathTraversal(stripped).into());
        }
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(InstallError::SymlinkRejected.into());
        }

        let target = dest.join(stripped_path);
        // Final paranoia check: ensure the resolved target stays under dest.
        // We can't canonicalize (target doesn't exist yet), so we walk
        // components one more time after composing.
        let target_components: Vec<_> = target.components().collect();
        let dest_components: Vec<_> = dest.components().collect();
        if !target_components.starts_with(dest_components.as_slice()) {
            return Err(InstallError::PathTraversal(stripped).into());
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
            // Read into a buffer so we can enforce `max_size`. Files inside
            // a SKILL bundle are small; copying through a buffer is fine.
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .with_context(|| format!("failed to read {}", path.display()))?;
            total_size = total_size.saturating_add(buf.len() as u64);
            if total_size > max_size {
                return Err(InstallError::OversizedTarball { limit: max_size }.into());
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

fn selected_root(prefix: &str, skill_root: &str) -> String {
    if skill_root.is_empty() {
        prefix.to_string()
    } else if prefix.is_empty() {
        skill_root.to_string()
    } else {
        format!("{prefix}/{skill_root}")
    }
}

fn is_within_selected_root(path: &str, prefix: &str, skill_root: &str) -> bool {
    let root = selected_root(prefix, skill_root);
    if root.is_empty() {
        return true;
    }
    path == root || path.starts_with(&format!("{root}/"))
}

/// Ensure a tar path has no `..` segments and is not absolute.
fn is_safe_path(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    for component in path.components() {
        match component {
            Component::ParentDir => return false,
            Component::Prefix(_) | Component::RootDir => return false,
            _ => {}
        }
    }
    true
}

/// Strip a leading directory prefix (e.g. `repo-main/`) from a tarball path.
fn strip_prefix<'a>(path: &'a str, prefix: &str) -> std::borrow::Cow<'a, str> {
    if prefix.is_empty() {
        return std::borrow::Cow::Borrowed(path);
    }
    let with_slash = format!("{prefix}/");
    if let Some(rest) = path.strip_prefix(&with_slash) {
        std::borrow::Cow::Owned(rest.to_string())
    } else if path == prefix {
        std::borrow::Cow::Borrowed("")
    } else {
        std::borrow::Cow::Borrowed(path)
    }
}

/// Extract `name:` and ensure `description:` exist in the SKILL.md frontmatter.
/// Also verifies the leading `---` fence so we reject malformed files early.
fn parse_frontmatter_name(bytes: &[u8]) -> Result<String> {
    let content = std::str::from_utf8(bytes).context("SKILL.md is not valid UTF-8")?;
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        bail!("SKILL.md is missing the leading '---' frontmatter fence");
    }
    let after_open = &trimmed[3..];
    let close = after_open.find("---").ok_or_else(|| {
        anyhow::anyhow!("SKILL.md is missing the closing '---' frontmatter fence")
    })?;
    let frontmatter = &after_open[..close];

    let mut name: Option<String> = None;
    let mut has_description = false;
    for raw in frontmatter.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            match key.as_str() {
                "name" if !value.is_empty() => name = Some(value),
                "description" if !value.is_empty() => has_description = true,
                _ => {}
            }
        }
    }

    let name = name.ok_or(InstallError::MissingFrontmatterField("name"))?;
    if !has_description {
        return Err(InstallError::MissingFrontmatterField("description").into());
    }
    // Sanity check: name must be a single path-safe segment.
    if name.contains('/')
        || name.contains('\\')
        || name == "."
        || name == ".."
        || name.contains(' ')
    {
        bail!("SKILL.md `name` must be a single path-safe segment (got '{name}')");
    }
    Ok(name)
}

fn source_spec_string(source: &InstallSource) -> String {
    match source {
        InstallSource::GitHubRepo(repo) => format!("github:{repo}"),
        InstallSource::DirectUrl(url) => url.clone(),
        InstallSource::Registry(name) => name.clone(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_source() {
        let s = InstallSource::parse("github:Hmbown/test-skill").unwrap();
        assert_eq!(
            s,
            InstallSource::GitHubRepo("Hmbown/test-skill".to_string())
        );
    }

    #[test]
    fn parse_github_source_rejects_missing_repo() {
        let err = InstallSource::parse("github:Hmbown").unwrap_err();
        assert!(err.to_string().contains("github source must"), "got: {err}");
    }

    #[test]
    fn parse_github_source_rejects_extra_slashes() {
        let err = InstallSource::parse("github:Hmbown/repo/extra").unwrap_err();
        assert!(err.to_string().contains("github source must"), "got: {err}");
    }

    #[test]
    fn parse_direct_url_source() {
        let s = InstallSource::parse("https://example.com/skill.tar.gz").unwrap();
        assert_eq!(
            s,
            InstallSource::DirectUrl("https://example.com/skill.tar.gz".to_string())
        );
        let s = InstallSource::parse("http://example.com/skill.tar.gz").unwrap();
        assert_eq!(
            s,
            InstallSource::DirectUrl("http://example.com/skill.tar.gz".to_string())
        );
    }

    #[test]
    fn parse_github_browser_url_routes_to_github_repo() {
        // Regression for #269: `https://github.com/<owner>/<repo>` was being
        // parsed as a DirectUrl, so the installer downloaded the HTML repo
        // page and tried to gzip-decode HTML ("invalid gzip header").
        for spec in [
            "https://github.com/obra/superpowers",
            "https://github.com/obra/superpowers/",
            "https://github.com/obra/superpowers.git",
            "https://github.com/obra/superpowers.git/",
            "https://www.github.com/obra/superpowers",
            "http://github.com/obra/superpowers",
            "  https://github.com/obra/superpowers  ",
        ] {
            let parsed = InstallSource::parse(spec)
                .unwrap_or_else(|err| panic!("parse({spec}) failed: {err}"));
            assert_eq!(
                parsed,
                InstallSource::GitHubRepo("obra/superpowers".to_string()),
                "spec {spec} must route to GitHubRepo",
            );
        }
    }

    #[test]
    fn parse_github_archive_url_stays_direct() {
        // URLs that point at a specific subresource (archive tarball, blob,
        // tree) are real direct URLs — the user picked that exact path.
        for spec in [
            "https://github.com/obra/superpowers/archive/refs/heads/main.tar.gz",
            "https://github.com/obra/superpowers/blob/main/README.md",
            "https://github.com/obra/superpowers/tree/main",
        ] {
            let parsed = InstallSource::parse(spec).unwrap();
            assert!(
                matches!(parsed, InstallSource::DirectUrl(_)),
                "spec {spec} must stay DirectUrl, got {parsed:?}",
            );
        }
    }

    #[test]
    fn parse_registry_source() {
        let s = InstallSource::parse("my-skill").unwrap();
        assert_eq!(s, InstallSource::Registry("my-skill".to_string()));
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(InstallSource::parse("").is_err());
        assert!(InstallSource::parse("   ").is_err());
    }

    #[test]
    fn is_safe_path_rejects_traversal() {
        assert!(!is_safe_path(Path::new("../etc/passwd")));
        assert!(!is_safe_path(Path::new("foo/../bar")));
        assert!(!is_safe_path(Path::new("/etc/passwd")));
        assert!(is_safe_path(Path::new("foo/bar/baz")));
        assert!(is_safe_path(Path::new("SKILL.md")));
    }

    #[test]
    fn parse_frontmatter_extracts_name() {
        let body = b"---\nname: hello\ndescription: greeter\n---\nbody\n";
        assert_eq!(parse_frontmatter_name(body).unwrap(), "hello");
    }

    #[test]
    fn parse_frontmatter_missing_name_fails() {
        let body = b"---\ndescription: x\n---\n";
        let err = parse_frontmatter_name(body).unwrap_err();
        assert!(format!("{err}").contains("name"));
    }

    #[test]
    fn parse_frontmatter_missing_description_fails() {
        let body = b"---\nname: x\n---\n";
        let err = parse_frontmatter_name(body).unwrap_err();
        assert!(format!("{err}").contains("description"));
    }

    #[test]
    fn parse_frontmatter_rejects_unsafe_name() {
        let body = b"---\nname: ../evil\ndescription: x\n---\n";
        assert!(parse_frontmatter_name(body).is_err());

        let body = b"---\nname: a name with spaces\ndescription: x\n---\n";
        assert!(parse_frontmatter_name(body).is_err());
    }

    #[test]
    fn parse_frontmatter_requires_opening_fence() {
        let body = b"name: hello\ndescription: x\n";
        assert!(parse_frontmatter_name(body).is_err());
    }

    #[test]
    fn strip_prefix_handles_all_cases() {
        assert_eq!(strip_prefix("foo/bar", "foo"), "bar");
        assert_eq!(strip_prefix("foo", "foo"), "");
        assert_eq!(strip_prefix("baz/bar", "foo"), "baz/bar");
        assert_eq!(strip_prefix("foo/bar", ""), "foo/bar");
    }

    #[test]
    fn source_spec_string_roundtrips() {
        assert_eq!(
            source_spec_string(&InstallSource::GitHubRepo("a/b".into())),
            "github:a/b"
        );
        assert_eq!(
            source_spec_string(&InstallSource::DirectUrl("https://x".into())),
            "https://x"
        );
        assert_eq!(
            source_spec_string(&InstallSource::Registry("x".into())),
            "x"
        );
    }
}
