//! Worktree provisioning owned by Runtime (not Fleet) — #4176 / #4016.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::DateTime;

/// Spec for an isolated worktree + branch for a lane.
#[derive(Debug, Clone)]
pub struct WorktreeProvision {
    /// Git repository root (must contain `.git`).
    pub repo_root: PathBuf,
    /// Branch to create (from `base_ref`).
    pub branch: String,
    /// Directory for the new worktree (created by `git worktree add`).
    pub path: PathBuf,
    /// Base ref to branch from (default `HEAD`).
    pub base_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProvisionedWorktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Create a git worktree + branch for a lane.
pub fn provision_worktree(spec: &WorktreeProvision) -> Result<ProvisionedWorktree> {
    if spec.branch.trim().is_empty() {
        bail!("worktree branch must not be empty");
    }
    if !spec.repo_root.exists() {
        bail!("repo root does not exist: {}", spec.repo_root.display());
    }
    if let Some(parent) = spec.path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create worktree parent {}", parent.display()))?;
    }
    let base = spec.base_ref.as_deref().unwrap_or("HEAD");
    // Capture git output instead of inheriting the caller's terminal. Runtime
    // callers include the raw-mode TUI launch screen, where even one inherited
    // progress/error line corrupts the alternate-screen buffer.
    let output = Command::new("git")
        .current_dir(&spec.repo_root)
        .args([
            "worktree",
            "add",
            "-b",
            &spec.branch,
            &spec.path.to_string_lossy(),
            base,
        ])
        .output()
        .context("git worktree add")?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "git worktree add failed for branch {} at {}{}{}",
            spec.branch,
            spec.path.display(),
            if detail.is_empty() { "" } else { ": " },
            detail
        );
    }
    Ok(ProvisionedWorktree {
        path: spec.path.clone(),
        branch: spec.branch.clone(),
    })
}

/// Remove a worktree when TTL has expired (or immediately when TTL is 0).
///
/// `stopped_at` is RFC3339. When `ttl_secs` is `None`, no cleanup is performed.
pub fn remove_worktree_if_expired(
    worktree_path: &Path,
    ttl_secs: Option<u64>,
    stopped_at: Option<&str>,
) -> Result<()> {
    let Some(ttl) = ttl_secs else {
        return Ok(());
    };
    if !worktree_path.exists() {
        return Ok(());
    }
    if ttl > 0 {
        let Some(stopped) = stopped_at else {
            return Ok(());
        };
        let stopped_ts = DateTime::parse_from_rfc3339(stopped)
            .with_context(|| format!("parse stopped_at {stopped}"))?
            .timestamp() as u64;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if now.saturating_sub(stopped_ts) < ttl {
            return Ok(());
        }
    }

    // Ask the worktree what it is before deleting it: once the directory is
    // gone, neither its branch nor its repository is recoverable from the path.
    let details = worktree_details(worktree_path);

    // Best-effort: git worktree remove --force, then rm -rf.
    let removed = details.as_ref().is_some_and(|details| {
        Command::new("git")
            .current_dir(&details.repo_root)
            .args([
                "worktree",
                "remove",
                "--force",
                &worktree_path.to_string_lossy(),
            ])
            .status()
            .is_ok_and(|status| status.success())
    });
    if worktree_path.exists() {
        fs::remove_dir_all(worktree_path)
            .with_context(|| format!("remove worktree {}", worktree_path.display()))?;
    }

    let Some(details) = details else {
        return Ok(());
    };
    if !removed {
        // The directory is gone but git still has it registered, and
        // `git worktree add` refuses a path it already knows about.
        let _ = Command::new("git")
            .current_dir(&details.repo_root)
            .args(["worktree", "prune"])
            .status();
    }
    if let Some(branch) = details.branch.as_deref() {
        delete_lane_branch(&details.repo_root, branch);
    }
    Ok(())
}

/// What a lane worktree is: which repository owns it, and which branch it has
/// checked out (`None` when detached).
struct WorktreeDetails {
    repo_root: PathBuf,
    branch: Option<String>,
}

fn worktree_details(worktree_path: &Path) -> Option<WorktreeDetails> {
    let listing = Command::new("git")
        .current_dir(worktree_path)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    // The main worktree is listed first, so its path is the repository root.
    let listing = String::from_utf8_lossy(&listing.stdout);
    let repo_root = listing
        .lines()
        .find_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)?;

    let branch = Command::new("git")
        .current_dir(worktree_path)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|branch| !branch.is_empty());

    Some(WorktreeDetails { repo_root, branch })
}

