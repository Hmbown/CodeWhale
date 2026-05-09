use super::*;

use crate::tools::spec::ToolContext;
use serde_json::{Value, json};
use tempfile::tempdir;

// `env_lock` exists only to serialize Unix-only env-mutating tests.
// Windows builds gate that test out, so the helper would be dead code
// under `-Dwarnings` if the import + helper were unconditional.
#[cfg(unix)]
use std::sync::{Mutex, OnceLock};

#[cfg(unix)]
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn echo_command(message: &str) -> String {
    format!("echo {message}")
}

fn sleep_command(seconds: u64) -> String {
    #[cfg(windows)]
    {
        let ping_count = seconds.saturating_add(1);
        let ps_path = r#"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"#;
        format!(
            "\"{ps_path}\" -NoProfile -Command \"Start-Sleep -Seconds {seconds}\" || ping 127.0.0.1 -n {ping_count} > NUL"
        )
    }
    #[cfg(not(windows))]
    {
        format!("sleep {seconds}")
    }
}

fn sleep_then_echo_command(seconds: u64, message: &str) -> String {
    #[cfg(windows)]
    {
        let ping_count = seconds.saturating_add(1);
        let ps_path = r#"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"#;
        format!(
            "\"{ps_path}\" -NoProfile -Command \"Start-Sleep -Seconds {seconds}; Write-Output {message}\" || (ping 127.0.0.1 -n {ping_count} > NUL && echo {message})"
        )
    }
    #[cfg(not(windows))]
    {
        format!("sleep {seconds} && echo {message}")
    }
}

fn echo_stdin_command() -> String {
    #[cfg(windows)]
    {
        "more".to_string()
    }
    #[cfg(not(windows))]
    {
        "cat".to_string()
    }
}

fn network_restricted_context(tmp: &std::path::Path) -> ToolContext {
    ToolContext::new(tmp)
        .with_elevated_sandbox_policy(ExecutionSandboxPolicy::WorkspaceWrite {
            writable_roots: vec![tmp.to_path_buf()],
            network_access: false,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        })
        .with_shell_network_denied_hint(
            "Shell command blocked: Plan mode runs shell commands in a network-restricted sandbox.",
        )
}

fn failed_network_shell_result(stdout: &str, stderr: &str) -> ShellResult {
    ShellResult {
        task_id: None,
        status: ShellStatus::Failed,
        exit_code: Some(6),
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        duration_ms: 25,
        stdout_len: stdout.len(),
        stderr_len: stderr.len(),
        stdout_omitted: 0,
        stderr_omitted: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        sandboxed: true,
        sandbox_type: Some("seatbelt".to_string()),
        sandbox_denied: false,
    }
}

#[test]
#[cfg(unix)]
fn shell_execution_scrubs_parent_env_and_keeps_explicit_env() {
    let _guard = env_lock().lock().expect("env lock");
    let previous = std::env::var_os("DEEPSEEK_CHILD_ENV_SHELL_SECRET");
    unsafe {
        std::env::set_var("DEEPSEEK_CHILD_ENV_SHELL_SECRET", "parent-secret");
    }

    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());
    let mut extra = std::collections::HashMap::new();
    extra.insert(
        "DEEPSEEK_CHILD_ENV_EXPLICIT".to_string(),
        "explicit-value".to_string(),
    );

    let result = manager
        .execute_with_options_env(
            "printf '%s\\n%s\\n' \"${DEEPSEEK_CHILD_ENV_SHELL_SECRET-unset}\" \"${DEEPSEEK_CHILD_ENV_EXPLICIT-unset}\"",
            None,
            5000,
            false,
            None,
            false,
            None,
            extra,
        )
        .expect("execute");

    match previous {
        Some(value) => unsafe {
            std::env::set_var("DEEPSEEK_CHILD_ENV_SHELL_SECRET", value);
        },
        None => unsafe {
            std::env::remove_var("DEEPSEEK_CHILD_ENV_SHELL_SECRET");
        },
    }

    assert_eq!(result.status, ShellStatus::Completed);
    assert_eq!(result.stdout, "unset\nexplicit-value\n");
}

#[test]
fn test_sync_execution() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result = manager
        .execute(&echo_command("hello"), None, 5000, false)
        .expect("execute");

    assert_eq!(result.status, ShellStatus::Completed);
    assert!(result.stdout.contains("hello"));
    assert!(result.task_id.is_none());
}

#[test]
fn test_background_execution() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result = manager
        .execute(&sleep_then_echo_command(1, "done"), None, 5000, true)
        .expect("execute");

    assert_eq!(result.status, ShellStatus::Running);
    assert!(result.task_id.is_some());

    let task_id = result
        .task_id
        .expect("background execution should return task_id");

    // Wait for completion
    let final_result = manager
        .get_output(&task_id, true, 5000)
        .expect("get_output");

    assert_eq!(final_result.status, ShellStatus::Completed);
    assert!(final_result.stdout.contains("done"));
}

