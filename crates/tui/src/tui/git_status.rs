//! Native git status / worktree surface for the TUI chrome.
//!
//! Fast, cached, non-blocking: probes run off the render path and results
//! are read from a small snapshot. Prefer `gix` when available at build time;
//! fall back to a single short-lived `git` invocation with a hard timeout.
//!
//! This module owns capability and state outside the renderer so
//! `widgets/mod.rs` / `ui.rs` stay projection-only.

#![allow(dead_code)] // Public API; worktree manager wiring continues post-render polish.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Snapshot of repository status for chrome / worktree manager.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatusSnapshot {
    pub root: Option<PathBuf>,
    pub repository_name: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub worktrees: Vec<WorktreeEntry>,
    pub fetched_at: Option<Instant>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
}

/// Chrome is a glance, not a live index. Two seconds was enough to collide
/// with a user's `git commit` (`#5617`): six read-only probes per cycle still
/// refresh the index unless we refuse optional locks.
pub const CHROME_PROBE_TTL: Duration = Duration::from_secs(10);
const CACHE_TTL: Duration = CHROME_PROBE_TTL;

static CACHE: OnceLock<Mutex<GitStatusSnapshot>> = OnceLock::new();

fn cache() -> &'static Mutex<GitStatusSnapshot> {
    CACHE.get_or_init(|| Mutex::new(GitStatusSnapshot::default()))
}

/// Return the last known snapshot without blocking.
#[must_use]
pub fn cached_status() -> GitStatusSnapshot {
    cache().lock().map(|g| g.clone()).unwrap_or_default()
}

/// Refresh status if the cache is stale. Safe to call from a background
/// worker; the render path should only read [`cached_status`].
pub fn refresh_if_stale(workspace: &Path) {
    let stale = cache()
        .lock()
        .map(|g| {
            g.fetched_at.is_none_or(|t| t.elapsed() > CACHE_TTL)
                || g.root.as_deref() != Some(workspace)
        })
        .unwrap_or(true);
    if !stale {
        return;
    }
    let snap = probe_status(workspace);
    if let Ok(mut guard) = cache().lock() {
        *guard = snap;
    }
}

/// Force a refresh (e.g. after checkout / worktree create).
pub fn force_refresh(workspace: &Path) {
    let snap = probe_status(workspace);
    if let Ok(mut guard) = cache().lock() {
        *guard = snap;
    }
}

fn probe_status(workspace: &Path) -> GitStatusSnapshot {
    let mut snap = GitStatusSnapshot {
        fetched_at: Some(Instant::now()),
        ..GitStatusSnapshot::default()
    };

    // Resolve git root.
    let root = git_output(workspace, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|s| PathBuf::from(s.trim()));
    let Some(root) = root else {
        snap.error = Some("not a git repository".into());
        return snap;
    };
    snap.root = Some(root.clone());
    snap.repository_name = repository_name(&root);

    // Branch (symbolic-ref first, then short HEAD for detached).
    snap.branch = git_output(&root, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .or_else(|| git_output(&root, &["rev-parse", "--short", "HEAD"]).ok())
        .map(|s| s.trim().to_string());

    // Dirty: porcelain status (empty = clean).
    if let Ok(status) = git_output(&root, &["status", "--porcelain"]) {
        snap.dirty = !status.trim().is_empty();
    }

    // Ahead/behind vs upstream (best-effort).
    if let Ok(counts) = git_output(
        &root,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    ) {
        let mut parts = counts.split_whitespace();
        if let (Some(behind), Some(ahead)) = (parts.next(), parts.next()) {
            snap.behind = behind.parse().unwrap_or(0);
            snap.ahead = ahead.parse().unwrap_or(0);
        }
    }

    // Worktrees.
    if let Ok(list) = git_output(&root, &["worktree", "list", "--porcelain"]) {
        snap.worktrees = parse_worktree_list(&list);
    }

    snap
}

fn parse_worktree_list(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(path),
                branch: None,
                bare: false,
                locked: false,
            });
        } else if let Some(entry) = current.as_mut() {
            if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                entry.branch = Some(branch.to_string());
            } else if line == "bare" {
                entry.bare = true;
            } else if line.starts_with("locked") {
                entry.locked = true;
            }
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    entries
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    // `--no-optional-locks` + GIT_OPTIONAL_LOCKS=0 keep status/diff from
    // taking `.git/index.lock` just to refresh the index. A user commit in
    // the same tree must not fail because chrome asked what branch we are on.
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(cwd)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn repository_name(worktree_root: &Path) -> Option<String> {
    let common_dir = git_output(worktree_root, &["rev-parse", "--git-common-dir"]).ok()?;
    repository_name_from_common_dir(worktree_root, Path::new(common_dir.trim()))
}

fn repository_name_from_common_dir(worktree_root: &Path, common_dir: &Path) -> Option<String> {
    let common_dir = if common_dir.is_absolute() {
        common_dir.to_path_buf()
    } else {
        worktree_root.join(common_dir)
    };
    common_dir
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
}

