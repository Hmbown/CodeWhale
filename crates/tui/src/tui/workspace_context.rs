//! Per-workspace git context shown in the composer header.
//!
//! The TUI shows a "branch | clean/N modified/…" badge sourced from
//! `git status` and `git rev-parse`. To avoid spawning git on every
//! render, the result is cached and only refreshed every
//! `REFRESH_SECS` seconds. The refresh prefers spawn-blocking on the
//! current Tokio runtime; tests and non-async callers fall through to
//! a synchronous call.

use crate::dependencies::{ExternalTool, Git};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::tui::app::App;

/// How often (seconds) the workspace context badge is allowed to
/// re-query git. Exposed for tests that exercise the TTL.
pub(crate) const REFRESH_SECS: u64 = 15;

/// Pull a fresh workspace context from disk if the cached value is
/// older than [`REFRESH_SECS`] and `allow_refresh` is true. Always
/// drains any pending async result into `app.workspace_context` first
/// so the render pass sees the latest value (#399 S1).
pub(super) fn refresh_if_needed(app: &mut App, now: Instant, allow_refresh: bool) {
    // Drain the async cell result into the live field first, so the render
    // path always reads the latest value (#399 S1).
    if let Ok(mut cell) = app.workspace_context_cell.lock()
        && let Some(ctx) = cell.take()
    {
        if app.workspace_context.as_deref() != Some(ctx.as_str()) {
            app.needs_redraw = true;
        }
        app.workspace_context = Some(ctx);
    }

    if app
        .workspace_context_refreshed_at
        .is_some_and(|refreshed_at| {
            now.duration_since(refreshed_at) < Duration::from_secs(REFRESH_SECS)
        })
    {
        return;
    }

    if !allow_refresh {
        return;
    }

    // The Session sidebar shows the memory file's size every frame it is
    // visible. Stat it here, on the same TTL as the git context, so the draw
    // closure reads a cached string instead of issuing a syscall per frame
    // (#3908). Cheap on a local disk; tens of ms on NFS/SSHFS/cloud-synced
    // home directories, which is exactly where the stutter was reported.
    refresh_memory_size_hint(app);

    // Offload git query to a background thread when a Tokio runtime is
    // available. Fall back to synchronous execution for tests and other
    // non-async contexts (#399 S1).
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let ctx = app.workspace_context_cell.clone();
        let workspace = app.workspace.clone();
        handle.spawn_blocking(move || {
            let result = collect(&workspace);
            if let Ok(mut guard) = ctx.lock() {
                *guard = result;
            }
        });
    } else {
        // No runtime — run synchronously so tests and one-shot callers
        // still get a result immediately.
        app.workspace_context = collect(&app.workspace);
    }
    app.workspace_context_refreshed_at = Some(now);
}

/// Re-read the memory file's size into [`App::memory_size_hint`].
///
/// A missing or unreadable file renders as an em dash, matching what the
/// sidebar showed when it stat-ed inline.
fn refresh_memory_size_hint(app: &mut App) {
    let hint = if app.use_memory {
        Some(
            std::fs::metadata(&app.memory_path)
                .map(|meta| format_size(meta.len()))
                .unwrap_or_else(|_| "\u{2014}".to_string()),
        )
    } else {
        None
    };
    if app.memory_size_hint != hint {
        app.needs_redraw = true;
        app.memory_size_hint = hint;
    }
}

/// Human-readable byte size, in the exact shape the sidebar rendered inline.
fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Force a workspace-context re-query on the next render tick, bypassing the
/// normal TTL. Keeps the current value visible while the background git query
/// is running.
pub(super) fn refresh_now(app: &mut App, now: Instant) {
    if let Ok(mut cell) = app.workspace_context_cell.lock() {
        *cell = None;
    }
    app.workspace_context_refreshed_at = None;
    refresh_if_needed(app, now, true);
}

#[derive(Debug, Default, Clone, Copy)]
struct ChangeSummary {
    staged: usize,
    modified: usize,
    untracked: usize,
    conflicts: usize,
}

impl ChangeSummary {
    fn is_clean(&self) -> bool {
        self.staged == 0 && self.modified == 0 && self.untracked == 0 && self.conflicts == 0
    }
}

/// Build the human-readable workspace context string ("branch | status")
/// from `git rev-parse` + `git status`. Returns `None` if the workspace
/// is not a git repository or git itself is unavailable.
pub(crate) fn collect(workspace: &Path) -> Option<String> {
    let branch = branch(workspace)?;
    let summary = change_summary(workspace)?;

    let mut parts = Vec::new();
    if summary.staged > 0 {
        parts.push(format!("{} staged", summary.staged));
    }
    if summary.modified > 0 {
        parts.push(format!("{} modified", summary.modified));
    }
    if summary.untracked > 0 {
        parts.push(format!("{} untracked", summary.untracked));
    }
    if summary.conflicts > 0 {
        parts.push(format!("{} conflicts", summary.conflicts));
    }

    let status = if summary.is_clean() {
        "clean".to_string()
    } else {
        parts.join(", ")
    };

    Some(format!("{branch} | {status}"))
}