#[test]
fn test_timeout() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result = manager
        .execute(&sleep_command(10), None, 1000, false)
        .expect("execute");

    assert_eq!(result.status, ShellStatus::TimedOut);
}

#[test]
fn test_kill() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result = manager
        .execute(&sleep_command(60), None, 5000, true)
        .expect("execute");

    let task_id = result
        .task_id
        .expect("background execution should return task_id");

    // Kill it
    let killed = manager.kill(&task_id).expect("kill");
    assert_eq!(killed.status, ShellStatus::Killed);
}

#[test]
fn test_write_stdin_streams_output() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result = manager
        .execute_with_options(&echo_stdin_command(), None, 5000, true, None, false, None)
        .expect("execute");

    let task_id = result
        .task_id
        .expect("background execution should return task_id");

    manager
        .write_stdin(&task_id, "hello\n", true)
        .expect("write stdin");

    let delta = manager
        .get_output_delta(&task_id, true, 5000)
        .expect("get_output_delta");

    assert!(delta.result.stdout.contains("hello"));

    let delta2 = manager
        .get_output_delta(&task_id, false, 0)
        .expect("get_output_delta");
    assert!(delta2.result.stdout.is_empty());
}

#[test]
fn test_job_list_poll_cancel_and_stale_snapshot() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let started = manager
        .execute(&sleep_then_echo_command(1, "done"), None, 5000, true)
        .expect("execute");
    let task_id = started.task_id.expect("task id");
    manager
        .tag_linked_task(&task_id, Some("task_123".to_string()))
        .expect("tag linked task");

    let running = manager.list_jobs();
    let job = running
        .iter()
        .find(|job| job.id == task_id)
        .expect("running job");
    assert_eq!(job.status, ShellStatus::Running);
    assert_eq!(job.linked_task_id.as_deref(), Some("task_123"));
    assert!(job.command.contains("done"));
    assert_eq!(job.cwd, tmp.path());

    let completed = manager
        .poll_delta(&task_id, true, 5000)
        .expect("poll delta");
    assert_eq!(completed.result.status, ShellStatus::Completed);
    assert!(completed.result.stdout.contains("done"));

    let detail = manager.inspect_job(&task_id).expect("inspect");
    assert!(detail.stdout.contains("done"));
    assert_eq!(detail.snapshot.status, ShellStatus::Completed);

    manager.remember_stale_job(
        "shell_stale",
        "cargo test",
        tmp.path().to_path_buf(),
        Some("task_old".to_string()),
    );
    let stale = manager
        .list_jobs()
        .into_iter()
        .find(|job| job.id == "shell_stale")
        .expect("stale job");
    assert!(stale.stale);
    assert_eq!(stale.linked_task_id.as_deref(), Some("task_old"));
}

#[test]
fn test_job_cancel_updates_completion_state() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let started = manager
        .execute(&sleep_command(60), None, 5000, true)
        .expect("execute");
    let task_id = started.task_id.expect("task id");

    let killed = manager.kill(&task_id).expect("kill");
    assert_eq!(killed.status, ShellStatus::Killed);
    let job = manager.inspect_job(&task_id).expect("inspect");
    assert_eq!(job.snapshot.status, ShellStatus::Killed);
    assert!(!job.snapshot.stdin_available);
}

#[test]
fn test_output_truncation() {
    let long_output = "x".repeat(50_000);
    let (truncated, _meta) = truncate_with_meta(&long_output);

    assert!(truncated.len() < long_output.len());
    assert!(truncated.contains("truncated"));
}

#[test]
fn test_truncate_with_meta_reports_omission_counts() {
    let long_output = format!("line1\nline2\n{}", "x".repeat(60_000));
    let (truncated, meta) = truncate_with_meta(&long_output);

    assert!(meta.truncated);
    assert!(meta.original_len >= long_output.len());
    assert!(meta.omitted > 0);
    assert!(truncated.contains("bytes omitted"));
}

#[test]
fn network_restricted_hint_detects_silent_curl_failure() {
    let tmp = tempdir().expect("tempdir");
    let ctx = network_restricted_context(tmp.path());
    let result = failed_network_shell_result("000", "");

    let hint = shell_network_restricted_hint(
        &ctx,
        "curl -s -o /dev/null -w '%{http_code}' https://api.github.com",
        &result,
    )
    .expect("network-restricted hint");

    assert!(hint.contains("Plan mode"));
}

#[test]
fn network_restricted_hint_ignores_local_failures() {
    let tmp = tempdir().expect("tempdir");
    let ctx = network_restricted_context(tmp.path());
    let result = failed_network_shell_result("", "No such file or directory");

    assert!(shell_network_restricted_hint(&ctx, "cat missing.txt", &result).is_none());
}

