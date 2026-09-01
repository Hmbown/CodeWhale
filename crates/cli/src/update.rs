//! Self-update for the `codewhale` binary.
//!
//! The `update` subcommand fetches the latest release from
//! `github.com/Hmbown/CodeWhale/releases/latest`, downloads the
//! platform-correct binary, verifies its SHA256 checksum, and atomically
//! replaces the currently running binary.

use std::cmp::Ordering;
use std::collections::HashMap;
#[cfg(target_os = "android")]
use std::ffi::CStr;
#[cfg(any(target_os = "android", all(test, unix)))]
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use codewhale_release::{
    CHECKSUM_MANIFEST_ASSET, InstallMethod, ReleaseChannel, ReleaseQuery, UPDATE_USER_AGENT,
    cnb_mirror_override_active, cnb_mirror_supports_target, cnb_release_base_url,
    compare_release_versions, is_beta_tag, mirror_asset_url, resolve_release_query,
    update_is_needed, update_network_fallback_hint,
};
use reqwest::Proxy;
use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const GITHUB_LATEST_RELEASE_PAGE_URL: &str = "https://github.com/Hmbown/CodeWhale/releases/latest";
const GITHUB_RELEASE_DOWNLOAD_BASE_URL: &str =
    "https://github.com/Hmbown/CodeWhale/releases/download";
const UPDATE_HTTP_ATTEMPTS: usize = 3;
const UPDATE_HTTP_RETRY_DELAY_MS: u64 = 100;
/// Ceiling for one asset download. Generous, because release binaries are tens
/// of megabytes and some of the networks this exists for are slow.
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Ceiling for one checksum-manifest probe. The manifest is a few hundred
/// bytes, so this is only a backstop against a source that accepts the
/// connection and then stalls — the winner is whichever probe *returns* first,
/// never whichever one outlasts a timeout.
const MANIFEST_PROBE_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(target_os = "android")]
const ANDROID_PROC_SELF_MAPS: &str = "/proc/self/maps";

/// Run the self-update workflow.
///
/// OpenHarmony (HarmonyOS) won't compile this file, so no need to handle
pub fn run_update(beta: bool, check_only: bool, proxy_arg: Option<String>) -> Result<()> {
    let executable_identity = update_executable_identity()?;
    let current_exe = executable_identity.path.clone();
    let legacy_binary = is_legacy_binary(&current_exe);
    ensure_supported_release_target(std::env::consts::OS, std::env::consts::ARCH)?;

    let plan = update_plan_for_exe(&current_exe);
    let channel = ReleaseChannel::from_beta_flag(beta);
    let current_version = env!("CARGO_PKG_VERSION");
    let proxy = proxy_arg
        .as_deref()
        .map(validate_and_build_proxy)
        .transpose()?;

    println!("Checking for {} updates...", channel.label());
    println!("Current binary: {}", current_exe.display());
    println!("Current version: v{current_version}");
    if legacy_binary {
        println!();
        println!("{}", legacy_binary_message(&current_exe));
    }
    if let Some(warning) = managed_install_warning(InstallMethod::detect(&current_exe)) {
        println!();
        println!("{warning}");
    }

    if check_only {
        let fetched = fetch_latest_release(channel, proxy.as_ref())
            .with_context(update_network_fallback_hint)?;
        let latest_tag = &fetched.release.tag_name;
        println!("Latest {} release: {latest_tag}", channel.label());
        if update_is_needed(channel, current_version, latest_tag)? {
            println!("Update available. Run `codewhale update` to install {latest_tag}.");
            println!(
                "Release source: {}",
                describe_release_source_for_check(&fetched, &plan.asset_stem, proxy.as_ref())
            );
        } else {
            match compare_release_versions(current_version, latest_tag)? {
                Ordering::Greater => {
                    println!("Current build is newer than the latest published release.");
                }
                Ordering::Less | Ordering::Equal => {
                    println!("Already up to date.");
                }
            }
        }
        return Ok(());
    }

    // Step 1: Fetch latest release metadata
    let fetched =
        fetch_latest_release(channel, proxy.as_ref()).with_context(update_network_fallback_hint)?;
    let release = &fetched.release;
    let latest_tag = &release.tag_name;
    println!("Latest {} release: {latest_tag}", channel.label());

    if fetched.source.is_pinned_mirror() {
        if channel == ReleaseChannel::Beta {
            println!(
                "Using {}; --beta does not select GitHub beta releases in mirror mode.",
                fetched.source.describe()
            );
        }
    } else if !update_is_needed(channel, current_version, latest_tag)? {
        println!("Already up to date; no download needed.");
        return Ok(());
    }

    // Step 2: Lock the checksum manifest and the binary to a single source. On
    // targets the CNB mirror publishes, this races the two tiny manifests so a
    // GitHub-blocked network never has to wait out a stalled asset download.
    let download = resolve_download_plan(&fetched, &plan.asset_stem, proxy.as_ref())?;
    println!("Release source: {}", download.source.describe());

    // Step 3: Download and verify the sole implementation binary once. The
    // installed `codew` and pre-0.9.5 `codewhale-tui` command paths are
    // compatibility names for these exact bytes, not separate release assets.
    println!("Downloading {}...", download.binary_name);
    let bytes = download_url(&download.binary_url, proxy.as_ref()).with_context(|| {
        format!(
            "failed to download {} from {}\n{}",
            download.binary_name,
            download.source.describe(),
            update_network_fallback_hint()
        )
    })?;

    verify_downloaded_asset(&download, &bytes)?;

    preflight_downloaded_binary(&download.binary_name, &bytes)?;

    println!(
        "SHA256 checksum verified against {CHECKSUM_MANIFEST_ASSET} from {}.",
        download.source.label()
    );

    // Step 4: Replace command paths only after the download and the running
    // executable identity verify. The preflight happens before a colocated
    // compatibility path can change, then the identity is checked just in time.
    replace_verified_downloads(&plan.target_paths, &bytes, || {
        validate_primary_update_identity(&executable_identity)
    })?;

    println!(
        "\n✅ Successfully updated to {latest_tag}!\n\
         Release source: {source}\n\
         Updated binaries:\n{targets}\n\
         \n\
         Restart the application to use the new version.",
        source = download.source.describe(),
        targets = plan
            .target_paths
            .iter()
            .map(|path| format!("  - {} ({})", path.display(), download.binary_name))
            .collect::<Vec<_>>()
            .join("\n")
    );

    Ok(())
}

/// Fail closed when the downloaded bytes do not match the manifest that came
/// from the same source. A mismatch is never a reason to install anyway, and
/// never a reason to retry against the source that lost the probe: the two
/// build their own artifacts, so their checksums are not interchangeable.
fn verify_downloaded_asset(download: &DownloadPlan, bytes: &[u8]) -> Result<()> {
    let expected = download
        .checksums
        .get(&download.binary_name)
        .with_context(|| {
            format!(
                "{CHECKSUM_MANIFEST_ASSET} from {} is missing {}",
                download.source.describe(),
                download.binary_name
            )
        })?;
    let actual = sha256_hex(bytes);
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "SHA256 mismatch for {} from {}!\n  expected: {expected}\n  actual:   {actual}",
            download.binary_name,
            download.source.describe()
        );
    }
    Ok(())
}

/// Warn when self-update would overwrite a binary a package manager owns.
///
/// We warn rather than refuse: the download still produces a working newer
/// binary, and refusing would break workflows that have been doing this for
/// releases. But the manager's metadata will then describe a version that is
/// no longer on disk, and its next upgrade silently reverts the user — so say
/// so, and name the command that would have done this properly.
fn managed_install_warning(method: InstallMethod) -> Option<String> {
    if method.supports_self_update() {
        return None;
    }
    Some(format!(
        "Warning: this binary looks like a {label} install.\n  \
         `{command}` is the command that updates it cleanly.\n  \
         Self-updating in place still works, but leaves {label} describing a version\n  \
         that is no longer on disk, and its next upgrade will revert this update.",
        label = method.label(),
        command = method.update_command()
    ))
}

/// Resolve the executable that the updater is allowed to replace.
///
/// Android's `std::env::current_exe()`, `AT_EXECFN`, and `/proc/self/exe` can
/// all identify Bionic's runtime linker rather than the launched program. On
/// Android, locate a marker compiled into this executable with `dladdr`, then
/// require the executable `/proc/self/maps` row containing that same address
/// to agree by canonical path, device, and inode.
#[derive(Debug, Clone)]
struct UpdateExecutableIdentity {
    path: PathBuf,
    #[cfg(target_os = "android")]
    android_proof: AndroidExecutableProof,
}

#[cfg(not(target_os = "android"))]
fn update_executable_identity() -> Result<UpdateExecutableIdentity> {
    let path = std::env::current_exe().context("failed to determine current executable path")?;
    Ok(UpdateExecutableIdentity { path })
}

#[cfg(target_os = "android")]
fn update_executable_identity() -> Result<UpdateExecutableIdentity> {
    let android_proof = android_loaded_executable_proof()?;
    Ok(UpdateExecutableIdentity {
        path: android_proof.path.clone(),
        android_proof,
    })
}

#[cfg(target_os = "android")]
#[inline(never)]
extern "C" fn android_update_image_marker() -> usize {
    android_update_image_marker as *const () as usize
}

#[cfg(target_os = "android")]
fn android_loaded_executable_proof() -> Result<AndroidExecutableProof> {
    let marker = android_update_image_marker as *const () as usize as u64;
    let dladdr_path = android_dladdr_path(android_update_image_marker as *const libc::c_void)?;
    let maps = std::fs::read_to_string(ANDROID_PROC_SELF_MAPS)
        .context("failed to read Android executable mappings from /proc/self/maps")?;
    android_loaded_executable_proof_report(&maps, marker, &dladdr_path)
}

#[cfg(target_os = "android")]
fn android_dladdr_path(marker: *const libc::c_void) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let mut info = std::mem::MaybeUninit::<libc::Dl_info>::zeroed();
    // SAFETY: `marker` points to a function in this loaded image and `info`
    // points to writable storage for the duration of the call.
    let found = unsafe { libc::dladdr(marker, info.as_mut_ptr()) };
    if found == 0 {
        bail!("Android dladdr could not locate the updater's loaded image");
    }
    // SAFETY: A non-zero dladdr result initializes `info`.
    let info = unsafe { info.assume_init() };
    if info.dli_fname.is_null() {
        bail!("Android dladdr returned an empty loaded-image path");
    }
    // SAFETY: `dli_fname` is a NUL-terminated string owned by the dynamic
    // loader and remains valid while this image is loaded.
    let bytes = unsafe { CStr::from_ptr(info.dli_fname) }.to_bytes();
    if bytes.is_empty() {
        bail!("Android dladdr returned an empty loaded-image path");
    }
    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(any(target_os = "android", all(test, unix)))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct AndroidImageMapping {
    start: u64,
    end: u64,
    device_major: u32,
    device_minor: u32,
    inode: u64,
    path: PathBuf,
}

#[cfg(any(target_os = "android", all(test, unix)))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AndroidExecutableProofKind {
    DladdrAndProcMaps,
}

#[cfg(any(target_os = "android", all(test, unix)))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct AndroidExecutableProof {
    path: PathBuf,
    device_major: u32,
    device_minor: u32,
    inode: u64,
    proof_kind: AndroidExecutableProofKind,
}

#[cfg(any(target_os = "android", all(test, unix)))]
fn parse_android_image_mapping(maps: &str, marker: u64) -> Result<AndroidImageMapping> {
    let mut matching = None;
    for (line_index, line) in maps.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let range = fields
            .next()
            .with_context(|| format!("malformed /proc/self/maps line {}", line_index + 1))?;
        let (start, end) = range
            .split_once('-')
            .with_context(|| format!("malformed mapping range `{range}`"))?;
        let start = u64::from_str_radix(start, 16)
            .with_context(|| format!("invalid mapping start `{start}`"))?;
        let end =
            u64::from_str_radix(end, 16).with_context(|| format!("invalid mapping end `{end}`"))?;
        if !(start <= marker && marker < end) {
            continue;
        }

        let permissions = fields
            .next()
            .context("loaded-image mapping is missing permissions")?;
        let _offset = fields
            .next()
            .context("loaded-image mapping is missing its file offset")?;
        let device = fields
            .next()
            .context("loaded-image mapping is missing its device")?;
        let inode = fields
            .next()
            .context("loaded-image mapping is missing its inode")?
            .parse::<u64>()
            .context("loaded-image mapping has an invalid inode")?;
        let path = fields.collect::<Vec<_>>().join(" ");

        if permissions.as_bytes().get(2) != Some(&b'x') {
            bail!("loaded-image mapping for updater marker is not executable");
        }
        if inode == 0 {
            bail!("loaded-image mapping for updater marker has no file inode");
        }
        let (device_major, device_minor) = device
            .split_once(':')
            .context("loaded-image mapping has an invalid device")?;
        let device_major = u32::from_str_radix(device_major, 16)
            .context("loaded-image mapping has an invalid device major number")?;
        let device_minor = u32::from_str_radix(device_minor, 16)
            .context("loaded-image mapping has an invalid device minor number")?;
        if path.is_empty() {
            bail!("loaded-image mapping for updater marker has no pathname");
        }

        let mapping = AndroidImageMapping {
            start,
            end,
            device_major,
            device_minor,
            inode,
            path: PathBuf::from(path),
        };
        if matching.replace(mapping).is_some() {
            bail!("multiple /proc/self/maps rows contain the updater marker");
        }
    }

    matching.ok_or_else(|| anyhow!("no /proc/self/maps row contains the updater marker"))
}

#[cfg(all(test, unix))]
fn resolve_android_loaded_executable_report(
    maps: &str,
    marker: u64,
    dladdr_path: &Path,
) -> Result<PathBuf> {
    Ok(android_loaded_executable_proof_report(maps, marker, dladdr_path)?.path)
}

#[cfg(any(target_os = "android", all(test, unix)))]
fn android_loaded_executable_proof_report(
    maps: &str,
    marker: u64,
    dladdr_path: &Path,
) -> Result<AndroidExecutableProof> {
    let mapping = parse_android_image_mapping(maps, marker)?;
    validate_android_reported_path("dladdr", dladdr_path)?;
    validate_android_reported_path("/proc/self/maps", &mapping.path)?;

    let resolved_dladdr = dladdr_path.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize Android dladdr path {}",
            dladdr_path.display()
        )
    })?;
    let resolved_mapping = mapping.path.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize Android loaded-image mapping {}",
            mapping.path.display()
        )
    })?;
    if resolved_dladdr != resolved_mapping {
        bail!(
            "Android loaded-image authorities disagree: dladdr resolved to {}, but /proc/self/maps resolved to {}",
            resolved_dladdr.display(),
            resolved_mapping.display()
        );
    }
    if is_android_linker_name(&resolved_mapping) {
        bail!(
            "Android loaded-image authorities resolved to runtime linker {}; refusing to use the linker as an update target",
            resolved_mapping.display()
        );
    }
    if !is_executable_file(&resolved_mapping) {
        bail!(
            "Android loaded image `{}` is not an executable regular file; refusing to select an update target",
            resolved_mapping.display()
        );
    }

    validate_android_mapping_identity(&mapping, &resolved_mapping)?;
    Ok(AndroidExecutableProof {
        path: resolved_mapping,
        device_major: mapping.device_major,
        device_minor: mapping.device_minor,
        inode: mapping.inode,
        proof_kind: AndroidExecutableProofKind::DladdrAndProcMaps,
    })
}

#[cfg(any(target_os = "android", all(test, unix)))]
fn validate_android_reported_path(authority: &str, path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!(
            "Android {authority} reported non-absolute loaded-image path `{}`",
            path.display()
        );
    }
    if path.to_string_lossy().ends_with(" (deleted)") {
        bail!(
            "Android {authority} reported deleted loaded image `{}`",
            path.display()
        );
    }
    if is_android_linker_name(path) {
        bail!(
            "Android {authority} identifies runtime linker `{}`; refusing to use the linker as an update target",
            path.display()
        );
    }
    Ok(())
}

