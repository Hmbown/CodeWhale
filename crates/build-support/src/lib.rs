//! Shared build-script helpers for the `codewhale-cli`, `codewhale-tui`, and
//! `codewhale-telemetry` build scripts: rerun-condition declarations, the
//! embedded `CODEWHALE_BUILD_VERSION` metadata, and the release-only build sha.
//! Only call these functions from a build script — they emit `cargo:`
//! directives on stdout.
//!
//! Two different shas live here and they are not interchangeable.
//! `CODEWHALE_BUILD_VERSION`/`CODEWHALE_BUILD_COMMIT` describe *the build the
//! environment asked for* (`CODEWHALE_BUILD_SHA`/`DEEPSEEK_BUILD_SHA`/`GITHUB_SHA`); an unstamped
//! local checkout renders `(dev)`; an unstamped Cargo source package displays
//! its package version without claiming a release-binary SHA.
//! `CODEWHALE_RELEASE_BUILD_SHA` describes a *published* binary and has no
//! fallback at all, because it leaves the machine.
//!
//! ## Why the stamp never reads the local checkout (#5245)
//!
//! These helpers used to watch `.git/HEAD`/refs and fall back to
//! `git rev-parse HEAD`, so every local commit invalidated the two largest
//! compile units in the workspace (a ~14-minute release rebuild with zero
//! code changes). And the alternative — resolving the sha at *runtime* —
//! would lie: the binary runs inside users' repositories, and a stale binary
//! would report whatever the checkout's HEAD is *now*, which breaks the
//! dogfood-receipt identity `scripts/release/install-dogfood.sh` verifies.
//! So the contract is: a sha appears in the version string only when the
//! build environment supplied one (`CODEWHALE_BUILD_SHA` wins over
//! `GITHUB_SHA`), the build script reruns only when those variables change,
//! and an unstamped checkout says `(dev)`. Cargo source packages use the plain
//! package version. CI and release builds are
//! byte-identical to the old behavior; dogfood builds pass the sha
//! explicitly (the install script prints the exact command).

use std::path::Path;

