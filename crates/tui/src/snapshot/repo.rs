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
use std::process::Output;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::dependencies::ExternalTool;

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
    /// Session this snapshot belongs to, when recorded (encoded as a
    /// `[sid=...] ` label prefix). `None` for legacy snapshots taken
    /// before session tagging existed.
    pub session_id: Option<String>,
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

const BYTES_PER_MB: u64 = 1024 * 1024;

/// Grace margin below `MAX_SNAPSHOT_SIZE_MB` used as the prune target
/// so the repo doesn't hit the limit again one snapshot later.
const PRUNE_TARGET_MB: u64 = 400;

/// Default workspace-size ceiling above which snapshots self-disable
/// on first use (2 GB of non-excluded content). Reports from users with
/// multi-hundred-GB project directories — datasets, model weights,
/// docker image dumps that fall outside the built-in excludes —
/// surfaced that `git add -A` on first init would hang the TUI for
/// minutes-to-hours while indexing the workspace. Snapshots are a
/// rollback safety net, not a backup tool; bailing out on workspaces
/// that big is the right tradeoff. Users with legitimate large
/// monorepos can raise `[snapshots] max_workspace_gb` (or set it to
/// `0` to disable the cap entirely).
pub const DEFAULT_MAX_WORKSPACE_BYTES_FOR_SNAPSHOT: u64 = 2 * 1024 * 1024 * 1024;

/// Hard cap on the number of file entries the bounded size estimator
/// will inspect before declaring the workspace "too large". Protects
/// against a workspace with millions of tiny files (no individual
/// file is large, but `git add -A` would still take forever).
const SIZE_WALK_MAX_ENTRIES: usize = 200_000;

/// Top-level directory and extension patterns that the snapshot path
/// already excludes via `BUILTIN_EXCLUDES`. The estimator skips these
/// up front so the size walk reflects what would actually land in the
/// snapshot commit. Kept narrow to common build-output dirs — anything
/// else falls back to the `.gitignore` filter.
const SIZE_WALK_SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    "vendor",
    ".cargo",
    ".rustup",
    ".npm",
    ".bun",
    ".yarn",
    ".pnpm-store",
    ".cache",
    ".venv",
    "venv",
    ".tox",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".gradle",
    ".m2",
    ".local",
    ".git",
];

