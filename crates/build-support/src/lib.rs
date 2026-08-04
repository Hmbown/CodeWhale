//! Shared build-script helpers for the `codewhale-cli`, `codewhale-tui`, and
//! `codewhale-telemetry` build scripts: rerun-condition declarations, the
//! embedded `DEEPSEEK_BUILD_VERSION` metadata, and the release-only build sha.
//! Only call these functions from a build script — they emit `cargo:`
//! directives on stdout.
//!
//! Two different shas live here and they are not interchangeable.
//! `DEEPSEEK_BUILD_VERSION`/`CODEWHALE_BUILD_COMMIT` describe *this checkout*
//! and fall back to local `git rev-parse`, which is right for a `--version`
//! string a developer reads about their own build.
//! `CODEWHALE_RELEASE_BUILD_SHA` describes a *published* binary and has no
//! fallback at all, because it leaves the machine.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Declare the rerun conditions for the build-metadata directives: the
/// SHA-override environment variables plus the git files that track `HEAD`.
///
/// `manifest_dir` is the calling build script's `CARGO_MANIFEST_DIR`.
pub fn declare_rerun_conditions(manifest_dir: &Path) {
    println!("cargo:rerun-if-env-changed=DEEPSEEK_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    declare_git_head_rerun(manifest_dir);
}

/// Emit `cargo:rustc-env=DEEPSEEK_BUILD_VERSION=...` — the package version,
/// suffixed with the short build SHA when one can be determined.
///
/// `manifest_dir` and `package_version` are the calling build script's
/// `CARGO_MANIFEST_DIR` and `CARGO_PKG_VERSION`.
pub fn emit_build_version(manifest_dir: &Path, package_version: &str) {
    let commit = build_commit(manifest_dir);
    let build_version = commit
        .as_ref()
        .and_then(|sha| short_sha(sha.clone()))
        .map(|sha| format!("{package_version} ({sha})"))
        .unwrap_or_else(|| package_version.to_string());

    println!("cargo:rustc-env=DEEPSEEK_BUILD_VERSION={build_version}");
    if let Some(commit) = commit {
        println!("cargo:rustc-env=CODEWHALE_BUILD_COMMIT={commit}");
    }
}

/// Declare the rerun conditions for [`emit_release_build_sha`] alone: the two
/// release-CI SHA variables, and nothing about the local checkout.
///
/// Deliberately not [`declare_rerun_conditions`]: watching `.git/HEAD` would
/// make the build script rerun on every local commit, for a value that is
/// `None` on every local build by design.
pub fn declare_release_sha_rerun() {
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
/// - [`build_commit`] falls back to `git_commit`, so `CODEWHALE_BUILD_COMMIT`
///   is the builder's own private `HEAD` on every local build. A maintainer
///   running the release build this repo's own guidance prescribes would ship
///   the sha of an unpublished tree.
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
/// `DEEPSEEK_BUILD_SHA` wins over `GITHUB_SHA`; both must be a full 40-hex sha
/// to be believed, and the result is the first 12 characters.
#[must_use]
pub fn release_build_sha(read_env: impl Fn(&str) -> Option<String>) -> Option<String> {
    read_env("DEEPSEEK_BUILD_SHA")
        .and_then(full_sha)
        .or_else(|| read_env("GITHUB_SHA").and_then(full_sha))
        .and_then(short_sha)
}

/// Tell Cargo to invalidate the cached build script output when `HEAD`
/// moves, so the embedded short-SHA stays in sync with the checkout.
///
/// `.git/HEAD` only changes on branch switches and detached-HEAD moves —
/// `git commit` on the current branch updates the underlying ref file
/// (loose `refs/heads/<name>`, or `packed-refs` after `git pack-refs`)
/// without touching `HEAD` itself. So when `HEAD` is a symbolic ref we
/// also watch the resolved target and `packed-refs`. Linked worktrees keep
/// `HEAD` in a private gitdir but store branch refs in the shared common gitdir,
/// so the symbolic target must be watched from that common directory. A
/// non-existent `rerun-if-changed` path is treated as "always changed" by
/// Cargo, which covers the loose→packed transition.
fn declare_git_head_rerun(manifest_dir: &Path) {
    let workspace_root = manifest_dir.join("..").join("..");
    let git_meta = workspace_root.join(".git");

    let gitdir = if git_meta.is_dir() {
        git_meta
    } else if git_meta.is_file() {
        // Worktree pointer file: watch it directly, then follow `gitdir:`.
        println!("cargo:rerun-if-changed={}", git_meta.display());
        let Ok(contents) = std::fs::read_to_string(&git_meta) else {
            return;
        };
        let Some(rest) = contents.lines().find_map(|l| l.strip_prefix("gitdir:")) else {
            return;
        };
        let trimmed = rest.trim();
        if Path::new(trimmed).is_absolute() {
            PathBuf::from(trimmed)
        } else {
            workspace_root.join(trimmed)
        }
    } else {
        return;
    };

    let head = gitdir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());

    if let Ok(contents) = std::fs::read_to_string(&head)
        && let Some(target) = parse_symbolic_ref(&contents)
    {
        let common_gitdir = git_common_dir(&gitdir);
        println!(
            "cargo:rerun-if-changed={}",
            common_gitdir.join(target).display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            common_gitdir.join("packed-refs").display()
        );
    }
}

/// Resolve the shared ref store for a normal repository or a linked worktree.
/// Git writes `commondir` in a linked worktree's private gitdir; its value is
/// relative to that directory unless Git supplied an absolute path.
fn git_common_dir(gitdir: &Path) -> PathBuf {
    let commondir = gitdir.join("commondir");
    let Ok(contents) = std::fs::read_to_string(commondir) else {
        return gitdir.to_path_buf();
    };
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return gitdir.to_path_buf();
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        gitdir.join(path)
    }
}