#[cfg(any(target_os = "android", all(test, unix)))]
fn validate_android_mapping_identity(
    mapping: &AndroidImageMapping,
    candidate: &Path,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let candidate_metadata = std::fs::metadata(candidate).with_context(|| {
        format!(
            "failed to stat Android update target {}",
            candidate.display()
        )
    })?;
    let (candidate_major, candidate_minor) = android_device_parts(candidate_metadata.dev());
    let identity_matches = mapping.device_major == candidate_major
        && mapping.device_minor == candidate_minor
        && mapping.inode == candidate_metadata.ino();
    if !identity_matches {
        bail!(
            "Android loaded-image identity changed: /proc/self/maps has device/inode {:x}:{:x}:{}, but update target {} is {:x}:{:x}:{}; refusing to replace it",
            mapping.device_major,
            mapping.device_minor,
            mapping.inode,
            candidate.display(),
            candidate_major,
            candidate_minor,
            candidate_metadata.ino()
        );
    }
    Ok(())
}

#[cfg(any(target_os = "android", all(test, unix)))]
fn android_device_parts(device: u64) -> (u32, u32) {
    // Linux/Bionic's dev_t encoding, matching makedev(3), major(3), and
    // minor(3). `/proc/self/maps` renders these components in hexadecimal.
    let major = ((device >> 8) & 0xfff) as u32;
    let minor = ((device & 0xff) | ((device >> 12) & 0xfff00)) as u32;
    (major, minor)
}

fn validate_primary_update_identity(identity: &UpdateExecutableIdentity) -> Result<()> {
    #[cfg(target_os = "android")]
    {
        let fresh = android_loaded_executable_proof()?;
        if fresh != identity.android_proof {
            bail!(
                "Android loaded-image proof changed from {:?} to {:?}; refusing to replace the update target",
                identity.android_proof,
                fresh
            );
        }
        return Ok(());
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = identity;
        Ok(())
    }
}

fn replace_verified_downloads<F>(
    target_paths: &[PathBuf],
    verified_bytes: &[u8],
    validate_primary_identity: F,
) -> Result<()>
where
    F: Fn() -> Result<()>,
{
    // Fail before mutating a sibling if the primary pathname no longer names
    // the process image that initiated this update.
    validate_primary_identity()?;
    for path in target_paths.iter().rev() {
        replace_binary_with_validation(path, verified_bytes, || {
            // Re-check after each temp file is fully staged and immediately
            // before every destructive rename. The running command is first
            // in the plan and therefore replaced last, after its colocated
            // compatibility names have received the same verified bytes.
            validate_primary_identity()
        })?;
    }
    Ok(())
}

#[cfg(any(target_os = "android", all(test, unix)))]
fn is_android_linker_name(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            matches!(
                name,
                "linker"
                    | "linker64"
                    | "linker_asan"
                    | "linker_asan64"
                    | "linker_hwasan"
                    | "linker_hwasan64"
            )
        })
}

#[cfg(any(target_os = "android", all(test, unix)))]
fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FetchedRelease {
    release: Release,
    source: UpdateReleaseSource,
}

/// Where a release's assets come from.
///
/// This names the *asset* origin, which is not always the origin of the release
/// metadata: without an override the tag is resolved from GitHub, and only then
/// is the asset source chosen between GitHub and the first-party CNB mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateReleaseSource {
    /// Canonical GitHub Releases.
    GitHub,
    /// The first-party CNB mirror release for this exact tag (Linux x64 only).
    Cnb { base_url: String },
    /// An operator-supplied asset directory (`CODEWHALE_RELEASE_BASE_URL`).
    Mirror { base_url: String },
}

impl UpdateReleaseSource {
    /// Short, stable name for status output.
    fn label(&self) -> &'static str {
        match self {
            Self::GitHub => "GitHub Releases",
            Self::Cnb { .. } => "CNB mirror",
            Self::Mirror { .. } => "release mirror",
        }
    }

    /// The asset directory this source serves from, when it has one.
    fn base_url(&self) -> Option<&str> {
        match self {
            Self::GitHub => None,
            Self::Cnb { base_url } | Self::Mirror { base_url } => Some(base_url),
        }
    }

    /// Label plus asset directory — what status lines and the final receipt
    /// print, so "which source did this binary come from?" is answerable
    /// without rerunning the updater.
    fn describe(&self) -> String {
        match self.base_url() {
            Some(base_url) => format!("{} ({base_url})", self.label()),
            None => self.label().to_string(),
        }
    }

    /// True when an environment override, not a probe, chose this source. Such
    /// a source also carries the pinned version, so the "already up to date"
    /// shortcut does not apply to it.
    fn is_pinned_mirror(&self) -> bool {
        !matches!(self, Self::GitHub)
    }
}

/// One source that could serve this release, and the two URLs that must come
/// from it together: the checksum manifest, and the binary that manifest
/// covers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseSourceCandidate {
    source: UpdateReleaseSource,
    manifest_url: String,
    binary_name: String,
    binary_url: String,
}

/// A source locked in for this update, with its manifest already fetched,
/// parsed, and confirmed to cover the binary we are about to download.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DownloadPlan {
    source: UpdateReleaseSource,
    binary_name: String,
    binary_url: String,
    /// Parsed checksums from this same source, already confirmed to cover
    /// `binary_name`. A plan cannot exist without this proof.
    checksums: HashMap<String, String>,
}

/// Fetches one candidate's checksum manifest. Injected so the selection logic
/// can be tested without a network.
type ManifestFetcher = dyn Fn(&ReleaseSourceCandidate) -> Result<Vec<u8>> + Send + Sync;

/// Build the candidate list for proactive source selection, or `None` when this
/// update keeps a single canonical source.
///
/// Selection applies only when the release metadata came from GitHub (an
/// explicit override already named the source) and the target is one the CNB
/// mirror actually publishes. Every other target is left exactly as it was.
fn proactive_source_candidates(
    fetched: &FetchedRelease,
    asset_stem: &str,
    os: &str,
    rust_arch: &str,
) -> Option<Vec<ReleaseSourceCandidate>> {
    if fetched.source != UpdateReleaseSource::GitHub || !cnb_mirror_supports_target(os, rust_arch) {
        return None;
    }
    let mut candidates = Vec::new();
    if let Some(github) = github_source_candidate(&fetched.release, asset_stem) {
        candidates.push(github);
    }
    candidates.push(cnb_source_candidate(
        &fetched.release.tag_name,
        os,
        rust_arch,
    ));
    Some(candidates)
}

/// The canonical GitHub candidate for a release the API already described.
///
/// Asset URLs come from the release payload when it advertises them. A release
/// that lists the platform binary but not the manifest still gets a candidate:
/// GitHub serves release assets from a stable per-tag path, so the manifest is
/// addressable even when the payload omits it.
fn github_source_candidate(release: &Release, asset_stem: &str) -> Option<ReleaseSourceCandidate> {
    let asset = select_platform_asset(release, asset_stem)?;
    let manifest_url = select_checksum_manifest_asset(release)
        .map(|manifest| manifest.browser_download_url.clone())
        .unwrap_or_else(|| {
            let tag_name = format!("v{}", release.tag_name.trim_start_matches('v'));
            mirror_asset_url(
                &format!("{GITHUB_RELEASE_DOWNLOAD_BASE_URL}/{tag_name}"),
                CHECKSUM_MANIFEST_ASSET,
            )
        });
    Some(ReleaseSourceCandidate {
        source: UpdateReleaseSource::GitHub,
        manifest_url,
        binary_name: asset.name.clone(),
        binary_url: asset.browser_download_url.clone(),
    })
}

/// The first-party CNB candidate for this exact tag.
///
/// CNB builds its own artifacts from the tagged source, so its manifest only
/// describes its own binaries — which is precisely why the manifest and the
/// binary have to be taken from the same source.
fn cnb_source_candidate(tag_name: &str, os: &str, rust_arch: &str) -> ReleaseSourceCandidate {
    let base_url = cnb_release_base_url(tag_name);
    let binary_name = release_asset_name_for_prefix("codewhale", os, rust_arch);
    ReleaseSourceCandidate {
        manifest_url: mirror_asset_url(&base_url, CHECKSUM_MANIFEST_ASSET),
        binary_url: mirror_asset_url(&base_url, &binary_name),
        binary_name,
        source: UpdateReleaseSource::Cnb { base_url },
    }
}

/// Decide where this update's bytes come from, and prove the choice before
/// committing to it.
fn resolve_download_plan(
    fetched: &FetchedRelease,
    asset_stem: &str,
    proxy: Option<&Proxy>,
) -> Result<DownloadPlan> {
    match proactive_source_candidates(
        fetched,
        asset_stem,
        std::env::consts::OS,
        std::env::consts::ARCH,
    ) {
        Some(candidates) => {
            println!(
                "Probing {CHECKSUM_MANIFEST_ASSET} for {} from {}...",
                fetched.release.tag_name,
                candidate_labels(&candidates)
            );
            select_release_source(candidates, manifest_probe_fetcher(proxy))
                .with_context(update_network_fallback_hint)
        }
        None => single_source_download_plan(fetched, asset_stem, proxy),
    }
}

fn candidate_labels(candidates: &[ReleaseSourceCandidate]) -> String {
    candidates
        .iter()
        .map(|candidate| candidate.source.label())
        .collect::<Vec<_>>()
        .join(" and ")
}

/// Name the source `--check` would download from, without downloading anything
/// bigger than a manifest — and without contacting anything at all when an
/// override already fixed the answer.
fn describe_release_source_for_check(
    fetched: &FetchedRelease,
    asset_stem: &str,
    proxy: Option<&Proxy>,
) -> String {
    let Some(candidates) = proactive_source_candidates(
        fetched,
        asset_stem,
        std::env::consts::OS,
        std::env::consts::ARCH,
    ) else {
        return fetched.source.describe();
    };
    match select_release_source(candidates, manifest_probe_fetcher(proxy)) {
        Ok(plan) => plan.source.describe(),
        // A failed probe is a real answer for `--check` to report, not a reason
        // to fail a command whose whole job is to describe the release.
        Err(error) => format!("unresolved — {error:#}"),
    }
}

/// Resolve a source that does not participate in the Linux-x64 race.
///
/// This path is still fail-closed: every platform and every explicit mirror
/// must publish a valid manifest from the same source that covers the selected
/// binary. The binary is not downloaded until that proof exists.
fn single_source_download_plan(
    fetched: &FetchedRelease,
    asset_stem: &str,
    proxy: Option<&Proxy>,
) -> Result<DownloadPlan> {
    let release = &fetched.release;
    let asset = select_platform_asset(release, asset_stem).with_context(|| {
        format!(
            "no asset found for platform {asset_stem} in release {}. \
             Available assets: {}",
            release.tag_name,
            release
                .assets
                .iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    let checksum_asset = select_checksum_manifest_asset(release).with_context(|| {
        format!(
            "release {} from {} does not publish required {CHECKSUM_MANIFEST_ASSET}; refusing to download {} without checksum verification",
            release.tag_name,
            fetched.source.describe(),
            asset.name
        )
    })?;
    println!("Downloading {}...", checksum_asset.name);
    let checksum_bytes =
        download_url(&checksum_asset.browser_download_url, proxy).with_context(|| {
            format!(
                "failed to download {} from {}\n{}",
                checksum_asset.name,
                fetched.source.describe(),
                update_network_fallback_hint()
            )
        })?;
    let checksum_text = std::str::from_utf8(&checksum_bytes)
        .with_context(|| format!("{} is not valid UTF-8", checksum_asset.name))?;
    let checksums = parse_checksum_manifest(checksum_text).with_context(|| {
        format!(
            "failed to parse {} from {}",
            checksum_asset.name,
            fetched.source.describe()
        )
    })?;
    if !checksums.contains_key(&asset.name) {
        bail!(
            "{} from {} does not list {}; refusing to download an unverified update",
            checksum_asset.name,
            fetched.source.describe(),
            asset.name
        );
    }

    Ok(DownloadPlan {
        source: fetched.source.clone(),
        binary_name: asset.name.clone(),
        binary_url: asset.browser_download_url.clone(),
        checksums,
    })
}

fn manifest_probe_fetcher(proxy: Option<&Proxy>) -> Arc<ManifestFetcher> {
    let proxy = proxy.cloned();
    Arc::new(move |candidate: &ReleaseSourceCandidate| {
        download_url_with_timeout(
            &candidate.manifest_url,
            proxy.as_ref(),
            MANIFEST_PROBE_TIMEOUT,
        )
    })
}

/// Probe every candidate at once and take the first one that answers with a
/// usable manifest.
///
/// "First" means first to *return*, not first in the list and not whichever one
/// survives a timeout: a source that is slow or unreachable simply loses, and a
/// source that answers with a manifest that does not cover this platform's
/// binary loses too. Once a winner is chosen its receiver is dropped, so a
/// straggler's result has nowhere to land and is ignored.
fn select_release_source(
    candidates: Vec<ReleaseSourceCandidate>,
    fetch_manifest: Arc<ManifestFetcher>,
) -> Result<DownloadPlan> {
    if candidates.is_empty() {
        bail!("no release source publishes an asset for this platform");
    }

    let (result_tx, result_rx) = mpsc::channel();
    for candidate in candidates {
        let result_tx = result_tx.clone();
        let fetch_manifest = Arc::clone(&fetch_manifest);
        thread::spawn(move || {
            let outcome = probe_release_source(&candidate, &*fetch_manifest);
            let _ = result_tx.send((candidate, outcome));
        });
    }
    drop(result_tx);

    let mut failures = Vec::new();
    while let Ok((candidate, outcome)) = result_rx.recv() {
        match outcome {
            Ok(checksums) => {
                return Ok(DownloadPlan {
                    source: candidate.source,
                    binary_name: candidate.binary_name,
                    binary_url: candidate.binary_url,
                    checksums,
                });
            }
            Err(error) => failures.push(format!("  - {}: {error:#}", candidate.source.describe())),
        }
    }

    bail!(
        "no release source published a usable {CHECKSUM_MANIFEST_ASSET} for this platform:\n{}",
        failures.join("\n")
    )
}

fn probe_release_source(
    candidate: &ReleaseSourceCandidate,
    fetch_manifest: &ManifestFetcher,
) -> Result<HashMap<String, String>> {
    let bytes = fetch_manifest(candidate)
        .with_context(|| format!("failed to fetch {}", candidate.manifest_url))?;
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("{} is not valid UTF-8", candidate.manifest_url))?;
    let checksums = parse_checksum_manifest(text)
        .with_context(|| format!("failed to parse {}", candidate.manifest_url))?;
    if !checksums.contains_key(&candidate.binary_name) {
        bail!(
            "{} does not list {}",
            candidate.manifest_url,
            candidate.binary_name
        );
    }
    Ok(checksums)
}

fn ensure_supported_release_target(os: &str, arch: &str) -> Result<()> {
    if os == "linux" && arch == "riscv64" {
        bail!(
            "Linux riscv64 release assets are temporarily unavailable because \
             rquickjs-sys 0.12.0 does not ship riscv64gc-unknown-linux-gnu bindings. \
             See docs/INSTALL.md for the current platform matrix."
        );
    }
    Ok(())
}

pub(crate) fn release_arch_for_rust_arch(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

/// Returns true when the binary name belongs to the pre-rebrand `deepseek-tui` era.
pub(crate) fn is_legacy_binary(current_exe: &Path) -> bool {
    let exe_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    exe_name.starts_with("deepseek")
}

fn legacy_binary_message(current_exe: &Path) -> String {
    format!(
        "\
this binary ({exe}) is using the legacy deepseek/deepseek-tui command name.

The package has been renamed to `codewhale`. This update will install the
canonical `codewhale` command and refresh any existing `codew` or
`codewhale-tui` compatibility command from the same binary beside the legacy
command when the install directory is writable.
DeepSeek provider support is unchanged.

If this update cannot write to the install directory, reinstall using your
original install method:

  npm:
    npm uninstall -g deepseek-tui
    npm install -g codewhale

  Cargo:
    cargo uninstall deepseek-tui-cli 2>/dev/null || true
    cargo uninstall deepseek-tui 2>/dev/null || true
    cargo install codewhale-cli --locked

  Homebrew:
    brew upgrade codewhale
    # existing Cellar/deepseek-tui installs can still:
    brew upgrade deepseek-tui

  Manual binary:
    download the matched codewhale asset from
    https://github.com/Hmbown/CodeWhale/releases/latest

Once `codewhale` is on your PATH, run `codewhale update` for future updates.",
        exe = current_exe.display(),
    )
}

fn command_name_for_exe(current_exe: &Path) -> String {
    let exe_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("codewhale")
        .to_ascii_lowercase();
    exe_name
        .strip_suffix(".exe")
        .unwrap_or(&exe_name)
        .to_string()
}

fn command_path_beside(current_exe: &Path, command: &str) -> PathBuf {
    current_exe.with_file_name(format!("{command}{}", std::env::consts::EXE_SUFFIX))
}

fn installed_command_path(current_exe: &Path, command: &str) -> PathBuf {
    if command_name_for_exe(current_exe) == command {
        current_exe.to_path_buf()
    } else {
        command_path_beside(current_exe, command)
    }
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn legacy_tui_command_exists_beside(current_exe: &Path) -> bool {
    command_name_for_exe(current_exe) == "deepseek-tui"
        || command_path_beside(current_exe, "deepseek-tui").exists()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdatePlan {
    target_paths: Vec<PathBuf>,
    asset_stem: String,
}

fn update_plan_for_exe(current_exe: &Path) -> UpdatePlan {
    let mut target_paths = Vec::new();

    // Keep the process image first so reverse-order replacement updates the
    // command currently running the updater last. Pre-rebrand command names
    // retain their historical migration behavior: install canonical commands
    // beside them instead of overwriting the legacy path.
    if !is_legacy_binary(current_exe) {
        push_unique_path(&mut target_paths, current_exe.to_path_buf());
    }

    let primary = installed_command_path(current_exe, "codewhale");
    push_unique_path(&mut target_paths, primary);

    for alias in ["codew", "codewhale-tui"] {
        let alias_path = installed_command_path(current_exe, alias);
        let migrate_legacy_tui = alias == "codewhale-tui"
            && is_legacy_binary(current_exe)
            && legacy_tui_command_exists_beside(current_exe);
        if alias_path.exists() || command_name_for_exe(current_exe) == alias || migrate_legacy_tui {
            push_unique_path(&mut target_paths, alias_path);
        }
    }

    UpdatePlan {
        target_paths,
        asset_stem: release_asset_stem_for_prefix(
            "codewhale",
            std::env::consts::OS,
            std::env::consts::ARCH,
        ),
    }
}

fn release_asset_stem_for_prefix(prefix: &str, os: &str, rust_arch: &str) -> String {
    let arch = release_arch_for_rust_arch(rust_arch);
    format!("{prefix}-{os}-{arch}")
}

fn release_asset_name_for_prefix(prefix: &str, os: &str, rust_arch: &str) -> String {
    let stem = release_asset_stem_for_prefix(prefix, os, rust_arch);
    if os == "windows" {
        format!("{stem}.exe")
    } else {
        stem
    }
}

#[cfg(test)]
fn release_asset_stem_for(current_exe: &Path, os: &str, rust_arch: &str) -> String {
    let _ = current_exe;
    release_asset_stem_for_prefix("codewhale", os, rust_arch)
}

pub(crate) fn asset_matches_platform(asset_name: &str, binary_name: &str) -> bool {
    if asset_name.ends_with(".sha256") {
        return false;
    }
    asset_name == binary_name
        || asset_name == format!("{binary_name}.exe")
        || asset_name.starts_with(&format!("{binary_name}."))
}

fn asset_is_exact_platform_binary(asset_name: &str, binary_name: &str) -> bool {
    asset_name == binary_name || asset_name == format!("{binary_name}.exe")
}

fn select_platform_asset<'a>(release: &'a Release, binary_name: &str) -> Option<&'a Asset> {
    release
        .assets
        .iter()
        .find(|asset| asset_is_exact_platform_binary(&asset.name, binary_name))
        .or_else(|| {
            release
                .assets
                .iter()
                .find(|asset| asset_matches_platform(&asset.name, binary_name))
        })
}

fn select_checksum_manifest_asset(release: &Release) -> Option<&Asset> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == CHECKSUM_MANIFEST_ASSET)
}

fn parse_checksum_manifest(text: &str) -> Result<HashMap<String, String>> {
    let mut checksums = HashMap::new();

    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.len() < 66 {
            bail!("invalid SHA256 manifest line {}: {trimmed}", index + 1);
        }

        let (hash, rest) = trimmed.split_at(64);
        if !hash.chars().all(|ch| ch.is_ascii_hexdigit())
            || rest.is_empty()
            || !rest.chars().next().is_some_and(char::is_whitespace)
        {
            bail!("invalid SHA256 manifest line {}: {trimmed}", index + 1);
        }

        let mut asset_name = rest.trim_start();
        if let Some(stripped) = asset_name.strip_prefix('*') {
            asset_name = stripped;
        }
        if asset_name.is_empty() {
            bail!("invalid SHA256 manifest line {}: {trimmed}", index + 1);
        }

        checksums.insert(asset_name.to_string(), hash.to_ascii_lowercase());
    }

    Ok(checksums)
}