/// Declare the rerun conditions for the build-metadata directives: the two
/// SHA-override environment variables, and deliberately nothing about the
/// local checkout — watching `.git` files is what made every local commit
/// rebuild the whole crate (#5245).
///
/// `manifest_dir` is accepted (and ignored) so build scripts keep one call
/// shape; it documents that the decision is per-crate, not global state.
pub fn declare_rerun_conditions(_manifest_dir: &Path) {
    println!("cargo:rerun-if-env-changed=CODEWHALE_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=DEEPSEEK_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
}

/// Emit `cargo:rustc-env=CODEWHALE_BUILD_VERSION=...` — the package version,
/// suffixed with the short build SHA when the environment supplied one
/// (`CODEWHALE_BUILD_SHA`, then `DEEPSEEK_BUILD_SHA`, then `GITHUB_SHA`).
/// Unstamped Cargo source packages show the plain package version; unpackaged
/// checkouts retain `(dev)`. `CODEWHALE_BUILD_COMMIT` is emitted only when stamped.
///
/// `package_version` is the calling build script's `CARGO_PKG_VERSION`;
/// Cargo writes `Cargo.toml.orig` when normalizing a distributable package.
/// Its presence classifies the source layout, not release provenance: no VCS
/// metadata is read and no additional commit value is emitted.
pub fn emit_build_version(manifest_dir: &Path, package_version: &str) {
    let commit = build_commit();
    let build_version = format_build_version(
        package_version,
        commit.as_deref(),
        manifest_dir.join("Cargo.toml.orig").is_file(),
    );

    println!("cargo:rustc-env=CODEWHALE_BUILD_VERSION={build_version}");
    // Keep the pre-rebrand compile-time name through the 0.9.x compatibility
    // window for downstream crates that still use `env!` with it.
    println!("cargo:rustc-env=DEEPSEEK_BUILD_VERSION={build_version}");
    if let Some(commit) = commit {
        println!("cargo:rustc-env=CODEWHALE_BUILD_COMMIT={commit}");
    }
}

fn format_build_version(
    package_version: &str,
    commit: Option<&str>,
    packaged_source: bool,
) -> String {
    match commit {
        Some(sha) => format!("{package_version} ({})", &sha[..12]),
        None if packaged_source => package_version.to_string(),
        None => format!("{package_version} (dev)"),
    }
}

/// Declare the rerun conditions for [`emit_release_build_sha`] alone: the two
/// release-CI SHA variables, and nothing about the local checkout.
///
/// Deliberately not [`declare_rerun_conditions`]: watching `.git/HEAD` would
/// make the build script rerun on every local commit, for a value that is
/// `None` on every local build by design.
pub fn declare_release_sha_rerun() {
    println!("cargo:rerun-if-env-changed=CODEWHALE_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=DEEPSEEK_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
}

/// Emit `cargo:rustc-env=CODEWHALE_RELEASE_BUILD_SHA=...` — the first 12 hex
/// characters of the build sha — **only** when the build environment supplied
/// one.
///
/// This is provenance for a *published* binary, and it is the only sha a
/// telemetry payload may carry. There is deliberately no fallback to the local
/// checkout:
///
/// - `CODEWHALE_BUILD_COMMIT` historically fell back to the builder's own
///   private `HEAD` on every local build; since #5245 it is env-only too,
///   but this value keeps its own name and rule because it is the only sha
///   a telemetry payload may carry.
/// - The "was this a published release" gate proposed earlier cannot be built:
///   `codewhale_release::latest_release_tag_{async,blocking}` are **network
///   calls** to `api.github.com` that return *tag names*, not shas, so the only
///   available comparison is version-vs-version — and a private tree at the
///   same version compares equal.
///
/// Build-time provenance is deterministic, network-free, and verifiable from
/// the repository. Absent the release environment the value is simply absent,
/// and `option_env!` in the consuming crate yields `None`.
pub fn emit_release_build_sha() {
    if let Some(sha) = release_build_sha(|name| std::env::var(name).ok()) {
        println!("cargo:rustc-env=CODEWHALE_RELEASE_BUILD_SHA={sha}");
    }
}

/// The decision behind [`emit_release_build_sha`], with the environment
/// injected so it can be tested without mutating the process.
///
/// `CODEWHALE_BUILD_SHA` wins over the legacy `DEEPSEEK_BUILD_SHA`, which wins over
/// `GITHUB_SHA`; each must be a full 40-hex sha
/// to be believed, and the result is the first 12 characters.
#[must_use]
pub fn release_build_sha(read_env: impl Fn(&str) -> Option<String>) -> Option<String> {
    read_env("CODEWHALE_BUILD_SHA")
        .and_then(full_sha)
        .or_else(|| read_env("DEEPSEEK_BUILD_SHA").and_then(full_sha))
        .or_else(|| read_env("GITHUB_SHA").and_then(full_sha))
        .and_then(short_sha)
}

fn build_commit() -> Option<String> {
    build_commit_with(|name| std::env::var(name).ok())
}

/// The stamping decision with the environment injected, so the no-local-
/// fallback contract is testable without mutating the process (#5245).
fn build_commit_with(read_env: impl Fn(&str) -> Option<String>) -> Option<String> {
    read_env("CODEWHALE_BUILD_SHA")
        .and_then(full_sha)
        .or_else(|| read_env("DEEPSEEK_BUILD_SHA").and_then(full_sha))
        .or_else(|| read_env("GITHUB_SHA").and_then(full_sha))
}

fn full_sha(value: String) -> Option<String> {
    let trimmed = value.trim().to_ascii_lowercase();
    if trimmed.len() != 40 || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(trimmed)
}

fn short_sha(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(12).collect())
}

#[cfg(test)]
mod tests {
    use super::{full_sha, release_build_sha, short_sha};

    #[test]
    fn packaged_sources_do_not_claim_to_be_unreleased_or_stamped() {
        assert_eq!(super::format_build_version("0.9.13", None, true), "0.9.13");
        assert_eq!(
            super::format_build_version("0.9.13", None, false),
            "0.9.13 (dev)"
        );
        let sha = "abcdef0123456789abcdef0123456789abcdef01";
        for packaged in [true, false] {
            assert_eq!(
                super::format_build_version("0.9.13", Some(sha), packaged),
                "0.9.13 (abcdef012345)"
            );
        }
        // Source packaging must not create a telemetry/release provenance SHA.
        assert_eq!(release_build_sha(|_| None), None);
    }

    #[test]
    fn full_commit_requires_exact_forty_hex_characters() {
        assert_eq!(
            full_sha("ABCDEF0123456789ABCDEF0123456789ABCDEF01".to_string()),
            Some("abcdef0123456789abcdef0123456789abcdef01".to_string())
        );
        assert_eq!(full_sha("abc123".to_string()), None);
        assert_eq!(
            full_sha("gggggggggggggggggggggggggggggggggggggggg".to_string()),
            None
        );
        assert_eq!(
            short_sha("abcdef0123456789abcdef0123456789abcdef01".to_string()),
            Some("abcdef012345".to_string())
        );
    }

    #[test]
    fn the_release_build_sha_is_absent_for_every_local_build() {
        // No release environment: nothing is emitted, so `option_env!` in the
        // consuming crate is `None` and a telemetry payload carries `git_sha:
        // null`. This is the property that keeps a maintainer's private HEAD
        // out of a shipped binary.
        assert_eq!(release_build_sha(|_| None), None);
    }

    #[test]
    fn the_release_build_sha_comes_only_from_a_release_environment() {
        let ci = "abcdef0123456789abcdef0123456789abcdef01";
        assert_eq!(
            release_build_sha(|name| (name == "GITHUB_SHA").then(|| ci.to_string())),
            Some("abcdef012345".to_string())
        );
        // The canonical Codewhale variable wins over the legacy
        // DeepSeek-era one, which wins over the GitHub one.
        assert_eq!(
            release_build_sha(|name| match name {
                "CODEWHALE_BUILD_SHA" => Some("e".repeat(40)),
                "DEEPSEEK_BUILD_SHA" => Some("f".repeat(40)),
                "GITHUB_SHA" => Some(ci.to_string()),
                _ => None,
            }),
            Some("e".repeat(12))
        );
        // The legacy name still stamps during the 0.9.x compatibility
        // window, so existing release tooling keeps working.
        assert_eq!(
            release_build_sha(|name| match name {
                "DEEPSEEK_BUILD_SHA" => Some("f".repeat(40)),
                "GITHUB_SHA" => Some(ci.to_string()),
                _ => None,
            }),
            Some("f".repeat(12))
        );
        // A value that is not a full sha is not believed, and does not fall
        // through to the local checkout.
        assert_eq!(
            release_build_sha(|name| (name == "DEEPSEEK_BUILD_SHA").then(|| "abc123".to_string())),
            None
        );
        // `CODEWHALE_BUILD_COMMIT` is a different value with a different rule
        // and is never a source here.
        assert_eq!(
            release_build_sha(|name| (name == "CODEWHALE_BUILD_COMMIT").then(|| ci.to_string())),
            None
        );
    }

    /// #5245 contract: the version stamp reads ONLY the two environment
    /// variables. There is no fallback to the local checkout, so a plain
    /// local build renders `(dev)` and — the actual point — the build script
    /// declares no `.git` rerun paths, meaning `git commit` no longer
    /// invalidates the two largest compile units in the workspace.
    #[test]
    fn the_build_commit_never_reads_the_local_checkout() {
        // This test runs inside the real repository; if a git fallback still
        // existed it would resolve a sha here. Absent env vars must mean
        // absent commit, in the repo or out of it.
        assert_eq!(super::build_commit_with(|_| None), None);
        let ci = "abcdef0123456789abcdef0123456789abcdef01";
        assert_eq!(
            super::build_commit_with(|name| (name == "GITHUB_SHA").then(|| ci.to_string())),
            Some(ci.to_string())
        );
        assert_eq!(
            super::build_commit_with(|name| match name {
                "DEEPSEEK_BUILD_SHA" => Some("f".repeat(40)),
                "GITHUB_SHA" => Some(ci.to_string()),
                _ => None,
            }),
            Some("f".repeat(40))
        );
        assert_eq!(
            super::build_commit_with(|name| match name {
                "CODEWHALE_BUILD_SHA" => Some("e".repeat(40)),
                "DEEPSEEK_BUILD_SHA" => Some("f".repeat(40)),
                _ => None,
            }),
            Some("e".repeat(40))
        );
    }
}
