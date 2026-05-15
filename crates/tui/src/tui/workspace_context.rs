//! Per-workspace git context shown in the composer header.
//!
//! The TUI shows a "branch | clean/N modified/…" badge sourced from
//! `git status` and `git rev-parse`. To avoid spawning git on every
//! render, the result is cached and only refreshed every
//! `REFRESH_SECS` seconds. The refresh prefers spawn-blocking on the
//! current Tokio runtime; tests and non-async callers fall through to
//! a synchronous call.

use std::path::Path;
use std::process::Command;
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
fn collect(workspace: &Path) -> Option<String> {
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
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other("git command failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