#[cfg(test)]
fn expected_sha256_from_manifest(text: &str, asset_name: &str) -> Result<String> {
    let checksums = parse_checksum_manifest(text)?;
    checksums
        .get(asset_name)
        .cloned()
        .with_context(|| format!("checksum manifest is missing {asset_name}"))
}

/// GitHub release metadata.
#[derive(serde::Deserialize, Debug, Clone, PartialEq, Eq)]
struct Release {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

/// A single release asset.
#[derive(serde::Deserialize, Debug, Clone, PartialEq, Eq)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Validate the proxy URL format and build a proxy for update HTTP requests.
pub(crate) fn validate_and_build_proxy(proxy_str: &str) -> Result<Proxy> {
    let proxy_url = reqwest::Url::parse(proxy_str).with_context(|| {
        format!(
            "invalid proxy URL: {proxy_str}\n\
             Expected format: http://host:port, https://host:port, or socks5://host:port"
        )
    })?;
    Proxy::all(proxy_url).context("failed to configure update proxy")
}

fn update_http_client(proxy: Option<&Proxy>) -> Result<reqwest::blocking::Client> {
    update_http_client_with_timeout(proxy, UPDATE_DOWNLOAD_TIMEOUT)
}

fn update_http_client_with_timeout(
    proxy: Option<&Proxy>,
    timeout: Duration,
) -> Result<reqwest::blocking::Client> {
    let mut builder = codewhale_release::platform_blocking_http_client_builder();
    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy.clone());
    }
    builder
        .user_agent(UPDATE_USER_AGENT)
        .timeout(timeout)
        .build()
        .context("failed to build update HTTP client")
}

/// Fetch the latest release metadata from GitHub.
fn fetch_latest_release(channel: ReleaseChannel, proxy: Option<&Proxy>) -> Result<FetchedRelease> {
    match resolve_release_query(channel) {
        ReleaseQuery::Mirror { base_url, version } => Ok(FetchedRelease {
            release: release_from_mirror_base_url(
                &base_url,
                &version,
                std::env::consts::OS,
                std::env::consts::ARCH,
            ),
            source: pinned_mirror_source(base_url),
        }),
        ReleaseQuery::GitHubLatest { url } => match fetch_latest_release_from_url(url, proxy) {
            Ok(release) => Ok(FetchedRelease {
                release,
                source: UpdateReleaseSource::GitHub,
            }),
            Err(api_error) => {
                eprintln!(
                    "GitHub API release lookup failed; trying github.com releases/latest fallback..."
                );
                Ok(FetchedRelease {
                    release: fetch_latest_stable_release_from_redirect(proxy).with_context(
                        || format!("GitHub API release lookup failed first: {api_error:#}"),
                    )?,
                    source: UpdateReleaseSource::GitHub,
                })
            }
        },
        ReleaseQuery::GitHubReleaseList { url } => Ok(FetchedRelease {
            release: fetch_latest_beta_release_from_url(url, proxy)?,
            source: UpdateReleaseSource::GitHub,
        }),
    }
}

/// Name the source an environment override selected.
///
/// `CODEWHALE_USE_CNB_MIRROR` and `CODEWHALE_RELEASE_BASE_URL` both resolve to
/// a base URL, but only the first is the first-party mirror — reporting them
/// alike would hide which one a user actually asked for.
fn pinned_mirror_source(base_url: String) -> UpdateReleaseSource {
    if cnb_mirror_override_active() {
        UpdateReleaseSource::Cnb { base_url }
    } else {
        UpdateReleaseSource::Mirror { base_url }
    }
}

fn release_from_mirror_base_url(
    base_url: &str,
    version: &str,
    os: &str,
    rust_arch: &str,
) -> Release {
    let tag_name = format!("v{}", version.trim_start_matches('v'));
    release_from_asset_base_url(&tag_name, base_url, os, rust_arch)
}

fn release_from_github_download_tag(tag_name: &str, os: &str, rust_arch: &str) -> Release {
    let tag_name = format!("v{}", tag_name.trim_start_matches('v'));
    let base_url = format!("{GITHUB_RELEASE_DOWNLOAD_BASE_URL}/{tag_name}");
    release_from_asset_base_url(&tag_name, &base_url, os, rust_arch)
}

fn release_from_asset_base_url(
    tag_name: &str,
    base_url: &str,
    os: &str,
    rust_arch: &str,
) -> Release {
    let mut assets = vec![Asset {
        name: CHECKSUM_MANIFEST_ASSET.to_string(),
        browser_download_url: mirror_asset_url(base_url, CHECKSUM_MANIFEST_ASSET),
    }];

    let name = release_asset_name_for_prefix("codewhale", os, rust_arch);
    assets.push(Asset {
        browser_download_url: mirror_asset_url(base_url, &name),
        name,
    });

    Release {
        tag_name: tag_name.to_string(),
        prerelease: false,
        assets,
    }
}

fn fetch_release_json_once(
    url: &str,
    description: &str,
    proxy: Option<&Proxy>,
) -> Result<(reqwest::StatusCode, String)> {
    let client = update_http_client(proxy)?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .with_context(|| format!("failed to fetch {description} from {url}"))?;
    let status = response.status();
    let body = response
        .text()
        .with_context(|| format!("failed to read {description} response body from {url}"))?;
    Ok((status, body))
}

