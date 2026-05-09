//! Side-git repository wrapper for workspace snapshots.
//!
//! `SnapshotRepo` shells out to the system `git` binary (we deliberately
//! avoid `git2` to dodge its LGPL surface). The two paths that matter:
//!
//! - `git_dir`  → `~/.deepseek/snapshots/<project_hash>/<worktree_hash>/.git`
//! - `work_tree` → the user's actual workspace
//!
//! Every git invocation passes both `--git-dir` AND `--work-tree`. That is
//! the single biggest safety mechanism: it guarantees we never accidentally
//! mutate the user's own `.git` directory. If git can't find the side
//! repo, the command fails fast instead of falling back to "current
//! directory".

use std::collections::HashSet;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::paths::{ensure_snapshot_dir, snapshot_git_dir};

/// Identifier for a snapshot — currently the underlying git commit SHA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotId(pub String);

impl SnapshotId {
    /// Borrow the SHA as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A single snapshot record (one row in `git log`).
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Commit SHA inside the side repo.
    pub id: SnapshotId,
    /// Subject line — the label passed to [`SnapshotRepo::snapshot`].
    pub label: String,
    /// Author timestamp (Unix seconds).
    pub timestamp: i64,
}

/// Wrapper around the per-workspace side-git repo.
pub struct SnapshotRepo {
    git_dir: PathBuf,
    work_tree: PathBuf,
}

const STALE_TMP_PACK_AGE: Duration = Duration::from_secs(60 * 60);

/// Maximum total snapshot storage in megabytes before pruning kicks in at
/// snapshot time. Keeps the side repo from blowing up the user's disk during
/// long-running or high-churn sessions (#1112).
const MAX_SNAPSHOT_SIZE_MB: u64 = 500;

/// Grace margin below `MAX_SNAPSHOT_SIZE_MB` used as the prune target
/// so the repo doesn't hit the limit again one snapshot later.
const PRUNE_TARGET_MB: u64 = 400;

const BUILTIN_EXCLUDES: &str = "\
# DeepSeek TUI built-in snapshot exclusions
node_modules/
target/
dist/
build/
.build/
.next/
.nuxt/
.svelte-kit/
.turbo/
.parcel-cache/
vendor/
.cargo/
.rustup/
.npm/
.bun/
.yarn/
.pnpm-store/
.cache/
.venv/
venv/
.tox/
__pycache__/
*.pyc
.mypy_cache/
.pytest_cache/
.ruff_cache/
.gradle/
.m2/
.local/
.DS_Store

# Binary and generated artifacts. Snapshots are source rollback checkpoints,
# not a full binary backup; keeping these out avoids side-repo bloat.
*.exe
*.dll
*.so
*.dylib
*.wasm
*.o
*.obj
*.class
*.pdb
*.dSYM
*.zip
*.tar
*.tar.gz
*.tgz
*.tar.bz2
*.tar.xz
*.7z
*.rar
*.iso
*.dmg
*.bin
*.mp4
*.mov
*.mkv
*.avi
*.webm
*.mp3
*.wav
*.flac
*.aac
";