pub(crate) fn branch_from_context(context: &str) -> Option<&str> {
    let (branch, _) = context.rsplit_once(" | ")?;
    (!branch.is_empty()).then_some(branch)
}

/// Concise, factual workspace identity for the footer status chip (#3188).
///
/// The identity is sourced from workspace/git detection only — never from
/// model narration or config text. `name` is the workspace basename, `branch`
/// is `Some` only when the workspace is a git repository (carrying the cached
/// `"detached:<hash>"` form for detached HEAD), and `is_git` distinguishes a
/// real repo from a plain directory so the footer can show an explicit
/// non-repo state instead of an empty `Repo:` label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceIdentity {
    pub name: String,
    pub branch: Option<String>,
    pub is_git: bool,
}

/// Basename used as the workspace identity. Falls back to a stable sentinel
/// when the path has no final component (filesystem root). Derived purely
/// from the workspace path, so it never spawns git on the render path.
pub(crate) fn workspace_basename(workspace: &Path) -> String {
    workspace
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("(root)")
        .to_string()
}

/// Resolve the footer identity from the workspace path plus the cached
/// "branch | status" context string. `context` is `None` when the workspace
/// is not a git repository (or git is unavailable), which we surface as an
/// explicit non-repo state rather than hiding the chip.
pub(crate) fn identity_from_context(workspace: &Path, context: Option<&str>) -> WorkspaceIdentity {
    let branch = context.and_then(branch_from_context).map(str::to_string);
    WorkspaceIdentity {
        name: workspace_basename(workspace),
        is_git: branch.is_some(),
        branch,
    }
}

pub(super) fn branch(workspace: &Path) -> Option<String> {
    let branch = run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let branch = branch.trim().to_string();
    if branch == "HEAD" || branch.is_empty() {
        let short_hash = run_git(workspace, &["rev-parse", "--short", "HEAD"]).ok()?;
        let short_hash = short_hash.trim();
        if short_hash.is_empty() {
            return None;
        }
        return Some(format!("detached:{short_hash}"));
    }
    Some(branch)
}

fn change_summary(workspace: &Path) -> Option<ChangeSummary> {
    let status = run_git(
        workspace,
        &["status", "--short", "--untracked-files=normal"],
    )
    .ok()?;

    if status.trim().is_empty() {
        return Some(ChangeSummary::default());
    }

    let mut summary = ChangeSummary::default();
    for line in status.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let mut chars = line.chars();
        let staged = chars.next()?;
        let modified = chars.next().unwrap_or(' ');

        if staged == ' ' && modified == ' ' {
            continue;
        }
        if staged == '?' && modified == '?' {
            summary.untracked = summary.untracked.saturating_add(1);
            continue;
        }

        if staged == 'U' || modified == 'U' {
            summary.conflicts = summary.conflicts.saturating_add(1);
        }
        if staged != ' ' && staged != '?' {
            summary.staged = summary.staged.saturating_add(1);
        }
        if modified != ' ' && modified != '?' {
            summary.modified = summary.modified.saturating_add(1);
        }
    }

    Some(summary)
}

fn run_git(workspace: &Path, args: &[&str]) -> std::io::Result<String> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("--no-optional-locks");
    argv.extend_from_slice(args);
    let output = Git::output(&argv, workspace)?;
    if !output.status.success() {
        return Err(std::io::Error::other("git command failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_size_hint_is_cached_off_the_render_path() {
        // #3908: the Session sidebar rendered this by stat-ing the memory file
        // inside the draw closure, once per frame. The stat now happens here,
        // on the workspace-context TTL, so the sidebar reads a plain String.
        let dir = tempfile::tempdir().expect("temp dir");
        let memory = dir.path().join("MEMORY.md");
        std::fs::write(&memory, vec![b'x'; 2048]).unwrap();

        let mut app = crate::tui::app::App::new(
            crate::test_support::test_tui_options(dir.path()),
            &crate::config::Config::default(),
        );
        app.use_memory = true;
        app.memory_path = memory.clone();

        refresh_memory_size_hint(&mut app);
        assert_eq!(app.memory_size_hint.as_deref(), Some("2.0 KB"));

        // A file that is not there reads the same as one we cannot stat: the
        // sidebar's original em dash, not a crash or a stale number.
        std::fs::remove_file(&memory).unwrap();
        refresh_memory_size_hint(&mut app);
        assert_eq!(app.memory_size_hint.as_deref(), Some("\u{2014}"));

        // Memory off means nothing to show at all.
        app.use_memory = false;
        refresh_memory_size_hint(&mut app);
        assert_eq!(app.memory_size_hint, None);
    }

    #[test]
    fn memory_size_formats_match_the_sidebar_original() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn workspace_basename_handles_root_path() {
        assert_eq!(workspace_basename(Path::new("/")), "(root)");
        assert_eq!(workspace_basename(Path::new("/a/b/project")), "project");
    }
}