fn fetch_release_json(url: &str, description: &str, proxy: Option<&Proxy>) -> Result<String> {
    let mut last_error = None;
    for attempt in 1..=UPDATE_HTTP_ATTEMPTS {
        match fetch_release_json_once(url, description, proxy) {
            Ok((status, body)) if status.is_success() => return Ok(body),
            Ok((status, body)) => {
                let error =
                    anyhow!("failed to fetch {description} from {url}: HTTP {status}\n{body}");
                if should_retry_http_status(status) && attempt < UPDATE_HTTP_ATTEMPTS {
                    last_error = Some(error);
                    sleep_before_update_retry(attempt);
                    continue;
                }
                return Err(error);
            }
            Err(error) if attempt < UPDATE_HTTP_ATTEMPTS => {
                last_error = Some(error);
                sleep_before_update_retry(attempt);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("failed to fetch {description} from {url}")))
}

fn should_retry_http_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn sleep_before_update_retry(attempt: usize) {
    std::thread::sleep(Duration::from_millis(
        UPDATE_HTTP_RETRY_DELAY_MS * attempt as u64,
    ));
}

fn fetch_latest_release_from_url(url: &str, proxy: Option<&Proxy>) -> Result<Release> {
    let body = fetch_release_json(url, "release info", proxy)?;
    let release: Release = serde_json::from_str(&body).with_context(|| {
        format!("failed to parse release JSON from GitHub API. Response: {body}")
    })?;

    Ok(release)
}

fn fetch_latest_stable_release_from_redirect(proxy: Option<&Proxy>) -> Result<Release> {
    let tag_name =
        fetch_latest_stable_tag_from_redirect_url(GITHUB_LATEST_RELEASE_PAGE_URL, proxy)?;
    Ok(release_from_github_download_tag(
        &tag_name,
        std::env::consts::OS,
        std::env::consts::ARCH,
    ))
}

fn fetch_latest_stable_tag_from_redirect_url(url: &str, proxy: Option<&Proxy>) -> Result<String> {
    let client = update_http_client(proxy)?;
    let mut last_error = None;
    for attempt in 1..=UPDATE_HTTP_ATTEMPTS {
        match fetch_latest_stable_tag_from_redirect_url_once(&client, url) {
            Ok(tag_name) => return Ok(tag_name),
            Err(error) if attempt < UPDATE_HTTP_ATTEMPTS => {
                last_error = Some(error);
                sleep_before_update_retry(attempt);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("failed to resolve latest stable release from {url}")))
}

fn fetch_latest_stable_tag_from_redirect_url_once(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<String> {
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("failed to fetch release redirect from {url}"))?;
    let status = response.status();
    let final_url = response.url().clone();
    if status.is_success() {
        if let Some(tag_name) = release_tag_from_github_release_url(&final_url) {
            return Ok(tag_name);
        }
        let body = response
            .text()
            .with_context(|| format!("failed to read release redirect response from {url}"))?;
        if let Some(tag_name) = release_tag_from_github_release_html(&body) {
            return Ok(tag_name);
        }
        bail!("release redirect did not resolve to a tag URL: {final_url}");
    }

    let body = response
        .text()
        .with_context(|| format!("failed to read release redirect response from {url}"))?;
    bail!("failed to fetch release redirect from {url}: HTTP {status}\n{body}");
}

fn release_tag_from_github_release_url(url: &reqwest::Url) -> Option<String> {
    let segments = url.path_segments()?.collect::<Vec<_>>();
    segments
        .windows(3)
        .find(|window| window[0] == "releases" && window[1] == "tag")
        .map(|window| window[2].to_string())
        .filter(|tag| !tag.is_empty())
}

fn release_tag_from_github_release_html(body: &str) -> Option<String> {
    const MARKERS: &[&str] = &[
        "/Hmbown/CodeWhale/releases/tag/",
        "/hmbown/CodeWhale/releases/tag/",
        "/releases/tag/",
    ];
    for marker in MARKERS {
        for rest in body.split(marker).skip(1) {
            let tag = rest
                .split(['"', '\'', '<', '>', '?', '#', '&'])
                .next()
                .unwrap_or("")
                .trim();
            if !tag.is_empty() {
                return Some(tag.to_string());
            }
        }
    }
    None
}

fn fetch_latest_beta_release_from_url(url: &str, proxy: Option<&Proxy>) -> Result<Release> {
    let body = fetch_release_json(url, "release list", proxy)?;
    // GitHub caps this endpoint at 100 releases per page. Codewhale uses the
    // first page as the latest-beta search window, matching GitHub's ordering.
    let releases: Vec<Release> = serde_json::from_str(&body).with_context(|| {
        format!("failed to parse release list JSON from GitHub API. Response: {body}")
    })?;

    releases
        .into_iter()
        .find(|release| is_beta_tag(&release.tag_name))
        .context("no beta release found in GitHub releases")
}

/// Download a URL to bytes.
fn download_url(url: &str, proxy: Option<&Proxy>) -> Result<Vec<u8>> {
    download_url_with_timeout(url, proxy, UPDATE_DOWNLOAD_TIMEOUT)
}

fn download_url_with_timeout(
    url: &str,
    proxy: Option<&Proxy>,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let mut last_error = None;
    for attempt in 1..=UPDATE_HTTP_ATTEMPTS {
        match download_url_once(url, proxy, timeout) {
            Ok((status, bytes)) if status.is_success() => return Ok(bytes),
            Ok((status, bytes)) => {
                let body = String::from_utf8_lossy(&bytes);
                let error = anyhow!("download failed with HTTP {status}: {body}");
                if should_retry_http_status(status) && attempt < UPDATE_HTTP_ATTEMPTS {
                    last_error = Some(error);
                    sleep_before_update_retry(attempt);
                    continue;
                }
                return Err(error);
            }
            Err(error) if attempt < UPDATE_HTTP_ATTEMPTS => {
                last_error = Some(error);
                sleep_before_update_retry(attempt);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("failed to download {url}")))
}

fn download_url_once(
    url: &str,
    proxy: Option<&Proxy>,
    timeout: Duration,
) -> Result<(reqwest::StatusCode, Vec<u8>)> {
    let client = update_http_client_with_timeout(proxy, timeout)?;
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("failed to download {url}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .with_context(|| format!("failed to read response body from {url}"))?;

    Ok((status, bytes.to_vec()))
}

/// Compute the SHA256 hex digest of data.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(data);
    hex_bytes(hash)
}

fn hex_bytes(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct GlibcVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

impl GlibcVersion {
    fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    fn display(self) -> String {
        if self.patch == 0 {
            format!("{}.{}", self.major, self.minor)
        } else {
            format!("{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
}

fn parse_glibc_version(text: &str) -> Option<GlibcVersion> {
    text.split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .filter(|part| part.contains('.'))
        .find_map(parse_glibc_version_token)
}

fn parse_glibc_version_token(token: &str) -> Option<GlibcVersion> {
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    Some(GlibcVersion::new(major, minor, patch))
}

fn highest_required_glibc(bytes: &[u8]) -> Option<GlibcVersion> {
    const MARKER: &[u8] = b"GLIBC_";
    let mut offset = 0;
    let mut highest = None;

    while let Some(found) = find_bytes(&bytes[offset..], MARKER) {
        let start = offset + found + MARKER.len();
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
            end += 1;
        }
        if end > start
            && let Ok(token) = std::str::from_utf8(&bytes[start..end])
            && let Some(version) = parse_glibc_version_token(token)
            && highest.is_none_or(|current| version > current)
        {
            highest = Some(version);
        }
        offset = start;
    }

    highest
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn glibc_check_disabled() -> bool {
    [
        "CODEWHALE_SKIP_GLIBC_CHECK",
        "DEEPSEEK_TUI_SKIP_GLIBC_CHECK",
        "DEEPSEEK_SKIP_GLIBC_CHECK",
    ]
    .into_iter()
    .any(|name| std::env::var_os(name).is_some_and(|value| value == std::ffi::OsStr::new("1")))
}

fn preflight_downloaded_binary(asset_name: &str, bytes: &[u8]) -> Result<()> {
    // GNU libc preflight is Linux-only (#4241). Rust treats `target_os = "android"`
    // as distinct from `"linux"`, so Termux/Android builds skip this check entirely
    // — Android uses Bionic libc, not glibc.
    if !cfg!(target_os = "linux") || glibc_check_disabled() {
        return Ok(());
    }

    let Some(required) = highest_required_glibc(bytes) else {
        return Ok(());
    };
    let host = detect_host_glibc();
    if host.is_some_and(|host| host >= required) {
        return Ok(());
    }

    bail!(
        "{}",
        glibc_compatibility_message(asset_name, required, host)
    );
}

fn detect_host_glibc() -> Option<GlibcVersion> {
    let getconf = std::process::Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|output| parse_glibc_version(&output));
    if getconf.is_some() {
        return getconf;
    }

    std::process::Command::new("ldd")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            let mut text = String::from_utf8_lossy(&output.stdout).to_string();
            if text.trim().is_empty() {
                text = String::from_utf8_lossy(&output.stderr).to_string();
            }
            parse_glibc_version(&text)
        })
}

fn glibc_compatibility_message(
    asset_name: &str,
    required: GlibcVersion,
    host: Option<GlibcVersion>,
) -> String {
    let host_line = match host {
        Some(host) => format!(
            "this system has glibc {}, which is too old for that asset.",
            host.display()
        ),
        None => "this system does not appear to provide GNU libc.".to_string(),
    };
    format!(
        "\
Prebuilt Codewhale asset `{asset_name}` requires GLIBC_{required}, but {host_line}

Official Linux release binaries are GNU libc builds. Ubuntu 22.04 ships glibc
2.35, so it cannot run a binary that was built against Ubuntu 24.04/glibc 2.39.

Install from source on this host instead:

  cargo install codewhale-cli --locked

Release engineering follow-up: build Linux GNU assets against an older glibc
baseline, or add a musl/static Linux asset. Set CODEWHALE_SKIP_GLIBC_CHECK=1 to
bypass this preflight at your own risk.",
        required = required.display(),
    )
}

/// Replace the running binary.
///
/// Writes the new binary to a secure temp file in the target directory, then
/// installs it in place. Unix can atomically replace the executable path. On
/// Windows, replacing a running executable can fail, so rename the current file
/// out of the way before moving the new binary into the original path.
#[cfg(test)]
fn replace_binary(target: &Path, new_bytes: &[u8]) -> Result<()> {
    replace_binary_with_validation(target, new_bytes, || Ok(()))
}

fn replace_binary_with_validation<F>(
    target: &Path,
    new_bytes: &[u8],
    validate_before_replace: F,
) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    replace_binary_with_validation_and_permission_setter(
        target,
        new_bytes,
        validate_before_replace,
        |path, permissions| std::fs::set_permissions(path, permissions),
    )
}

/// `apply_permissions` is a seam for `std::fs::set_permissions` so tests can
/// exercise permission-setup failures without host-specific filesystem state.
fn replace_binary_with_validation_and_permission_setter<F, P>(
    target: &Path,
    new_bytes: &[u8],
    validate_before_replace: F,
    apply_permissions: P,
) -> Result<()>
where
    F: FnOnce() -> Result<()>,
    P: Fn(&Path, std::fs::Permissions) -> std::io::Result<()>,
{
    let parent = target
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut tmp = tempfile::Builder::new()
        .prefix(".codewhale-update-")
        .tempfile_in(parent)
        .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    tmp.write_all(new_bytes)
        .with_context(|| format!("failed to write temp file at {}", tmp.path().display()))?;

    // Permission setup is part of pre-replacement validation: a staged binary
    // that cannot receive correct permissions must never replace a working
    // target, so every failure below aborts before any destructive rename.
    if target.exists() {
        // Preserve permissions from the original binary.
        let meta = std::fs::metadata(target).with_context(|| {
            format!(
                "failed to read permissions of update target {}",
                target.display()
            )
        })?;
        apply_permissions(tmp.path(), meta.permissions()).with_context(|| {
            format!(
                "failed to set permissions on staged update {} before replacing {}",
                tmp.path().display(),
                target.display()
            )
        })?;
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            apply_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755)).with_context(
                || {
                    format!(
                        "failed to set permissions on staged update {} before installing {}",
                        tmp.path().display(),
                        target.display()
                    )
                },
            )?;
        }
    }

    // Independently verify the staged binary is executable before it may
    // replace the target; a chmod that silently did not stick would otherwise
    // install a binary that cannot run.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let staged_mode = tmp
            .as_file()
            .metadata()
            .with_context(|| {
                format!(
                    "failed to inspect staged update at {}",
                    tmp.path().display()
                )
            })?
            .permissions()
            .mode();
        if staged_mode & 0o111 == 0 {
            bail!(
                "staged update {} is not executable (mode {:03o}); refusing to replace {}",
                tmp.path().display(),
                staged_mode & 0o7777,
                target.display()
            );
        }
    }

    validate_before_replace()?;

    #[cfg(windows)]
    {
        let backup = backup_path_for(target);
        if target.exists() {
            std::fs::rename(target, &backup).with_context(|| {
                format!(
                    "failed to move current executable {} to {}",
                    target.display(),
                    backup.display()
                )
            })?;
        }

        if let Err(err) = tmp.persist(target) {
            if backup.exists() {
                let _ = std::fs::rename(&backup, target);
            }
            bail!(
                "failed to install new binary at {}: {}",
                target.display(),
                err.error
            );
        }

        let _ = std::fs::remove_file(&backup);
    }

    #[cfg(not(windows))]
    {
        tmp.persist(target)
            .map_err(|err| err.error)
            .with_context(|| format!("failed to rename temp file to {}", target.display()))?;
    }

    Ok(())
}

