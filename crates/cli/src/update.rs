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
    CHECKSUM_MANIFEST_ASSET, ReleaseChannel, ReleaseQuery, UPDATE_USER_AGENT,
    compare_release_versions, is_beta_tag, mirror_asset_url, resolve_release_query,
    update_is_needed, update_network_fallback_hint,
};
use reqwest::Proxy;
use std::io::Write;
use std::time::Duration;

const GITHUB_LATEST_RELEASE_PAGE_URL: &str = "https://github.com/Hmbown/CodeWhale/releases/latest";
const GITHUB_RELEASE_DOWNLOAD_BASE_URL: &str =
    "https://github.com/Hmbown/CodeWhale/releases/download";
const UPDATE_HTTP_ATTEMPTS: usize = 3;
const UPDATE_HTTP_RETRY_DELAY_MS: u64 = 100;
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

    let targets = update_targets_for_exe(&current_exe);
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

    if check_only {
        let latest_tag = latest_release_tag(channel, proxy.as_ref())
            .with_context(update_network_fallback_hint)?;
        println!("Latest {} release: {latest_tag}", channel.label());
        if update_is_needed(channel, current_version, &latest_tag)? {
            println!("Update available. Run `codewhale update` to install {latest_tag}.");
        } else {
            match compare_release_versions(current_version, &latest_tag)? {
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

    if let UpdateReleaseSource::Mirror { base_url } = &fetched.source {
        if channel == ReleaseChannel::Beta {
            println!(
                "Using release mirror {base_url}; --beta does not select GitHub beta releases in mirror mode."
            );
        }
    } else if !update_is_needed(channel, current_version, latest_tag)? {
        println!("Already up to date; no download needed.");
        return Ok(());
    }

    // Step 2: Download the aggregated SHA256 checksum manifest if available
    let checksum_manifest = match select_checksum_manifest_asset(release) {
        Some(checksum_asset) => {
            println!("Downloading {}...", checksum_asset.name);
            let checksum_bytes = download_url(&checksum_asset.browser_download_url, proxy.as_ref())
                .with_context(|| {
                    format!(
                        "failed to download {}\n{}",
                        checksum_asset.name,
                        update_network_fallback_hint()
                    )
                })?;
            let checksum_text = std::str::from_utf8(&checksum_bytes)
                .with_context(|| format!("{} is not valid UTF-8", checksum_asset.name))?;
            Some(parse_checksum_manifest(checksum_text)?)
        }
        None => {
            println!("  (no SHA256 checksum manifest found; skipping verification)");
            None
        }
    };

    // Step 3: Download and verify every colocated binary in the install.
    let mut downloads = Vec::new();
    for target in &targets {
        let asset = select_platform_asset(release, &target.asset_stem).with_context(|| {
            format!(
                "no asset found for platform {} in release {latest_tag}. \
                     Available assets: {}",
                target.asset_stem,
                release
                    .assets
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

        println!("Downloading {}...", asset.name);
        let bytes =
            download_url(&asset.browser_download_url, proxy.as_ref()).with_context(|| {
                format!(
                    "failed to download {}\n{}",
                    asset.name,
                    update_network_fallback_hint()
                )
            })?;

        if let Some(checksums) = &checksum_manifest {
            let expected = checksums
                .get(&asset.name)
                .with_context(|| format!("checksum manifest is missing {}", asset.name))?;
            let actual = sha256_hex(&bytes);
            if !actual.eq_ignore_ascii_case(expected) {
                bail!(
                    "SHA256 mismatch for {}!\n  expected: {expected}\n  actual:   {actual}",
                    asset.name
                );
            }
        }

        preflight_downloaded_binary(&asset.name, &bytes)?;
        downloads.push((target.path.clone(), asset.name.clone(), bytes));
    }

    if checksum_manifest.is_some() {
        println!("SHA256 checksum verified.");
    }

    // Step 4: Replace binaries only after all downloads and the primary
    // executable identity verify. The preflight happens before a colocated
    // sibling can change, then the primary is checked again just in time.
    replace_verified_downloads(&downloads, || {
        validate_primary_update_identity(&executable_identity)
    })?;

    println!(
        "\n✅ Successfully updated to {latest_tag}!\n\
         Updated binaries:\n{}\n\
         \n\
         Restart the application to use the new version.",
        downloads
            .iter()
            .map(|(path, asset, _)| format!("  - {} ({asset})", path.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );

    Ok(())
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
    downloads: &[(PathBuf, String, Vec<u8>)],
    validate_primary_identity: F,
) -> Result<()>
where
    F: Fn() -> Result<()>,
{
    // Fail before mutating a sibling if the primary pathname no longer names
    // the process image that initiated this update.
    validate_primary_identity()?;
    for (path, _, bytes) in downloads.iter().rev() {
        replace_binary_with_validation(path, bytes, || {
            // Re-check after each temp file is fully staged and immediately
            // before every destructive rename. This protects paired installs
            // before the sibling as well as just in time for the primary.
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateReleaseSource {
    GitHub,
    Mirror { base_url: String },
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

The package has been renamed to `codewhale`. This update will install canonical
Codewhale binaries (`codewhale` and, when present, `codewhale-tui`) beside the
legacy command when the install directory is writable. DeepSeek provider support
is unchanged.

If this update cannot write to the install directory, reinstall using your
original install method:

  npm:
    npm uninstall -g deepseek-tui
    npm install -g codewhale

  Cargo:
    cargo uninstall deepseek-tui-cli 2>/dev/null || true
    cargo uninstall deepseek-tui 2>/dev/null || true
    cargo install codewhale-cli --locked
    cargo install codewhale-tui --locked

  Homebrew:
    brew upgrade deepseek-tui

  Manual binary:
    download the matched codewhale and codewhale-tui assets from
    https://github.com/Hmbown/CodeWhale/releases/latest

Once `codewhale` is on your PATH, run `codewhale update` for future updates.",
        exe = current_exe.display(),
    )
}

pub(crate) fn binary_prefix_for_exe(current_exe: &Path) -> &'static str {
    let exe_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("codewhale")
        .to_ascii_lowercase();
    if exe_name.contains("codewhale-tui") || exe_name.contains("deepseek-tui") {
        "codewhale-tui"
    } else {
        "codewhale"
    }
}

fn sibling_prefix_for(prefix: &str) -> &'static str {
    if prefix == "codewhale-tui" {
        "codewhale"
    } else {
        "codewhale-tui"
    }
}

fn sibling_binary_path(current_exe: &Path, sibling_prefix: &str) -> PathBuf {
    current_exe.with_file_name(format!("{sibling_prefix}{}", std::env::consts::EXE_SUFFIX))
}

fn canonical_binary_path_for_prefix(current_exe: &Path, prefix: &str) -> PathBuf {
    if is_legacy_binary(current_exe) {
        current_exe.with_file_name(format!("{prefix}{}", std::env::consts::EXE_SUFFIX))
    } else {
        current_exe.to_path_buf()
    }
}

fn legacy_binary_name_for_prefix(prefix: &str) -> &'static str {
    if prefix == "codewhale-tui" {
        "deepseek-tui"
    } else {
        "deepseek"
    }
}

fn legacy_sibling_binary_path(current_exe: &Path, sibling_prefix: &str) -> PathBuf {
    current_exe.with_file_name(format!(
        "{}{}",
        legacy_binary_name_for_prefix(sibling_prefix),
        std::env::consts::EXE_SUFFIX
    ))
}

fn should_update_sibling(
    current_exe: &Path,
    canonical_sibling: &Path,
    sibling_prefix: &str,
) -> bool {
    canonical_sibling.exists()
        || (is_legacy_binary(current_exe)
            && legacy_sibling_binary_path(current_exe, sibling_prefix).exists())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateTarget {
    path: PathBuf,
    asset_stem: String,
}

fn update_targets_for_exe(current_exe: &Path) -> Vec<UpdateTarget> {
    let current_prefix = binary_prefix_for_exe(current_exe);
    let mut targets = vec![UpdateTarget {
        path: canonical_binary_path_for_prefix(current_exe, current_prefix),
        asset_stem: release_asset_stem_for_prefix(
            current_prefix,
            std::env::consts::OS,
            std::env::consts::ARCH,
        ),
    }];

    let sibling_prefix = sibling_prefix_for(current_prefix);
    let sibling = sibling_binary_path(current_exe, sibling_prefix);
    if should_update_sibling(current_exe, &sibling, sibling_prefix) {
        targets.push(UpdateTarget {
            path: sibling,
            asset_stem: release_asset_stem_for_prefix(
                sibling_prefix,
                std::env::consts::OS,
                std::env::consts::ARCH,
            ),
        });
    }

    targets
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
    let prefix = binary_prefix_for_exe(current_exe);
    release_asset_stem_for_prefix(prefix, os, rust_arch)
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
    let mut builder = codewhale_release::platform_blocking_http_client_builder();
    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy.clone());
    }
    builder
        .user_agent(UPDATE_USER_AGENT)
        .timeout(Duration::from_secs(5 * 60))
        .build()
        .context("failed to build update HTTP client")
}

fn latest_release_tag(channel: ReleaseChannel, proxy: Option<&Proxy>) -> Result<String> {
    let FetchedRelease { release, .. } = fetch_latest_release(channel, proxy)?;
    Ok(release.tag_name)
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
            source: UpdateReleaseSource::Mirror { base_url },
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

    for prefix in ["codewhale", "codewhale-tui"] {
        let name = release_asset_name_for_prefix(prefix, os, rust_arch);
        assets.push(Asset {
            browser_download_url: mirror_asset_url(base_url, &name),
            name,
        });
    }

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
    let mut last_error = None;
    for attempt in 1..=UPDATE_HTTP_ATTEMPTS {
        match download_url_once(url, proxy) {
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

fn download_url_once(url: &str, proxy: Option<&Proxy>) -> Result<(reqwest::StatusCode, Vec<u8>)> {
    let client = update_http_client(proxy)?;
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
  cargo install codewhale-tui --locked

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

    // Preserve permissions from the original binary (if it exists)
    if target.exists() {
        if let Ok(meta) = std::fs::metadata(target) {
            let _ = std::fs::set_permissions(tmp.path(), meta.permissions());
        }
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755));
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    #[cfg(unix)]
    fn write_test_executable(path: &Path) {
        std::fs::write(path, b"test executable").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        assert_eq!(update_targets_for_exe(&resolved)[0].path, resolved);
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
        let target_paths = update_targets_for_exe(&resolved)
            .into_iter()
            .map(|target| target.path)
            .collect::<Vec<_>>();

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

        let downloads = vec![
            (
                primary.clone(),
                "codewhale-android-arm64".to_string(),
                b"downloaded primary".to_vec(),
            ),
            (
                sibling.clone(),
                "codewhale-tui-android-arm64".to_string(),
                b"downloaded sibling".to_vec(),
            ),
        ];
        let error = replace_verified_downloads(&downloads, || {
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

        let downloads = vec![
            (
                primary.clone(),
                "codewhale-android-arm64".to_string(),
                b"downloaded primary".to_vec(),
            ),
            (
                sibling.clone(),
                "codewhale-tui-android-arm64".to_string(),
                b"downloaded sibling".to_vec(),
            ),
        ];
        let validation_calls = Cell::new(0);
        let error = replace_verified_downloads(&downloads, || {
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

        let downloads = vec![(
            primary.clone(),
            "codewhale-android-arm64".to_string(),
            b"downloaded primary".to_vec(),
        )];
        let validation_calls = Cell::new(0);
        let error = replace_verified_downloads(&downloads, || {
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

    /// Verify binary prefix detection for dispatcher vs TUI binary.
    #[test]
    fn test_binary_prefix_detection() {
        // TUI binary should use codewhale-tui prefix
        assert_eq!(
            binary_prefix_for_exe(Path::new("codewhale-tui")),
            "codewhale-tui"
        );
        assert_eq!(
            binary_prefix_for_exe(Path::new("codewhale-tui.exe")),
            "codewhale-tui"
        );
        assert_eq!(
            binary_prefix_for_exe(Path::new("CodeWhale-TUI.exe")),
            "codewhale-tui"
        );
        assert_eq!(
            binary_prefix_for_exe(Path::new("/usr/local/bin/codewhale-tui")),
            "codewhale-tui"
        );

        // Dispatcher binary should use codewhale prefix
        assert_eq!(binary_prefix_for_exe(Path::new("codewhale")), "codewhale");
        assert_eq!(
            binary_prefix_for_exe(Path::new("codewhale.exe")),
            "codewhale"
        );
        assert_eq!(
            binary_prefix_for_exe(Path::new("/usr/local/bin/codewhale")),
            "codewhale"
        );

        // Fallback for unknown names
        assert_eq!(
            binary_prefix_for_exe(Path::new("other-binary")),
            "codewhale"
        );

        // Legacy names still map to the canonical update asset prefixes.
        assert_eq!(
            binary_prefix_for_exe(Path::new("deepseek-tui")),
            "codewhale-tui"
        );
        assert_eq!(
            binary_prefix_for_exe(Path::new("/usr/local/bin/deepseek-tui")),
            "codewhale-tui"
        );
        assert_eq!(
            binary_prefix_for_exe(Path::new("DeepSeek-TUI.exe")),
            "codewhale-tui"
        );
        assert_eq!(binary_prefix_for_exe(Path::new("deepseek")), "codewhale");
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
    fn legacy_binary_message_gives_copy_pasteable_migration_steps() {
        let message = legacy_binary_message(Path::new("/usr/local/bin/deepseek-tui"));

        assert!(message.contains("legacy deepseek/deepseek-tui command name"));
        assert!(message.contains("install canonical"));
        assert!(message.contains("DeepSeek provider support"));
        assert!(message.contains("is unchanged"));
        assert!(message.contains("npm uninstall -g deepseek-tui"));
        assert!(message.contains("npm install -g codewhale"));
        assert!(message.contains("cargo uninstall deepseek-tui-cli 2>/dev/null || true"));
        assert!(message.contains("cargo uninstall deepseek-tui 2>/dev/null || true"));
        assert!(message.contains("cargo install codewhale-cli --locked"));
        assert!(message.contains("cargo install codewhale-tui --locked"));
        assert!(message.contains("brew upgrade deepseek-tui"));
        assert!(message.contains("https://github.com/Hmbown/CodeWhale/releases/latest"));
    }

    #[test]
    fn legacy_dispatcher_update_targets_canonical_codewhale_pair() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = dir
            .path()
            .join(format!("deepseek{}", std::env::consts::EXE_SUFFIX));
        let tui = dir
            .path()
            .join(format!("deepseek-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"legacy dispatcher").unwrap();
        std::fs::write(&tui, b"legacy tui").unwrap();

        let targets = update_targets_for_exe(&dispatcher);
        let paths = targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                dir.path()
                    .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX)),
                dir.path()
                    .join(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX))
            ]
        );
        assert!(targets[0].asset_stem.starts_with("codewhale-"));
        assert!(targets[1].asset_stem.starts_with("codewhale-tui-"));
    }

    #[test]
    fn legacy_tui_update_targets_canonical_tui_pair() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = dir
            .path()
            .join(format!("deepseek{}", std::env::consts::EXE_SUFFIX));
        let tui = dir
            .path()
            .join(format!("deepseek-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"legacy dispatcher").unwrap();
        std::fs::write(&tui, b"legacy tui").unwrap();

        let targets = update_targets_for_exe(&tui);
        let paths = targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                dir.path()
                    .join(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX)),
                dir.path()
                    .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX))
            ]
        );
        assert!(targets[0].asset_stem.starts_with("codewhale-tui-"));
        assert!(targets[1].asset_stem.starts_with("codewhale-"));
    }

    #[test]
    fn test_release_asset_stem_for_supported_platforms() {
        let cases = [
            ("codewhale", "macos", "aarch64", "codewhale-macos-arm64"),
            ("codewhale", "macos", "x86_64", "codewhale-macos-x64"),
            ("codewhale", "linux", "x86_64", "codewhale-linux-x64"),
            ("codewhale", "windows", "x86_64", "codewhale-windows-x64"),
            (
                "codewhale-tui",
                "macos",
                "aarch64",
                "codewhale-tui-macos-arm64",
            ),
            (
                "codewhale-tui",
                "linux",
                "x86_64",
                "codewhale-tui-linux-x64",
            ),
        ];

        for (exe, os, arch, expected) in cases {
            assert_eq!(release_asset_stem_for(Path::new(exe), os, arch), expected);
        }
    }

    #[test]
    fn update_targets_include_existing_sibling_tui_for_dispatcher() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = dir
            .path()
            .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX));
        let tui = dir
            .path()
            .join(format!("codewhale-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"dispatcher").unwrap();
        std::fs::write(&tui, b"tui").unwrap();

        let targets = update_targets_for_exe(&dispatcher);
        let paths = targets
            .iter()
            .map(|target| target.path.as_path())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![dispatcher.as_path(), tui.as_path()]);
        assert!(targets[0].asset_stem.starts_with("codewhale-"));
        assert!(targets[1].asset_stem.starts_with("codewhale-tui-"));
    }

    #[test]
    fn update_targets_skip_missing_sibling() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = dir
            .path()
            .join(format!("codewhale{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"dispatcher").unwrap();

        let targets = update_targets_for_exe(&dispatcher);

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].path, dispatcher);
        assert!(targets[0].asset_stem.starts_with("codewhale-"));
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
        std::fs::write(&target, b"old binary").unwrap();

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

    /// Mocked GitHub release payload covering both the dispatcher (`codewhale`)
    /// and the legacy TUI (`codewhale-tui`) binaries across our published
    /// platform/arch matrix, plus a checksum sibling that must never be picked
    /// as the primary binary.
    fn mocked_release() -> Release {
        let json = r#"{
          "tag_name": "v0.8.8",
          "assets": [
            { "name": "codewhale-linux-x64",          "browser_download_url": "https://example.invalid/codewhale-linux-x64" },
            { "name": "codewhale-macos-x64",          "browser_download_url": "https://example.invalid/codewhale-macos-x64" },
            { "name": "codewhale-macos-arm64",        "browser_download_url": "https://example.invalid/codewhale-macos-arm64" },
            { "name": "codewhale-windows-x64.exe",    "browser_download_url": "https://example.invalid/codewhale-windows-x64.exe" },
            { "name": "codewhale-windows-x64.exe.sha256", "browser_download_url": "https://example.invalid/codewhale-windows-x64.exe.sha256" },
            { "name": "codewhale-tui-linux-x64",      "browser_download_url": "https://example.invalid/codewhale-tui-linux-x64" },
            { "name": "codewhale-tui-macos-x64",      "browser_download_url": "https://example.invalid/codewhale-tui-macos-x64" },
            { "name": "codewhale-tui-macos-arm64",    "browser_download_url": "https://example.invalid/codewhale-tui-macos-arm64" },
            { "name": "codewhale-tui-windows-x64.exe","browser_download_url": "https://example.invalid/codewhale-tui-windows-x64.exe" }
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
        ];

        for (os, arch, expected) in cases {
            let stem = release_asset_stem_for(Path::new("/usr/local/bin/codewhale"), os, arch);
            let asset = select_platform_asset(&release, &stem)
                .unwrap_or_else(|| panic!("no asset for {os}/{arch} (stem {stem})"));
            assert_eq!(asset.name, expected, "{os}/{arch}");
        }
    }

    #[test]
    fn mocked_release_selects_tui_asset_when_tui_binary_invokes_update() {
        let release = mocked_release();
        let stem = release_asset_stem_for(
            Path::new("/usr/local/bin/codewhale-tui"),
            "macos",
            "aarch64",
        );
        let asset = select_platform_asset(&release, &stem).expect("TUI platform asset");
        assert_eq!(asset.name, "codewhale-tui-macos-arm64");
    }

    #[test]
    fn android_arm64_maps_to_android_release_assets() {
        // The generic format!("{prefix}-{os}-{arch}") path naturally produces
        // Android asset stems. Verify the full stem for both dispatcher and TUI
        // binaries so `codewhale update` on Termux requests Android assets, not
        // linux-arm64 (#4241).
        assert_eq!(
            release_asset_stem_for_prefix("codewhale", "android", "aarch64"),
            "codewhale-android-arm64"
        );
        assert_eq!(
            release_asset_stem_for_prefix("codewhale-tui", "android", "aarch64"),
            "codewhale-tui-android-arm64"
        );
        assert_eq!(
            release_asset_stem_for_prefix("codew", "android", "aarch64"),
            "codew-android-arm64"
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
        let tui = select_platform_asset(&release, "codewhale-tui-linux-x64").expect("tui asset");
        assert_eq!(
            tui.browser_download_url,
            "https://mirror.example/releases/v0.8.36/codewhale-tui-linux-x64"
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
        assert!(
            select_platform_asset(&release, "codewhale-tui-windows-x64")
                .is_some_and(|asset| asset.name == "codewhale-tui-windows-x64.exe")
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
        let tui = select_platform_asset(&release, "codewhale-tui-macos-arm64").expect("tui asset");
        assert_eq!(
            tui.browser_download_url,
            "https://github.com/Hmbown/CodeWhale/releases/download/v0.8.61/codewhale-tui-macos-arm64"
        );
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
            "https://cnb.cool/Hmbown/CodeWhale/-/releases/v0.8.47"
        );
        assert_eq!(
            codewhale_release::cnb_release_base_url("v0.8.47"),
            "https://cnb.cool/Hmbown/CodeWhale/-/releases/v0.8.47"
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
        assert!(hint.contains("codewhale-tui --locked"), "{hint}");
    }

    fn serve_http_responses(
        responses: Vec<(&'static str, &'static str, &'static [u8])>,
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
                stream.write_all(body).expect("write test response body");
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
