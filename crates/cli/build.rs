use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-env-changed=DEEPSEEK_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    declare_git_head_rerun();

    let package_version = env!("CARGO_PKG_VERSION");
    let build_version = build_sha()
        .map(|sha| format!("{package_version} ({sha})"))
        .unwrap_or_else(|| package_version.to_string());

    println!("cargo:rustc-env=DEEPSEEK_BUILD_VERSION={build_version}");
}

/// Tell Cargo to invalidate the cached build script output when `HEAD`
/// moves, so the embedded short-SHA stays in sync with the checkout. With
/// only `rerun-if-env-changed` lines, Cargo otherwise caches the script
/// across commits and the SHA goes stale.
fn declare_git_head_rerun() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.join("..").join("..");
    let git_meta = workspace_root.join(".git");
    if git_meta.is_dir() {
        println!("cargo:rerun-if-changed={}", git_meta.join("HEAD").display());
    } else if git_meta.is_file() {
        // Worktree checkout: `.git` is a pointer file with `gitdir: <path>`.
        println!("cargo:rerun-if-changed={}", git_meta.display());
        if let Ok(contents) = std::fs::read_to_string(&git_meta) {
            for line in contents.lines() {
                if let Some(rest) = line.strip_prefix("gitdir:") {
                    let trimmed = rest.trim();
                    let gitdir = if Path::new(trimmed).is_absolute() {
                        PathBuf::from(trimmed)
                    } else {
                        workspace_root.join(trimmed)
                    };
                    println!("cargo:rerun-if-changed={}", gitdir.join("HEAD").display());
                    break;
                }
            }
        }
    }
}

fn build_sha() -> Option<String> {
    env_sha("DEEPSEEK_BUILD_SHA")
        .or_else(|| env_sha("GITHUB_SHA"))
        .or_else(git_sha)
}

fn env_sha(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(short_sha)
}

fn git_sha() -> Option<String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let top_level_output = Command::new("git")
        .args(["-C"])
        .arg(&manifest_dir)
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
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    short_sha(String::from_utf8_lossy(&output.stdout).to_string())
}

fn short_sha(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(12).collect())
}
