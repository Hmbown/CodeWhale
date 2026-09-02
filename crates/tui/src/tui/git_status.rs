//! Native git status / worktree surface for the TUI chrome.
//!
//! Cached and non-blocking: probes run off the render path on a background
//! thread and the renderer only ever reads [`cached_status`].
//!
//! A probe shells out to the real `git` binary — up to seven invocations
//! (`rev-parse --show-toplevel`, `rev-parse --git-common-dir`,
//! `symbolic-ref --short HEAD` or its `rev-parse --short HEAD` fallback,
//! `status --porcelain`, `rev-list --left-right --count`,
//! `worktree list --porcelain`, and `remote get-url origin` by way of
//! [`crate::remote_control::observed_git_repo`]). There is no `gix`
//! dependency and no
//! per-invocation timeout; the earlier claim of both here was wrong, and it
//! misled a contributor reasoning about probe cost in #5617. All of these
//! run with `GIT_OPTIONAL_LOCKS=0` so a read never contends for
//! `.git/index.lock` in the user's repository.
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
    /// `owner/name` when `origin` resolves to a recognised forge, from the
    /// one normalizer that owns that judgement
    /// ([`crate::remote_control::normalize_observed_git_repo`]): paths,
    /// credentials, and unknown hosts are dropped rather than displayed.
    /// Cached here so chrome can name the repository without probing on the
    /// render path.
    pub remote_slug: Option<String>,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub worktrees: Vec<WorktreeEntry>,
    pub fetched_at: Option<Instant>,
    pub error: Option<String>,
    /// The workspace this snapshot was probed *from*, which is not the same
    /// as [`Self::root`]: launching in a subdirectory gives a `root` of the
    /// repository top level while the workspace stays the subdirectory.
    /// Staleness must compare the probe's own input, not its result.
    pub probed_workspace: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
}

const CACHE_TTL: Duration = Duration::from_secs(2);

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
/// Whether `snap` must be re-probed for `workspace`.
///
/// Split out and pure so the cache contract is testable without spawning
/// git. The workspace comparison uses [`GitStatusSnapshot::probed_workspace`]
/// deliberately: comparing `root` instead meant that any session launched
/// below the repository top level saw `root != workspace` forever, so this
/// returned `true` on every call and `CACHE_TTL` never applied. That turned
/// the two-second chrome tick into an unconditional six-command probe —
/// including the `git status` that contends for `.git/index.lock` (#5617).
fn snapshot_is_stale(snap: &GitStatusSnapshot, workspace: &Path) -> bool {
    snap.fetched_at.is_none_or(|t| t.elapsed() > CACHE_TTL)
        || snap.probed_workspace.as_deref() != Some(workspace)
}

pub fn refresh_if_stale(workspace: &Path) {
    let stale = cache()
        .lock()
        .map(|g| snapshot_is_stale(&g, workspace))
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
        probed_workspace: Some(workspace.to_path_buf()),
        ..GitStatusSnapshot::default()
    };

    // Fast-fail outside a repository. Without this a non-git workspace
    // spawns a doomed `git` process on every tick forever. `find_git_root`
    // walks parents and understands the `gitdir:` pointer file, so linked
    // worktrees and submodules are still recognised — a bare `.git`
    // directory test would not be (#5617). The `rev-parse` below still runs
    // for the cases this cannot see, such as bare repositories.
    if crate::project_context::find_git_root(workspace).is_none() {
        snap.error = Some("not a git repository".into());
        return snap;
    }

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

    // The forge slug (`owner/name`), reusing the remote-control probe rather
    // than parsing `origin` a second time. Rides this cached probe so the
    // topbar never shells out per frame.
    snap.remote_slug = crate::remote_control::observed_git_repo(&root);

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
    let output = Command::new("git")
        .args(args)
        // This probe runs against the user's own repository every two
        // seconds. `git status` opportunistically refreshes the index, and
        // that refresh takes `.git/index.lock` — colliding with a `git
        // commit` the user runs in their own shell (#5617). Optional locks
        // are exactly what we do not want here: we only ever read.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn probed(workspace: &Path, root: &Path) -> GitStatusSnapshot {
        GitStatusSnapshot {
            root: Some(root.to_path_buf()),
            probed_workspace: Some(workspace.to_path_buf()),
            fetched_at: Some(Instant::now()),
            ..GitStatusSnapshot::default()
        }
    }

    /// The cache TTL must actually apply when the session was launched below
    /// the repository top level. Comparing `root` to the workspace made this
    /// permanently stale, so the two-second chrome probe ran unconditionally
    /// and `git status` contended for the user's index lock (#5617).
    #[test]
    fn fresh_snapshot_from_a_subdirectory_is_not_stale() {
        let root = PathBuf::from("/repo");
        let workspace = PathBuf::from("/repo/crates/tui");
        let snap = probed(&workspace, &root);
        assert_ne!(snap.root.as_deref(), Some(workspace.as_path()));
        assert!(
            !snapshot_is_stale(&snap, &workspace),
            "a fresh probe from a subdirectory must satisfy the TTL"
        );
    }

    #[test]
    fn a_different_workspace_is_always_stale() {
        let snap = probed(Path::new("/repo/crates/tui"), Path::new("/repo"));
        assert!(snapshot_is_stale(&snap, Path::new("/other")));
    }

    #[test]
    fn an_unprobed_snapshot_is_stale() {
        assert!(snapshot_is_stale(
            &GitStatusSnapshot::default(),
            Path::new("/repo")
        ));
    }

    /// A workspace outside any repository must resolve without spawning git,
    /// and must record its own input so the TTL suppresses the next tick.
    #[test]
    fn non_git_workspace_fast_fails_and_caches_its_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let snap = probe_status(dir.path());
        assert_eq!(snap.error.as_deref(), Some("not a git repository"));
        assert_eq!(snap.root, None);
        assert_eq!(snap.probed_workspace.as_deref(), Some(dir.path()));
        assert!(
            !snapshot_is_stale(&snap, dir.path()),
            "the negative result must be cached, not re-probed every tick"
        );
    }

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
