//! Boot-time snapshot pruning.
//!
//! Called from `session_manager` once per session start. Failure is
//! never fatal — old snapshots taking disk space is annoying but not
//! correctness-breaking, so we log and move on.

use std::io;
use std::path::Path;
use std::time::Duration;

use super::paths::snapshot_git_dir;
use super::repo::SnapshotRepo;

/// Default snapshot retention window: 7 days.
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Prune snapshots older than `max_age` for the given workspace.
///
/// If no snapshot repo exists yet (first run) this is a cheap no-op.
/// Returns the number of snapshots removed.
pub fn prune_older_than(workspace: &Path, max_age: Duration) -> io::Result<usize> {
    let git_dir = snapshot_git_dir(workspace);
    if !git_dir.exists() {
        return Ok(0);
    }
    let repo = SnapshotRepo::open_or_init(workspace)?;
    let removed = repo.prune_older_than(max_age)?;
    repo.prune_unreachable_objects()?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_test_env;
    use std::sync::MutexGuard;
    use tempfile::tempdir;

    /// Same guard shape as in `repo::tests` — pins HOME for the lifetime
    /// of one test under the process-wide env mutex.
    struct ScopedHome {
        prev: Option<std::ffi::OsString>,
        _guard: MutexGuard<'static, ()>,
    }
    impl Drop for ScopedHome {
        fn drop(&mut self) {
            // SAFETY: process-wide lock still held.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }
    fn scoped_home(home: &std::path::Path) -> ScopedHome {
        let guard = lock_test_env();
        let prev = std::env::var_os("HOME");
        // SAFETY: serialised by the global env lock.
        unsafe {
            std::env::set_var("HOME", home);
        }
        ScopedHome {
            prev,
            _guard: guard,
        }
    }

    #[test]
    fn prune_no_repo_returns_zero() {
        let tmp = tempdir().unwrap();
        let _home = scoped_home(tmp.path());
        let removed = prune_older_than(tmp.path(), DEFAULT_MAX_AGE).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn prune_with_existing_repo_zero_age_clears_all() {
        let tmp = tempdir().unwrap();
        let _home = scoped_home(tmp.path());
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
        std::fs::write(workspace.join("f.txt"), "x").unwrap();
        repo.snapshot("turn:0").unwrap();

        // Same-second flake guard: see `repo::tests`.
        std::thread::sleep(Duration::from_millis(1100));

        let removed = prune_older_than(&workspace, Duration::from_secs(0)).unwrap();
        assert!(removed >= 1);
    }
}