const BUILTIN_EXCLUDES: &str = "\
# CodeWhale built-in snapshot exclusions
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
    /// Open an existing snapshot repo for `workspace` without creating or
    /// initializing anything on disk.
    ///
    /// This is useful for read-only UI surfaces that want to report checkpoint
    /// availability without paying the first-init size walk or surprising the
    /// user by creating a side repo from a view action.
    pub fn open_existing(workspace: &Path) -> io::Result<Option<Self>> {
        let work_tree = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let git_dir = snapshot_git_dir(&work_tree);
        if !git_dir.exists() || !git_dir.join("HEAD").exists() {
            return Ok(None);
        }
        Ok(Some(Self { git_dir, work_tree }))
    }

    /// Open or initialize the snapshot repo for `workspace`.
    ///
    /// On first use this:
    /// 1. Creates the `~/.deepseek/snapshots/<…>/.git` dir.
    /// 2. Runs `git init --bare=false --quiet`.
    /// 3. Sets a fixed `user.name` / `user.email` so commits don't pick up
    ///    the user's global git identity (we don't want our snapshots to
    ///    look like they came from the user).
    pub fn open_or_init(workspace: &Path) -> io::Result<Self> {
        Self::open_or_init_with_cap(workspace, DEFAULT_MAX_WORKSPACE_BYTES_FOR_SNAPSHOT)
    }

    /// Variant of [`Self::open_or_init`] that accepts an explicit
    /// workspace-size cap. `cap_bytes = 0` disables the cap entirely
    /// (always snapshot, regardless of size).
    ///
    /// When the workspace exceeds the cap and the side repo hasn't
    /// been initialized yet, returns `Err(InvalidInput)` with a
    /// "workspace too large" reason. Subsequent calls (after the user
    /// shrinks the workspace or raises the cap via config) succeed.
    pub fn open_or_init_with_cap(workspace: &Path, cap_bytes: u64) -> io::Result<Self> {
        let work_tree = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        if let Some(reason) = unsafe_workspace_snapshot_reason(
            &work_tree,
            crate::config::effective_home_dir().as_deref(),
        ) {
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
            // First-init size guard. Skipping this on subsequent opens
            // is intentional: paying a workspace walk on every snapshot
            // would defeat the purpose of the cap, and a workspace
            // that fit on first init is allowed to grow within the
            // existing repo's `MAX_SNAPSHOT_SIZE_MB` budget. Users on
            // workspaces that grew past the cap mid-session get the
            // existing aggressive-pruning path in `snapshot()`.
            if estimate_workspace_size_bounded(&work_tree, cap_bytes).is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "workspace too large for snapshots (over {} GB of non-excluded content or > {} entries): {}\n  raise `[snapshots] max_workspace_gb` in config.toml (or set it to 0 to disable the cap) if you want snapshots on this workspace.",
                        cap_bytes / (1024 * 1024 * 1024),
                        SIZE_WALK_MAX_ENTRIES,
                        work_tree.display()
                    ),
                ));
            }
            let parent = git_dir.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "snapshot dir has no parent")
            })?;
            std::fs::create_dir_all(parent)?;
            // `git init` here uses the parent directory as the work tree
            // and stores metadata in `.git`. We then continue to use
            // explicit `--git-dir` / `--work-tree` flags for every other
            // command so behaviour is invariant of cwd.
            let init = crate::dependencies::Git::command()
                .ok_or_else(|| io_other("git not found on PATH"))?
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
                &["config", "user.email", "snapshots@codewhale.local"],
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
    #[allow(dead_code)] // convenience entry kept for tests and legacy callers; production writes go through snapshot_with_session
    pub fn snapshot(&self, label: &str) -> io::Result<SnapshotId> {
        self.snapshot_with_session(label, None)
    }

    /// Take a snapshot, tagging it with the owning session id.
    ///
    /// The session id is encoded into the commit message as a `[sid=...] `
    /// label prefix. [`Self::list`] decodes it back into
    /// [`Snapshot::session_id`] and strips the prefix from the visible
    /// label, so existing listing surfaces keep showing the plain label.
    /// Legacy snapshots taken through [`Self::snapshot`] carry no prefix
    /// and decode with `session_id == None`.
    pub fn snapshot_with_session(
        &self,
        label: &str,
        session_id: Option<&str>,
    ) -> io::Result<SnapshotId> {
        // Guard against disk blowup (#1112): if the snapshot directory has
        // grown beyond the limit, prune aggressively before adding more.
        // When the prune actually destroys restore points the user is told
        // once per workspace — losing undo history to a log line is the S5
        // failure mode (2026-08-04 snapshot hunt).
        if let Ok(removed) = self.prune_size_pressure(
            MAX_SNAPSHOT_SIZE_MB * BYTES_PER_MB,
            PRUNE_TARGET_MB * BYTES_PER_MB,
        ) && removed > 0
        {
            notify_snapshot_history_pruned_once(&self.work_tree, removed);
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
        args.push(Self::encode_session_label(label, session_id));
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

    /// Prefix a snapshot label with its owning session id, if any.
    fn encode_session_label(label: &str, session_id: Option<&str>) -> String {
        match session_id {
            Some(sid) if !sid.is_empty() => format!("[sid={sid}] {label}"),
            _ => label.to_string(),
        }
    }

    /// Split a possibly session-tagged label back into `(session_id, label)`.
    ///
    /// Returns `(None, label)` for untagged labels. The decoded label is
    /// the original one without the `[sid=...] ` prefix, so consumers that
    /// match on `pre-turn:`/`tool:`/`redo:` prefixes keep working unchanged.
    fn decode_session_label(label: &str) -> (Option<String>, String) {
        let Some(rest) = label.strip_prefix("[sid=") else {
            return (None, label.to_string());
        };
        let Some(end) = rest.find("] ") else {
            return (None, label.to_string());
        };
        let sid = &rest[..end];
        let plain = &rest[end + 2..];
        if sid.is_empty() || plain.is_empty() {
            return (None, label.to_string());
        }
        (Some(sid.to_string()), plain.to_string())
    }
    /// Size-pressure prune (#1112): if the side repo exceeds `max_bytes`,
    /// walk backward from a 1-second retention toward zero until the store is
    /// at or under `target_bytes`, escalating to a full wipe when nothing
    /// else helps. Returns the total number of snapshots destroyed, so the
    /// caller can tell the user their undo history shrank (S5 — the wipe was
    /// previously announced only by a `tracing::warn`).
    fn prune_size_pressure(&self, max_bytes: u64, target_bytes: u64) -> io::Result<usize> {
        let current_bytes = dir_size_bytes(&self.git_dir)?;
        if current_bytes <= max_bytes {
            return Ok(0);
        }
        tracing::warn!(
            target: "snapshot",
            current_mb = current_bytes / BYTES_PER_MB,
            limit_mb = max_bytes / BYTES_PER_MB,
            "snapshot storage approaching limit — pruning aggressively"
        );
        let mut removed_total: usize = 0;
        // Walk backward from a 1-second retention to zero until
        // we're under the target, or until there's nothing left.
        let mut age = Duration::from_secs(1);
        for _ in 0..10 {
            if let Ok(removed) = self.prune_older_than(age) {
                removed_total = removed_total.saturating_add(removed);
            }
            if let Ok(new_size) = dir_size_bytes(&self.git_dir)
                && new_size <= target_bytes
            {
                tracing::info!(
                    target: "snapshot",
                    new_size_mb = new_size / BYTES_PER_MB,
                    "pruned snapshot storage back under limit"
                );
                break;
            }
            age = age.saturating_sub(Duration::from_millis(100));
        }
        // Fallback: if even 0-second pruning didn't help (shouldn't
        // happen but belt-and-suspenders), nuke the refs so the next
        // snapshot starts a fresh history.
        if let Ok(final_size) = dir_size_bytes(&self.git_dir)
            && final_size > max_bytes
        {
            tracing::warn!(
                target: "snapshot",
                "snapshot storage still over limit after pruning; wiping history"
            );
            if let Ok(removed) = self.prune_older_than(Duration::ZERO) {
                removed_total = removed_total.saturating_add(removed);
            }
            let _ = self.prune_unreachable_objects();
        }
        Ok(removed_total)
    }

    /// Restore the workspace to the state at `id`.
    ///
    /// Uses `git checkout <sha> -- :/` which checks out every path in the
    /// snapshot tree relative to the workspace root. We do NOT touch the
    /// user's own `.git` — snapshots only contain working-tree files.
    pub fn restore(&self, id: &SnapshotId) -> io::Result<()> {
        // Restore is the one destructive operation with no undo of its own.
        // Capture the pre-restore state first so the restore itself can be
        // reversed (2026-08-04 snapshot hunt: makes several other findings
        // recoverable instead of final). The `pre-restore:` prefix is
        // deliberately not a `/undo` or `revert_turn` candidate label, so the
        // safety net never changes snapshot selection. Best-effort: a failed
        // safety snapshot must never block the restore the user asked for.
        let target_short = &id.as_str()[..id.as_str().len().min(12)];
        if let Err(e) = self.snapshot_with_session(&format!("pre-restore:{target_short}"), None) {
            tracing::warn!(
                target: "snapshot",
                "pre-restore safety snapshot failed (restore will proceed): {e}"
            );
        }
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
            let (session_id, label) = Self::decode_session_label(&subject);
            out.push(Snapshot {
                id: SnapshotId(sha),
                label,
                timestamp: ts,
                session_id,
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
            // Keep the newest `cut` snapshots (indices [0..cut], newest-first)
            // and drop the older tail. This MUST rebuild the survivors as a
            // fresh orphan chain, not `update-ref HEAD <oldest survivor>`:
            // the snapshots are a parent-linked commit chain with the newest
            // at HEAD, so pointing HEAD at the oldest survivor orphaned every
            // NEWER snapshot (gc then destroyed them) while keeping the very
            // snapshots we meant to remove as its ancestors — the exact
            // inverse of the intent (2026-08-04 review, reproduced).
            self.rebuild_survivor_chain(&snapshots[..cut])?;
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

    /// Rebuild `survivors` (newest-first) as a fresh orphan commit chain and
    /// point HEAD at its tip, so every snapshot NOT in `survivors` becomes
    /// unreachable for gc to reclaim. Each survivor's tree, label, session
    /// id, and author/committer timestamp are preserved, so ages do not lie
    /// after a prune (finding: `prune_keep_last_n` previously reset them to
    /// "now"). Assumes `survivors` is non-empty.
    fn rebuild_survivor_chain(&self, survivors: &[Snapshot]) -> io::Result<()> {
        let mut prev_sha: Option<String> = None;
        for s in survivors.iter().rev() {
            let tree = run_git(
                &self.git_dir,
                &self.work_tree,
                &["rev-parse", &format!("{}^{{tree}}", s.id.as_str())],
            )?;
            if !tree.status.success() {
                return Err(io_other(format!(
                    "rev-parse {}^{{tree}} failed: {}",
                    s.id.as_str(),
                    String::from_utf8_lossy(&tree.stderr).trim()
                )));
            }
            let tree_hash = String::from_utf8_lossy(&tree.stdout).trim().to_string();

            let mut args = vec![
                "commit-tree".to_string(),
                "-m".to_string(),
                Self::encode_session_label(&s.label, s.session_id.as_deref()),
                tree_hash,
            ];
            if let Some(ref p) = prev_sha {
                args.push("-p".to_string());
                args.push(p.clone());
            }
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let new_sha = self.commit_tree_preserving_date(&arg_refs, s.timestamp)?;
            prev_sha = Some(new_sha);
        }

        if let Some(final_sha) = prev_sha {
            let up = run_git(
                &self.git_dir,
                &self.work_tree,
                &["update-ref", "HEAD", &final_sha],
            )?;
            if !up.status.success() {
                return Err(io_other(format!(
                    "update-ref HEAD failed: {}",
                    String::from_utf8_lossy(&up.stderr).trim()
                )));
            }
        }
        Ok(())
    }

    /// Run a `commit-tree` invocation with the author/committer dates pinned
    /// to `timestamp` (Unix seconds), so a rebuilt survivor keeps its real
    /// age instead of stamping "now".
    fn commit_tree_preserving_date(&self, args: &[&str], timestamp: i64) -> io::Result<String> {
        let date = format!("{timestamp} +0000");
        let out = crate::dependencies::Git::command()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "git not found on PATH"))?
            .arg("--git-dir")
            .arg(&self.git_dir)
            .arg("--work-tree")
            .arg(&self.work_tree)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .args(args)
            .output()?;
        if !out.status.success() {
            return Err(io_other(format!(
                "commit-tree failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Keep only the latest `max_count` snapshots, dropping older ones.
    ///
    /// Uses `commit-tree` with no `-p` to create a true orphan commit at
    /// the eldest survivor's tree, preserving its label.  The old chain
    /// has zero refs after gc and is physically reclaimed.
    /// Keep only the latest `max_count` snapshots by rebuilding the
    /// survivor chain as orphan commits.  Each survivor's tree and label
    /// are preserved — only the parent chain to older snapshots is cut.
    /// Old objects become unreachable and gc reclaims them.
    pub fn prune_keep_last_n(&self, max_count: usize) -> io::Result<usize> {
        let snapshots = self.list(usize::MAX)?;
        if snapshots.len() <= max_count {
            return Ok(0);
        }
        let keep = max_count;
        let removed = snapshots.len() - keep;
        // snapshots are newest-first: [0..keep] are the survivors. Rebuild
        // them as an orphan chain so the older tail is reclaimed.
        self.rebuild_survivor_chain(&snapshots[..keep])?;
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

/// Recursively compute the total size of a directory in bytes.
fn dir_size_bytes(root: &Path) -> io::Result<u64> {
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
    Ok(total)
}

/// One prominent notice per workspace per process when the size-pressure
/// prune destroys restore points — silent loss of undo history is the S5
/// failure mode (2026-08-04 snapshot hunt). The stderr print is deliberate:
/// headless/CLI stderr is the user surface for once-per-workspace snapshot
/// warnings, matching `maybe_notify_snapshots_disabled_once` in
/// `core/turn.rs`.
#[allow(clippy::print_stderr)]
fn notify_snapshot_history_pruned_once(workspace: &Path, removed: usize) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static NOTIFIED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let key = workspace.to_string_lossy().into_owned();
    let set = NOTIFIED.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut guard) = set.lock() else {
        return;
    };
    if !guard.insert(key) {
        return;
    }
    drop(guard);
    eprint!("{}", snapshot_history_pruned_message(workspace, removed));
}

/// Build the user-visible notice for a size-pressure prune. Kept pure and
/// separate from the emit/dedup shell so the content is unit-testable.
fn snapshot_history_pruned_message(workspace: &Path, removed: usize) -> String {
    format!(
        "warning: snapshot/undo history for {} was pruned to stay under the {} MB snapshot storage cap.
  {} snapshot(s) were removed and can no longer be restored.
  The cap bounds the undo side-repo's disk use; high-churn or large workspaces hit it sooner.
",
        workspace.display(),
        MAX_SNAPSHOT_SIZE_MB,
        removed
    )
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
    crate::dependencies::Git::command()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "git not found on PATH"))?
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

/// Walk `workspace` and accumulate file sizes, returning `Some(total)`
/// when the workspace fits under `cap_bytes` and `None` when the walk
/// exceeds the cap. Honors `.gitignore` (via the `ignore` crate's
/// `WalkBuilder` defaults) and the snapshot-specific skip list above,
/// so the measured size reflects what would actually land in a
/// snapshot commit rather than the raw `du -sh` total.
///
/// The walk is bounded by both `cap_bytes` and
/// [`SIZE_WALK_MAX_ENTRIES`] — either trip returns `None`. A
/// `cap_bytes` of `0` disables the cap entirely (returns `Some(total)`
/// no matter how large), so config can opt out.
pub fn estimate_workspace_size_bounded(workspace: &Path, cap_bytes: u64) -> Option<u64> {
    use ignore::WalkBuilder;
    let mut total: u64 = 0;
    let mut entries: usize = 0;
    let skip: HashSet<&'static str> = SIZE_WALK_SKIP_DIRS.iter().copied().collect();
    let walker = WalkBuilder::new(workspace)
        .hidden(false)
        .follow_links(false)
        .filter_entry(move |entry| {
            // Skip the well-known build-output directories at any depth.
            // The `ignore` crate calls `filter_entry` once per dir/file;
            // returning `false` here prunes the whole subtree.
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !skip.contains(name))
        })
        .build();
    for entry in walker.flatten() {
        entries += 1;
        if entries > SIZE_WALK_MAX_ENTRIES {
            return None;
        }
        if let Ok(meta) = entry.metadata()
            && meta.is_file()
        {
            total = total.saturating_add(meta.len());
            if cap_bytes > 0 && total > cap_bytes {
                return None;
            }
        }
    }
    Some(total)
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
    use tempfile::tempdir;

    /// Holds the home directory pinned to a tempdir for the lifetime of a test. Also
    /// owns the process-wide env-var mutex so tests across modules
    /// don't trample each other's home env vars.
    pub(super) struct ScopedHome {
        prev_vars: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _guard: crate::test_support::TestEnvLock,
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
    /// tempdir we're using for `HOME` — so the inner `crate::config::effective_home_dir()`
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
    fn open_existing_is_read_only_and_does_not_initialize() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let _home = scoped_home(tmp.path());

        let before = SnapshotRepo::open_existing(&workspace).expect("open existing");
        assert!(before.is_none());
        assert!(
            !snapshot_git_dir(&workspace).exists(),
            "read-only open must not create the side repo"
        );

        let repo = SnapshotRepo::open_or_init(&workspace).expect("open_or_init");
        std::fs::write(repo.work_tree().join("a.txt"), b"alpha").unwrap();
        repo.snapshot("pre-turn:1").expect("snapshot");

        let after = SnapshotRepo::open_existing(&workspace).expect("open existing");
        assert!(after.is_some());
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
    fn restore_takes_a_pre_restore_safety_snapshot_that_round_trips() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        let f = repo.work_tree().join("file.txt");

        std::fs::write(&f, b"v1").unwrap();
        let id1 = repo.snapshot("pre-turn:1").expect("snapshot v1");

        std::fs::write(&f, b"v2").unwrap();
        repo.snapshot("post-turn:1").expect("snapshot v2");

        repo.restore(&id1).expect("restore to v1");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v1");

        // The restore must have captured the pre-restore state (v2) under a
        // `pre-restore:` label naming its target, so the destructive op is
        // itself reversible (2026-08-04 snapshot hunt).
        let snapshots = repo.list(usize::MAX).expect("list");
        let safety = snapshots
            .iter()
            .find(|s| s.label.starts_with("pre-restore:"))
            .expect("a pre-restore safety snapshot must exist");
        assert!(
            safety.label.ends_with(&id1.as_str()[..12]),
            "safety label should name the restore target: {}",
            safety.label
        );

        repo.restore(&safety.id)
            .expect("restore the safety snapshot");
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "v2",
            "the safety snapshot must bring back the pre-restore state"
        );
    }

    #[test]
    fn snapshot_and_restore_do_not_move_user_git_head() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        crate::dependencies::Git::command()
            .expect("git not found")
            .arg("-C")
            .arg(&workspace)
            .arg("init")
            .arg("--quiet")
            .status()
            .unwrap();
        std::fs::write(workspace.join("tracked.txt"), b"committed").unwrap();
        crate::dependencies::Git::command()
            .expect("git not found")
            .arg("-C")
            .arg(&workspace)
            .arg("add")
            .arg("tracked.txt")
            .status()
            .unwrap();
        crate::dependencies::Git::command()
            .expect("git not found")
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
        let user_head_before = crate::dependencies::Git::command()
            .expect("git not found")
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

        let user_head_after = crate::dependencies::Git::command()
            .expect("git not found")
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

    /// The 2026-08-04 regression: with a cut in the MIDDLE of history,
    /// `prune_older_than` used to `update-ref HEAD <oldest survivor>`, which
    /// orphaned (and gc destroyed) the NEWEST snapshots while keeping the
    /// old ones as ancestors — the inverse of the intent, firing on every
    /// boot. This pins the correct partial-cut behavior.
    #[test]
    fn prune_older_than_keeps_the_newest_and_drops_only_the_old_tail() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());

        // Two "old" snapshots, then a pause, then two "new" ones.
        for i in 0..2 {
            std::fs::write(repo.work_tree().join("f.txt"), format!("old{i}")).unwrap();
            repo.snapshot(&format!("old:{i}")).unwrap();
            std::thread::sleep(Duration::from_millis(1100));
        }
        // A wide gap so git's whole-second commit timestamps land the cut
        // unambiguously between the old and new pairs. The margins are
        // deliberately generous: this test runs under full-suite parallelism
        // where a sleep can overrun, and the cut is wall-clock. At prune time
        // the newest pair is ~0-1.2s old against a 6s cutoff, and the old
        // pair is ~9s old — ~5s of slack in both directions.
        std::thread::sleep(Duration::from_secs(8));
        for i in 0..2 {
            std::fs::write(repo.work_tree().join("f.txt"), format!("new{i}")).unwrap();
            repo.snapshot(&format!("new:{i}")).unwrap();
            if i == 0 {
                std::thread::sleep(Duration::from_millis(1100));
            }
        }
        let before = repo.list(usize::MAX).unwrap();
        assert_eq!(before.len(), 4);
        // Guard the fixture itself: if load skewed the timestamps so the cut
        // would not fall between the pairs, say so instead of failing later
        // with a confusing count mismatch.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(
            now - before[0].timestamp < 6 && now - before[2].timestamp > 6,
            "fixture ages unusable for a 6s cut (newest {}s, oldest-surviving-pair {}s)",
            now - before[0].timestamp,
            now - before[2].timestamp
        );

        // Cut 6s back: the two old snapshots drop, the two new ones survive.
        let removed = repo.prune_older_than(Duration::from_secs(6)).unwrap();
        assert_eq!(removed, 2, "only the old tail should be removed");

        let remaining = repo.list(usize::MAX).unwrap();
        assert_eq!(remaining.len(), 2, "the two newest must survive");
        assert_eq!(
            remaining[0].label, "new:1",
            "newest survives (was destroyed before)"
        );
        assert_eq!(remaining[1].label, "new:0");
        assert!(
            !remaining.iter().any(|s| s.label.starts_with("old:")),
            "old snapshots must be gone, not kept as ancestors: {:?}",
            remaining.iter().map(|s| &s.label).collect::<Vec<_>>()
        );

        // The survivors' contents are intact and restorable.
        repo.restore(&remaining[0].id).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.work_tree().join("f.txt")).unwrap(),
            "new1"
        );
    }

    #[test]
    fn prune_keep_last_n_keeps_latest_and_gc_reclaims_rest() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());

        for i in 0..3 {
            std::fs::write(repo.work_tree().join("f.txt"), format!("v{i}")).unwrap();
            repo.snapshot(&format!("turn:{i}")).unwrap();
            std::thread::sleep(Duration::from_millis(1100));
        }

        assert_eq!(repo.list(usize::MAX).unwrap().len(), 3);

        let removed = repo.prune_keep_last_n(1).unwrap();
        assert_eq!(removed, 2);

        let remaining = repo.list(usize::MAX).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].label, "turn:2");

        // New snapshot starts a clean chain (not appending to old).
        std::fs::write(repo.work_tree().join("f.txt"), "fresh").unwrap();
        repo.snapshot("turn:new").unwrap();
        assert_eq!(repo.list(usize::MAX).unwrap().len(), 2);
    }

    #[test]
    fn prune_keep_last_n_preserves_multiple_snapshots_in_order() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());

        for i in 0..4 {
            std::fs::write(repo.work_tree().join("f.txt"), format!("v{i}")).unwrap();
            repo.snapshot(&format!("turn:{i}")).unwrap();
            std::thread::sleep(Duration::from_millis(1100));
        }

        assert_eq!(repo.list(usize::MAX).unwrap().len(), 4);

        let removed = repo.prune_keep_last_n(2).unwrap();
        assert_eq!(removed, 2);

        let remaining = repo.list(usize::MAX).unwrap();
        assert_eq!(remaining.len(), 2);
        // Should be newest-first: turn:3 (newest), turn:2 (second newest)
        assert_eq!(remaining[0].label, "turn:3");
        assert_eq!(remaining[1].label, "turn:2");

        // New snapshot continues the chain.
        std::fs::write(repo.work_tree().join("f.txt"), "fresh").unwrap();
        repo.snapshot("turn:new").unwrap();
        let after = repo.list(usize::MAX).unwrap();
        assert_eq!(after.len(), 3);
        assert_eq!(after[0].label, "turn:new");
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
    fn dir_size_bytes_measures_directory_bytes() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("sizedir");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        // 3 bytes per file.
        std::fs::write(dir.join("a.txt"), b"abc").unwrap();
        std::fs::write(dir.join("sub/b.txt"), b"xyz").unwrap();

        let size = dir_size_bytes(&dir).expect("dir_size_bytes");
        assert_eq!(size, 6, "two 3-byte files should measure 6 bytes");

        // Write 2 MB of data.
        let big = dir.join("big.bin");
        std::fs::write(&big, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let size = dir_size_bytes(&dir).expect("dir_size_bytes after big write");
        assert_eq!(
            size,
            2 * 1024 * 1024 + 6,
            "expected 2 MB + 6 bytes after writing a 2 MB file"
        );
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

    #[test]
    fn prune_size_pressure_counts_and_removes_history_when_over_limit() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        for i in 0..3 {
            std::fs::write(repo.work_tree().join("f.txt"), format!("v{i}")).unwrap();
            repo.snapshot(&format!("pre-turn:{i}")).expect("snapshot");
        }
        assert_eq!(repo.list(usize::MAX).unwrap().len(), 3);
        // A zero byte limit makes any non-empty side repo "over limit", so the
        // prune must run and report exactly what it destroyed. This is the S5
        // wipe path; the count is what the user-visible notice is built from.
        let removed = repo.prune_size_pressure(0, 0).expect("prune_size_pressure");
        assert_eq!(removed, 3, "every snapshot must be reported as removed");
        assert!(
            repo.list(usize::MAX).unwrap().is_empty(),
            "history should be empty after the forced wipe"
        );
    }

    #[test]
    fn prune_size_pressure_is_a_noop_under_the_limit() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        std::fs::write(repo.work_tree().join("f.txt"), b"v0").unwrap();
        repo.snapshot("pre-turn:0").expect("snapshot");
        let removed = repo
            .prune_size_pressure(u64::MAX, u64::MAX)
            .expect("prune_size_pressure");
        assert_eq!(removed, 0, "under the limit nothing may be removed");
        assert_eq!(repo.list(usize::MAX).unwrap().len(), 1);
    }

    #[test]
    fn snapshot_history_pruned_message_names_workspace_count_and_cap() {
        let msg = snapshot_history_pruned_message(Path::new("/tmp/ws"), 7);
        assert!(msg.contains("/tmp/ws"), "message must name the workspace");
        assert!(msg.contains("7"), "message must state the removed count");
        assert!(
            msg.contains(&MAX_SNAPSHOT_SIZE_MB.to_string()),
            "message must state the storage cap"
        );
    }

    #[test]
    fn estimate_workspace_size_bounded_returns_total_when_under_cap() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), vec![b'a'; 100]).unwrap();
        std::fs::write(workspace.join("b.txt"), vec![b'b'; 50]).unwrap();
        let total = estimate_workspace_size_bounded(&workspace, 10_000)
            .expect("under-cap walk must return Some");
        assert!(
            total >= 150,
            "total ({total}) must include both files (≥150 bytes)"
        );
    }

    #[test]
    fn estimate_workspace_size_bounded_returns_none_when_over_cap() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        // Two 1 KB files, cap at 1 KB — second file should trip the cap.
        std::fs::write(workspace.join("a.bin"), vec![b'a'; 1024]).unwrap();
        std::fs::write(workspace.join("b.bin"), vec![b'b'; 1024]).unwrap();
        assert!(
            estimate_workspace_size_bounded(&workspace, 1024).is_none(),
            "over-cap walk must return None for early bailout"
        );
    }

    #[test]
    fn estimate_workspace_size_bounded_skips_builtin_excluded_dirs() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(workspace.join("node_modules")).unwrap();
        std::fs::create_dir_all(workspace.join("target")).unwrap();
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        // 2 MB of "build output" in excluded dirs — must not count toward
        // the cap.
        std::fs::write(workspace.join("node_modules/big.bin"), vec![0u8; 1_000_000]).unwrap();
        std::fs::write(workspace.join("target/big.bin"), vec![0u8; 1_000_000]).unwrap();
        std::fs::write(workspace.join("src/lib.rs"), b"// real source").unwrap();
        let total = estimate_workspace_size_bounded(&workspace, 500_000)
            .expect("walk must succeed since real source is tiny");
        assert!(
            total < 1_000,
            "total ({total}) must reflect only src/, not node_modules/ or target/"
        );
    }

    #[test]
    fn estimate_workspace_size_bounded_cap_zero_disables_cap() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        // 10 KB file — would trip a 1 KB cap, but cap=0 means no cap.
        std::fs::write(workspace.join("big.bin"), vec![0u8; 10 * 1024]).unwrap();
        let total =
            estimate_workspace_size_bounded(&workspace, 0).expect("cap=0 must always return Some");
        assert!(
            total >= 10 * 1024,
            "total ({total}) must include the 10 KB file when cap is disabled"
        );
    }

    #[test]
    fn open_or_init_with_cap_rejects_oversized_workspace() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let _home = scoped_home(tmp.path());
        // Drop a 4 KB file under a 1 KB cap.
        std::fs::write(workspace.join("big.bin"), vec![0u8; 4096]).unwrap();
        let outcome = SnapshotRepo::open_or_init_with_cap(&workspace, 1024);
        let err = match outcome {
            Ok(_) => panic!("oversized workspace must fail open_or_init_with_cap"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("workspace too large for snapshots"),
            "error must call out the size cap; got: {msg}"
        );
        assert!(
            msg.contains("max_workspace_gb"),
            "error must reference the config knob users can raise; got: {msg}"
        );
    }

    #[test]
    fn open_or_init_with_cap_zero_disables_size_check() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let _home = scoped_home(tmp.path());
        // 4 KB file but cap=0 → should still succeed.
        std::fs::write(workspace.join("big.bin"), vec![0u8; 4096]).unwrap();
        let repo = SnapshotRepo::open_or_init_with_cap(&workspace, 0)
            .expect("cap=0 must skip the size check");
        let id = repo
            .snapshot("pre-turn:1")
            .expect("snapshot under disabled cap");
        assert_eq!(id.as_str().len(), 40);
    }

    #[test]
    fn session_tagged_snapshot_round_trips_through_list() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        std::fs::write(repo.work_tree().join("a.txt"), b"x").unwrap();

        repo.snapshot_with_session("pre-turn:1", Some("sess-42"))
            .expect("snapshot with session");

        let list = repo.list(10).expect("list");
        assert_eq!(list.len(), 1);
        // The visible label stays clean; the session id is decoded separately.
        assert_eq!(list[0].label, "pre-turn:1");
        assert_eq!(list[0].session_id.as_deref(), Some("sess-42"));
    }

    #[test]
    fn untagged_snapshot_decodes_without_session() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        std::fs::write(repo.work_tree().join("a.txt"), b"x").unwrap();

        repo.snapshot("pre-turn:1").expect("snapshot");

        let list = repo.list(10).expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label, "pre-turn:1");
        assert_eq!(list[0].session_id, None);
    }

    #[test]
    fn prune_keep_last_n_preserves_session_tags() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        let file = repo.work_tree().join("a.txt");

        // More snapshots than DEFAULT_MAX_SNAPSHOTS (50) so the survivor
        // chain is rebuilt as orphan commits — the path that previously
        // dropped the [sid=...] label prefix and turned every surviving
        // snapshot into a "legacy" (untagged) one.
        for i in 0..55 {
            std::fs::write(&file, format!("v{i}")).unwrap();
            repo.snapshot_with_session(&format!("pre-turn:{i}"), Some("sess-p"))
                .expect("tagged snapshot");
        }

        let removed = repo.prune_keep_last_n(50).expect("prune");
        assert!(removed > 0, "expected prune to drop older snapshots");

        let list = repo.list(usize::MAX).expect("list");
        assert_eq!(list.len(), 50);
        assert!(
            list.iter()
                .all(|s| s.session_id.as_deref() == Some("sess-p")),
            "prune must preserve [sid=...] prefixes; got untagged survivors"
        );
    }

    #[test]
    fn tagged_and_untagged_snapshots_coexist_in_one_chain() {
        let tmp = tempdir().unwrap();
        let (repo, _home) = make_repo(tmp.path());
        std::fs::write(repo.work_tree().join("a.txt"), b"v1").unwrap();

        // Legacy untagged snapshot, then a session-tagged one.
        repo.snapshot("pre-turn:1").expect("legacy snapshot");
        std::fs::write(repo.work_tree().join("a.txt"), b"v2").unwrap();
        repo.snapshot_with_session("pre-turn:1", Some("sess-a"))
            .expect("tagged snapshot");

        let list = repo.list(10).expect("list");
        assert_eq!(list.len(), 2);
        // Newest first.
        assert_eq!(list[0].session_id.as_deref(), Some("sess-a"));
        assert_eq!(list[1].session_id, None);
        assert_eq!(list[1].label, "pre-turn:1");
    }
}
