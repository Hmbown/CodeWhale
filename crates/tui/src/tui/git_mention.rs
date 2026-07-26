//! `@git` and `@diff` composer mentions (#4067).
//!
//! The `@` mention system is otherwise path-centric: every token resolves to
//! a file or directory. These two tokens resolve to *curated git context*
//! instead, so a user can attach "what is going on in this working tree"
//! inline rather than making the model spend a round-trip on `git_diff` or a
//! shell command that may need approval.
//!
//! Two deliberate boundaries:
//!
//! * **Read-only and bounded.** Only `git status` and `git diff` run, always
//!   with an explicit byte budget. A repository with a huge working-tree diff
//!   truncates with a visible marker rather than flooding the turn.
//! * **Honest when unavailable.** No git binary, or a directory that is not a
//!   repository, produces an explicit `<git-unavailable>` block. A mention
//!   never silently contributes nothing.

use std::path::Path;

use crate::dependencies::{ExternalTool, Git};

/// Byte ceiling for the inlined `@diff` payload. Documented here because the
/// context inspector reports the budget alongside actual size.
pub const MAX_GIT_DIFF_MENTION_BYTES: usize = 32 * 1024;
/// Byte ceiling for the inlined `@git` status summary. Status output is
/// bounded in practice, but an unignored `node_modules` can still produce
/// tens of thousands of lines.
pub const MAX_GIT_STATUS_MENTION_BYTES: usize = 8 * 1024;

/// Which curated git payload a mention token asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitMentionKind {
    /// `@git` — a bounded `git status` summary plus the current branch.
    Status,
    /// `@diff` — the working-tree diff, staged and unstaged.
    Diff,
}

impl GitMentionKind {
    /// The mention token that selects this payload, without the `@`.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::Status => "git",
            Self::Diff => "diff",
        }
    }

    /// Byte budget for the inlined payload.
    #[must_use]
    pub fn byte_budget(self) -> usize {
        match self {
            Self::Status => MAX_GIT_STATUS_MENTION_BYTES,
            Self::Diff => MAX_GIT_DIFF_MENTION_BYTES,
        }
    }

    /// Every git mention kind, in completion-menu order.
    pub fn iter_all() -> impl Iterator<Item = Self> {
        GIT_MENTION_KINDS.into_iter()
    }

    /// Short label for composer previews and the context inspector.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Status => "git status",
            Self::Diff => "working-tree diff",
        }
    }
}

/// Every git mention token, in completion-menu order.
pub const GIT_MENTION_KINDS: [GitMentionKind; 2] = [GitMentionKind::Status, GitMentionKind::Diff];

/// Classify a raw mention token. Case-insensitive so `@Git` and `@Diff`
/// behave like the lowercase spellings; a path that merely *starts* with
/// `git` (`@git/config`, `@diff.txt`) stays a file mention.
#[must_use]
pub fn git_mention_kind(raw: &str) -> Option<GitMentionKind> {
    let token = raw.trim();
    GIT_MENTION_KINDS
        .into_iter()
        .find(|kind| token.eq_ignore_ascii_case(kind.token()))
}

/// Outcome of resolving a git mention against a working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitMentionPayload {
    /// The model-facing block, already wrapped in its tag.
    pub block: String,
    /// Byte size of the payload actually inlined (the tag itself excluded).
    pub bytes: usize,
    /// Whether the payload hit its budget and was cut.
    pub truncated: bool,
    /// Present when git could not produce the payload at all.
    pub unavailable_reason: Option<String>,
}

impl GitMentionPayload {
    fn unavailable(kind: GitMentionKind, reason: &str) -> Self {
        Self {
            block: format!(
                "<git-unavailable mention=\"@{token}\" reason=\"{reason}\" />",
                token = kind.token(),
            ),
            bytes: 0,
            truncated: false,
            unavailable_reason: Some(reason.to_string()),
        }
    }
}

/// Run the git commands for `kind` in `cwd` and render the model-facing block.
///
/// Never returns an error: an unavailable git, a non-repository directory, or
/// a failing command all resolve to an explicit `<git-unavailable>` block so
/// the turn records why the mention contributed nothing.
#[must_use]
pub fn resolve_git_mention(kind: GitMentionKind, cwd: &Path) -> GitMentionPayload {
    if Git::command().is_none() {
        return GitMentionPayload::unavailable(kind, "git not found on PATH");
    }
    if !is_git_repository(cwd) {
        return GitMentionPayload::unavailable(kind, "not a git repository");
    }

    let raw = match kind {
        GitMentionKind::Status => git_status_payload(cwd),
        GitMentionKind::Diff => git_output(&["diff", "HEAD"], cwd),
    };
    let Some(raw) = raw else {
        return GitMentionPayload::unavailable(kind, "git command failed");
    };

    if raw.trim().is_empty() {
        let reason = match kind {
            GitMentionKind::Status => "working tree clean",
            GitMentionKind::Diff => "no working-tree changes",
        };
        return GitMentionPayload::unavailable(kind, reason);
    }

    let (body, truncated) = truncate_on_char_boundary(&raw, kind.byte_budget());
    let tag = match kind {
        GitMentionKind::Status => "git-status",
        GitMentionKind::Diff => "git-diff",
    };
    let truncated_attr = if truncated {
        format!(
            " truncated=\"true\" budget-bytes=\"{}\"",
            kind.byte_budget()
        )
    } else {
        String::new()
    };
    let block = format!(
        "<{tag} mention=\"@{token}\" bytes=\"{bytes}\"{truncated_attr}>\n{body}\n</{tag}>",
        token = kind.token(),
        bytes = body.len(),
    );

    GitMentionPayload {
        block,
        bytes: body.len(),
        truncated,
        unavailable_reason: None,
    }
}

