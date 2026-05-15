//! Self-update for the `deepseek` binary.
//!
//! The `update` subcommand fetches the latest release from
//! `github.com/Hmbown/DeepSeek-TUI/releases/latest`, downloads the
//! platform-correct binary, verifies its SHA256 checksum, and atomically
//! replaces the currently running binary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use std::io::Write;

const CHECKSUM_MANIFEST_ASSET: &str = "deepseek-artifacts-sha256.txt";
const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/Hmbown/DeepSeek-TUI/releases/latest";
const CNB_REPO_URL: &str = "https://cnb.cool/deepseek-tui.com/DeepSeek-TUI";
const RELEASE_BASE_URL_ENV: &str = "DEEPSEEK_TUI_RELEASE_BASE_URL";
const LEGACY_RELEASE_BASE_URL_ENV: &str = "DEEPSEEK_RELEASE_BASE_URL";
const UPDATE_VERSION_ENV: &str = "DEEPSEEK_TUI_VERSION";
const LEGACY_UPDATE_VERSION_ENV: &str = "DEEPSEEK_VERSION";
const UPDATE_USER_AGENT: &str = "deepseek-tui-updater";

/// Run the self-update workflow.
pub fn run_update() -> Result<()> {
    let current_exe =
        std::env::current_exe().context("failed to determine current executable path")?;
    let targets = update_targets_for_exe(&current_exe);

    println!("Checking for updates...");
    println!("Current binary: {}", current_exe.display());

    // Step 1: Fetch latest release metadata
    let release = fetch_latest_release().with_context(update_network_fallback_hint)?;
    let latest_tag = &release.tag_name;
    println!("Latest release: {latest_tag}");

    // Step 2: Download the aggregated SHA256 checksum manifest if available
    let checksum_manifest = match select_checksum_manifest_asset(&release) {
        Some(checksum_asset) => {
            println!("Downloading {}...", checksum_asset.name);
            let checksum_bytes =
                download_url(&checksum_asset.browser_download_url).with_context(|| {
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
        let asset = select_platform_asset(&release, &target.asset_stem).with_context(|| {
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
        let bytes = download_url(&asset.browser_download_url).with_context(|| {
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

        downloads.push((target.path.clone(), asset.name.clone(), bytes));
    }

    if checksum_manifest.is_some() {
        println!("SHA256 checksum verified.");
    }

    // Step 4: Replace binaries atomically after all downloads verify.
    for (path, _, bytes) in downloads.iter().rev() {
        replace_binary(path, bytes)?;
    }

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

pub(crate) fn release_arch_for_rust_arch(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

pub(crate) fn binary_prefix_for_exe(current_exe: &Path) -> &'static str {
    let exe_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("deepseek");
    if exe_name.contains("deepseek-tui") {
        "deepseek-tui"
    } else {
        "deepseek"
    }
}

fn sibling_prefix_for(prefix: &str) -> &'static str {
    if prefix == "deepseek-tui" {
        "deepseek"
    } else {
        "deepseek-tui"
    }
}

fn sibling_binary_path(current_exe: &Path, sibling_prefix: &str) -> PathBuf {
    current_exe.with_file_name(format!("{sibling_prefix}{}", std::env::consts::EXE_SUFFIX))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateTarget {
    path: PathBuf,
    asset_stem: String,
}

fn update_targets_for_exe(current_exe: &Path) -> Vec<UpdateTarget> {
    let current_prefix = binary_prefix_for_exe(current_exe);
    let mut targets = vec![UpdateTarget {
        path: current_exe.to_path_buf(),
        asset_stem: release_asset_stem_for_prefix(
            current_prefix,
            std::env::consts::OS,
            std::env::consts::ARCH,
        ),
    }];

    let sibling_prefix = sibling_prefix_for(current_prefix);
    let sibling = sibling_binary_path(current_exe, sibling_prefix);
    if sibling.exists() {
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

fn select_platform_asset<'a>(release: &'a Release, binary_name: &str) -> Option<&'a Asset> {
    release
        .assets
        .iter()
        .find(|asset| asset_matches_platform(&asset.name, binary_name))
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
#[derive(serde::Deserialize, Debug)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

/// A single release asset.
#[derive(serde::Deserialize, Debug)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn update_http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(UPDATE_USER_AGENT)
        .build()
        .context("failed to build update HTTP client")
}

/// Fetch the latest release metadata from GitHub.
fn fetch_latest_release() -> Result<Release> {
    if let Some(base_url) = release_base_url_from_env() {
        let version = update_version_from_env().unwrap_or_else(|| env!("CARGO_PKG_VERSION").into());
        return Ok(release_from_mirror_base_url(
            &base_url,
            &version,
            std::env::consts::OS,
            std::env::consts::ARCH,
        ));
    }
    fetch_latest_release_from_url(LATEST_RELEASE_URL)
}

fn release_base_url_from_env() -> Option<String> {
    std::env::var(RELEASE_BASE_URL_ENV)
        .ok()
        .or_else(|| std::env::var(LEGACY_RELEASE_BASE_URL_ENV).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn update_version_from_env() -> Option<String> {
    std::env::var(UPDATE_VERSION_ENV)
        .ok()
        .or_else(|| std::env::var(LEGACY_UPDATE_VERSION_ENV).ok())
        .map(|value| value.trim().trim_start_matches('v').to_string())
        .filter(|value| !value.is_empty())
}

fn release_from_mirror_base_url(
    base_url: &str,
    version: &str,
    os: &str,
    rust_arch: &str,
) -> Release {
    let tag_name = format!("v{}", version.trim_start_matches('v'));
    let mut assets = vec![Asset {
        name: CHECKSUM_MANIFEST_ASSET.to_string(),
        browser_download_url: mirror_asset_url(base_url, CHECKSUM_MANIFEST_ASSET),
    }];

    for prefix in ["deepseek", "deepseek-tui"] {
        let name = release_asset_name_for_prefix(prefix, os, rust_arch);
        assets.push(Asset {
            browser_download_url: mirror_asset_url(base_url, &name),
            name,
        });
    }

    Release { tag_name, assets }
}

fn mirror_asset_url(base_url: &str, asset_name: &str) -> String {
    format!("{}/{}", base_url.trim_end_matches('/'), asset_name)
}

fn update_network_fallback_hint() -> String {
    format!(
        "GitHub release downloads may be blocked or slow on this network.\n\
         For mainland China, use one of these fallback paths:\n\
           1. Source build from the CNB mirror, installing both shipped binaries:\n\
              cargo install --git {CNB_REPO_URL} --tag vX.Y.Z deepseek-tui-cli --locked --force\n\
              cargo install --git {CNB_REPO_URL} --tag vX.Y.Z deepseek-tui --locked --force\n\
           2. Use a binary asset mirror:\n\
              {RELEASE_BASE_URL_ENV}=https://<mirror>/<release-assets>/ {UPDATE_VERSION_ENV}=X.Y.Z deepseek update\n\
         The mirror directory must contain {CHECKSUM_MANIFEST_ASSET} and the platform binaries."
    )
}

fn fetch_latest_release_from_url(url: &str) -> Result<Release> {
    let client = update_http_client()?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .with_context(|| format!("failed to fetch release info from {url}"))?;
    let status = response.status();
    let body = response
        .text()
        .with_context(|| format!("failed to read release response from {url}"))?;

    if !status.is_success() {
        bail!("GitHub release request failed with HTTP {status}: {body}");
    }

    let release: Release = serde_json::from_str(&body).with_context(|| {
        format!("failed to parse release JSON from GitHub API. Response: {body}")
    })?;

    Ok(release)
}

/// Download a URL to bytes.
fn download_url(url: &str) -> Result<Vec<u8>> {
    let client = update_http_client()?;
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("failed to download {url}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .with_context(|| format!("failed to read response body from {url}"))?;

    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        bail!("download failed with HTTP {status}: {body}");
    }

    Ok(bytes.to_vec())
}

/// Compute the SHA256 hex digest of data.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(data);
    format!("{hash:x}")
}

/// Replace the running binary.
///
/// Writes the new binary to a secure temp file in the target directory, then
/// installs it in place. Unix can atomically replace the executable path. On
/// Windows, replacing a running executable can fail, so rename the current file
/// out of the way before moving the new binary into the original path.
fn replace_binary(target: &Path, new_bytes: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut tmp = tempfile::Builder::new()
        .prefix(".deepseek-update-")
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

    /// Verify binary prefix detection for dispatcher vs TUI binary.
    #[test]
    fn test_binary_prefix_detection() {
        // TUI binary should use deepseek-tui prefix
        assert_eq!(
            binary_prefix_for_exe(Path::new("deepseek-tui")),
            "deepseek-tui"
        );
        assert_eq!(
            binary_prefix_for_exe(Path::new("deepseek-tui.exe")),
            "deepseek-tui"
        );
        assert_eq!(
            binary_prefix_for_exe(Path::new("/usr/local/bin/deepseek-tui")),
            "deepseek-tui"
        );

        // Dispatcher binary should use deepseek prefix
        assert_eq!(binary_prefix_for_exe(Path::new("deepseek")), "deepseek");
        assert_eq!(binary_prefix_for_exe(Path::new("deepseek.exe")), "deepseek");
        assert_eq!(
            binary_prefix_for_exe(Path::new("/usr/local/bin/deepseek")),
            "deepseek"
        );

        // Fallback for unknown names
        assert_eq!(binary_prefix_for_exe(Path::new("other-binary")), "deepseek");
    }

    #[test]
    fn test_release_asset_stem_for_supported_platforms() {
        let cases = [
            ("deepseek", "macos", "aarch64", "deepseek-macos-arm64"),
            ("deepseek", "macos", "x86_64", "deepseek-macos-x64"),
            ("deepseek", "linux", "x86_64", "deepseek-linux-x64"),
            ("deepseek", "windows", "x86_64", "deepseek-windows-x64"),
            (
                "deepseek-tui",
                "macos",
                "aarch64",
                "deepseek-tui-macos-arm64",
            ),
            ("deepseek-tui", "linux", "x86_64", "deepseek-tui-linux-x64"),
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
            .join(format!("deepseek{}", std::env::consts::EXE_SUFFIX));
        let tui = dir
            .path()
            .join(format!("deepseek-tui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"dispatcher").unwrap();
        std::fs::write(&tui, b"tui").unwrap();

        let targets = update_targets_for_exe(&dispatcher);
        let paths = targets
            .iter()
            .map(|target| target.path.as_path())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![dispatcher.as_path(), tui.as_path()]);
        assert!(targets[0].asset_stem.starts_with("deepseek-"));
        assert!(targets[1].asset_stem.starts_with("deepseek-tui-"));
    }

    #[test]
    fn update_targets_skip_missing_sibling() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = dir
            .path()
            .join(format!("deepseek{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&dispatcher, b"dispatcher").unwrap();

        let targets = update_targets_for_exe(&dispatcher);

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].path, dispatcher);
        assert!(targets[0].asset_stem.starts_with("deepseek-"));
    }

    #[test]
    fn test_asset_matching_accepts_binary_assets_and_rejects_checksums() {
        assert!(asset_matches_platform(
            "deepseek-macos-arm64",
            "deepseek-macos-arm64"
        ));
        assert!(asset_matches_platform(
            "deepseek-macos-arm64.tar.gz",
            "deepseek-macos-arm64"
        ));
        assert!(asset_matches_platform(
            "deepseek-tui-windows-x64.exe",
            "deepseek-tui-windows-x64"
        ));
        assert!(!asset_matches_platform(
            "deepseek-tui-windows-x64.exe.sha256",
            "deepseek-tui-windows-x64"
        ));
        assert!(!asset_matches_platform(
            "deepseek-macos-aarch64.tar.gz",
            "deepseek-macos-arm64"
        ));
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
    fn parse_checksum_manifest_accepts_sha256sum_format() {
        let manifest = "\
2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  deepseek-macos-arm64
E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855  *deepseek-windows-x64.exe
";
        let checksums = parse_checksum_manifest(manifest).expect("valid manifest");

        assert_eq!(
            checksums.get("deepseek-macos-arm64").map(String::as_str),
            Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
        assert_eq!(
            checksums
                .get("deepseek-windows-x64.exe")
                .map(String::as_str),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    #[test]
    fn parse_checksum_manifest_rejects_malformed_lines() {
        let err = parse_checksum_manifest("not-a-hash  deepseek-macos-arm64")
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
        let err = expected_sha256_from_manifest(manifest, "deepseek-macos-arm64")
            .expect_err("missing asset should fail");
        assert!(
            err.to_string()
                .contains("checksum manifest is missing deepseek-macos-arm64"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn test_replace_binary_creates_and_replaces() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("deepseek-test");
        // Write initial content
        std::fs::write(&target, b"old binary").unwrap();

        replace_binary(&target, b"new binary content").unwrap();
        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(content, "new binary content");
    }

    #[test]
    fn test_replace_binary_creates_new_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("deepseek-new-test");

        replace_binary(&target, b"fresh binary").unwrap();
        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(content, "fresh binary");
    }

    /// Mocked GitHub release payload covering both the dispatcher (`deepseek`)
    /// and the legacy TUI (`deepseek-tui`) binaries across our published
    /// platform/arch matrix, plus a checksum sibling that must never be picked
    /// as the primary binary.
    fn mocked_release() -> Release {
        let json = r#"{
          "tag_name": "v0.8.8",
          "assets": [
            { "name": "deepseek-linux-x64",          "browser_download_url": "https://example.invalid/deepseek-linux-x64" },
            { "name": "deepseek-macos-x64",          "browser_download_url": "https://example.invalid/deepseek-macos-x64" },
            { "name": "deepseek-macos-arm64",        "browser_download_url": "https://example.invalid/deepseek-macos-arm64" },
            { "name": "deepseek-windows-x64.exe",    "browser_download_url": "https://example.invalid/deepseek-windows-x64.exe" },
            { "name": "deepseek-windows-x64.exe.sha256", "browser_download_url": "https://example.invalid/deepseek-windows-x64.exe.sha256" },
            { "name": "deepseek-tui-linux-x64",      "browser_download_url": "https://example.invalid/deepseek-tui-linux-x64" },
            { "name": "deepseek-tui-macos-x64",      "browser_download_url": "https://example.invalid/deepseek-tui-macos-x64" },
            { "name": "deepseek-tui-macos-arm64",    "browser_download_url": "https://example.invalid/deepseek-tui-macos-arm64" },
            { "name": "deepseek-tui-windows-x64.exe","browser_download_url": "https://example.invalid/deepseek-tui-windows-x64.exe" }
          ]
        }"#;
        serde_json::from_str(json).expect("mock release JSON")
    }

    #[test]
    fn mocked_release_selects_dispatcher_asset_for_supported_platforms() {
        let release = mocked_release();
        let cases = [
            ("macos", "aarch64", "deepseek-macos-arm64"),
            ("macos", "x86_64", "deepseek-macos-x64"),
            ("linux", "x86_64", "deepseek-linux-x64"),
            ("windows", "x86_64", "deepseek-windows-x64.exe"),
        ];

        for (os, arch, expected) in cases {
            let stem = release_asset_stem_for(Path::new("/usr/local/bin/deepseek"), os, arch);
            let asset = select_platform_asset(&release, &stem)
                .unwrap_or_else(|| panic!("no asset for {os}/{arch} (stem {stem})"));
            assert_eq!(asset.name, expected, "{os}/{arch}");
        }
    }

    #[test]
    fn mocked_release_selects_tui_asset_when_tui_binary_invokes_update() {
        let release = mocked_release();
        let stem =
            release_asset_stem_for(Path::new("/usr/local/bin/deepseek-tui"), "macos", "aarch64");
        let asset = select_platform_asset(&release, &stem).expect("TUI platform asset");
        assert_eq!(asset.name, "deepseek-tui-macos-arm64");
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
            "https://mirror.example/releases/v0.8.36/deepseek-artifacts-sha256.txt"
        );

        let dispatcher =
            select_platform_asset(&release, "deepseek-linux-x64").expect("dispatcher asset");
        assert_eq!(
            dispatcher.browser_download_url,
            "https://mirror.example/releases/v0.8.36/deepseek-linux-x64"
        );
        let tui = select_platform_asset(&release, "deepseek-tui-linux-x64").expect("tui asset");
        assert_eq!(
            tui.browser_download_url,
            "https://mirror.example/releases/v0.8.36/deepseek-tui-linux-x64"
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
            select_platform_asset(&release, "deepseek-windows-x64")
                .is_some_and(|asset| asset.name == "deepseek-windows-x64.exe")
        );
        assert!(
            select_platform_asset(&release, "deepseek-tui-windows-x64")
                .is_some_and(|asset| asset.name == "deepseek-tui-windows-x64.exe")
        );
    }

    #[test]
    fn update_fallback_hint_points_china_users_to_cnb_and_asset_mirrors() {
        let hint = update_network_fallback_hint();

        assert!(hint.contains(CNB_REPO_URL), "{hint}");
        assert!(hint.contains(RELEASE_BASE_URL_ENV), "{hint}");
        assert!(hint.contains(UPDATE_VERSION_ENV), "{hint}");
        assert!(hint.contains("deepseek-tui-cli"), "{hint}");
        assert!(hint.contains("deepseek-tui --locked"), "{hint}");
    }

    fn serve_http_once(
        status: &'static str,
        content_type: &'static str,
        body: &'static [u8],
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("test server addr");
        let (request_tx, request_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
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
        });

        (format!("http://{addr}/release"), request_rx, handle)
    }

    #[test]
    fn fetch_latest_release_from_url_reads_mocked_release_json() {
        let body = br#"{
          "tag_name": "v9.9.9",
          "assets": [
            { "name": "deepseek-linux-x64", "browser_download_url": "http://example.invalid/deepseek-linux-x64" },
            { "name": "deepseek-artifacts-sha256.txt", "browser_download_url": "http://example.invalid/deepseek-artifacts-sha256.txt" }
          ]
        }"#;
        let (url, request_rx, handle) = serve_http_once("200 OK", "application/json", body);
        let release = fetch_latest_release_from_url(&url).expect("release JSON should parse");

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
            request_lower.contains("user-agent: deepseek-tui-updater"),
            "got {request:?}"
        );
        handle.join().expect("test server thread");
    }

    #[test]
    fn fetch_latest_release_from_url_reports_http_errors() {
        let (url, _request_rx, handle) =
            serve_http_once("500 Internal Server Error", "text/plain", b"server broke");
        let err = fetch_latest_release_from_url(&url).expect_err("HTTP 500 should fail");

        assert!(
            err.to_string().contains("HTTP 500"),
            "unexpected error: {err:#}"
        );
        handle.join().expect("test server thread");
    }

    #[test]
    fn download_url_reads_binary_body_with_updater_user_agent() {
        let (url, request_rx, handle) =
            serve_http_once("200 OK", "application/octet-stream", b"\0binary bytes");
        let bytes = download_url(&url).expect("binary download should succeed");

        assert_eq!(bytes, b"\0binary bytes");

        let request = request_rx.recv().expect("captured request");
        let request_lower = request.to_ascii_lowercase();
        assert!(request.starts_with("GET /release "), "got {request:?}");
        assert!(
            request_lower.contains("user-agent: deepseek-tui-updater"),
            "got {request:?}"
        );
        handle.join().expect("test server thread");
    }
}
