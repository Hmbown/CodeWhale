use std::{path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=DEEPSEEK_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");

    let package_version = env!("CARGO_PKG_VERSION");
    let build_version = build_sha()
        .map(|sha| format!("{package_version} ({sha})"))
        .unwrap_or_else(|| package_version.to_string());

    println!("cargo:rustc-env=DEEPSEEK_BUILD_VERSION={build_version}");
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