#[cfg(windows)]
fn backup_path_for(target: &Path) -> std::path::PathBuf {
    let pid = std::process::id();
    for index in 0..100 {
        let mut candidate = target.to_path_buf();
        let suffix = if index == 0 {
            format!("old-{pid}")
        } else {
            format!("old-{pid}-{index}")
        };
        candidate.set_extension(suffix);
        if !candidate.exists() {
            return candidate;
        }
    }
    target.with_extension(format!("old-{pid}-fallback"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::sync::{Condvar, Mutex, MutexGuard};
    use std::thread;

    /// Release-source environment variables are process-wide, so the tests that
    /// exercise override precedence take this lock and restore what they found.
    static UPDATE_ENV_LOCK: Mutex<()> = Mutex::new(());
    const UPDATE_ENV_VARS: &[&str] = &[
        codewhale_release::RELEASE_BASE_URL_ENV,
        codewhale_release::LEGACY_RELEASE_BASE_URL_ENV,
        codewhale_release::DEEPSEEK_RELEASE_BASE_URL_ENV,
        codewhale_release::CNB_MIRROR_ENV,
        codewhale_release::UPDATE_VERSION_ENV,
        codewhale_release::LEGACY_UPDATE_VERSION_ENV,
    ];

    struct UpdateEnvGuard {
        previous: Vec<(&'static str, Option<OsString>)>,
        _lock: MutexGuard<'static, ()>,
    }

    impl UpdateEnvGuard {
        fn clear() -> Self {
            let lock = UPDATE_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = UPDATE_ENV_VARS
                .iter()
                .map(|&name| (name, std::env::var_os(name)))
                .collect();
            for &name in UPDATE_ENV_VARS {
                // SAFETY: tests that mutate these process-wide vars hold UPDATE_ENV_LOCK.
                unsafe { std::env::remove_var(name) };
            }
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for UpdateEnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.previous {
                // SAFETY: the guard still holds UPDATE_ENV_LOCK while restoring state.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    fn set_update_env(name: &str, value: &str) {
        // SAFETY: callers hold an UpdateEnvGuard, which serializes env mutation.
        unsafe { std::env::set_var(name, value) };
    }

    #[cfg(unix)]
    fn write_test_executable(path: &Path) {
        std::fs::write(path, b"test executable").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Write a stand-in installed binary: real update targets carry an
    /// executable mode on Unix, which the updater now preserves and verifies.
    fn write_installed_binary(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Verify the arch mapping used when constructing asset names.
    /// The mapping must use release-asset naming (arm64/x64), not Rust
    /// stdlib constants (aarch64/x86_64).
    #[test]
    fn test_arch_mapping() {
        assert_eq!(release_arch_for_rust_arch("aarch64"), "arm64");
        assert_eq!(release_arch_for_rust_arch("x86_64"), "x64");
        // Pass-through for unknown arches
        assert_eq!(release_arch_for_rust_arch("riscv64"), "riscv64");
        // The currently-compiled arch maps to a release asset name
        let compiled_arch = std::env::consts::ARCH;
        let asset_arch = release_arch_for_rust_arch(compiled_arch);
        // Must not contain the raw Rust constant names
        assert!(
            !asset_arch.contains("aarch64") && !asset_arch.contains("x86_64"),
            "asset arch '{asset_arch}' still uses raw Rust constant name"
        );
    }

    #[test]
    fn linux_riscv64_update_is_explicitly_unsupported() {
        let err = ensure_supported_release_target("linux", "riscv64")
            .expect_err("linux riscv64 should not claim a release asset");
        let message = err.to_string();
        assert!(message.contains("Linux riscv64 release assets are temporarily unavailable"));
        assert!(message.contains("rquickjs-sys 0.12.0"));
        ensure_supported_release_target("linux", "aarch64").unwrap();
        ensure_supported_release_target("macos", "aarch64").unwrap();
    }

    #[cfg(unix)]
    const TEST_ANDROID_MARKER: u64 = 0x1800;

    #[cfg(unix)]
    fn test_android_mapping_line(path: &Path, permissions: &str) -> String {
        use std::os::unix::fs::MetadataExt;

        let metadata = std::fs::metadata(path).unwrap();
        let (device_major, device_minor) = android_device_parts(metadata.dev());
        format!(
            "1000-2000 {permissions} 00000000 {:x}:{:x} {} {}\n",
            device_major,
            device_minor,
            metadata.ino(),
            path.display()
        )
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_resolves_agreed_mapping() {
        let dir = tempfile::TempDir::new().unwrap();
        let executable = dir.path().join("codewhale");
        write_test_executable(&executable);
        let maps = test_android_mapping_line(&executable, "r-xp");

        let resolved =
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &executable)
                .unwrap();

        assert_eq!(resolved, executable.canonicalize().unwrap());
        assert_eq!(update_plan_for_exe(&resolved).target_paths[0], resolved);
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_canonicalizes_symlink_and_sibling_policy() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();
        let canonical_dir = dir.path().join("canonical");
        let install_dir = dir.path().join("install");
        std::fs::create_dir(&canonical_dir).unwrap();
        std::fs::create_dir(&install_dir).unwrap();
        let canonical_dispatcher = canonical_dir.join("codewhale");
        let canonical_tui = canonical_dir.join("codewhale-tui");
        let invoked = install_dir.join("codewhale");
        write_test_executable(&canonical_dispatcher);
        write_test_executable(&canonical_tui);
        symlink(&canonical_dispatcher, &invoked).unwrap();
        let maps = test_android_mapping_line(&invoked, "r-xp");

        let resolved =
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &invoked).unwrap();
        let target_paths = update_plan_for_exe(&resolved).target_paths;

        assert_eq!(
            target_paths,
            vec![
                canonical_dispatcher.canonicalize().unwrap(),
                canonical_tui.canonicalize().unwrap()
            ]
        );
        assert!(!target_paths.contains(&invoked));
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_requires_marker_mapping() {
        let dir = tempfile::TempDir::new().unwrap();
        let executable = dir.path().join("codewhale");
        write_test_executable(&executable);
        let maps = test_android_mapping_line(&executable, "r-xp");

        let error = resolve_android_loaded_executable_report(&maps, 0x3000, &executable)
            .expect_err("a marker outside every mapping must fail closed");

        assert!(
            error.to_string().contains("no /proc/self/maps row"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_requires_executable_mapping() {
        let dir = tempfile::TempDir::new().unwrap();
        let executable = dir.path().join("codewhale");
        write_test_executable(&executable);
        let maps = test_android_mapping_line(&executable, "rw-p");

        let error =
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &executable)
                .expect_err("a non-executable marker mapping must fail closed");

        assert!(
            error
                .to_string()
                .contains("mapping for updater marker is not executable"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_rejects_anonymous_mapping() {
        let dir = tempfile::TempDir::new().unwrap();
        let executable = dir.path().join("codewhale");
        write_test_executable(&executable);
        let maps = "1000-2000 r-xp 00000000 00:00 0\n";

        let error =
            resolve_android_loaded_executable_report(maps, TEST_ANDROID_MARKER, &executable)
                .expect_err("an anonymous marker mapping must fail closed");

        assert!(
            error.to_string().contains("has no file inode"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_rejects_relative_or_deleted_paths() {
        let dir = tempfile::TempDir::new().unwrap();
        let executable = dir.path().join("codewhale");
        write_test_executable(&executable);
        let metadata = std::fs::metadata(&executable).unwrap();
        use std::os::unix::fs::MetadataExt;
        let (device_major, device_minor) = android_device_parts(metadata.dev());
        let relative_maps = format!(
            "1000-2000 r-xp 00000000 {:x}:{:x} {} codewhale\n",
            device_major,
            device_minor,
            metadata.ino()
        );
        let deleted = PathBuf::from(format!("{} (deleted)", executable.display()));

        let relative_error = resolve_android_loaded_executable_report(
            &relative_maps,
            TEST_ANDROID_MARKER,
            &executable,
        )
        .expect_err("a relative maps pathname must fail closed");
        let deleted_error = resolve_android_loaded_executable_report(
            &test_android_mapping_line(&executable, "r-xp"),
            TEST_ANDROID_MARKER,
            &deleted,
        )
        .expect_err("a deleted dladdr pathname must fail closed");

        assert!(relative_error.to_string().contains("non-absolute"));
        assert!(deleted_error.to_string().contains("deleted loaded image"));
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_rejects_linker_and_symlink_to_linker() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();
        let runtime_linker = dir.path().join("linker64");
        let invoked = dir.path().join("codewhale");
        write_test_executable(&runtime_linker);
        symlink(&runtime_linker, &invoked).unwrap();
        let maps = test_android_mapping_line(&invoked, "r-xp");

        let direct_error = resolve_android_loaded_executable_report(
            &maps,
            TEST_ANDROID_MARKER,
            Path::new("/system/bin/linker64"),
        )
        .expect_err("a directly reported Bionic linker must fail closed");
        let symlink_error =
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &invoked)
                .expect_err("a symlink to a linker must fail closed");

        assert!(
            direct_error
                .to_string()
                .contains("identifies runtime linker")
        );
        assert!(
            symlink_error
                .to_string()
                .contains("resolved to runtime linker")
        );
    }

    #[cfg(unix)]
    #[test]
    fn android_linker_name_recognizes_bionic_loader_variants() {
        for name in [
            "linker",
            "linker64",
            "linker_asan",
            "linker_asan64",
            "linker_hwasan",
            "linker_hwasan64",
        ] {
            assert!(
                is_android_linker_name(
                    Path::new("/apex/com.android.runtime/bin")
                        .join(name)
                        .as_path()
                ),
                "{name} must never become an updater target"
            );
        }
        assert!(!is_android_linker_name(Path::new("codewhale")));
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_rejects_authority_disagreement() {
        let dir = tempfile::TempDir::new().unwrap();
        let mapped = dir.path().join("mapped-codewhale");
        let dladdr = dir.path().join("dladdr-codewhale");
        write_test_executable(&mapped);
        write_test_executable(&dladdr);
        let maps = test_android_mapping_line(&mapped, "r-xp");

        let error = resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &dladdr)
            .expect_err("dladdr and maps path disagreement must fail closed");

        assert!(
            error.to_string().contains("authorities disagree"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_rejects_non_executable_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let executable = dir.path().join("codewhale");
        std::fs::write(&executable, b"not executable").unwrap();
        let maps = test_android_mapping_line(&executable, "r-xp");

        let error =
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &executable)
                .expect_err("a non-executable target file must fail closed");

        assert!(
            error.to_string().contains("not an executable regular file"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_rejects_device_inode_mismatch() {
        let dir = tempfile::TempDir::new().unwrap();
        let executable = dir.path().join("codewhale");
        write_test_executable(&executable);
        let metadata = std::fs::metadata(&executable).unwrap();
        use std::os::unix::fs::MetadataExt;
        let (device_major, device_minor) = android_device_parts(metadata.dev());
        let maps = format!(
            "1000-2000 r-xp 00000000 {:x}:{:x} {} {}\n",
            device_major,
            device_minor,
            metadata.ino() + 1,
            executable.display()
        );

        let error =
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &executable)
                .expect_err("a different maps device/inode must fail closed");

        assert!(
            error.to_string().contains("loaded-image identity changed"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn android_loaded_image_recheck_detects_pre_replace_swap() {
        let dir = tempfile::TempDir::new().unwrap();
        let candidate = dir.path().join("codewhale");
        let replacement = dir.path().join("replacement");
        write_test_executable(&candidate);
        let maps = test_android_mapping_line(&candidate, "r-xp");

        write_test_executable(&replacement);
        std::fs::rename(&replacement, &candidate).unwrap();
        let error =
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &candidate)
                .expect_err("a path swap after download must fail before replacement");

        assert!(
            error.to_string().contains("loaded-image identity changed"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn android_identity_preflight_prevents_all_paired_replacements() {
        let dir = tempfile::TempDir::new().unwrap();
        let primary = dir.path().join("codewhale");
        let sibling = dir.path().join("codewhale-tui");
        let swapped_primary = dir.path().join("swapped-primary");

        write_test_executable(&primary);
        std::fs::write(&primary, b"original running primary").unwrap();
        let maps = test_android_mapping_line(&primary, "r-xp");
        write_test_executable(&sibling);
        std::fs::write(&sibling, b"original sibling").unwrap();
        write_test_executable(&swapped_primary);
        std::fs::write(&swapped_primary, b"externally swapped primary").unwrap();
        std::fs::rename(&swapped_primary, &primary).unwrap();

        let target_paths = vec![primary.clone(), sibling.clone()];
        let error = replace_verified_downloads(&target_paths, b"downloaded binary", || {
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &primary)
                .map(|_| ())
        })
        .expect_err("identity mismatch must fail before either binary changes");

        assert!(
            error.to_string().contains("loaded-image identity changed"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            std::fs::read(&primary).unwrap(),
            b"externally swapped primary"
        );
        assert_eq!(std::fs::read(&sibling).unwrap(), b"original sibling");
    }

    #[cfg(unix)]
    #[test]
    fn android_identity_recheck_before_sibling_prevents_pair_split() {
        use std::cell::Cell;

        let dir = tempfile::TempDir::new().unwrap();
        let primary = dir.path().join("codewhale");
        let sibling = dir.path().join("codewhale-tui");
        let swapped_primary = dir.path().join("swapped-primary");
        write_test_executable(&primary);
        std::fs::write(&primary, b"original running primary").unwrap();
        let maps = test_android_mapping_line(&primary, "r-xp");
        write_test_executable(&sibling);
        std::fs::write(&sibling, b"original sibling").unwrap();
        write_test_executable(&swapped_primary);
        std::fs::write(&swapped_primary, b"externally swapped primary").unwrap();

        let target_paths = vec![primary.clone(), sibling.clone()];
        let validation_calls = Cell::new(0);
        let error = replace_verified_downloads(&target_paths, b"downloaded binary", || {
            let call = validation_calls.get() + 1;
            validation_calls.set(call);
            if call == 1 {
                return Ok(());
            }
            std::fs::rename(&swapped_primary, &primary).unwrap();
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &primary)
                .map(|_| ())
        })
        .expect_err("identity mismatch must fail before the staged sibling persists");

        assert_eq!(validation_calls.get(), 2);
        assert!(
            error.to_string().contains("loaded-image identity changed"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            std::fs::read(&primary).unwrap(),
            b"externally swapped primary"
        );
        assert_eq!(std::fs::read(&sibling).unwrap(), b"original sibling");
    }

    #[cfg(unix)]
    #[test]
    fn android_identity_jit_recheck_runs_after_staging_before_persist() {
        use std::cell::Cell;

        let dir = tempfile::TempDir::new().unwrap();
        let primary = dir.path().join("codewhale");
        let swapped_primary = dir.path().join("swapped-primary");
        write_test_executable(&primary);
        std::fs::write(&primary, b"original running primary").unwrap();
        let maps = test_android_mapping_line(&primary, "r-xp");
        write_test_executable(&swapped_primary);
        std::fs::write(&swapped_primary, b"externally swapped primary").unwrap();

        let target_paths = vec![primary.clone()];
        let validation_calls = Cell::new(0);
        let error = replace_verified_downloads(&target_paths, b"downloaded binary", || {
            let call = validation_calls.get() + 1;
            validation_calls.set(call);
            if call == 1 {
                return Ok(());
            }
            std::fs::rename(&swapped_primary, &primary).unwrap();
            resolve_android_loaded_executable_report(&maps, TEST_ANDROID_MARKER, &primary)
                .map(|_| ())
        })
        .expect_err("the post-staging identity swap must fail before persist");

        assert_eq!(validation_calls.get(), 2);
        assert!(
            error.to_string().contains("loaded-image identity changed"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            std::fs::read(&primary).unwrap(),
            b"externally swapped primary"
        );
        assert!(
            std::fs::read_dir(dir.path()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".codewhale-update-")
            }),
            "failed validation must clean the staged temp file"
        );
    }

    /// Every command name resolves to the sole implementation asset.
    #[test]
    fn every_invocation_name_uses_codewhale_release_asset() {
        for command in [
            "codewhale",
            "codewhale.exe",
            "codew",
            "codew.exe",
            "codewhale-tui",
            "CodeWhale-TUI.exe",
            "deepseek",
            "deepseek-tui",
            "other-binary",
        ] {
            assert_eq!(
                release_asset_stem_for(Path::new(command), "macos", "aarch64"),
                "codewhale-macos-arm64"
            );
        }
    }

    #[test]
    fn test_is_legacy_binary_detection() {
        assert!(is_legacy_binary(Path::new("deepseek")));
        assert!(is_legacy_binary(Path::new("deepseek-tui")));
        assert!(is_legacy_binary(Path::new("/usr/local/bin/deepseek")));
        assert!(is_legacy_binary(Path::new("/usr/local/bin/deepseek-tui")));
        assert!(is_legacy_binary(Path::new("DeepSeek.exe")));
        assert!(is_legacy_binary(Path::new("DeepSeek-TUI.exe")));
        assert!(!is_legacy_binary(Path::new("codewhale")));
        assert!(!is_legacy_binary(Path::new("codewhale-tui")));
        assert!(!is_legacy_binary(Path::new("codew")));
    }

    #[test]
    fn managed_installs_are_warned_before_self_update_overwrites_them() {
        let npm = managed_install_warning(InstallMethod::Npm).expect("npm is package-managed");
        assert!(npm.contains("npm install -g codewhale@latest"));
        assert!(npm.contains("revert this update"));

        let brew =
            managed_install_warning(InstallMethod::Homebrew).expect("brew is package-managed");
        assert!(brew.contains("brew upgrade codewhale"));

        assert!(managed_install_warning(InstallMethod::Cargo).is_some());

        let omarchy =
            managed_install_warning(InstallMethod::Omarchy).expect("Omarchy is package-managed");
        assert!(omarchy.contains("omarchy update"));

        // A plain release binary is exactly what this updater is for.
        assert!(managed_install_warning(InstallMethod::Binary).is_none());
    }

    #[test]
    fn legacy_binary_message_gives_copy_pasteable_migration_steps() {
        let message = legacy_binary_message(Path::new("/usr/local/bin/deepseek-tui"));

        assert!(message.contains("legacy deepseek/deepseek-tui command name"));
        assert!(message.contains("canonical `codewhale` command"));
        assert!(message.contains("DeepSeek provider support"));
        assert!(message.contains("is unchanged"));
        assert!(message.contains("npm uninstall -g deepseek-tui"));
        assert!(message.contains("npm install -g codewhale"));
        assert!(message.contains("cargo uninstall deepseek-tui-cli 2>/dev/null || true"));
        assert!(message.contains("cargo uninstall deepseek-tui 2>/dev/null || true"));
        assert!(message.contains("cargo install codewhale-cli --locked"));
        assert!(!message.contains("cargo install codewhale-tui --locked"));
        assert!(message.contains("brew upgrade codewhale"));
        assert!(message.contains("brew upgrade deepseek-tui"));
        assert!(message.contains("https://github.com/Hmbown/CodeWhale/releases/latest"));
    }

    #[test]
    fn legacy_dispatcher_update_targets_canonical_compatibility_commands() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = dir
            .path()
            .join(format!("deepseek{}", std::env::consts::EXE_SUFFIX));
        let tui = dir
            .path()
            .join(format!("deepseek-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"legacy dispatcher").unwrap();
        std::fs::write(&tui, b"legacy tui").unwrap();

        let plan = update_plan_for_exe(&dispatcher);

        assert_eq!(
            plan.target_paths,
            vec![
                dir.path()
                    .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX)),
                dir.path()
                    .join(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX))
            ]
        );
        assert!(plan.asset_stem.starts_with("codewhale-"));
        assert!(!plan.asset_stem.starts_with("codewhale-tui-"));
    }

    #[test]
    fn legacy_tui_update_targets_canonical_compatibility_commands() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = dir
            .path()
            .join(format!("deepseek{}", std::env::consts::EXE_SUFFIX));
        let tui = dir
            .path()
            .join(format!("deepseek-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"legacy dispatcher").unwrap();
        std::fs::write(&tui, b"legacy tui").unwrap();

        let plan = update_plan_for_exe(&tui);

        assert_eq!(
            plan.target_paths,
            vec![
                dir.path()
                    .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX)),
                dir.path()
                    .join(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX))
            ]
        );
        assert!(plan.asset_stem.starts_with("codewhale-"));
        assert!(!plan.asset_stem.starts_with("codewhale-tui-"));
    }

    #[test]
    fn test_release_asset_stem_for_supported_platforms() {
        let cases = [
            ("codewhale", "macos", "aarch64", "codewhale-macos-arm64"),
            ("codewhale", "macos", "x86_64", "codewhale-macos-x64"),
            ("codewhale", "linux", "x86_64", "codewhale-linux-x64"),
            ("codewhale", "windows", "x86_64", "codewhale-windows-x64"),
            ("codewhale", "windows", "aarch64", "codewhale-windows-arm64"),
            ("codew", "macos", "aarch64", "codewhale-macos-arm64"),
            ("codewhale-tui", "linux", "x86_64", "codewhale-linux-x64"),
        ];

        for (exe, os, arch, expected) in cases {
            assert_eq!(release_asset_stem_for(Path::new(exe), os, arch), expected);
        }
    }

    #[test]
    fn update_plan_includes_existing_compatibility_tui_for_primary() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = dir
            .path()
            .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX));
        let tui = dir
            .path()
            .join(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"dispatcher").unwrap();
        std::fs::write(&tui, b"tui").unwrap();

        let plan = update_plan_for_exe(&dispatcher);
        let paths = plan
            .target_paths
            .iter()
            .map(PathBuf::as_path)
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![dispatcher.as_path(), tui.as_path()]);
        assert!(plan.asset_stem.starts_with("codewhale-"));
        assert!(!plan.asset_stem.starts_with("codewhale-tui-"));
    }

    #[test]
    fn update_plan_skips_missing_compatibility_commands() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = dir
            .path()
            .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"dispatcher").unwrap();

        let plan = update_plan_for_exe(&dispatcher);

        assert_eq!(plan.target_paths, vec![dispatcher]);
        assert!(plan.asset_stem.starts_with("codewhale-"));
    }

    #[test]
    fn v094_three_command_install_updates_every_path_from_primary_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        let primary = dir
            .path()
            .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX));
        let codew = dir
            .path()
            .join(format!("codew{}", std::env::consts::EXE_SUFFIX));
        let legacy_tui = dir
            .path()
            .join(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
        for path in [&primary, &codew, &legacy_tui] {
            write_installed_binary(path, b"v0.9.4 old bytes");
        }

        let plan = update_plan_for_exe(&primary);
        assert_eq!(
            plan.target_paths,
            vec![primary.clone(), codew.clone(), legacy_tui.clone()]
        );
        assert!(plan.asset_stem.starts_with("codewhale-"));
        assert!(!plan.asset_stem.contains("codewhale-tui"));

        replace_verified_downloads(&plan.target_paths, b"v0.9.5 primary bytes", || Ok(())).unwrap();

        for path in [&primary, &codew, &legacy_tui] {
            assert_eq!(std::fs::read(path).unwrap(), b"v0.9.5 primary bytes");
        }
        assert_ne!(std::fs::read(codew).unwrap(), b"v0.9.4 old bytes");
    }

    #[test]
    fn direct_alias_invocation_keeps_running_path_first_and_updates_primary() {
        let dir = tempfile::TempDir::new().unwrap();
        let primary = dir
            .path()
            .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX));
        let codew = dir
            .path()
            .join(format!("codew{}", std::env::consts::EXE_SUFFIX));
        let legacy_tui = dir
            .path()
            .join(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
        for invoked in [&codew, &legacy_tui] {
            for path in [&primary, &codew, &legacy_tui] {
                write_installed_binary(path, b"old");
            }
            let plan = update_plan_for_exe(invoked);
            assert_eq!(plan.target_paths.first(), Some(invoked));
            assert!(plan.target_paths.contains(&primary));
            assert!(plan.target_paths.contains(&codew));
            assert!(plan.target_paths.contains(&legacy_tui));
            assert!(plan.asset_stem.starts_with("codewhale-"));
            assert!(!plan.asset_stem.starts_with("codewhale-tui-"));

            replace_verified_downloads(&plan.target_paths, b"new primary bytes", || Ok(()))
                .unwrap();
            for path in [&primary, &codew, &legacy_tui] {
                assert_eq!(std::fs::read(path).unwrap(), b"new primary bytes");
            }
        }
    }

    #[test]
    fn test_asset_matching_accepts_binary_assets_and_rejects_checksums() {
        assert!(asset_matches_platform(
            "codewhale-macos-arm64",
            "codewhale-macos-arm64"
        ));
        assert!(asset_matches_platform(
            "codewhale-macos-arm64.tar.gz",
            "codewhale-macos-arm64"
        ));
        assert!(asset_matches_platform(
            "codewhale-tui-windows-x64.exe",
            "codewhale-tui-windows-x64"
        ));
        assert!(!asset_matches_platform(
            "codewhale-tui-windows-x64.exe.sha256",
            "codewhale-tui-windows-x64"
        ));
        assert!(!asset_matches_platform(
            "codewhale-macos-aarch64.tar.gz",
            "codewhale-macos-arm64"
        ));
    }

    #[test]
    fn select_platform_asset_prefers_bare_binary_over_archive() {
        let release = Release {
            tag_name: "v0.8.8".to_string(),
            prerelease: false,
            assets: vec![
                Asset {
                    name: "codewhale-macos-arm64.tar.gz".to_string(),
                    browser_download_url: "https://example.invalid/codewhale-macos-arm64.tar.gz"
                        .to_string(),
                },
                Asset {
                    name: "codewhale-macos-arm64".to_string(),
                    browser_download_url: "https://example.invalid/codewhale-macos-arm64"
                        .to_string(),
                },
            ],
        };

        let asset =
            select_platform_asset(&release, "codewhale-macos-arm64").expect("platform asset");

        assert_eq!(asset.name, "codewhale-macos-arm64");
    }

    #[test]
    fn select_platform_asset_falls_back_to_archive_when_bare_binary_is_missing() {
        let release = Release {
            tag_name: "v0.8.8".to_string(),
            prerelease: false,
            assets: vec![Asset {
                name: "codewhale-macos-arm64.tar.gz".to_string(),
                browser_download_url: "https://example.invalid/codewhale-macos-arm64.tar.gz"
                    .to_string(),
            }],
        };

        let asset =
            select_platform_asset(&release, "codewhale-macos-arm64").expect("platform asset");

        assert_eq!(asset.name, "codewhale-macos-arm64.tar.gz");
    }

    #[test]
    fn test_sha256_hex_known_value() {
        let data = b"hello";
        let hash = sha256_hex(data);
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_sha256_hex_empty() {
        let hash = sha256_hex(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn glibc_version_parser_reads_getconf_and_symbol_text() {
        assert_eq!(
            parse_glibc_version("glibc 2.35\n"),
            Some(GlibcVersion::new(2, 35, 0))
        );
        assert_eq!(
            parse_glibc_version("requires GLIBC_2.39"),
            Some(GlibcVersion::new(2, 39, 0))
        );
        assert_eq!(parse_glibc_version("not glibc"), None);
    }

    #[test]
    fn highest_required_glibc_finds_highest_binary_symbol() {
        let bytes = b"\0GLIBC_2.17\0other\0GLIBC_2.39\0GLIBC_2.35";

        assert_eq!(
            highest_required_glibc(bytes),
            Some(GlibcVersion::new(2, 39, 0))
        );
    }

    #[test]
    fn glibc_compatibility_message_is_codewhale_branded_and_actionable() {
        let message = glibc_compatibility_message(
            "codewhale-linux-x64",
            GlibcVersion::new(2, 39, 0),
            Some(GlibcVersion::new(2, 35, 0)),
        );

        assert!(message.contains("Prebuilt Codewhale asset `codewhale-linux-x64`"));
        assert!(message.contains("requires GLIBC_2.39"));
        assert!(message.contains("this system has glibc 2.35"));
        assert!(message.contains("cargo install codewhale-cli --locked"));
        assert!(message.contains("build Linux GNU assets against an older glibc"));
    }

    #[test]
    fn parse_checksum_manifest_accepts_sha256sum_format() {
        let manifest = "\
2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  codewhale-macos-arm64
E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855  *codewhale-windows-x64.exe
";
        let checksums = parse_checksum_manifest(manifest).expect("valid manifest");

        assert_eq!(
            checksums.get("codewhale-macos-arm64").map(String::as_str),
            Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
        assert_eq!(
            checksums
                .get("codewhale-windows-x64.exe")
                .map(String::as_str),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    #[test]
    fn parse_checksum_manifest_rejects_malformed_lines() {
        let err = parse_checksum_manifest("not-a-hash  codewhale-macos-arm64")
            .expect_err("invalid manifest line should fail");
        assert!(
            err.to_string().contains("invalid SHA256 manifest line"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn expected_sha256_from_manifest_requires_matching_asset() {
        let manifest =
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  other-asset\n";
        let err = expected_sha256_from_manifest(manifest, "codewhale-macos-arm64")
            .expect_err("missing asset should fail");
        assert!(
            err.to_string()
                .contains("checksum manifest is missing codewhale-macos-arm64"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn test_replace_binary_creates_and_replaces() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("codewhale-test");
        // Write initial content
        write_installed_binary(&target, b"old binary");

        replace_binary(&target, b"new binary content").unwrap();
        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(content, "new binary content");
    }

    #[test]
    fn test_replace_binary_creates_new_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("codewhale-new-test");

        replace_binary(&target, b"fresh binary").unwrap();
        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(content, "fresh binary");
    }

    fn assert_no_staged_temp_files(dir: &Path) {
        assert!(
            std::fs::read_dir(dir).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".codewhale-update-")
            }),
            "a failed permission setup must clean the staged temp file"
        );
    }

    /// Regression test for #5727: a permission-setup failure on the staged
    /// binary must abort the update before the existing target is replaced.
    #[test]
    fn permission_failure_on_existing_target_aborts_before_replacement() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("codewhale-test");
        write_installed_binary(&target, b"old binary");

        let error = replace_binary_with_validation_and_permission_setter(
            &target,
            b"new binary content",
            || Ok(()),
            |_, _| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        )
        .expect_err("a chmod failure must fail the update");

        assert!(
            error
                .to_string()
                .contains("failed to set permissions on staged update"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"old binary",
            "the working binary must survive a permission-setup failure"
        );
        assert_no_staged_temp_files(dir.path());
    }

    /// Regression test for #5727, new-target path: when no binary exists yet
    /// the staged file still needs its 0o755 mode, and a chmod failure must
    /// abort instead of installing a non-executable file.
    #[cfg(unix)]
    #[test]
    fn permission_failure_on_new_target_aborts_install() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("codewhale-new-test");

        let error = replace_binary_with_validation_and_permission_setter(
            &target,
            b"fresh binary",
            || Ok(()),
            |_, _| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        )
        .expect_err("a chmod failure must fail a fresh install");

        assert!(
            error
                .to_string()
                .contains("failed to set permissions on staged update"),
            "unexpected error: {error:#}"
        );
        assert!(
            !target.exists(),
            "a failed fresh install must not leave a target behind"
        );
        assert_no_staged_temp_files(dir.path());
    }

    /// Regression test for #5727: even when permission setup reports success,
    /// a staged binary without an executable mode must never replace the
    /// working target.
    #[cfg(unix)]
    #[test]
    fn non_executable_staged_update_aborts_before_replacement() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("codewhale-test");
        write_test_executable(&target);
        std::fs::write(&target, b"old binary").unwrap();

        // A no-op setter models a chmod that claims success without sticking,
        // leaving the staged temp file at its default non-executable 0o600.
        let error = replace_binary_with_validation_and_permission_setter(
            &target,
            b"new binary content",
            || Ok(()),
            |_, _| Ok(()),
        )
        .expect_err("a non-executable staged binary must fail the update");

        assert!(
            error.to_string().contains("is not executable"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"old binary",
            "the working binary must survive a non-executable staged update"
        );
        assert_no_staged_temp_files(dir.path());
    }

    /// Mocked GitHub release payload covering the sole implementation binary
    /// across the published platform/arch matrix, plus a checksum sibling that
    /// must never be picked as the binary.
    fn mocked_release() -> Release {
        let json = r#"{
          "tag_name": "v0.8.8",
          "assets": [
            { "name": "codewhale-linux-x64",          "browser_download_url": "https://example.invalid/codewhale-linux-x64" },
            { "name": "codewhale-macos-x64",          "browser_download_url": "https://example.invalid/codewhale-macos-x64" },
            { "name": "codewhale-macos-arm64",        "browser_download_url": "https://example.invalid/codewhale-macos-arm64" },
            { "name": "codewhale-windows-x64.exe",    "browser_download_url": "https://example.invalid/codewhale-windows-x64.exe" },
            { "name": "codewhale-windows-x64.exe.sha256", "browser_download_url": "https://example.invalid/codewhale-windows-x64.exe.sha256" },
            { "name": "codewhale-windows-arm64.exe",  "browser_download_url": "https://example.invalid/codewhale-windows-arm64.exe" }
          ]
        }"#;
        serde_json::from_str(json).expect("mock release JSON")
    }

    #[test]
    fn mocked_release_selects_dispatcher_asset_for_supported_platforms() {
        let release = mocked_release();
        let cases = [
            ("macos", "aarch64", "codewhale-macos-arm64"),
            ("macos", "x86_64", "codewhale-macos-x64"),
            ("linux", "x86_64", "codewhale-linux-x64"),
            ("windows", "x86_64", "codewhale-windows-x64.exe"),
            ("windows", "aarch64", "codewhale-windows-arm64.exe"),
        ];

        for (os, arch, expected) in cases {
            let stem = release_asset_stem_for(Path::new("/usr/local/bin/codewhale"), os, arch);
            let asset = select_platform_asset(&release, &stem)
                .unwrap_or_else(|| panic!("no asset for {os}/{arch} (stem {stem})"));
            assert_eq!(asset.name, expected, "{os}/{arch}");
        }
    }

    #[test]
    fn mocked_release_selects_primary_asset_when_compatibility_alias_invokes_update() {
        let release = mocked_release();
        let stem = release_asset_stem_for(
            Path::new("/usr/local/bin/codewhale-tui"),
            "macos",
            "aarch64",
        );
        let asset = select_platform_asset(&release, &stem).expect("primary platform asset");
        assert_eq!(asset.name, "codewhale-macos-arm64");

        let windows_stem = release_asset_stem_for(Path::new("C:\\codew.exe"), "windows", "aarch64");
        let windows_asset =
            select_platform_asset(&release, &windows_stem).expect("Windows ARM64 primary asset");
        assert_eq!(windows_asset.name, "codewhale-windows-arm64.exe");
    }

    #[test]
    fn android_arm64_maps_to_android_release_assets() {
        // The generic format!("{prefix}-{os}-{arch}") path naturally produces
        // Android asset stems. Verify every supported command name resolves to
        // the primary Android asset, never Linux or a removed TUI asset (#4241).
        assert_eq!(
            release_asset_stem_for_prefix("codewhale", "android", "aarch64"),
            "codewhale-android-arm64"
        );
        assert_eq!(
            release_asset_stem_for(Path::new("codewhale-tui"), "android", "aarch64"),
            "codewhale-android-arm64"
        );
        assert_eq!(
            release_asset_stem_for(Path::new("codew"), "android", "aarch64"),
            "codewhale-android-arm64"
        );
    }

    #[test]
    fn ensure_supported_release_target_accepts_android() {
        // Android/Termux is a supported release target (#4241).
        assert!(ensure_supported_release_target("android", "aarch64").is_ok());
    }

    #[test]
    fn android_release_assets_never_select_linux_arm64() {
        // Sanity: the stem formatter must never produce a linux-* stem for android.
        let stem = release_asset_stem_for_prefix("codewhale", "android", "aarch64");
        assert!(
            !stem.contains("linux"),
            "android stem must not contain linux: {stem}"
        );
    }

    #[test]
    fn mirror_release_uses_base_url_and_platform_assets() {
        let release = release_from_mirror_base_url(
            "https://mirror.example/releases/v0.8.36/",
            "0.8.36",
            "linux",
            "x86_64",
        );

        assert_eq!(release.tag_name, "v0.8.36");
        assert_eq!(release.assets[0].name, CHECKSUM_MANIFEST_ASSET);
        assert_eq!(
            release.assets[0].browser_download_url,
            "https://mirror.example/releases/v0.8.36/codewhale-artifacts-sha256.txt"
        );

        let dispatcher =
            select_platform_asset(&release, "codewhale-linux-x64").expect("dispatcher asset");
        assert_eq!(
            dispatcher.browser_download_url,
            "https://mirror.example/releases/v0.8.36/codewhale-linux-x64"
        );
        assert_eq!(release.assets.len(), 2);
        assert!(
            select_platform_asset(&release, "codewhale-tui-linux-x64").is_none(),
            "mirror fallback must not synthesize a removed TUI asset"
        );
    }

    #[test]
    fn mirror_release_uses_windows_exe_asset_names() {
        let release = release_from_mirror_base_url(
            "https://mirror.example/releases/v0.8.36",
            "v0.8.36",
            "windows",
            "x86_64",
        );

        assert_eq!(release.tag_name, "v0.8.36");
        assert!(
            select_platform_asset(&release, "codewhale-windows-x64")
                .is_some_and(|asset| asset.name == "codewhale-windows-x64.exe")
        );
        assert!(select_platform_asset(&release, "codewhale-tui-windows-x64").is_none());

        let arm_release = release_from_mirror_base_url(
            "https://mirror.example/releases/v0.9.1",
            "v0.9.1",
            "windows",
            "aarch64",
        );
        assert!(
            select_platform_asset(&arm_release, "codewhale-windows-arm64")
                .is_some_and(|asset| asset.name == "codewhale-windows-arm64.exe")
        );
    }

    #[test]
    fn github_release_url_parser_extracts_tag() {
        let url = reqwest::Url::parse("https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.61")
            .unwrap();

        assert_eq!(
            release_tag_from_github_release_url(&url).as_deref(),
            Some("v0.8.61")
        );
    }

    #[test]
    fn github_release_download_fallback_uses_deterministic_asset_urls() {
        let release = release_from_github_download_tag("0.8.61", "macos", "aarch64");

        assert_eq!(release.tag_name, "v0.8.61");
        assert_eq!(
            release.assets[0].browser_download_url,
            "https://github.com/Hmbown/CodeWhale/releases/download/v0.8.61/codewhale-artifacts-sha256.txt"
        );
        let dispatcher =
            select_platform_asset(&release, "codewhale-macos-arm64").expect("dispatcher asset");
        assert_eq!(
            dispatcher.browser_download_url,
            "https://github.com/Hmbown/CodeWhale/releases/download/v0.8.61/codewhale-macos-arm64"
        );
        assert_eq!(release.assets.len(), 2);
        assert!(select_platform_asset(&release, "codewhale-tui-macos-arm64").is_none());
    }

    #[test]
    fn latest_stable_redirect_fallback_reads_tag_url() {
        let (url, request_rx, handle) = serve_http_once("200 OK", "text/html", b"<html></html>");
        let tag_url = url.replace("/release", "/Hmbown/CodeWhale/releases/tag/v9.9.9");

        let tag = fetch_latest_stable_tag_from_redirect_url(&tag_url, None)
            .expect("tag should parse from final URL");

        assert_eq!(tag, "v9.9.9");
        let request = request_rx.recv().expect("captured request");
        assert!(
            request.starts_with("GET /Hmbown/CodeWhale/releases/tag/v9.9.9 "),
            "got {request:?}"
        );
        handle.join().expect("test server thread");
    }

    #[test]
    fn github_release_html_parser_skips_empty_first_marker() {
        let body = r#"
            <a href="/Hmbown/CodeWhale/releases/tag/?expanded=true">generic</a>
            <a href="/Hmbown/CodeWhale/releases/tag/v9.9.9">latest</a>
        "#;

        assert_eq!(
            release_tag_from_github_release_html(body).as_deref(),
            Some("v9.9.9")
        );
    }

    #[test]
    fn cnb_release_base_url_includes_tag_directory() {
        assert_eq!(
            codewhale_release::cnb_release_base_url("0.8.47"),
            "https://cnb.cool/codewhale.net/codewhale/-/releases/download/v0.8.47"
        );
        assert_eq!(
            codewhale_release::cnb_release_base_url("v0.8.47"),
            "https://cnb.cool/codewhale.net/codewhale/-/releases/download/v0.8.47"
        );
    }

    #[test]
    fn stable_update_is_needed_only_when_latest_is_newer() {
        assert!(update_is_needed(ReleaseChannel::Stable, "0.8.45", "v0.8.46").unwrap());
        assert!(update_is_needed(ReleaseChannel::Stable, "0.8.45", "v0.9.0-beta.1").unwrap());
        assert!(!update_is_needed(ReleaseChannel::Stable, "0.8.45", "v0.8.45").unwrap());
        assert!(!update_is_needed(ReleaseChannel::Stable, "0.9.0", "v0.9.0-beta.1").unwrap());
        assert!(
            !update_is_needed(ReleaseChannel::Stable, "0.9.0-beta.2", "v0.9.0-beta.1").unwrap()
        );
    }

    #[test]
    fn beta_update_allows_switching_from_same_stable_to_beta() {
        assert!(update_is_needed(ReleaseChannel::Beta, "1.0.0", "v1.0.0-beta.2").unwrap());
        assert!(!update_is_needed(ReleaseChannel::Beta, "1.0.0-beta.2", "v1.0.0-beta.2").unwrap());
        assert!(!update_is_needed(ReleaseChannel::Beta, "1.0.0-beta.3", "v1.0.0-beta.2").unwrap());
        assert!(update_is_needed(ReleaseChannel::Beta, "1.0.0-beta.2", "v1.0.0-beta.3").unwrap());
        assert!(!update_is_needed(ReleaseChannel::Beta, "2.0.0", "v1.0.0-beta.3").unwrap());
        assert!(!update_is_needed(ReleaseChannel::Beta, "1.0.0-rc.1", "v1.0.0-beta.3").unwrap());
    }

    #[test]
    fn parse_release_version_accepts_tags_and_build_suffixes() {
        assert_eq!(
            codewhale_release::parse_release_version("v0.9.0-beta.1").unwrap(),
            semver::Version::parse("0.9.0-beta.1").unwrap()
        );
        assert_eq!(
            codewhale_release::parse_release_version("0.8.45 (abcdef123456)").unwrap(),
            semver::Version::parse("0.8.45").unwrap()
        );
    }

    #[test]
    fn beta_release_detection_requires_beta_tag() {
        let rc_prerelease = Release {
            tag_name: "v0.9.0-rc.1".to_string(),
            prerelease: true,
            assets: vec![],
        };
        let beta_tag = Release {
            tag_name: "v0.9.0-beta.1".to_string(),
            prerelease: false,
            assets: vec![],
        };
        let stable = Release {
            tag_name: "v0.9.0".to_string(),
            prerelease: false,
            assets: vec![],
        };

        assert!(!is_beta_tag(&rc_prerelease.tag_name));
        assert!(is_beta_tag(&beta_tag.tag_name));
        assert!(!is_beta_tag(&stable.tag_name));
    }

    #[test]
    fn update_fallback_hint_points_china_users_to_cnb_and_asset_mirrors() {
        let hint = update_network_fallback_hint();

        assert!(hint.contains(codewhale_release::CNB_REPO_URL), "{hint}");
        assert!(
            hint.contains(codewhale_release::RELEASE_BASE_URL_ENV),
            "{hint}"
        );
        assert!(
            hint.contains(codewhale_release::UPDATE_VERSION_ENV),
            "{hint}"
        );
        assert!(hint.contains("codewhale-cli"), "{hint}");
        assert!(!hint.contains("codewhale-tui --locked"), "{hint}");
    }

    fn serve_http_responses(
        responses: Vec<(&'static str, &'static str, &'static [u8])>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        serve_http_owned_responses(
            responses
                .into_iter()
                .map(|(status, content_type, body)| (status, content_type, body.to_vec()))
                .collect(),
        )
    }

    fn serve_http_owned_responses(
        responses: Vec<(&'static str, &'static str, Vec<u8>)>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("test server addr");
        let (request_tx, request_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            for (status, content_type, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept test request");
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).expect("read test request");
                request_tx
                    .send(String::from_utf8_lossy(&buf[..n]).to_string())
                    .expect("send captured request");

                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .expect("write test response headers");
                stream.write_all(&body).expect("write test response body");
            }
        });

        (format!("http://{addr}/release"), request_rx, handle)
    }

    fn serve_http_once(
        status: &'static str,
        content_type: &'static str,
        body: &'static [u8],
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        serve_http_responses(vec![(status, content_type, body)])
    }

    // ---------------------------------------------------------------------
    // Proactive first-party source selection.
    //
    // Every test here is deterministic and offline. Probe order is fixed by a
    // gate rather than by sleeping, because the contract under test is "the
    // first source to *answer* wins" — a timing-based test would be asserting
    // the scheduler, not the selector.
    // ---------------------------------------------------------------------

    /// A one-shot gate that holds one probe until another has answered.
    #[derive(Default)]
    struct ProbeGate {
        opened: Mutex<bool>,
        ready: Condvar,
    }

    impl ProbeGate {
        fn open(&self) {
            *self.opened.lock().unwrap_or_else(|err| err.into_inner()) = true;
            self.ready.notify_all();
        }

        fn wait(&self) {
            let mut opened = self.opened.lock().unwrap_or_else(|err| err.into_inner());
            while !*opened {
                opened = self
                    .ready
                    .wait(opened)
                    .unwrap_or_else(|err| err.into_inner());
            }
        }
    }

    /// One source's scripted response, plus when it is allowed to answer.
    struct ScriptedProbe {
        wait_for: Option<Arc<ProbeGate>>,
        then_open: Option<Arc<ProbeGate>>,
        answer: Result<String, String>,
    }

    impl ScriptedProbe {
        fn ready(answer: Result<String, String>) -> Self {
            Self {
                wait_for: None,
                then_open: None,
                answer,
            }
        }

        fn held(gate: &Arc<ProbeGate>, answer: Result<String, String>) -> Self {
            Self {
                wait_for: Some(Arc::clone(gate)),
                then_open: None,
                answer,
            }
        }

        fn opening(mut self, gate: &Arc<ProbeGate>) -> Self {
            self.then_open = Some(Arc::clone(gate));
            self
        }
    }

    fn scripted_manifest_fetcher(
        script: Vec<(&'static str, ScriptedProbe)>,
    ) -> Arc<ManifestFetcher> {
        let script: HashMap<&'static str, ScriptedProbe> = script.into_iter().collect();
        Arc::new(move |candidate: &ReleaseSourceCandidate| {
            let label = candidate.source.label();
            let probe = script
                .get(label)
                .unwrap_or_else(|| panic!("no scripted probe for {label}"));
            if let Some(gate) = &probe.wait_for {
                gate.wait();
            }
            let answer = probe
                .answer
                .clone()
                .map(String::into_bytes)
                .map_err(|message| anyhow!(message));
            if let Some(gate) = &probe.then_open {
                gate.open();
            }
            answer
        })
    }

    fn manifest_covering_linux_x64() -> String {
        format!(
            "{}  codewhale-linux-x64\n{}  codew-linux-x64\n",
            "a".repeat(64),
            "b".repeat(64)
        )
    }

    fn manifest_missing_linux_x64() -> String {
        format!("{}  codewhale-macos-arm64\n", "c".repeat(64))
    }

    fn github_fetched_release(tag_name: &str) -> FetchedRelease {
        FetchedRelease {
            release: Release {
                tag_name: tag_name.to_string(),
                prerelease: is_beta_tag(tag_name),
                assets: vec![
                    Asset {
                        name: "codewhale-linux-x64".to_string(),
                        browser_download_url: format!(
                            "https://github.com/Hmbown/CodeWhale/releases/download/{tag_name}/codewhale-linux-x64"
                        ),
                    },
                    Asset {
                        name: CHECKSUM_MANIFEST_ASSET.to_string(),
                        browser_download_url: format!(
                            "https://github.com/Hmbown/CodeWhale/releases/download/{tag_name}/{CHECKSUM_MANIFEST_ASSET}"
                        ),
                    },
                ],
            },
            source: UpdateReleaseSource::GitHub,
        }
    }

    fn candidates_for(
        fetched: &FetchedRelease,
        os: &str,
        arch: &str,
    ) -> Option<Vec<ReleaseSourceCandidate>> {
        proactive_source_candidates(fetched, "codewhale-linux-x64", os, arch)
    }

    fn linux_x64_candidates(tag_name: &str) -> Vec<ReleaseSourceCandidate> {
        candidates_for(&github_fetched_release(tag_name), "linux", "x86_64")
            .expect("linux x64 must race GitHub against the CNB mirror")
    }

    #[test]
    fn cnb_wins_when_its_manifest_answers_first() {
        let github_gate = Arc::new(ProbeGate::default());
        let fetch = scripted_manifest_fetcher(vec![
            (
                "GitHub Releases",
                ScriptedProbe::held(&github_gate, Ok(manifest_covering_linux_x64())),
            ),
            (
                "CNB mirror",
                ScriptedProbe::ready(Ok(manifest_covering_linux_x64())),
            ),
        ]);

        let plan = select_release_source(linux_x64_candidates("v9.9.9"), fetch)
            .expect("a source must win");
        github_gate.open();

        assert_eq!(
            plan.source,
            UpdateReleaseSource::Cnb {
                base_url: cnb_release_base_url("v9.9.9"),
            }
        );
        assert_eq!(
            plan.binary_url,
            "https://cnb.cool/codewhale.net/codewhale/-/releases/download/v9.9.9/codewhale-linux-x64",
            "the binary must come from the source whose manifest won"
        );
        assert_eq!(plan.binary_name, "codewhale-linux-x64");
        assert!(
            plan.checksums.contains_key("codewhale-linux-x64"),
            "the winning manifest must be carried forward, not refetched"
        );
    }

    #[test]
    fn github_wins_when_its_manifest_answers_first() {
        let cnb_gate = Arc::new(ProbeGate::default());
        let fetch = scripted_manifest_fetcher(vec![
            (
                "GitHub Releases",
                ScriptedProbe::ready(Ok(manifest_covering_linux_x64())),
            ),
            (
                "CNB mirror",
                ScriptedProbe::held(&cnb_gate, Ok(manifest_covering_linux_x64())),
            ),
        ]);

        let plan = select_release_source(linux_x64_candidates("v9.9.9"), fetch)
            .expect("a source must win");
        // The losing worker may finish now that the selector has observed the
        // GitHub result. Opening this gate from inside GitHub's fetch closure
        // was too early: CNB could parse and send first after both fetches had
        // returned, making the test assert scheduler luck rather than arrival.
        cnb_gate.open();

        assert_eq!(plan.source, UpdateReleaseSource::GitHub);
        assert_eq!(
            plan.binary_url,
            "https://github.com/Hmbown/CodeWhale/releases/download/v9.9.9/codewhale-linux-x64"
        );
    }

    #[test]
    fn a_source_that_answers_first_but_cannot_serve_this_platform_loses() {
        // GitHub answers first every time here; it just answers with something
        // unusable. The loser of a race is decided by the manifest, not by
        // arrival order alone.
        for first_answer in [
            Err("connection refused".to_string()),
            Ok(manifest_missing_linux_x64()),
            Ok("not a checksum manifest".to_string()),
            Ok(String::new()),
        ] {
            let cnb_gate = Arc::new(ProbeGate::default());
            let fetch = scripted_manifest_fetcher(vec![
                (
                    "GitHub Releases",
                    ScriptedProbe::ready(first_answer.clone()).opening(&cnb_gate),
                ),
                (
                    "CNB mirror",
                    ScriptedProbe::held(&cnb_gate, Ok(manifest_covering_linux_x64())),
                ),
            ]);

            let plan = select_release_source(linux_x64_candidates("v9.9.9"), fetch)
                .unwrap_or_else(|err| panic!("CNB must win over {first_answer:?}: {err:#}"));

            assert_eq!(
                plan.source,
                UpdateReleaseSource::Cnb {
                    base_url: cnb_release_base_url("v9.9.9"),
                },
                "unusable first answer {first_answer:?} must not win"
            );
        }
    }

    #[test]
    fn selection_fails_closed_when_no_source_is_usable() {
        let fetch = scripted_manifest_fetcher(vec![
            (
                "GitHub Releases",
                ScriptedProbe::ready(Err("dns failure".to_string())),
            ),
            (
                "CNB mirror",
                ScriptedProbe::ready(Ok(manifest_missing_linux_x64())),
            ),
        ]);

        let err = select_release_source(linux_x64_candidates("v9.9.9"), fetch)
            .expect_err("no usable source must fail rather than download unverified bytes");
        let message = format!("{err:#}");

        assert!(
            message.contains("no release source published a usable"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("dns failure"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("does not list codewhale-linux-x64"),
            "the unusable manifest must be reported as unusable: {message}"
        );
        assert!(
            message.contains("GitHub Releases") && message.contains("CNB mirror"),
            "both failures must be attributed: {message}"
        );
    }

    #[test]
    fn only_linux_x64_races_the_cnb_mirror() {
        let fetched = github_fetched_release("v9.9.9");

        for (os, arch) in [
            ("linux", "aarch64"),
            ("linux", "riscv64"),
            ("macos", "x86_64"),
            ("macos", "aarch64"),
            ("windows", "x86_64"),
            ("android", "aarch64"),
        ] {
            assert!(
                candidates_for(&fetched, os, arch).is_none(),
                "{os}/{arch} must keep its single canonical source"
            );
        }

        let raced = candidates_for(&fetched, "linux", "x86_64").expect("linux x64 races");
        assert_eq!(raced.len(), 2);
    }

    #[test]
    fn cnb_candidate_targets_the_exact_tag_and_platform_asset() {
        let candidate = cnb_source_candidate("v0.9.0-beta.2", "linux", "x86_64");

        assert_eq!(
            candidate.source,
            UpdateReleaseSource::Cnb {
                base_url: cnb_release_base_url("v0.9.0-beta.2"),
            }
        );
        assert_eq!(
            candidate.manifest_url,
            "https://cnb.cool/codewhale.net/codewhale/-/releases/download/v0.9.0-beta.2/codewhale-artifacts-sha256.txt"
        );
        assert_eq!(
            candidate.binary_url,
            "https://cnb.cool/codewhale.net/codewhale/-/releases/download/v0.9.0-beta.2/codewhale-linux-x64"
        );
        assert_eq!(candidate.binary_name, "codewhale-linux-x64");
    }

    #[test]
    fn github_candidate_addresses_the_manifest_even_when_the_payload_omits_it() {
        let release = Release {
            tag_name: "0.9.9".to_string(),
            prerelease: false,
            assets: vec![Asset {
                name: "codewhale-linux-x64".to_string(),
                browser_download_url: "https://cdn.example/codewhale-linux-x64".to_string(),
            }],
        };

        let candidate =
            github_source_candidate(&release, "codewhale-linux-x64").expect("github candidate");

        assert_eq!(
            candidate.manifest_url,
            "https://github.com/Hmbown/CodeWhale/releases/download/v0.9.9/codewhale-artifacts-sha256.txt"
        );
        assert_eq!(
            candidate.binary_url,
            "https://cdn.example/codewhale-linux-x64"
        );
    }

    #[test]
    fn every_single_source_platform_requires_a_checksum_manifest() {
        for (os, arch) in [
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("macos", "x86_64"),
            ("macos", "aarch64"),
            ("windows", "x86_64"),
            ("windows", "aarch64"),
            ("android", "aarch64"),
        ] {
            let asset_stem = release_asset_stem_for_prefix("codewhale", os, arch);
            let asset_name = release_asset_name_for_prefix("codewhale", os, arch);
            let fetched = FetchedRelease {
                release: Release {
                    tag_name: "v9.9.9".to_string(),
                    prerelease: false,
                    assets: vec![Asset {
                        name: asset_name.clone(),
                        browser_download_url: format!("https://cdn.example/{asset_name}"),
                    }],
                },
                source: UpdateReleaseSource::GitHub,
            };

            let err = single_source_download_plan(&fetched, &asset_stem, None)
                .expect_err("a missing manifest must fail before the binary download");
            let message = format!("{err:#}");
            assert!(
                message.contains("does not publish required codewhale-artifacts-sha256.txt"),
                "{os}/{arch} unexpectedly allowed an unverifiable plan: {message}"
            );
            assert!(message.contains(&asset_name), "{os}/{arch}: {message}");
        }
    }

    #[test]
    fn a_malformed_single_source_manifest_fails_before_binary_download() {
        let (manifest_url, request_rx, handle) =
            serve_http_once("200 OK", "text/plain", b"not a checksum manifest\n");
        let fetched = FetchedRelease {
            release: Release {
                tag_name: "v9.9.9".to_string(),
                prerelease: false,
                assets: vec![
                    Asset {
                        name: CHECKSUM_MANIFEST_ASSET.to_string(),
                        browser_download_url: manifest_url,
                    },
                    Asset {
                        name: "codewhale-macos-arm64".to_string(),
                        browser_download_url: "https://cdn.example/should-not-download".to_string(),
                    },
                ],
            },
            source: UpdateReleaseSource::GitHub,
        };

        let err = single_source_download_plan(&fetched, "codewhale-macos-arm64", None)
            .expect_err("a malformed manifest must fail closed");
        let message = format!("{err:#}");
        assert!(message.contains("failed to parse"), "{message}");
        assert!(
            message.contains("invalid SHA256 manifest line"),
            "{message}"
        );
        let request = request_rx.recv().expect("manifest request");
        assert!(request.starts_with("GET /release "), "got {request:?}");
        handle.join().expect("test server thread");
    }

    #[test]
    fn a_single_source_manifest_must_cover_the_exact_platform_binary() {
        let manifest = format!("{}  codewhale-linux-x64\n", "a".repeat(64));
        let (manifest_url, request_rx, handle) =
            serve_http_owned_responses(vec![("200 OK", "text/plain", manifest.into_bytes())]);
        let fetched = FetchedRelease {
            release: Release {
                tag_name: "v9.9.9".to_string(),
                prerelease: false,
                assets: vec![
                    Asset {
                        name: CHECKSUM_MANIFEST_ASSET.to_string(),
                        browser_download_url: manifest_url,
                    },
                    Asset {
                        name: "codewhale-windows-x64.exe".to_string(),
                        browser_download_url: "https://cdn.example/should-not-download.exe"
                            .to_string(),
                    },
                ],
            },
            source: UpdateReleaseSource::GitHub,
        };

        let err = single_source_download_plan(&fetched, "codewhale-windows-x64", None)
            .expect_err("a manifest for another platform must fail closed");
        let message = format!("{err:#}");
        assert!(
            message.contains("does not list codewhale-windows-x64.exe"),
            "{message}"
        );
        let request = request_rx.recv().expect("manifest request");
        assert!(request.starts_with("GET /release "), "got {request:?}");
        handle.join().expect("test server thread");
    }

    #[test]
    fn an_explicit_mirror_remains_pinned_and_verified_from_that_mirror() {
        let bytes = b"verified mirror bytes";
        let manifest = format!("{}  codewhale-macos-arm64\n", sha256_hex(bytes));
        let (url, request_rx, handle) =
            serve_http_owned_responses(vec![("200 OK", "text/plain", manifest.into_bytes())]);
        let base_url = url.trim_end_matches("/release").to_string();
        let fetched = FetchedRelease {
            release: release_from_mirror_base_url(&base_url, "9.9.9", "macos", "aarch64"),
            source: UpdateReleaseSource::Mirror {
                base_url: base_url.clone(),
            },
        };

        let plan = single_source_download_plan(&fetched, "codewhale-macos-arm64", None)
            .expect("the explicit mirror's valid manifest should produce a plan");
        assert_eq!(
            plan.source,
            UpdateReleaseSource::Mirror {
                base_url: base_url.clone(),
            }
        );
        assert_eq!(
            plan.binary_url,
            mirror_asset_url(&base_url, "codewhale-macos-arm64")
        );
        verify_downloaded_asset(&plan, bytes)
            .expect("the pinned mirror's checksum must verify its bytes");
        let request = request_rx.recv().expect("manifest request");
        assert!(
            request.starts_with("GET /codewhale-artifacts-sha256.txt "),
            "got {request:?}"
        );
        handle.join().expect("test server thread");
    }

    #[test]
    fn a_github_release_without_this_platform_leaves_cnb_as_the_only_candidate() {
        let fetched = FetchedRelease {
            release: Release {
                tag_name: "v9.9.9".to_string(),
                prerelease: false,
                assets: vec![Asset {
                    name: "codewhale-macos-arm64".to_string(),
                    browser_download_url: "https://cdn.example/codewhale-macos-arm64".to_string(),
                }],
            },
            source: UpdateReleaseSource::GitHub,
        };

        let candidates = candidates_for(&fetched, "linux", "x86_64").expect("linux x64 races");

        assert_eq!(candidates.len(), 1);
        assert!(matches!(
            candidates[0].source,
            UpdateReleaseSource::Cnb { .. }
        ));
    }

    #[test]
    fn beta_tags_race_the_same_two_sources_as_stable_tags() {
        let candidates = linux_x64_candidates("v0.9.0-beta.2");

        assert_eq!(candidates[0].source, UpdateReleaseSource::GitHub);
        assert_eq!(
            candidates[1].source,
            UpdateReleaseSource::Cnb {
                base_url: cnb_release_base_url("v0.9.0-beta.2"),
            },
            "the beta tag must be carried into the CNB URL verbatim"
        );
    }

    #[test]
    fn explicit_overrides_take_precedence_over_probing() {
        {
            let _env = UpdateEnvGuard::clear();
            set_update_env(
                codewhale_release::RELEASE_BASE_URL_ENV,
                "https://mirror.example/assets",
            );
            set_update_env(codewhale_release::UPDATE_VERSION_ENV, "9.9.9");

            let fetched = fetch_latest_release(ReleaseChannel::Stable, None)
                .expect("a pinned mirror resolves without a network");

            assert_eq!(
                fetched.source,
                UpdateReleaseSource::Mirror {
                    base_url: "https://mirror.example/assets".to_string(),
                }
            );
            assert!(fetched.source.is_pinned_mirror());
            assert!(
                candidates_for(&fetched, "linux", "x86_64").is_none(),
                "an explicit base URL must never be raced against CNB"
            );
            assert_eq!(
                describe_release_source_for_check(&fetched, "codewhale-linux-x64", None),
                "release mirror (https://mirror.example/assets)"
            );
        }

        {
            let _env = UpdateEnvGuard::clear();
            set_update_env(codewhale_release::CNB_MIRROR_ENV, "1");
            set_update_env(codewhale_release::UPDATE_VERSION_ENV, "9.9.9");

            let fetched = fetch_latest_release(ReleaseChannel::Stable, None)
                .expect("the CNB override resolves without a network");

            assert_eq!(
                fetched.source,
                UpdateReleaseSource::Cnb {
                    base_url: cnb_release_base_url("9.9.9"),
                },
                "an explicit CNB request must be reported as CNB, not as a generic mirror"
            );
            assert!(
                candidates_for(&fetched, "linux", "x86_64").is_none(),
                "an explicit CNB request must not be raced back against GitHub"
            );
        }

        {
            let _env = UpdateEnvGuard::clear();
            set_update_env(codewhale_release::CNB_MIRROR_ENV, "1");
            set_update_env(
                codewhale_release::RELEASE_BASE_URL_ENV,
                "https://mirror.example/assets",
            );

            let fetched = fetch_latest_release(ReleaseChannel::Stable, None)
                .expect("a pinned mirror resolves without a network");

            assert_eq!(
                fetched.source,
                UpdateReleaseSource::Mirror {
                    base_url: "https://mirror.example/assets".to_string(),
                },
                "an explicit base URL outranks the CNB flag"
            );
        }
    }

    #[test]
    fn a_locked_source_serves_both_the_manifest_and_the_binary() {
        const BINARY: &[u8] = b"\x7fELF codewhale linux x64 payload";
        let manifest = format!("{}  codewhale-linux-x64\n", sha256_hex(BINARY));
        let (url, request_rx, handle) = serve_http_owned_responses(vec![
            ("200 OK", "text/plain", manifest.into_bytes()),
            ("200 OK", "application/octet-stream", BINARY.to_vec()),
        ]);
        let origin = url.trim_end_matches("/release").to_string();
        let candidate = ReleaseSourceCandidate {
            source: UpdateReleaseSource::Cnb {
                base_url: origin.clone(),
            },
            manifest_url: mirror_asset_url(&origin, CHECKSUM_MANIFEST_ASSET),
            binary_name: "codewhale-linux-x64".to_string(),
            binary_url: mirror_asset_url(&origin, "codewhale-linux-x64"),
        };

        let plan = select_release_source(vec![candidate], manifest_probe_fetcher(None))
            .expect("the only reachable source must win");
        let bytes = download_url(&plan.binary_url, None).expect("binary download");
        verify_downloaded_asset(&plan, &bytes)
            .expect("bytes from the locked source must match its own manifest");

        assert_eq!(bytes, BINARY);
        let manifest_request = request_rx.recv().expect("manifest request");
        let binary_request = request_rx.recv().expect("binary request");
        assert!(
            manifest_request.starts_with("GET /codewhale-artifacts-sha256.txt "),
            "got {manifest_request:?}"
        );
        assert!(
            binary_request.starts_with("GET /codewhale-linux-x64 "),
            "got {binary_request:?}"
        );
        handle.join().expect("test server thread");
    }

    #[test]
    fn a_checksum_mismatch_fails_closed_and_names_the_source() {
        let mut plan = DownloadPlan {
            source: UpdateReleaseSource::Cnb {
                base_url: cnb_release_base_url("v9.9.9"),
            },
            binary_name: "codewhale-linux-x64".to_string(),
            binary_url: "https://cnb.example/codewhale-linux-x64".to_string(),
            checksums: HashMap::from([("codewhale-linux-x64".to_string(), "a".repeat(64))]),
        };

        let err = verify_downloaded_asset(&plan, b"tampered bytes")
            .expect_err("a mismatch must never install");
        let message = format!("{err:#}");
        assert!(message.contains("SHA256 mismatch"), "{message}");
        assert!(message.contains("CNB mirror"), "{message}");

        plan.checksums = HashMap::from([("codew-linux-x64".to_string(), "a".repeat(64))]);
        let err = verify_downloaded_asset(&plan, b"bytes")
            .expect_err("an uncovered asset must never install");
        let message = format!("{err:#}");
        assert!(
            message.contains("is missing codewhale-linux-x64"),
            "{message}"
        );
    }

    #[test]
    fn release_sources_describe_themselves_for_status_and_receipts() {
        assert_eq!(UpdateReleaseSource::GitHub.describe(), "GitHub Releases");
        assert!(!UpdateReleaseSource::GitHub.is_pinned_mirror());

        let cnb = UpdateReleaseSource::Cnb {
            base_url: cnb_release_base_url("v9.9.9"),
        };
        assert_eq!(
            cnb.describe(),
            "CNB mirror (https://cnb.cool/codewhale.net/codewhale/-/releases/download/v9.9.9)"
        );
        assert!(cnb.is_pinned_mirror());

        let mirror = UpdateReleaseSource::Mirror {
            base_url: "https://mirror.example/assets".to_string(),
        };
        assert_eq!(
            mirror.describe(),
            "release mirror (https://mirror.example/assets)"
        );
        assert!(mirror.is_pinned_mirror());
    }

    #[test]
    fn validate_and_build_proxy_accepts_supported_proxy_urls() {
        validate_and_build_proxy("http://localhost:7897").expect("http proxy");
        validate_and_build_proxy("https://proxy.example.com:8080").expect("https proxy");
        validate_and_build_proxy("socks5://127.0.0.1:1080").expect("socks proxy");
    }

    #[test]
    fn validate_and_build_proxy_rejects_malformed_urls() {
        let err = validate_and_build_proxy("not a valid url").expect_err("malformed URL");
        assert!(err.to_string().contains("invalid proxy URL"));
    }

    #[test]
    fn fetch_latest_release_from_url_reads_mocked_release_json() {
        let body = br#"{
          "tag_name": "v9.9.9",
          "assets": [
            { "name": "codewhale-linux-x64", "browser_download_url": "http://example.invalid/codewhale-linux-x64" },
            { "name": "codewhale-artifacts-sha256.txt", "browser_download_url": "http://example.invalid/codewhale-artifacts-sha256.txt" }
          ]
        }"#;
        let (url, request_rx, handle) = serve_http_once("200 OK", "application/json", body);
        let release = fetch_latest_release_from_url(&url, None).expect("release JSON should parse");

        assert_eq!(release.tag_name, "v9.9.9");
        assert_eq!(release.assets.len(), 2);

        let request = request_rx.recv().expect("captured request");
        let request_lower = request.to_ascii_lowercase();
        assert!(request.starts_with("GET /release "), "got {request:?}");
        assert!(
            request_lower.contains("accept: application/vnd.github+json"),
            "got {request:?}"
        );
        assert!(
            request_lower.contains("user-agent: codewhale-updater"),
            "got {request:?}"
        );
        handle.join().expect("test server thread");
    }

    #[test]
    fn fetch_latest_release_from_url_retries_transient_gateway_error() {
        let body = br#"{
          "tag_name": "v9.9.9",
          "assets": [
            { "name": "codewhale-linux-x64", "browser_download_url": "http://example.invalid/codewhale-linux-x64" }
          ]
        }"#;
        let (url, request_rx, handle) = serve_http_responses(vec![
            ("504 Gateway Timeout", "text/plain", b"gateway timeout"),
            ("200 OK", "application/json", body),
        ]);
        let release = fetch_latest_release_from_url(&url, None)
            .expect("release JSON should parse after retry");

        assert_eq!(release.tag_name, "v9.9.9");
        let first = request_rx.recv().expect("first request");
        let second = request_rx.recv().expect("second request");
        assert!(first.starts_with("GET /release "), "got {first:?}");
        assert!(second.starts_with("GET /release "), "got {second:?}");
        handle.join().expect("test server thread");
    }

    #[test]
    fn fetch_latest_release_from_url_reports_http_errors() {
        let (url, _request_rx, handle) = serve_http_responses(vec![
            ("500 Internal Server Error", "text/plain", b"server broke"),
            ("500 Internal Server Error", "text/plain", b"server broke"),
            ("500 Internal Server Error", "text/plain", b"server broke"),
        ]);
        let err = fetch_latest_release_from_url(&url, None).expect_err("HTTP 500 should fail");

        assert!(
            err.to_string().contains("HTTP 500"),
            "unexpected error: {err:#}"
        );
        handle.join().expect("test server thread");
    }

    #[test]
    fn fetch_latest_beta_release_from_url_selects_first_beta_release() {
        let body = br#"[
          { "tag_name": "v0.9.0", "prerelease": false, "assets": [] },
          { "tag_name": "v0.9.0-rc.1", "prerelease": true, "assets": [] },
          { "tag_name": "v0.9.0-beta.2", "prerelease": true, "assets": [
            { "name": "codewhale-linux-x64", "browser_download_url": "http://example.invalid/codewhale-linux-x64" }
          ] },
          { "tag_name": "v0.9.0-beta.1", "prerelease": true, "assets": [] }
        ]"#;
        let (url, request_rx, handle) = serve_http_once("200 OK", "application/json", body);
        let release =
            fetch_latest_beta_release_from_url(&url, None).expect("beta release JSON should parse");

        assert_eq!(release.tag_name, "v0.9.0-beta.2");
        assert!(release.prerelease);

        let request = request_rx.recv().expect("captured request");
        let request_lower = request.to_ascii_lowercase();
        assert!(request.starts_with("GET /release "), "got {request:?}");
        assert!(
            request_lower.contains("accept: application/vnd.github+json"),
            "got {request:?}"
        );
        handle.join().expect("test server thread");
    }

    #[test]
    fn fetch_latest_beta_release_from_url_reports_missing_beta() {
        let body = br#"[
          { "tag_name": "v0.9.0", "prerelease": false, "assets": [] }
        ]"#;
        let (url, _request_rx, handle) = serve_http_once("200 OK", "application/json", body);
        let err =
            fetch_latest_beta_release_from_url(&url, None).expect_err("missing beta should fail");

        assert!(
            err.to_string().contains("no beta release found"),
            "unexpected error: {err:#}"
        );
        handle.join().expect("test server thread");
    }

    #[test]
    fn download_url_retries_transient_gateway_error() {
        let (url, request_rx, handle) = serve_http_responses(vec![
            ("503 Service Unavailable", "text/plain", b"try again"),
            ("200 OK", "application/octet-stream", b"\0binary bytes"),
        ]);
        let bytes = download_url(&url, None).expect("binary download should retry and succeed");

        assert_eq!(bytes, b"\0binary bytes");
        let first = request_rx.recv().expect("first request");
        let second = request_rx.recv().expect("second request");
        assert!(first.starts_with("GET /release "), "got {first:?}");
        assert!(second.starts_with("GET /release "), "got {second:?}");
        handle.join().expect("test server thread");
    }

    #[test]
    fn download_url_reads_binary_body_with_updater_user_agent() {
        let (url, request_rx, handle) =
            serve_http_once("200 OK", "application/octet-stream", b"\0binary bytes");
        let bytes = download_url(&url, None).expect("binary download should succeed");

        assert_eq!(bytes, b"\0binary bytes");

        let request = request_rx.recv().expect("captured request");
        let request_lower = request.to_ascii_lowercase();
        assert!(request.starts_with("GET /release "), "got {request:?}");
        assert!(
            request_lower.contains("user-agent: codewhale-updater"),
            "got {request:?}"
        );
        handle.join().expect("test server thread");
    }
}
