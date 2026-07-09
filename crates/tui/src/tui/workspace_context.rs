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
use unicode_width::UnicodeWidthStr;

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
/// "detached:<hash>" form for detached HEAD), and `is_git` distinguishes a
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

/// Render the footer repo label, keeping the most useful identity when width
/// is constrained (#3188 acceptance criteria). Layout priority, widest first:
///
/// 1. `Repo: <name> @ <branch>` (git repo, room for both)
/// 2. `Repo: <name>` (drop the branch before truncating the name)
/// 3. `Repo: <truncated name…>` then the bare label when truly tiny
///
/// Non-git workspaces render `Repo: <name> (no git)`, degrading to
/// `Repo: <name>` and then truncation under width pressure. Returns an empty
/// string only when `max_width` cannot fit even the `Repo:` prefix.
pub(crate) fn format_repo_identity(identity: &WorkspaceIdentity, max_width: usize) -> String {
    use crate::localization::truncate_to_width;

    const PREFIX: &str = "Repo: ";
    let prefix_width = PREFIX.width();
    if max_width < prefix_width {
        return String::new();
    }

    // Candidates from richest to leanest; the first that fits wins.
    let mut candidates: Vec<String> = Vec::new();
    match (&identity.branch, identity.is_git) {
        (Some(branch), _) => {
            candidates.push(format!("{PREFIX}{} @ {branch}", identity.name));
            candidates.push(format!("{PREFIX}{}", identity.name));
        }
        (None, _) => {
            candidates.push(format!("{PREFIX}{} (no git)", identity.name));
            candidates.push(format!("{PREFIX}{}", identity.name));
        }
    }

    for candidate in &candidates {
        if candidate.width() <= max_width {
            return candidate.clone();
        }
    }

    // Even the lean form overflows: keep the prefix + a truncated name so the
    // identity never collapses into a bare, useless `Repo:` label.
    truncate_to_width(&format!("{PREFIX}{}", identity.name), max_width)
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
    let output = Git::output(args, workspace)?;
    if !output.status.success() {
        return Err(std::io::Error::other("git command failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn identity_in_git_repo_carries_name_and_branch() {
        let id = identity_from_context(
            &PathBuf::from("/work/CodeWhale"),
            Some("codex/v0.8.61 | 3 modified"),
        );
        assert_eq!(id.name, "CodeWhale");
        assert_eq!(id.branch.as_deref(), Some("codex/v0.8.61"));
        assert!(id.is_git);
        // Full-width render keeps both the repo identity and the branch.
        assert_eq!(
            format_repo_identity(&id, 80),
            "Repo: CodeWhale @ codex/v0.8.61"
        );
    }

    #[test]
    fn identity_outside_git_uses_cwd_basename_with_explicit_state() {
        // `None` context == not a git repo / git unavailable. We must not show
        // a stale repo, but we also must not collapse to an empty `Repo:`.
        let id = identity_from_context(&PathBuf::from("/tmp/scratch-dir"), None);
        assert_eq!(id.name, "scratch-dir");
        assert_eq!(id.branch, None);
        assert!(!id.is_git);
        assert_eq!(format_repo_identity(&id, 80), "Repo: scratch-dir (no git)");
    }

    #[test]
    fn detached_head_branch_passes_through_to_label() {
        // `branch()` encodes detached HEAD as "detached:<hash>"; the footer
        // must surface that verbatim rather than dropping the identity.
        let id = identity_from_context(
            &PathBuf::from("/work/CodeWhale"),
            Some("detached:ae101a1 | clean"),
        );
        assert_eq!(id.branch.as_deref(), Some("detached:ae101a1"));
        assert_eq!(
            format_repo_identity(&id, 80),
            "Repo: CodeWhale @ detached:ae101a1"
        );
    }

    #[test]
    fn narrow_width_keeps_identity_over_branch_then_truncates() {
        let id = identity_from_context(
            &PathBuf::from("/work/CodeWhale"),
            Some("codex/v0.8.61 | clean"),
        );

        // Too narrow for "name @ branch" -> drop the branch, keep the name.
        let dropped = format_repo_identity(&id, 20);
        assert_eq!(dropped, "Repo: CodeWhale");
        assert!(dropped.width() <= 20);

        // Too narrow even for the name -> truncate but keep the prefix so the
        // chip never becomes a bare, useless "Repo:" label.
        let truncated = format_repo_identity(&id, 11);
        assert!(truncated.width() <= 11, "{truncated:?} must fit width 11");
        assert!(truncated.starts_with("Repo: "), "{truncated:?}");
        assert!(truncated.ends_with('…'), "{truncated:?}");

        // Below the bare "Repo:" prefix -> render nothing so the footer hides
        // the chip cleanly instead of printing garbage.
        assert_eq!(format_repo_identity(&id, 3), "");
    }

    #[test]
    fn non_git_identity_degrades_before_truncating() {
        let id = identity_from_context(&PathBuf::from("/tmp/scratch-dir"), None);
        // No room for the "(no git)" suffix -> fall back to just the name.
        assert_eq!(format_repo_identity(&id, 18), "Repo: scratch-dir");
    }

    #[test]
    fn workspace_basename_handles_root_path() {
        assert_eq!(workspace_basename(Path::new("/")), "(root)");
        assert_eq!(workspace_basename(Path::new("/a/b/project")), "project");
    }

    #[test]
    fn collect_and_identity_agree_on_a_real_repo() {
        // Real-git integration: in an actual worktree, `collect()` yields a
        // "branch | status" string and `identity_from_context` must read a
        // git identity back out of it. Skipped when git is unavailable
        // (mirrors dependencies::external_tool_output_respects_cwd).
        if !Git::available() {
            return;
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // `git init` so the directory is a real repo with a HEAD.
        let init = Git::output(&["init", "-q"], root);
        if init.is_err() || !init.unwrap().status.success() {
            return; // hermetic CI without writable git config: skip.
        }
        let _ = Git::output(&["config", "user.email", "t@example.com"], root);
        let _ = Git::output(&["config", "user.name", "Test"], root);

        match collect(root) {
            Some(ctx) => {
                let id = identity_from_context(root, Some(ctx.as_str()));
                assert!(id.is_git, "fresh repo should detect a git identity");
                assert!(id.branch.is_some(), "repo must report a branch/HEAD");
                let label = format_repo_identity(&id, 80);
                assert!(label.starts_with("Repo: "), "{label:?}");
            }
            None => {
                // Some sandboxes report no branch on an empty repo; the
                // non-git fallback must still produce a usable label.
                let id = identity_from_context(root, None);
                assert!(!id.is_git);
                assert!(format_repo_identity(&id, 80).starts_with("Repo: "));
            }
        }
    }
}