/// `git status` plus the branch line, so the model does not have to infer the
/// branch from a porcelain listing.
fn git_status_payload(cwd: &Path) -> Option<String> {
    let status = git_output(&["status", "--short", "--branch"], cwd)?;
    Some(status)
}

/// True when `cwd` is inside a git work tree.
fn is_git_repository(cwd: &Path) -> bool {
    git_output(&["rev-parse", "--is-inside-work-tree"], cwd).is_some_and(|out| out.trim() == "true")
}

/// Run git and return stdout, or `None` when the binary is missing or the
/// command exits non-zero.
fn git_output(args: &[&str], cwd: &Path) -> Option<String> {
    let output = Git::output(args, cwd).ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Cut `text` to at most `budget` bytes without splitting a UTF-8 scalar.
/// Returns the slice and whether anything was dropped.
fn truncate_on_char_boundary(text: &str, budget: usize) -> (&str, bool) {
    if text.len() <= budget {
        return (text, false);
    }
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo(dir: &Path) {
        for args in [
            vec!["init", "--initial-branch=main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
        ] {
            let status = Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .expect("git available in tests");
            assert!(status.status.success(), "git {args:?} failed");
        }
    }

    fn commit_all(dir: &Path, message: &str) {
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn only_exact_tokens_are_git_mentions() {
        assert_eq!(git_mention_kind("git"), Some(GitMentionKind::Status));
        assert_eq!(git_mention_kind("Diff"), Some(GitMentionKind::Diff));
        // Paths that merely start with the token stay file mentions.
        assert_eq!(git_mention_kind("git/config"), None);
        assert_eq!(git_mention_kind("diff.txt"), None);
        assert_eq!(git_mention_kind("gitignore"), None);
    }

    #[test]
    fn non_repository_directory_is_explicitly_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let payload = resolve_git_mention(GitMentionKind::Diff, dir.path());
        assert_eq!(payload.bytes, 0);
        assert!(payload.block.contains("git-unavailable"));
        assert!(
            payload
                .unavailable_reason
                .as_deref()
                .is_some_and(|r| r.contains("not a git repository")),
            "unexpected reason: {:?}",
            payload.unavailable_reason
        );
    }

    #[test]
    fn empty_repository_reports_clean_rather_than_an_empty_block() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        commit_all(dir.path(), "initial");

        let payload = resolve_git_mention(GitMentionKind::Diff, dir.path());
        assert_eq!(payload.bytes, 0);
        assert!(payload.block.contains("no working-tree changes"));
    }

    #[test]
    fn status_reports_branch_and_dirty_paths() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        commit_all(dir.path(), "initial");
        std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();

        let payload = resolve_git_mention(GitMentionKind::Status, dir.path());
        assert!(payload.unavailable_reason.is_none());
        assert!(payload.block.starts_with("<git-status mention=\"@git\""));
        assert!(payload.block.contains("b.txt"), "{}", payload.block);
        assert!(!payload.truncated);
    }

    #[test]
    fn diff_covers_staged_and_unstaged_changes() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "one\n").unwrap();
        commit_all(dir.path(), "initial");

        std::fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
        Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join("b.txt"), "unstaged\n").unwrap();

        let payload = resolve_git_mention(GitMentionKind::Diff, dir.path());
        assert!(payload.unavailable_reason.is_none());
        assert!(payload.block.contains("staged"), "{}", payload.block);
        assert!(payload.block.contains("unstaged"), "{}", payload.block);
    }

    #[test]
    fn large_diff_truncates_at_the_documented_budget() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("big.txt"), "seed\n").unwrap();
        commit_all(dir.path(), "initial");

        let bulk: String = (0..40_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join("big.txt"), bulk).unwrap();

        let payload = resolve_git_mention(GitMentionKind::Diff, dir.path());
        assert!(payload.truncated, "expected truncation");
        assert!(payload.bytes <= MAX_GIT_DIFF_MENTION_BYTES);
        assert!(payload.block.contains("truncated=\"true\""));
        assert!(payload.block.contains("budget-bytes=\"32768\""));
    }

    #[test]
    fn truncation_never_splits_a_utf8_scalar() {
        // Budget lands mid-scalar: "é" is two bytes starting at index 1.
        let (cut, truncated) = truncate_on_char_boundary("aéb", 2);
        assert!(truncated);
        assert_eq!(cut, "a");
    }
}