/// Delete the branch a removed lane worktree was on.
///
/// Lane branch names are derived from the user's launch name (`codex/{slug}`),
/// not from a UUID, so leaving the branch behind makes reusing that name fail
/// with "branch already exists" — a worktree directory that no longer exists
/// still blocking a legitimate lane.
///
/// This uses `branch -d`, not `-D`: a lane branch with nothing on it beyond
/// its base is merged and deletes cleanly, which is the case that was broken.
/// A branch carrying unmerged commits is someone's work, and a TTL timer is
/// not a mandate to throw it away — that one is kept, and the name stays taken
/// until a human decides otherwise.
fn delete_lane_branch(repo_root: &Path, branch: &str) {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(["branch", "-d", branch])
        .output();
    match output {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            tracing::debug!(
                "kept lane branch {branch} after worktree cleanup: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Err(err) => {
            tracing::debug!("could not delete lane branch {branch}: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_repo(root: &Path) {
        assert!(
            Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "user.email", "lane@test"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "user.name", "lane"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("README"), "lane").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "README"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn provision_and_ttl_zero_cleanup() {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        let wt_path = dir.path().join("wt-lane");
        let provisioned = provision_worktree(&WorktreeProvision {
            repo_root: repo,
            branch: "codex/lane-test".into(),
            path: wt_path.clone(),
            base_ref: Some("main".into()),
        })
        .unwrap();
        assert!(provisioned.path.is_dir());
        assert!(wt_path.join("README").is_file());

        remove_worktree_if_expired(&wt_path, Some(0), Some("2020-01-01T00:00:00Z")).unwrap();
        assert!(
            !wt_path.exists(),
            "TTL 0 should remove worktree immediately"
        );
    }

    fn branch_exists(repo: &Path, branch: &str) -> bool {
        Command::new("git")
            .current_dir(repo)
            .args(["rev-parse", "--verify", "--quiet", branch])
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn expired_cleanup_deletes_the_branch_so_the_lane_name_is_reusable() {
        // #4731: cleanup removed the worktree directory but left the branch.
        // Lane branches are named from the user's launch name, so reusing that
        // name then failed with "branch already exists" — pointing at a
        // worktree that no longer existed.
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        let wt_path = dir.path().join("wt-lane");
        let spec = WorktreeProvision {
            repo_root: repo.clone(),
            branch: "codex/reused-name".into(),
            path: wt_path.clone(),
            base_ref: Some("main".into()),
        };
        provision_worktree(&spec).unwrap();
        assert!(branch_exists(&repo, "codex/reused-name"));

        remove_worktree_if_expired(&wt_path, Some(0), Some("2020-01-01T00:00:00Z")).unwrap();
        assert!(!wt_path.exists());
        assert!(
            !branch_exists(&repo, "codex/reused-name"),
            "an unused lane branch must not outlive its worktree"
        );

        // The whole point: the same launch name provisions again.
        provision_worktree(&spec).expect("re-provisioning the same lane name must succeed");
        assert!(wt_path.join("README").is_file());
    }

    #[test]
    fn expired_cleanup_keeps_a_branch_with_unmerged_work() {
        // A TTL timer is not a mandate to discard commits. The worktree goes;
        // the branch carrying work stays, and the name stays taken until a
        // human decides otherwise.
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        let wt_path = dir.path().join("wt-lane");
        provision_worktree(&WorktreeProvision {
            repo_root: repo.clone(),
            branch: "codex/has-work".into(),
            path: wt_path.clone(),
            base_ref: Some("main".into()),
        })
        .unwrap();

        fs::write(wt_path.join("work.txt"), "unmerged").unwrap();
        for args in [
            vec!["add", "work.txt"],
            vec!["commit", "-m", "lane work worth keeping"],
        ] {
            assert!(
                Command::new("git")
                    .args(&args)
                    .current_dir(&wt_path)
                    .status()
                    .unwrap()
                    .success()
            );
        }

        remove_worktree_if_expired(&wt_path, Some(0), Some("2020-01-01T00:00:00Z")).unwrap();
        assert!(!wt_path.exists(), "the worktree directory is disposable");
        assert!(
            branch_exists(&repo, "codex/has-work"),
            "a branch with unmerged commits must survive worktree cleanup"
        );
    }
}
