//! Git power tools: `git_status` and `git_diff`.
//!
//! These tools are read-only wrappers around common git inspection commands,
//! scoped to the workspace and optionally to a sub-path within it.

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::dependencies::ExternalTool;

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, optional_u64,
};

const MAX_OUTPUT_CHARS: usize = 40_000;
const DEFAULT_UNIFIED: u64 = 3;
const MAX_UNIFIED: u64 = 50;

// === GitStatusTool ===

/// Tool for reading the concise git status of the workspace.
pub struct GitStatusTool;

#[async_trait]
impl ToolSpec for GitStatusTool {
    fn name(&self) -> &'static str {
        "git_status"
    }

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Run `git status --porcelain=v1 -b` in the workspace (optionally scoped to a path)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional subdirectory or file to scope the status to (must be within the workspace)."
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let git_ctx = resolve_git_context(context, optional_str(&input, "path")?)?;

        let mut args = vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "status".to_string(),
            "--porcelain=v1".to_string(),
            "-b".to_string(),
        ];
        if let Some(pathspec) = &git_ctx.pathspec {
            args.push("--".to_string());
            args.push(pathspec.display().to_string());
        }

        let command_str = format_command(&git_ctx.working_dir, &args);
        let output = run_git_command_async(git_ctx.working_dir.clone(), args).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = format!("git status failed: {}", stderr.trim());
            return Ok(ToolResult::error(message).with_metadata(json!({
                "command": command_str,
                "exit_code": output.status.code(),
                "stderr": stderr.trim(),
            })));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (content, truncated, omitted_chars) = truncate_with_note(&stdout, MAX_OUTPUT_CHARS);

        Ok(ToolResult::success(content).with_metadata(json!({
            "command": command_str,
            "working_dir": git_ctx.working_dir,
            "pathspec": git_ctx.pathspec,
            "truncated": truncated,
            "omitted_chars": omitted_chars,
        })))
    }
}

// === GitDiffTool ===

/// Tool for reading git diffs in the workspace.
pub struct GitDiffTool;

#[async_trait]
impl ToolSpec for GitDiffTool {
    fn name(&self) -> &'static str {
        "git_diff"
    }

    fn model_visible(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Run `git diff` in the workspace with sensible defaults and safe truncation."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional subdirectory or file to scope the diff to (must be within the workspace)."
                },
                "cached": {
                    "type": "boolean",
                    "description": "When true, diff staged changes (`--cached`)."
                },
                "unified": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": MAX_UNIFIED,
                    "default": DEFAULT_UNIFIED,
                    "description": "Number of context lines to include around changes."
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let git_ctx = resolve_git_context(context, optional_str(&input, "path")?)?;
        let cached = optional_bool(&input, "cached", false)?;
        let unified = optional_u64(&input, "unified", DEFAULT_UNIFIED)?.min(MAX_UNIFIED);

        let mut args = vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "diff".to_string(),
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
            format!("--unified={unified}"),
        ];
        if cached {
            args.push("--cached".to_string());
        }
        if let Some(pathspec) = &git_ctx.pathspec {
            args.push("--".to_string());
            args.push(pathspec.display().to_string());
        }

        let command_str = format_command(&git_ctx.working_dir, &args);
        let output = run_git_command_async(git_ctx.working_dir.clone(), args).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = format!("git diff failed: {}", stderr.trim());
            return Ok(ToolResult::error(message).with_metadata(json!({
                "command": command_str,
                "exit_code": output.status.code(),
                "stderr": stderr.trim(),
            })));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (content, truncated, omitted_chars) = truncate_with_note(&stdout, MAX_OUTPUT_CHARS);

        Ok(ToolResult::success(content).with_metadata(json!({
            "command": command_str,
            "working_dir": git_ctx.working_dir,
            "pathspec": git_ctx.pathspec,
            "cached": cached,
            "unified": unified,
            "truncated": truncated,
            "omitted_chars": omitted_chars,
        })))
    }
}

// === Helpers ===

struct GitContext {
    working_dir: PathBuf,
    pathspec: Option<PathBuf>,
}

