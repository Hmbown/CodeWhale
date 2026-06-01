//! Git worktree management for whaleFlow.
//!
//! [`WorktreeManager`] creates lightweight git worktrees so sub-agents
//! can work in isolation without colliding on the main workspace.  After
//! the agent completes, the scheduler extracts changes as a patch, removes
//! the worktree, and applies the patch back to the main workspace.
//!
//! The module uses only `std::process::Command` and the crate's own
//! [`SpawnError`] error type — it has zero dependencies on the TUI crate.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::spawner::SpawnError;

/// Manager for git worktree lifecycle: create → extract → remove → apply.
pub struct WorktreeManager;

impl WorktreeManager {
    /// Create a new worktree for the given task.
    ///
    /// Runs `git worktree add .worktrees/whaleflow-{task_id} HEAD` inside
    /// `workspace`.  If the worktree directory already exists the call is
    /// a no-op (idempotent) and the existing path is returned.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError::WorktreeError`] if the git command fails.
    pub fn create(task_id: &str, workspace: &Path) -> Result<PathBuf, SpawnError> {
        let relative = format!(".worktrees/whaleflow-{}", task_id);
        let worktree_path = workspace.join(&relative);

        // Idempotent: skip creation if the worktree already exists.
        if worktree_path.exists() {
            return Ok(worktree_path);
        }

        let output = Command::new("git")
            .arg("worktree")
            .arg("add")
            .arg(&relative)
            .arg("HEAD")
            .current_dir(workspace)
            .output()
            .map_err(|e| SpawnError::WorktreeError(format!("git worktree add failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SpawnError::WorktreeError(format!(
                "git worktree add failed: {}",
                stderr.trim()
            )));
        }

        Ok(worktree_path)
    }

    /// Extract outstanding changes from a worktree as a unified-diff patch.
    ///
    /// Runs `git -C .worktrees/whaleflow-{task_id} diff HEAD`.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError::WorktreeError`] if the git command fails.
    pub fn extract_changes(task_id: &str, workspace: &Path) -> Result<String, SpawnError> {
        let relative = format!(".worktrees/whaleflow-{}", task_id);
        let worktree_path = workspace.join(&relative);

        let output = Command::new("git")
            .arg("-C")
            .arg(&worktree_path)
            .arg("diff")
            .arg("HEAD")
            .output()
            .map_err(|e| {
                SpawnError::WorktreeError(format!("git diff in worktree failed: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SpawnError::WorktreeError(format!(
                "git diff in worktree failed: {}",
                stderr.trim()
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Remove a worktree (and its directory) with `git worktree remove --force`.
    ///
    /// If the worktree directory does not exist the call is a no-op — this
    /// is not treated as an error.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError::CleanupError`] if the git command fails.
    pub fn remove(task_id: &str, workspace: &Path) -> Result<(), SpawnError> {
        let relative = format!(".worktrees/whaleflow-{}", task_id);
        let worktree_path = workspace.join(&relative);

        // No-op if the worktree does not exist.
        if !worktree_path.exists() {
            return Ok(());
        }

        let output = Command::new("git")
            .arg("worktree")
            .arg("remove")
            .arg(&relative)
            .arg("--force")
            .current_dir(workspace)
            .output()
            .map_err(|e| {
                SpawnError::CleanupError(format!("git worktree remove failed: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SpawnError::CleanupError(format!(
                "git worktree remove failed: {}",
                stderr.trim()
            )));
        }

        Ok(())
    }

    /// Apply a patch to the main workspace via `git apply`.
    ///
    /// The patch text is written to stdin of the `git apply` process.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError::WorktreeError`] if the git command fails.
    pub fn apply_patch(workspace: &Path, patch: &str) -> Result<(), SpawnError> {
        let mut child = Command::new("git")
            .arg("apply")
            .current_dir(workspace)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| SpawnError::WorktreeError(format!("git apply spawn failed: {}", e)))?;

        // Write the patch to stdin, then drop the handle to close the pipe
        // before waiting on the child — otherwise `git apply` deadlocks
        // waiting for EOF.
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| SpawnError::WorktreeError("failed to open git apply stdin".into()))?;
            stdin
                .write_all(patch.as_bytes())
                .map_err(|e| SpawnError::WorktreeError(format!("write patch to git apply failed: {}", e)))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| SpawnError::WorktreeError(format!("git apply wait failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SpawnError::WorktreeError(format!(
                "git apply failed: {}",
                stderr.trim()
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command as StdCommand;

    /// Helper: initialise a temporary git repo.
    fn init_temp_repo(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wf-test-{}-{}", std::process::id(), suffix));
        fs::create_dir_all(&dir).unwrap();
        StdCommand::new("git")
            .arg("init")
            .current_dir(&dir)
            .output()
            .unwrap();
        // Create an initial commit so HEAD exists.
        let readme = dir.join("README.md");
        fs::write(&readme, "# test\n").unwrap();
        StdCommand::new("git")
            .args(["add", "README.md"])
            .current_dir(&dir)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&dir)
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn create_worktree_and_extract_changes() {
        let repo = init_temp_repo("create");
        let task_id = "test-1";

        // Create worktree.
        let path = WorktreeManager::create(task_id, &repo).unwrap();
        assert!(path.exists());
        assert!(path.join("README.md").exists());

        // Make a change inside the worktree.
        fs::write(path.join("README.md"), "# modified\n").unwrap();

        // Extract changes.
        let patch = WorktreeManager::extract_changes(task_id, &repo).unwrap();
        assert!(patch.contains("# modified"));

        // Cleanup.
        WorktreeManager::remove(task_id, &repo).unwrap();
        assert!(!path.exists());

        // Cleanup repo.
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn create_is_idempotent() {
        let repo = init_temp_repo("idemp");
        let task_id = "test-idemp";

        let path1 = WorktreeManager::create(task_id, &repo).unwrap();
        let path2 = WorktreeManager::create(task_id, &repo).unwrap();
        assert_eq!(path1, path2);

        WorktreeManager::remove(task_id, &repo).unwrap();
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn remove_nonexistent_is_noop() {
        // Should not error when worktree doesn't exist.
        let result = WorktreeManager::remove("no-such-task", Path::new("/tmp"));
        assert!(result.is_ok());
    }

    #[test]
    fn apply_patch_applies_changes() {
        let repo = init_temp_repo("apply");
        let task_id = "test-apply";

        // Create worktree, make a change, extract patch, remove worktree.
        let path = WorktreeManager::create(task_id, &repo).unwrap();
        fs::write(path.join("README.md"), "# patched\n").unwrap();
        let patch = WorktreeManager::extract_changes(task_id, &repo).unwrap();
        WorktreeManager::remove(task_id, &repo).unwrap();

        // Apply patch to main workspace.
        WorktreeManager::apply_patch(&repo, &patch).unwrap();

        // Verify the change landed.
        let contents = fs::read_to_string(repo.join("README.md")).unwrap();
        assert_eq!(contents, "# patched\n");

        let _ = fs::remove_dir_all(&repo);
    }
}
