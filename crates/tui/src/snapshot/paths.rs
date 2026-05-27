//! Path resolution for the per-workspace snapshot side-repos.
//!
//! Snapshots live under the resolved state directory
//! (`~/.codewhale/snapshots` or legacy `~/.deepseek/snapshots`) with
//! a two-level hash split so we can snapshot multiple worktrees of the
//! same project independently — `git worktree list` users won't get
//! cross-talk between feature branches.

use std::io;
use std::path::{Path, PathBuf};

/// Compute the snapshot directory for a given workspace path.
///
/// Returns `$STATE_DIR/snapshots/<project_hash>/<worktree_hash>/` where
/// `$STATE_DIR` is resolved via `codewhale_config::resolve_state_dir`.
/// The caller is responsible for creating it on disk; we purposefully
/// don't touch the filesystem here so this is cheap to call repeatedly.
///
/// The `project_hash` is derived from the canonicalized workspace path
/// after stripping any `.worktrees/<name>` suffix — multiple worktrees
/// of the same repo share the same `project_hash` so users can browse
/// snapshots cross-worktree if they want, but the `worktree_hash` keeps
/// commits isolated by default.
pub fn snapshot_dir_for(workspace: &Path) -> PathBuf {
    snapshot_dir_with_home(workspace, dirs::home_dir())
}

/// Same as [`snapshot_dir_for`] but with an injectable home directory.
/// Used by tests so they never touch the user's real state directory.
pub fn snapshot_dir_with_home(workspace: &Path, home: Option<PathBuf>) -> PathBuf {
    let home = home.unwrap_or_else(|| PathBuf::from("."));
    let canonical = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let project_root = strip_worktree_suffix(&canonical);
    let project_hash = stable_hex(&project_root);
    let worktree_hash = stable_hex(&canonical);
    snapshot_base_with_home(Some(home))
        .join(project_hash)
        .join(worktree_hash)
}

fn snapshot_base_with_home(home: Option<PathBuf>) -> PathBuf {
    let home = home.unwrap_or_else(|| PathBuf::from("."));
    // Prefer .codewhale, fall back to .deepseek
    let primary = home.join(".codewhale").join("snapshots");
    if primary.exists() {
        return primary;
    }
    home.join(".deepseek").join("snapshots")
}

/// Resolve the `.git` directory inside the snapshot dir.
pub fn snapshot_git_dir(workspace: &Path) -> PathBuf {
    snapshot_dir_for(workspace).join(".git")
}

/// Ensure the snapshot dir exists on disk and return its path.
pub fn ensure_snapshot_dir(workspace: &Path) -> io::Result<PathBuf> {
    let dir = snapshot_dir_for(workspace);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Strip a trailing `.worktrees/<name>` segment so all worktrees of the
/// same checkout share a `project_hash`. If the path doesn't look like a
/// worktree it's returned unchanged.
fn strip_worktree_suffix(path: &Path) -> PathBuf {
    let mut components: Vec<_> = path.components().collect();
    if components.len() >= 2
        && let Some(parent) = components.get(components.len() - 2)
        && parent.as_os_str() == ".worktrees"
    {
        components.truncate(components.len() - 2);
        let mut p = PathBuf::new();
        for c in components {
            p.push(c.as_os_str());
        }
        return p;
    }
    path.to_path_buf()
}

/// Hex-encoded deterministic FNV-1a digest. This is only a directory tag, not
/// a security boundary, but it must remain stable across process launches.
fn stable_hex(path: &Path) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn snapshot_dir_layout_two_levels_under_deepseek() {
        let tmp = tempdir().expect("tempdir");
        let dir = snapshot_dir_with_home(tmp.path(), Some(tmp.path().to_path_buf()));
        let mut iter = dir.strip_prefix(tmp.path()).unwrap().components();
        assert_eq!(iter.next().unwrap().as_os_str(), ".deepseek");
        assert_eq!(iter.next().unwrap().as_os_str(), "snapshots");
        assert!(iter.next().is_some()); // project_hash
        assert!(iter.next().is_some()); // worktree_hash
        assert!(iter.next().is_none());
    }

    #[test]
    fn worktree_suffix_stripped_for_project_hash() {
        let tmp = tempdir().expect("tempdir");
        let main_path = tmp.path().join("repo");
        let wt_path = tmp.path().join("repo").join(".worktrees").join("featX");
        std::fs::create_dir_all(&main_path).unwrap();
        std::fs::create_dir_all(&wt_path).unwrap();

        let main_dir = snapshot_dir_with_home(&main_path, Some(tmp.path().to_path_buf()));
        let wt_dir = snapshot_dir_with_home(&wt_path, Some(tmp.path().to_path_buf()));

        // Same project_hash (parent component before the worktree-specific tail).
        let main_components: Vec<_> = main_dir.components().collect();
        let wt_components: Vec<_> = wt_dir.components().collect();
        assert_eq!(
            main_components[main_components.len() - 2],
            wt_components[wt_components.len() - 2],
            "worktrees should share project_hash",
        );
        // But different worktree_hash (the tail).
        assert_ne!(main_components.last(), wt_components.last());
    }

    #[test]
    fn ensure_snapshot_dir_creates_path() {
        let tmp = tempdir().expect("tempdir");
        // Use scoped HOME so we don't pollute the real one.
        let dir = snapshot_dir_with_home(tmp.path(), Some(tmp.path().to_path_buf()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(dir.exists());
    }

    #[test]
    fn snapshot_git_dir_appends_dot_git() {
        let tmp = tempdir().expect("tempdir");
        let git_dir = snapshot_git_dir(tmp.path());
        assert_eq!(git_dir.file_name().unwrap(), ".git");
    }
}