/// If `.git/HEAD` is a symbolic ref (`ref: refs/heads/...`) return the
/// target ref path. Returns `None` for a detached HEAD (raw SHA).
fn parse_symbolic_ref(head_contents: &str) -> Option<&str> {
    head_contents
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("ref:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn build_commit(manifest_dir: &Path) -> Option<String> {
    env_commit("DEEPSEEK_BUILD_SHA")
        .or_else(|| env_commit("GITHUB_SHA"))
        .or_else(|| git_commit(manifest_dir))
}

fn env_commit(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(full_sha)
}

fn git_commit(manifest_dir: &Path) -> Option<String> {
    let top_level_output = Command::new("git")
        .args(["-C"])
        .arg(manifest_dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !top_level_output.status.success() {
        return None;
    }
    let top_level = PathBuf::from(String::from_utf8_lossy(&top_level_output.stdout).trim());
    if !top_level.join("Cargo.toml").is_file() || !top_level.join("crates/tui").is_dir() {
        return None;
    }

    let output = Command::new("git")
        .args(["-C"])
        .arg(top_level)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    full_sha(String::from_utf8_lossy(&output.stdout).to_string())
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
    use super::{full_sha, git_common_dir, parse_symbolic_ref, release_build_sha, short_sha};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn symbolic_ref_strips_prefix_and_whitespace() {
        assert_eq!(
            parse_symbolic_ref("ref: refs/heads/main\n"),
            Some("refs/heads/main")
        );
    }

    #[test]
    fn symbolic_ref_handles_no_trailing_newline() {
        assert_eq!(
            parse_symbolic_ref("ref: refs/heads/work/v0.8.26-security"),
            Some("refs/heads/work/v0.8.26-security")
        );
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
        // The Codewhale variable wins over the GitHub one.
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

    #[test]
    fn detached_head_is_not_a_symbolic_ref() {
        assert_eq!(
            parse_symbolic_ref("506343f44e48b9c2c8d6b2d3e8e8e8e8e8e8e8e8\n"),
            None
        );
    }

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(parse_symbolic_ref(""), None);
        assert_eq!(parse_symbolic_ref("ref: \n"), None);
    }

    #[test]
    fn linked_worktree_uses_the_common_ref_store() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codewhale-build-support-{}-{unique}",
            std::process::id()
        ));
        let common = root.join(".git");
        let worktree_gitdir = common.join("worktrees/candidate");
        fs::create_dir_all(&worktree_gitdir).expect("create worktree gitdir");
        fs::write(worktree_gitdir.join("commondir"), "../..\n").expect("write commondir");

        assert_eq!(
            fs::canonicalize(git_common_dir(&worktree_gitdir)).expect("canonical common gitdir"),
            fs::canonicalize(&common).expect("canonical expected gitdir")
        );

        fs::remove_dir_all(root).expect("remove isolated test directory");
    }
}
