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
/// moves, so the embedded short-SHA stays in sync with the checkout.
///
/// `.git/HEAD` only changes on branch switches and detached-HEAD moves —
/// `git commit` on the current branch updates the underlying ref file
/// (loose `refs/heads/<name>`, or `packed-refs` after `git pack-refs`)
/// without touching `HEAD` itself. So when `HEAD` is a symbolic ref we
/// also watch the resolved target and `packed-refs`. A non-existent
/// `rerun-if-changed` path is treated as "always changed" by Cargo, which
/// covers the loose→packed transition.
fn declare_git_head_rerun() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
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
        println!("cargo:rerun-if-changed={}", gitdir.join(target).display());
        println!(
            "cargo:rerun-if-changed={}",
            gitdir.join("packed-refs").display()
        );
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

#[cfg(test)]
mod tests {
    use super::parse_symbolic_ref;

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