fn resolve_git_context(context: &ToolContext, path: Option<&str>) -> Result<GitContext, ToolError> {
    let workspace = canonical_or_workspace(&context.workspace);
    let mut working_dir = workspace.clone();
    let mut pathspec = None;

    if let Some(raw) = path {
        let resolved = context.resolve_path(raw)?;
        let metadata = fs::metadata(&resolved).map_err(|e| {
            ToolError::invalid_input(format!(
                "Path does not exist or is not accessible: {raw} ({e})"
            ))
        })?;

        if metadata.is_dir() {
            working_dir = resolved;
            pathspec = Some(PathBuf::from("."));
        } else {
            // For file paths, run from the parent and scope to the file name.
            let parent = resolved.parent().ok_or_else(|| {
                ToolError::invalid_input(format!("Path has no parent directory: {raw}"))
            })?;
            working_dir = parent.to_path_buf();
            pathspec = Some(pathspec_from(&working_dir, &resolved));
        }
    }

    if !working_dir.exists() {
        return Err(ToolError::invalid_input(format!(
            "Working directory does not exist: {}",
            working_dir.display()
        )));
    }

    Ok(GitContext {
        working_dir,
        pathspec,
    })
}

fn canonical_or_workspace(workspace: &Path) -> PathBuf {
    workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf())
}

fn pathspec_from(working_dir: &Path, resolved: &Path) -> PathBuf {
    match resolved.strip_prefix(working_dir) {
        Ok(rel) if rel.as_os_str().is_empty() => PathBuf::from("."),
        Ok(rel) => rel.to_path_buf(),
        Err(_) => PathBuf::from("."),
    }
}

fn run_git_command(working_dir: &Path, args: &[String]) -> Result<std::process::Output, ToolError> {
    let Some(mut cmd) = crate::dependencies::Git::command() else {
        return Err(ToolError::not_available(
            "git is not installed or not in PATH",
        ));
    };
    cmd.args(args).current_dir(working_dir);
    cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ToolError::not_available("git is not installed or not in PATH")
        } else {
            ToolError::execution_failed(format!("Failed to run git: {e}"))
        }
    })
}

/// Offloads the blocking `git` invocation onto the blocking pool so the
/// async worker is never stalled.
///
/// This is not cosmetic. Both tools here declare `supports_parallel() ==
/// true`, and the engine drains an advertised-parallel batch from a single
/// task (`crates/tui/src/core/engine/tool_execution.rs`), so an inline
/// blocking spawn inside `execute` parks the worker and serializes the
/// *entire* batch, not just this tool (#5616, reported by
/// @rafaelcavalheri). `Git::command()` resolution — including its one-time
/// synchronous `git --version` probe (`OnceLock`-cached after the first
/// call) — happens inside [`run_git_command`], so the offload keeps that
/// first-call probe off the worker too.
///
/// The wrapper must keep routing through [`run_git_command`]: that is what
/// attaches `GIT_OPTIONAL_LOCKS=0` from `Git::command()` (#5617, b9fd28367)
/// and the `not_available` error mapping. Do not rewrite it to spawn git
/// directly; `readonly_tools_do_not_rewrite_the_users_index` below pins
/// this end to end. Character-identical twin in `git_history.rs`
/// (346bfe3b6).
async fn run_git_command_async(
    working_dir: PathBuf,
    args: Vec<String>,
) -> Result<std::process::Output, ToolError> {
    tokio::task::spawn_blocking(move || run_git_command(&working_dir, &args))
        .await
        .map_err(|e| ToolError::execution_failed(format!("git task panicked: {e}")))?
}

fn format_command(working_dir: &Path, args: &[String]) -> String {
    // `[String]::join` produces the same string as collecting `&str` first, so
    // join the slice directly and skip the intermediate `Vec<&str>` allocation.
    format!("git -C {} {}", working_dir.display(), args.join(" "))
}

fn truncate_with_note(text: &str, max_chars: usize) -> (String, bool, usize) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false, 0);
    }
    let end = char_boundary_index(text, max_chars);
    let truncated = &text[..end];
    let omitted_chars = text
        .chars()
        .count()
        .saturating_sub(truncated.chars().count());
    let note = format!(
        "\n\n[output truncated to {max_chars} characters; {omitted_chars} characters omitted]"
    );
    (format!("{truncated}{note}"), true, omitted_chars)
}