#[test]
fn shell_delta_result_surfaces_network_restricted_hint() {
    let tmp = tempdir().expect("tempdir");
    let ctx = network_restricted_context(tmp.path());
    let result = failed_network_shell_result("000", "");

    let tool_result = build_shell_delta_tool_result(
        ShellDeltaResult {
            command: "gh issue list".to_string(),
            result,
            stdout_total_len: 3,
            stderr_total_len: 0,
        },
        &ctx,
    );

    assert!(!tool_result.success);
    assert!(tool_result.content.starts_with("Shell command blocked"));
    let metadata = tool_result.metadata.expect("metadata");
    assert_eq!(
        metadata
            .get("sandbox_network_restricted")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn test_summarize_output_strips_truncation_note() {
    let long_output = "x".repeat(60_000);
    let (truncated, _meta) = truncate_with_meta(&long_output);
    let summary = summarize_output(&truncated);
    assert!(!summary.contains("Output truncated at"));
}

#[tokio::test]
async fn test_exec_shell_metadata_includes_summaries() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let tool = ExecShellTool;

    let result = tool
        .execute(json!({"command": echo_command("hello")}), &ctx)
        .await
        .expect("execute");
    assert!(result.success);

    let meta = result.metadata.expect("metadata");
    let summary = meta
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(summary.contains("hello"));
    assert!(meta.get("stdout_len").is_some());
    assert!(meta.get("stdout_truncated").is_some());
}

#[tokio::test]
async fn test_exec_shell_foreground_timeout_guides_background_rerun() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let tool = ExecShellTool;

    let result = tool
        .execute(
            json!({
                "command": sleep_command(10),
                "timeout_ms": 1000
            }),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(!result.success);
    assert!(result.content.contains("task_shell_start"));
    assert!(result.content.contains("background: true"));
    assert!(result.content.contains("process killed"));
    let meta = result.metadata.expect("metadata");
    assert_eq!(meta.get("status").and_then(Value::as_str), Some("TimedOut"));
    let recovery = meta
        .get("foreground_timeout_recovery")
        .expect("timeout recovery metadata");
    assert_eq!(
        recovery
            .get("exec_shell_background")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(
        recovery
            .get("hint")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("exec_shell_wait")
    );
}

#[tokio::test]
async fn test_exec_shell_foreground_cancel_kills_process() {
    let tmp = tempdir().expect("tempdir");
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext::new(tmp.path()).with_cancel_token(cancel_token.clone());
    let command = sleep_command(30);

    let task = tokio::spawn(async move {
        ExecShellTool
            .execute(
                json!({
                    "command": command,
                    "timeout_ms": 600_000
                }),
                &ctx,
            )
            .await
            .expect("execute")
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    cancel_token.cancel();

    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("foreground shell should observe cancellation")
        .expect("task should not panic");

    assert!(!result.success);
    assert!(result.content.contains("Command canceled"));
    let meta = result.metadata.expect("metadata");
    assert_eq!(meta.get("status").and_then(Value::as_str), Some("Killed"));
    assert_eq!(meta.get("canceled").and_then(Value::as_bool), Some(true));
}

#[tokio::test]
async fn test_exec_shell_foreground_can_move_to_background() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let shell_manager = ctx.shell_manager.clone();
    let command = sleep_command(30);
    let task_ctx = ctx.clone();

    let task = tokio::spawn(async move {
        ExecShellTool
            .execute(
                json!({
                    "command": command,
                    "timeout_ms": 600_000
                }),
                &task_ctx,
            )
            .await
            .expect("execute")
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    shell_manager
        .lock()
        .expect("shell manager lock")
        .request_foreground_background();

    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("foreground shell should detach")
        .expect("task should not panic");

    assert!(result.success);
    assert!(result.content.contains("Command moved to background"));
    assert!(result.content.contains("exec_shell_cancel"));

    let meta = result.metadata.expect("metadata");
    assert_eq!(meta.get("status").and_then(Value::as_str), Some("Running"));
    assert_eq!(
        meta.get("backgrounded").and_then(Value::as_bool),
        Some(true)
    );
    let task_id = meta
        .get("task_id")
        .and_then(Value::as_str)
        .expect("task id")
        .to_string();

    let mut manager = shell_manager.lock().expect("shell manager lock");
    let job = manager.inspect_job(&task_id).expect("inspect job");
    assert_eq!(job.snapshot.status, ShellStatus::Running);
    let killed = manager.kill(&task_id).expect("kill");
    assert_eq!(killed.status, ShellStatus::Killed);
}

#[tokio::test]
async fn test_exec_shell_wait_cancel_leaves_background_process_running() {
    let tmp = tempdir().expect("tempdir");
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext::new(tmp.path()).with_cancel_token(cancel_token.clone());
    let shell_manager = ctx.shell_manager.clone();
    let started = shell_manager
        .lock()
        .expect("shell manager lock")
        .execute(&sleep_command(30), None, 600_000, true)
        .expect("execute");
    let task_id = started.task_id.expect("task id");
    let wait_task_id = task_id.clone();
    let task_ctx = ctx.clone();

    let task = tokio::spawn(async move {
        ShellWaitTool::new("exec_shell_wait")
            .execute(
                json!({
                    "task_id": wait_task_id,
                    "wait": true,
                    "timeout_ms": 600_000
                }),
                &task_ctx,
            )
            .await
            .expect("wait")
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    cancel_token.cancel();

    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("wait should observe cancellation")
        .expect("task should not panic");

    assert!(result.success);
    assert!(result.content.contains("still running"));
    let meta = result.metadata.expect("metadata");
    assert_eq!(meta.get("status").and_then(Value::as_str), Some("Running"));
    assert_eq!(
        meta.get("wait_canceled").and_then(Value::as_bool),
        Some(true)
    );

    let mut manager = shell_manager.lock().expect("shell manager lock");
    let job = manager.inspect_job(&task_id).expect("inspect job");
    assert_eq!(job.snapshot.status, ShellStatus::Running);
    let killed = manager.kill(&task_id).expect("kill");
    assert_eq!(killed.status, ShellStatus::Killed);
}

#[tokio::test]
async fn test_completed_background_shell_releases_process_handles() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let shell_manager = ctx.shell_manager.clone();
    let started = shell_manager
        .lock()
        .expect("shell manager lock")
        .execute(&echo_command("done"), None, 600_000, true)
        .expect("execute");
    let task_id = started.task_id.expect("task id");

    let result = ShellWaitTool::new("exec_shell_wait")
        .execute(
            json!({
                "task_id": task_id.clone(),
                "wait": true,
                "timeout_ms": 5_000
            }),
            &ctx,
        )
        .await
        .expect("wait");

    assert!(result.success);
    let mut manager = shell_manager.lock().expect("shell manager lock");
    let shell = manager.processes.get_mut(&task_id).expect("tracked shell");
    shell.poll();
    assert_eq!(shell.status, ShellStatus::Completed);
    assert!(shell.stdin.is_none());
    assert!(shell.child.is_none());
    assert!(shell.stdout_thread.is_none());
    assert!(shell.stderr_thread.is_none());
}

#[tokio::test]
async fn test_exec_shell_cancel_tool_kills_background_process() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let shell_manager = ctx.shell_manager.clone();
    let started = shell_manager
        .lock()
        .expect("shell manager lock")
        .execute(&sleep_command(30), None, 600_000, true)
        .expect("execute");
    let task_id = started.task_id.expect("task id");

    let result = ShellCancelTool
        .execute(json!({ "task_id": task_id }), &ctx)
        .await
        .expect("cancel");

    assert!(result.success);
    assert!(result.content.contains("Canceled background shell job"));
    let meta = result.metadata.expect("metadata");
    assert_eq!(meta.get("status").and_then(Value::as_str), Some("Killed"));

    let task_id = meta
        .get("task_id")
        .and_then(Value::as_str)
        .expect("task id");
    let mut manager = shell_manager.lock().expect("shell manager lock");
    let job = manager.inspect_job(task_id).expect("inspect job");
    assert_eq!(job.snapshot.status, ShellStatus::Killed);
}

#[tokio::test]
async fn test_exec_shell_cancel_tool_can_kill_all_running_processes() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let shell_manager = ctx.shell_manager.clone();
    let first = shell_manager
        .lock()
        .expect("shell manager lock")
        .execute(&sleep_command(30), None, 600_000, true)
        .expect("execute first")
        .task_id
        .expect("first task id");
    let second = shell_manager
        .lock()
        .expect("shell manager lock")
        .execute(&sleep_command(30), None, 600_000, true)
        .expect("execute second")
        .task_id
        .expect("second task id");

    let result = ShellCancelTool
        .execute(json!({ "all": true }), &ctx)
        .await
        .expect("cancel all");

    assert!(result.success);
    let meta = result.metadata.expect("metadata");
    assert_eq!(meta.get("status").and_then(Value::as_str), Some("Killed"));
    assert_eq!(meta.get("canceled").and_then(Value::as_u64), Some(2));

    let mut manager = shell_manager.lock().expect("shell manager lock");
    let first_job = manager.inspect_job(&first).expect("inspect first");
    let second_job = manager.inspect_job(&second).expect("inspect second");
    assert_eq!(first_job.snapshot.status, ShellStatus::Killed);
    assert_eq!(second_job.snapshot.status, ShellStatus::Killed);
}