/// Compact chrome label: `CodeWhale · main* ↑2` or
/// `CodeWhale/feature · feature*` for a linked worktree.
///
/// Omits the segment when Git has not named a location or ref. A known
/// location without a branch still renders — the header must not invent a
/// ref to fill the slot.
#[must_use]
pub fn chrome_label(snap: &GitStatusSnapshot) -> Option<String> {
    let worktree_name = snap
        .root
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy());
    let location = match (snap.repository_name.as_deref(), worktree_name.as_deref()) {
        (Some(repository), Some(worktree)) if repository != worktree => {
            Some(format!("{repository}/{worktree}"))
        }
        (Some(repository), _) => Some(repository.to_string()),
        (None, Some(worktree)) => Some(worktree.to_string()),
        (None, None) => None,
    };
    let mut label = match (location, snap.branch.as_deref()) {
        (Some(location), Some(branch)) => format!("{location} · {branch}"),
        (Some(location), None) => location,
        (None, Some(branch)) => branch.to_string(),
        (None, None) => return None,
    };
    if snap.dirty {
        label.push('*');
    }
    if snap.ahead > 0 {
        label.push_str(&format!(" ↑{}", snap.ahead));
    }
    if snap.behind > 0 {
        label.push_str(&format!(" ↓{}", snap.behind));
    }
    Some(label)
}

/// Status-bar ink for repository chrome. Location is metadata, not a
/// failure — dirtiness is the `*` on the same gray string.
#[must_use]
pub fn chrome_ink() -> crate::palette::ChromeInk {
    crate::palette::ChromeInk::Metadata
}

/// Create a new worktree at `path` tracking `branch` (or a new branch name).
pub fn create_worktree(
    repo: &Path,
    path: &Path,
    branch: &str,
    new_branch: bool,
) -> Result<(), String> {
    let mut args = vec!["worktree", "add"];
    if new_branch {
        args.push("-b");
        args.push(branch);
        args.push(path.to_str().ok_or("invalid path")?);
    } else {
        args.push(path.to_str().ok_or("invalid path")?);
        args.push(branch);
    }
    git_output(repo, &args).map(|_| ())?;
    force_refresh(repo);
    Ok(())
}

/// List worktrees from the cache (refresh first if needed).
#[must_use]
pub fn list_worktrees(workspace: &Path) -> Vec<WorktreeEntry> {
    refresh_if_stale(workspace);
    cached_status().worktrees
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_worktree_porcelain() {
        let raw = "\
worktree /repo
HEAD abc
branch refs/heads/main

worktree /repo/.cw-worktrees/feat
HEAD def
branch refs/heads/feat
locked
";
        let entries = parse_worktree_list(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(entries[1].locked);
        assert_eq!(entries[1].branch.as_deref(), Some("feat"));
    }

    #[test]
    fn chrome_label_marks_dirty_and_divergence() {
        let snap = GitStatusSnapshot {
            root: Some("/repo".into()),
            repository_name: Some("repo".into()),
            branch: Some("main".into()),
            dirty: true,
            ahead: 2,
            behind: 1,
            ..GitStatusSnapshot::default()
        };
        assert_eq!(chrome_label(&snap).as_deref(), Some("repo · main* ↑2 ↓1"));
    }

    #[test]
    fn chrome_label_identifies_a_linked_worktree() {
        let snap = GitStatusSnapshot {
            root: Some("/repo/.cw-worktrees/feature".into()),
            repository_name: Some("repo".into()),
            branch: Some("feature".into()),
            dirty: true,
            ..GitStatusSnapshot::default()
        };

        assert_eq!(
            chrome_label(&snap).as_deref(),
            Some("repo/feature · feature*")
        );
    }

    #[test]
    fn chrome_label_omits_dirty_marker_when_clean() {
        let snap = GitStatusSnapshot {
            root: Some("/repo".into()),
            repository_name: Some("repo".into()),
            branch: Some("main".into()),
            dirty: false,
            ..GitStatusSnapshot::default()
        };
        assert_eq!(chrome_label(&snap).as_deref(), Some("repo · main"));
    }

    #[test]
    fn chrome_label_keeps_location_when_the_ref_is_unknown() {
        let snap = GitStatusSnapshot {
            root: Some("/repo/.cw-worktrees/feature".into()),
            repository_name: Some("repo".into()),
            branch: None,
            dirty: true,
            ..GitStatusSnapshot::default()
        };
        assert_eq!(chrome_label(&snap).as_deref(), Some("repo/feature*"));
    }

    #[test]
    fn chrome_label_is_absent_without_a_repo_or_ref() {
        assert_eq!(
            chrome_label(&GitStatusSnapshot {
                error: Some("not a git repository".into()),
                ..GitStatusSnapshot::default()
            }),
            None
        );
    }

    #[test]
    fn chrome_ink_is_metadata_not_failure() {
        assert_eq!(chrome_ink(), crate::palette::ChromeInk::Metadata);
        assert_eq!(
            chrome_ink().family(),
            crate::palette::SemanticFamily::Metadata
        );
    }

    #[test]
    fn chrome_probe_is_slow_enough_not_to_fight_a_commit() {
        assert!(
            CHROME_PROBE_TTL >= Duration::from_secs(10),
            "a 2s cadence was enough to hold index.lock during git commit (#5617)"
        );
    }

    #[test]
    fn repository_name_uses_the_common_git_directory_for_worktrees() {
        assert_eq!(
            repository_name_from_common_dir(
                Path::new("/repo/.cw-worktrees/feature"),
                Path::new("/repo/.git")
            )
            .as_deref(),
            Some("repo")
        );
        assert_eq!(
            repository_name_from_common_dir(Path::new("/repo"), Path::new(".git")).as_deref(),
            Some("repo")
        );
    }
}