fn char_boundary_index(text: &str, max_chars: usize) -> usize {
    if max_chars == 0 {
        return 0;
    }
    for (count, (idx, _)) in text.char_indices().enumerate() {
        if count == max_chars {
            return idx;
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn git_available() -> bool {
        crate::dependencies::Git::available()
    }

    fn init_git_repo(root: &Path) {
        let run = |args: &[&str]| {
            let status = crate::dependencies::Git::status(args, root).expect("git should spawn");
            assert!(status.success(), "git {args:?} failed");
        };

        run(&["init", "-q"]);
        run(&["config", "core.autocrlf", "false"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test User"]);
    }

    fn commit_all(root: &Path, message: &str) {
        let run = |args: &[&str]| {
            let status = crate::dependencies::Git::status(args, root).expect("git should spawn");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["add", "."]);
        run(&["commit", "-q", "-m", message]);
    }

    #[tokio::test]
    async fn git_status_reports_branch_and_changes() {
        if !git_available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let file = tmp.path().join("file.txt");
        fs::write(&file, "hello\n").expect("write");
        commit_all(tmp.path(), "init");

        fs::write(&file, "hello\nworld\n").expect("modify");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitStatusTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");
        assert!(result.success);
        assert!(result.content.contains("##"));
        assert!(result.content.contains("file.txt"));
    }

    #[tokio::test]
    async fn git_status_reports_unquoted_unicode_paths() {
        if !git_available() {
            return;
        }

        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let file = tmp.path().join("中文-данные.txt");
        fs::write(&file, "hello\n").expect("write");
        commit_all(tmp.path(), "init");

        fs::write(&file, "hello\nworld\n").expect("modify");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitStatusTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");
        assert!(result.success);
        assert!(
            result
                .metadata
                .as_ref()
                .and_then(|m| m.get("command"))
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains("-c core.quotepath=false"))
        );
        assert!(result.content.contains("中文-данные.txt"));
        assert!(!result.content.contains("\\344"));
        assert!(!result.content.contains("\\320"));
    }

    #[tokio::test]
    async fn git_diff_supports_cached_and_path_scoping() {
        if !git_available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let subdir = tmp.path().join("src");
        fs::create_dir_all(&subdir).expect("mkdir");
        let file = subdir.join("lib.rs");
        fs::write(&file, "pub fn one() -> i32 { 1 }\n").expect("write");
        commit_all(tmp.path(), "init");

        fs::write(&file, "pub fn one() -> i32 { 2 }\n").expect("modify");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitDiffTool;

        let uncached = tool
            .execute(json!({ "path": "src" }), &ctx)
            .await
            .expect("diff");
        assert!(uncached.success);
        assert!(uncached.content.contains("diff --git"));
        assert!(uncached.content.contains("lib.rs"));

        let _ =
            crate::dependencies::Git::status(&["add", "src/lib.rs"], tmp.path()).expect("git add");

        let cached = tool
            .execute(json!({ "path": "src", "cached": true }), &ctx)
            .await
            .expect("diff cached");
        assert!(cached.success);
        assert!(cached.content.contains("diff --git"));
        assert!(
            cached
                .metadata
                .as_ref()
                .and_then(|m| m.get("cached"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn git_diff_reports_unquoted_unicode_paths() {
        if !git_available() {
            return;
        }

        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let unicode_name = "\u{4e2d}\u{6587}-\u{0434}\u{0430}\u{043d}\u{043d}\u{044b}\u{0435}.txt";
        let file = tmp.path().join(unicode_name);
        fs::write(&file, "hello\n").expect("write");
        commit_all(tmp.path(), "init");

        fs::write(&file, "hello\nworld\n").expect("modify");

        let ctx = ToolContext::new(tmp.path());
        let tool = GitDiffTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");

        assert!(result.success);
        assert!(
            result
                .metadata
                .as_ref()
                .and_then(|m| m.get("command"))
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains("-c core.quotepath=false"))
        );
        assert!(result.content.contains(unicode_name));
        assert!(!result.content.contains("\\344"));
        assert!(!result.content.contains("\\320"));
    }

    #[tokio::test]
    async fn async_offload_matches_the_sync_path_byte_for_byte() {
        if !git_available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let file = tmp.path().join("file.txt");
        fs::write(&file, "hello\n").expect("write");
        commit_all(tmp.path(), "init");
        fs::write(&file, "hello\nworld\n").expect("modify");

        let args = vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "status".to_string(),
            "--porcelain=v1".to_string(),
            "-b".to_string(),
        ];
        let offloaded = run_git_command_async(tmp.path().to_path_buf(), args.clone())
            .await
            .expect("async git");
        let sync = run_git_command(tmp.path(), &args).expect("sync git");
        assert_eq!(offloaded.status.code(), sync.status.code());
        assert_eq!(offloaded.stdout, sync.stdout);
        assert_eq!(offloaded.stderr, sync.stderr);
    }

    #[tokio::test]
    async fn readonly_tools_do_not_rewrite_the_users_index() {
        if !git_available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        // ~200 stat-dirty files: the measured threshold at which an
        // opportunistic `git status` refresh actually rewrites .git/index
        // (b9fd28367). Fewer can leave the refresh too cheap to bother.
        for i in 0..200 {
            fs::write(
                tmp.path().join(format!("f{i}.rs")),
                "pub fn one() -> i32 { 1 }\n",
            )
            .expect("write");
        }
        commit_all(tmp.path(), "init");
        // Rewrite identical bytes: content stays clean, mtimes move, the
        // index goes stat-dirty — the state whose opportunistic refresh is
        // what takes `.git/index.lock` in the user's repo.
        let retouch_all = || {
            for i in 0..200 {
                fs::write(
                    tmp.path().join(format!("f{i}.rs")),
                    "pub fn one() -> i32 { 1 }\n",
                )
                .expect("retouch");
            }
        };
        retouch_all();

        // Teeth check: a raw unlocked `git status` — exactly what a refactor
        // that bypasses `Git::command()` would produce — must rewrite the
        // index on this fixture, or the lock below is vacuous. If this git
        // no longer writes opportunistically, say so and skip.
        let index = tmp.path().join(".git").join("index");
        let index_mtime = || {
            fs::metadata(&index)
                .expect("index exists")
                .modified()
                .expect("index mtime")
        };
        let before_raw = index_mtime();
        let unlocked = std::process::Command::new("git")
            .args([
                "-c",
                "core.quotepath=false",
                "status",
                "--porcelain=v1",
                "-b",
            ])
            .current_dir(tmp.path())
            .output()
            .expect("raw git status");
        assert!(unlocked.status.success());
        if index_mtime() == before_raw {
            // This git no longer rewrites the index opportunistically; the
            // no-rewrite assertion below would be vacuous, so skip rather
            // than pin platform-specific behavior the fix does not rely on.
            return;
        }

        // Re-dirty (the raw run refreshed the index) and prove both tools
        // leave it byte-identical through the async offload.
        retouch_all();
        let before_tools = index_mtime();
        let ctx = ToolContext::new(tmp.path());
        let status = GitStatusTool
            .execute(json!({}), &ctx)
            .await
            .expect("status");
        assert!(status.success);
        let diff = GitDiffTool.execute(json!({}), &ctx).await.expect("diff");
        assert!(diff.success);
        assert_eq!(
            index_mtime(),
            before_tools,
            "git_status/git_diff must not rewrite .git/index: the async offload \
             has to keep routing through run_git_command -> Git::command() so \
             GIT_OPTIONAL_LOCKS=0 stays attached (#5617, b9fd28367)"
        );
    }

    #[test]
    fn format_command_joins_args_without_intermediate_vec() {
        // Locks the output shape after dropping the collect-before-join
        // allocation: joining the `&[String]` slice directly must be byte-for-byte
        // identical to the previous `.map(String::as_str).collect().join(" ")`.
        let args = vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "status".to_string(),
            "--porcelain=v1".to_string(),
            "-b".to_string(),
        ];
        let rendered = format_command(Path::new("/tmp/repo"), &args);
        assert_eq!(
            rendered,
            "git -C /tmp/repo -c core.quotepath=false status --porcelain=v1 -b"
        );

        // Empty args still render cleanly (trailing space, matching prior behavior).
        assert_eq!(
            format_command(Path::new("/tmp/repo"), &[]),
            "git -C /tmp/repo "
        );
    }

    #[test]
    fn truncation_adds_note() {
        let long = "a".repeat(MAX_OUTPUT_CHARS + 100);
        let (truncated, did_truncate, omitted) = truncate_with_note(&long, MAX_OUTPUT_CHARS);
        assert!(did_truncate);
        assert!(omitted > 0);
        assert!(truncated.contains("output truncated"));
    }
}