impl SnapshotRepo {
    /// Open or initialize the snapshot repo for `workspace`.
    ///
    /// On first use this:
    /// 1. Creates the `~/.deepseek/snapshots/<…>/.git` dir.
    /// 2. Runs `git init --bare=false --quiet`.
    /// 3. Sets a fixed `user.name` / `user.email` so commits don't pick up
    ///    the user's global git identity (we don't want our snapshots to
    ///    look like they came from the user).
    pub fn open_or_init(workspace: &Path) -> io::Result<Self> {
        let work_tree = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        if let Some(reason) =
            unsafe_workspace_snapshot_reason(&work_tree, dirs::home_dir().as_deref())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "workspace snapshots are disabled for {reason}: {}",
                    work_tree.display()
                ),
            ));
        }

        let _ = ensure_snapshot_dir(&work_tree)?;
        let git_dir = snapshot_git_dir(&work_tree);

        let needs_init = !git_dir.exists();
        if needs_init {
            let parent = git_dir.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "snapshot dir has no parent")
            })?;
            std::fs::create_dir_all(parent)?;
            // `git init` here uses the parent directory as the work tree
            // and stores metadata in `.git`. We then continue to use
            // explicit `--git-dir` / `--work-tree` flags for every other
            // command so behaviour is invariant of cwd.
            let init = Command::new("git")
                .arg("init")
                .arg("--quiet")
                .arg(parent)
                .output()
                .map_err(|e| io_other(format!("failed to spawn git init: {e}")))?;
            if !init.status.success() {
                return Err(io_other(format!(
                    "git init failed: {}",
                    String::from_utf8_lossy(&init.stderr).trim()
                )));
            }

            // Pin a stable identity so snapshot commits are recognisable
            // and don't bleed into the user's git config.
            let _ = run_git(
                &git_dir,
                &work_tree,
                &["config", "user.name", "deepseek-snapshots"],
            );
            let _ = run_git(
                &git_dir,
                &work_tree,
                &["config", "user.email", "snapshots@deepseek-tui.local"],
            );
            // Don't auto-gc on every commit; we manage pruning ourselves.
            let _ = run_git(&git_dir, &work_tree, &["config", "gc.auto", "0"]);
            // Ignore CRLF rewriting — we want byte-for-byte fidelity.
            let _ = run_git(&git_dir, &work_tree, &["config", "core.autocrlf", "false"]);
        }

        write_builtin_excludes(&git_dir)?;
        if let Err(err) = cleanup_stale_pack_temps(&git_dir, STALE_TMP_PACK_AGE) {
            tracing::debug!(
                target: "snapshot",
                "failed to clean stale snapshot tmp_pack files: {err}"
            );
        }
        Ok(Self { git_dir, work_tree })
    }

    /// Take a snapshot of the current working tree.
    ///
    /// Internally: `git add -A`, `git write-tree`, `git commit-tree`, then
    /// `git update-ref HEAD <commit>`.
    /// `git add -A` honours the user's workspace ignore rules while staging
    /// into the side repo's index.
    ///
    /// Before committing, checks whether the snapshot directory exceeds
    /// [`MAX_SNAPSHOT_SIZE_MB`] and prunes the oldest snapshots if it does.
    ///
    /// Returns the snapshot's commit SHA.
    pub fn snapshot(&self, label: &str) -> io::Result<SnapshotId> {
        // Guard against disk blowup (#1112): if the snapshot directory has
        // grown beyond the limit, prune aggressively before adding more.
        if let Ok(current_mb) = dir_size_mb(&self.git_dir)
            && current_mb > MAX_SNAPSHOT_SIZE_MB
        {
            tracing::warn!(
                target: "snapshot",
                current_mb,
                limit_mb = MAX_SNAPSHOT_SIZE_MB,
                "snapshot storage approaching limit — pruning aggressively"
            );
            // Walk backward from a 1-second retention to zero until
            // we're under the target, or until there's nothing left.
            let mut age = Duration::from_secs(1);
            for _ in 0..10 {
                let _ = self.prune_older_than(age);
                if let Ok(new_size) = dir_size_mb(&self.git_dir)
                    && new_size <= PRUNE_TARGET_MB
                {
                    tracing::info!(
                        target: "snapshot",
                        new_size_mb = new_size,
                        "pruned snapshot storage back under limit"
                    );
                    break;
                }
                age = age.saturating_sub(Duration::from_millis(100));
            }
            // Fallback: if even 0-second pruning didn't help (shouldn't
            // happen but belt-and-suspenders), nuke the refs so the next
            // snapshot starts a fresh history.
            if let Ok(final_size) = dir_size_mb(&self.git_dir)
                && final_size > MAX_SNAPSHOT_SIZE_MB
            {
                tracing::warn!(
                    target: "snapshot",
                    "snapshot storage still over limit after pruning; wiping history"
                );
                let _ = self.prune_older_than(Duration::ZERO);
                let _ = self.prune_unreachable_objects();
            }
        }
        // Stage every tracked + untracked path the workspace exposes.
        // `--all` here means `add` + `update` + `remove` — the same set
        // `git status` would show.
        let add = run_git(&self.git_dir, &self.work_tree, &["add", "-A"])?;
        if !add.status.success() {
            return Err(io_other(format!(
                "git add -A failed: {}",
                String::from_utf8_lossy(&add.stderr).trim()
            )));
        }

        let tree = run_git(&self.git_dir, &self.work_tree, &["write-tree"])?;
        if !tree.status.success() {
            return Err(io_other(format!(
                "git write-tree failed: {}",
                String::from_utf8_lossy(&tree.stderr).trim()
            )));
        }
        let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();

        let parent = run_git(
            &self.git_dir,
            &self.work_tree,
            &["rev-parse", "--verify", "HEAD"],
        )?;
        let parent = parent
            .status
            .success()
            .then(|| String::from_utf8_lossy(&parent.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        let mut args = vec!["commit-tree".to_string(), tree];
        if let Some(parent) = parent {
            args.push("-p".to_string());
            args.push(parent);
        }
        args.push("-m".to_string());
        args.push(label.to_string());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

        // `commit-tree` creates marker commits even when the tree matches its
        // parent, and it does not run user/global commit hooks.
        let commit = run_git(&self.git_dir, &self.work_tree, &arg_refs)?;
        if !commit.status.success() {
            return Err(io_other(format!(
                "git commit-tree failed: {}",
                String::from_utf8_lossy(&commit.stderr).trim()
            )));
        }
        let sha = String::from_utf8_lossy(&commit.stdout).trim().to_string();

        let update = run_git(
            &self.git_dir,
            &self.work_tree,
            &["update-ref", "HEAD", &sha],
        )?;
        if !update.status.success() {
            return Err(io_other(format!(
                "git update-ref HEAD failed: {}",
                String::from_utf8_lossy(&update.stderr).trim()
            )));
        }

        Ok(SnapshotId(sha))
    }

    /// Restore the workspace to the state at `id`.
    ///
    /// Uses `git checkout <sha> -- :/` which checks out every path in the
    /// snapshot tree relative to the workspace root. We do NOT touch the
    /// user's own `.git` — snapshots only contain working-tree files.
    pub fn restore(&self, id: &SnapshotId) -> io::Result<()> {
        let current_paths = self.tree_paths("HEAD")?;
        let target_paths = self.tree_paths(id.as_str())?;
        let checkout = run_git(
            &self.git_dir,
            &self.work_tree,
            &["checkout", id.as_str(), "--", ":/"],
        )?;
        if !checkout.status.success() {
            return Err(io_other(format!(
                "git checkout failed: {}",
                String::from_utf8_lossy(&checkout.stderr).trim()
            )));
        }
        self.remove_paths_missing_from_target(&current_paths, &target_paths)?;
        Ok(())
    }

    /// Return whether the current workspace matches the given snapshot's
    /// tracked file content.
    ///
    /// This is intentionally narrower than a full "workspace identical"
    /// claim: it compares the current working tree against the snapshot's
    /// tracked paths via git's diff machinery. That is sufficient for
    /// `/undo` cursoring — if the diff is empty, restoring this snapshot
    /// again would be a no-op, so the caller should continue scanning
    /// older snapshots.
    pub fn work_tree_matches_snapshot(&self, id: &SnapshotId) -> io::Result<bool> {
        let diff = run_git(
            &self.git_dir,
            &self.work_tree,
            &["diff", "--quiet", id.as_str(), "--", ":/"],
        )?;
        Ok(diff.status.success())
    }

    fn tree_paths(&self, treeish: &str) -> io::Result<HashSet<PathBuf>> {
        let ls = run_git(
            &self.git_dir,
            &self.work_tree,
            &["ls-tree", "-r", "-z", "--name-only", treeish],
        )?;
        if !ls.status.success() {
            return Err(io_other(format!(
                "git ls-tree failed: {}",
                String::from_utf8_lossy(&ls.stderr).trim()
            )));
        }
        Ok(parse_nul_paths(&ls.stdout))
    }

    fn remove_paths_missing_from_target(
        &self,
        current_paths: &HashSet<PathBuf>,
        target_paths: &HashSet<PathBuf>,
    ) -> io::Result<()> {
        for rel in current_paths.difference(target_paths) {
            if !is_safe_relative_path(rel) {
                continue;
            }
            let path = self.work_tree.join(rel);
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_dir() {
                let _ = std::fs::remove_dir(&path);
            } else {
                std::fs::remove_file(&path)?;
            }
            self.prune_empty_parent_dirs(path.parent());
        }
        Ok(())
    }

    fn prune_empty_parent_dirs(&self, mut dir: Option<&Path>) {
        while let Some(path) = dir {
            if path == self.work_tree {
                break;
            }
            if std::fs::remove_dir(path).is_err() {
                break;
            }
            dir = path.parent();
        }
    }

    /// List up to `limit` most-recent snapshots, newest first.
    pub fn list(&self, limit: usize) -> io::Result<Vec<Snapshot>> {
        // `git log -<n>` is the short form of `--max-count=<n>`; if `limit`
        // is `usize::MAX` (caller asked for "everything") we pass an empty
        // count so git defaults to no upper bound.
        let mut args: Vec<String> = vec!["log".to_string()];
        if limit < usize::MAX {
            args.push(format!("--max-count={limit}"));
        }
        args.push("--pretty=format:%H%x09%at%x09%s".to_string());
        args.push("--no-color".to_string());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let log = run_git(&self.git_dir, &self.work_tree, &arg_refs)?;
        if !log.status.success() {
            // No commits yet → empty list.
            return Ok(Vec::new());
        }
        let stdout = String::from_utf8_lossy(&log.stdout);
        let mut out = Vec::new();
        for line in stdout.lines() {
            let mut parts = line.splitn(3, '\t');
            let sha = parts.next().unwrap_or("").to_string();
            let ts = parts
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let subject = parts.next().unwrap_or("").to_string();
            if sha.is_empty() {
                continue;
            }
            out.push(Snapshot {
                id: SnapshotId(sha),
                label: subject,
                timestamp: ts,
            });
        }
        Ok(out)
    }

    /// Drop snapshots older than `max_age`, returning the count removed.
    ///
    /// Strategy: identify keepable commits (younger than the cutoff),
    /// reset HEAD to the oldest survivor, then `git reflog expire` +
    /// `git gc --prune=now` to actually reclaim space. Cheap and avoids
    /// rewriting history when nothing has aged out.
    pub fn prune_older_than(&self, max_age: Duration) -> io::Result<usize> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| io_other(format!("clock error: {e}")))?
            .as_secs() as i64;
        let cutoff = now - max_age.as_secs() as i64;

        let snapshots = self.list(usize::MAX)?;
        if snapshots.is_empty() {
            return Ok(0);
        }

        // Snapshots are newest-first. Find the index of the first one
        // at-or-older than the cutoff — every entry from that index
        // onward is a candidate for removal. We use `<=` so a 0-second
        // retention drops same-second commits (otherwise tests calling
        // `prune_older_than(Duration::ZERO)` immediately after creating
        // a snapshot would never prune anything).
        let cut_index = snapshots.iter().position(|s| s.timestamp <= cutoff);
        let Some(cut) = cut_index else {
            return Ok(0);
        };
        let removed = snapshots.len() - cut;
        if removed == 0 {
            return Ok(0);
        }

        if cut == 0 {
            // Every snapshot is older than the cutoff — wipe the repo
            // entirely so the next snapshot starts a fresh history.
            // Removing `.git/refs/heads/*` is enough to orphan the old
            // commits, then gc reclaims them.
            let refs_dir = self.git_dir.join("refs").join("heads");
            if refs_dir.exists() {
                for entry in std::fs::read_dir(&refs_dir)? {
                    let path = entry?.path();
                    if path.is_file() {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            // Also drop HEAD's packed refs so `git log` returns nothing.
            let packed = self.git_dir.join("packed-refs");
            if packed.exists() {
                let _ = std::fs::remove_file(&packed);
            }
        } else {
            // Reset HEAD to the youngest commit older-than-cutoff's
            // *predecessor* — i.e. the oldest surviving snapshot.
            let survivor = &snapshots[cut - 1];
            let reset = run_git(
                &self.git_dir,
                &self.work_tree,
                &["update-ref", "HEAD", survivor.id.as_str()],
            )?;
            if !reset.status.success() {
                return Err(io_other(format!(
                    "git update-ref failed: {}",
                    String::from_utf8_lossy(&reset.stderr).trim()
                )));
            }
        }

        // Reclaim space.
        let _ = run_git(
            &self.git_dir,
            &self.work_tree,
            &["reflog", "expire", "--expire=now", "--all"],
        );
        let _ = run_git(
            &self.git_dir,
            &self.work_tree,
            &["gc", "--prune=now", "--quiet"],
        );

        Ok(removed)
    }

    /// Drop unreachable loose objects left behind by interrupted or
    /// orphaned side-repo operations.
    pub fn prune_unreachable_objects(&self) -> io::Result<()> {
        let prune = run_git(&self.git_dir, &self.work_tree, &["prune", "--expire=now"])?;
        if !prune.status.success() {
            return Err(io_other(format!(
                "git prune failed: {}",
                String::from_utf8_lossy(&prune.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Return the side-repo's `.git` directory (for diagnostics).
    #[allow(dead_code)]
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Return the work tree path (for diagnostics).
    #[allow(dead_code)]
    pub fn work_tree(&self) -> &Path {
        &self.work_tree
    }
}

fn write_builtin_excludes(git_dir: &Path) -> io::Result<()> {
    let info_dir = git_dir.join("info");
    std::fs::create_dir_all(&info_dir)?;
    std::fs::write(info_dir.join("exclude"), BUILTIN_EXCLUDES)
}

/// Recursively compute the total size of a directory in megabytes.
fn dir_size_mb(root: &Path) -> io::Result<u64> {
    fn walk(dir: &Path, total: &mut u64) -> io::Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                walk(&path, total)?;
            } else if ft.is_file() {
                *total = total.saturating_add(entry.metadata().map(|m| m.len()).unwrap_or(0));
            }
        }
        Ok(())
    }
    let mut total: u64 = 0;
    walk(root, &mut total)?;
    Ok(total / (1024 * 1024))
}

fn cleanup_stale_pack_temps(git_dir: &Path, stale_age: Duration) -> io::Result<usize> {
    let pack_dir = git_dir.join("objects").join("pack");
    if !pack_dir.exists() {
        return Ok(0);
    }
    cleanup_stale_pack_temps_in(&pack_dir, stale_age, SystemTime::now())
}

fn cleanup_stale_pack_temps_in(
    pack_dir: &Path,
    stale_age: Duration,
    now: SystemTime,
) -> io::Result<usize> {
    let mut removed = 0;
    for entry in std::fs::read_dir(pack_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("tmp_pack_") {
            continue;
        }
        if !entry.file_type()?.is_file() {
            continue;
        }

        let metadata = entry.metadata()?;
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age < stale_age {
            continue;
        }

        match std::fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(removed)
}

fn run_git(git_dir: &Path, work_tree: &Path, args: &[&str]) -> io::Result<Output> {
    Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .arg("--work-tree")
        .arg(work_tree)
        .args(args)
        .output()
}

fn io_other(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

fn unsafe_workspace_snapshot_reason(workspace: &Path, home: Option<&Path>) -> Option<&'static str> {
    let workspace = normalize_path_for_safety(workspace);
    if is_filesystem_root(&workspace) {
        return Some("filesystem root");
    }

    if is_home_directory(&workspace, home) {
        return Some("home directory");
    }

    let home = home.map(normalize_path_for_safety)?;
    if workspace.parent() == Some(home.as_path()) {
        let name = workspace.file_name().and_then(|name| name.to_str());
        if matches!(
            name,
            Some(
                "Desktop" | "Documents" | "Downloads" | "Library" | "Movies" | "Music" | "Pictures"
            )
        ) {
            return Some("home collection directory");
        }
    }

    None
}

fn normalize_path_for_safety(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_filesystem_root(path: &Path) -> bool {
    path.parent().is_none()
}

fn is_home_directory(work_tree: &Path, home: Option<&Path>) -> bool {
    let Some(home) = home else {
        return false;
    };

    let home_canonical = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    work_tree == home_canonical
}

fn parse_nul_paths(bytes: &[u8]) -> HashSet<PathBuf> {
    bytes
        .split(|b| *b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| PathBuf::from(String::from_utf8_lossy(chunk).into_owned()))
        .collect()
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_test_env;
    use std::fs::{File, FileTimes};
    use std::sync::MutexGuard;
    use tempfile::tempdir;

    /// Holds the home directory pinned to a tempdir for the lifetime of a test. Also
    /// owns the process-wide env-var mutex so tests across modules
    /// don't trample each other's home env vars.
    pub(super) struct ScopedHome {
        prev_vars: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _guard: MutexGuard<'static, ()>,
    }
    impl Drop for ScopedHome {
        fn drop(&mut self) {
            // SAFETY: process-wide lock still held.
            unsafe {
                for (key, prev) in self.prev_vars.drain(..) {
                    match prev {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }
    pub(super) fn scoped_home(home: &Path) -> ScopedHome {
        let guard = lock_test_env();
        let prev_vars = ["HOME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        // SAFETY: serialised by the global env lock.
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            std::env::remove_var("HOMEDRIVE");
            std::env::remove_var("HOMEPATH");
        }
        ScopedHome {
            prev_vars,
            _guard: guard,
        }
    }

    /// Build a side-repo whose snapshot dir lives under the same
    /// tempdir we're using for `HOME` — so the inner `dirs::home_dir()`
    /// lookup stays inside our sandbox. Returns the guard alongside so
    /// the caller can keep HOME pinned for the rest of the test.
    fn make_repo(tmp: &Path) -> (SnapshotRepo, ScopedHome) {
        let workspace = tmp.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let guard = scoped_home(tmp);
        let repo = SnapshotRepo::open_or_init(&workspace).expect("open_or_init");
        (repo, guard)
    }

    #[test]
    fn snapshot_creates_commit_in_side_repo_only() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        std::fs::write(repo.work_tree().join("a.txt"), b"alpha").unwrap();

        let id = repo.snapshot("pre-turn:1").expect("snapshot");
        assert_eq!(id.as_str().len(), 40);

        let list = repo.list(10).expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label, "pre-turn:1");

        // The user's workspace must NOT have a real `.git` because we
        // never created one in their workspace — only in the side dir.
        assert!(!repo.work_tree().join(".git").exists());
    }

    #[test]
    fn restore_reverts_workspace_files() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        let f = repo.work_tree().join("file.txt");

        std::fs::write(&f, b"original").unwrap();
        let id = repo.snapshot("pre-turn:1").expect("snapshot");

        std::fs::write(&f, b"clobbered").unwrap();
        repo.snapshot("post-turn:1").expect("snapshot 2");

        repo.restore(&id).expect("restore");
        let after = std::fs::read_to_string(&f).unwrap();
        assert_eq!(after, "original");
    }

    #[test]
    fn restore_removes_files_added_after_target_snapshot() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        let original = repo.work_tree().join("original.txt");
        let added = repo.work_tree().join("added.txt");

        std::fs::write(&original, b"original").unwrap();
        let id = repo.snapshot("pre-turn:1").expect("snapshot");

        std::fs::write(&added, b"new file").unwrap();
        repo.snapshot("post-turn:1").expect("snapshot 2");

        repo.restore(&id).expect("restore");
        assert!(original.exists());
        assert!(!added.exists(), "restore must remove tracked added files");
    }

    #[test]
    fn snapshot_and_restore_do_not_move_user_git_head() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .arg("init")
            .arg("--quiet")
            .status()
            .unwrap();
        std::fs::write(workspace.join("tracked.txt"), b"committed").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .arg("add")
            .arg("tracked.txt")
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .arg("-c")
            .arg("user.name=user")
            .arg("-c")
            .arg("user.email=user@example.test")
            .arg("commit")
            .arg("--quiet")
            .arg("-m")
            .arg("init")
            .status()
            .unwrap();
        let user_head_before = Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout;

        let _home = scoped_home(tmp.path());
        let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
        std::fs::write(workspace.join("tracked.txt"), b"dirty-before").unwrap();
        let id = repo.snapshot("pre-turn:1").unwrap();
        std::fs::write(workspace.join("tracked.txt"), b"dirty-after").unwrap();
        repo.snapshot("post-turn:1").unwrap();
        repo.restore(&id).unwrap();

        let user_head_after = Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(user_head_after, user_head_before);
        assert_eq!(
            std::fs::read_to_string(workspace.join("tracked.txt")).unwrap(),
            "dirty-before"
        );
    }

    #[test]
    fn list_respects_limit() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        for i in 0..5 {
            std::fs::write(repo.work_tree().join("f.txt"), format!("v{i}")).unwrap();
            repo.snapshot(&format!("turn:{i}")).unwrap();
        }
        let three = repo.list(3).unwrap();
        assert_eq!(three.len(), 3);
        // Newest first.
        assert_eq!(three[0].label, "turn:4");
    }

    #[test]
    fn prune_drops_snapshots_older_than_threshold() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        std::fs::write(repo.work_tree().join("f.txt"), "v0").unwrap();
        repo.snapshot("turn:0").unwrap();

        // Wait one second so the snapshot's commit timestamp is strictly
        // in the past relative to the prune call's "now" — otherwise
        // same-second comparisons make the assertion flaky.
        std::thread::sleep(Duration::from_millis(1100));

        let removed = repo.prune_older_than(Duration::from_secs(0)).unwrap();
        assert!(removed >= 1, "expected at least 1 pruned, got {removed}");

        // After pruning everything, the next snapshot should start a
        // fresh history.
        std::fs::write(repo.work_tree().join("f.txt"), "v1").unwrap();
        repo.snapshot("turn:1").unwrap();
        let list = repo.list(10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label, "turn:1");
    }

    #[test]
    fn open_or_init_removes_stale_tmp_pack_files_only() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        let workspace = repo.work_tree().to_path_buf();
        let pack_dir = repo.git_dir().join("objects").join("pack");
        std::fs::create_dir_all(&pack_dir).unwrap();

        let stale = pack_dir.join("tmp_pack_stale");
        let fresh = pack_dir.join("tmp_pack_fresh");
        let ordinary_pack = pack_dir.join("pack-kept.pack");
        std::fs::write(&stale, b"stale").unwrap();
        std::fs::write(&fresh, b"fresh").unwrap();
        std::fs::write(&ordinary_pack, b"pack").unwrap();

        let old_time = SystemTime::now() - STALE_TMP_PACK_AGE - Duration::from_secs(60);
        {
            let file = File::options().write(true).open(&stale).unwrap();
            file.set_times(FileTimes::new().set_modified(old_time))
                .unwrap();
        }

        SnapshotRepo::open_or_init(&workspace).unwrap();

        assert!(!stale.exists(), "stale tmp_pack file should be removed");
        assert!(fresh.exists(), "fresh tmp_pack file should be kept");
        assert!(ordinary_pack.exists(), "non-temp pack file should be kept");
    }

    #[test]
    fn snapshot_respects_workspace_gitignore() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        std::fs::write(repo.work_tree().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(repo.work_tree().join("ignored.txt"), b"secret").unwrap();
        std::fs::write(repo.work_tree().join("kept.txt"), b"public").unwrap();

        let id = repo.snapshot("pre-turn:1").expect("snapshot");

        // `git ls-tree` against the snapshot's commit shouldn't list ignored.txt.
        let ls = run_git(
            repo.git_dir(),
            repo.work_tree(),
            &["ls-tree", "-r", "--name-only", id.as_str()],
        )
        .expect("ls-tree");
        let names = String::from_utf8_lossy(&ls.stdout);
        assert!(names.contains("kept.txt"), "kept.txt missing: {names}");
        assert!(
            !names.contains("ignored.txt"),
            "ignored.txt should not be in snapshot: {names}",
        );
    }

    #[test]
    fn unsafe_workspace_rejects_home_directory_workspace() {
        let tmp = tempdir().unwrap();
        let home = tmp.path();

        assert_eq!(
            unsafe_workspace_snapshot_reason(home, Some(home)),
            Some("home directory")
        );
    }

    #[test]
    fn unsafe_workspace_rejects_home_collection_directories() {
        let tmp = tempdir().unwrap();
        let home = tmp.path();
        let desktop = tmp.path().join("Desktop");
        std::fs::create_dir_all(&desktop).unwrap();

        assert_eq!(
            unsafe_workspace_snapshot_reason(&desktop, Some(home)),
            Some("home collection directory")
        );
    }

    #[test]
    fn unsafe_workspace_allows_project_directories_under_home() {
        let tmp = tempdir().unwrap();
        let home = tmp.path();
        let workspace = tmp.path().join("code").join("project");
        std::fs::create_dir_all(&workspace).unwrap();

        assert_eq!(
            unsafe_workspace_snapshot_reason(&workspace, Some(home)),
            None
        );
    }

    #[test]
    fn snapshot_respects_builtin_excludes() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        std::fs::create_dir_all(repo.work_tree().join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(repo.work_tree().join(".next/cache")).unwrap();
        std::fs::create_dir_all(repo.work_tree().join("src")).unwrap();
        std::fs::write(
            repo.work_tree().join("node_modules/pkg/index.js"),
            b"generated",
        )
        .unwrap();
        std::fs::write(repo.work_tree().join(".next/cache/chunk.bin"), b"generated").unwrap();
        std::fs::write(repo.work_tree().join("debug.wasm"), b"binary").unwrap();
        std::fs::write(repo.work_tree().join("src/main.rs"), b"fn main() {}").unwrap();

        let excludes = std::fs::read_to_string(repo.git_dir().join("info/exclude")).unwrap();
        assert!(excludes.contains("node_modules/"));
        assert!(excludes.contains(".next/"));
        assert!(excludes.contains("*.wasm"));

        let id = repo.snapshot("pre-turn:1").expect("snapshot");
        let ls = run_git(
            repo.git_dir(),
            repo.work_tree(),
            &["ls-tree", "-r", "--name-only", id.as_str()],
        )
        .expect("ls-tree");
        let names = String::from_utf8_lossy(&ls.stdout);
        assert!(
            names.contains("src/main.rs"),
            "src/main.rs missing: {names}"
        );
        assert!(
            !names.contains("node_modules"),
            "node_modules should not be in snapshot: {names}",
        );
        assert!(
            !names.contains(".next"),
            ".next should not be in snapshot: {names}",
        );
        assert!(
            !names.contains("debug.wasm"),
            "binary artifacts should not be in snapshot: {names}",
        );
    }

    #[test]
    fn open_or_init_is_idempotent() {
        let tmp = tempdir().unwrap();
        let (_r, _h) = make_repo(tmp.path());
        // Second open should not panic and should reuse the existing
        // `.git`. We re-open via the public API rather than make_repo to
        // avoid double-acquiring HOME (the guard would deadlock).
        drop((_r, _h));
        let (_r2, _h2) = make_repo(tmp.path());
    }

    #[test]
    fn home_directory_guard_matches_canonical_paths() {
        let tmp = tempdir().unwrap();
        let home = tmp.path();
        let home_canonical = home.canonicalize().unwrap();
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let workspace_canonical = workspace.canonicalize().unwrap();

        assert!(is_home_directory(&home_canonical, Some(home)));
        assert!(!is_home_directory(&workspace_canonical, Some(home)));
        assert!(!is_home_directory(&home_canonical, None));
    }

    #[test]
    fn dir_size_mb_measures_directory_bytes() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("sizedir");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        // 3 bytes per file — well under 1 MB.
        std::fs::write(dir.join("a.txt"), b"abc").unwrap();
        std::fs::write(dir.join("sub/b.txt"), b"xyz").unwrap();

        let size = dir_size_mb(&dir).expect("dir_size_mb");
        assert_eq!(size, 0, "6 bytes should be 0 MB");

        // Write 2 MB of data.
        let big = dir.join("big.bin");
        std::fs::write(&big, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let size = dir_size_mb(&dir).expect("dir_size_mb after big write");
        assert_eq!(size, 2, "expected 2 MB after writing 2 MB file");
    }

    /// Regression: snapshot size cap (#1112). When the snapshot dir grows,
    /// `snapshot()` must prune old snapshots to stay under the limit.
    /// This test uses the real size constants, which are 500/400 MB —
    /// we can't easily blow up a temp dir to 500 MB in a unit test.
    /// Instead we verify the guard logic doesn't panic or error on a
    /// small repo (well under the cap), and that `snapshot()` still works.
    #[test]
    fn snapshot_succeeds_when_under_size_cap() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        // The side repo is tiny — well under 500 MB. Snapshot should work.
        std::fs::write(repo.work_tree().join("f.txt"), b"hello").unwrap();
        let id = repo.snapshot("pre-turn:1").expect("snapshot under cap");
        assert_eq!(id.as_str().len(), 40);
    }
}
