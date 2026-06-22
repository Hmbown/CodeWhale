use super::*;

use crate::tools::spec::ToolContext;
use serde_json::{Value, json};
use tempfile::tempdir;

#[cfg(windows)]
use windows::Win32::Foundation::{DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::System::Threading::GetCurrentProcess;

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

const BACKGROUND_COMPLETION_WAIT_MS: u64 = 30_000;

#[cfg(windows)]
const JOB_OBJECT_QUERY_ACCESS: u32 = 0x0004;

#[cfg(windows)]
fn duplicate_job_without_terminate_access(job: WindowsJob) -> WindowsJob {
    let process = unsafe { GetCurrentProcess() };
    let mut limited_handle = HANDLE::default();

    unsafe {
        DuplicateHandle(
            process,
            job.handle,
            process,
            &mut limited_handle,
            JOB_OBJECT_QUERY_ACCESS,
            false,
            DUPLICATE_HANDLE_OPTIONS(0),
        )
        .expect("duplicate job handle without terminate access");
    }

    drop(job);
    WindowsJob {
        handle: limited_handle,
    }
}

fn echo_command(message: &str) -> String {
    format!("echo {message}")
}

fn sleep_command(seconds: u64) -> String {
    let dispatcher = crate::shell_dispatcher::global_dispatcher();
    if dispatcher.kind().is_powershell() {
        return format!("Start-Sleep -Seconds {seconds}");
    }
    #[cfg(windows)]
    {
        let ping_count = seconds.saturating_add(1);
        format!("ping 127.0.0.1 -n {ping_count} > NUL")
    }
    #[cfg(not(windows))]
    {
        format!("sleep {seconds}")
    }
}

fn sleep_then_echo_command(seconds: u64, message: &str) -> String {
    let dispatcher = crate::shell_dispatcher::global_dispatcher();
    if dispatcher.kind().is_powershell() {
        return format!("Start-Sleep -Seconds {seconds}; echo {message}");
    }
    #[cfg(windows)]
    {
        let ping_count = seconds.saturating_add(1);
        format!("ping 127.0.0.1 -n {ping_count} > NUL && echo {message}")
    }
    #[cfg(not(windows))]
    {
        format!("sleep {seconds} && echo {message}")
    }
}

fn echo_stdin_command() -> String {
    let dispatcher = crate::shell_dispatcher::global_dispatcher();
    if dispatcher.kind().is_powershell() {
        return "[Console]::In.ReadToEnd()".to_string();
    }
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

fn wait_for_completed_shell(manager: &mut ShellManager, task_id: &str) -> ShellResult {
    let deadline = Instant::now() + Duration::from_millis(BACKGROUND_COMPLETION_WAIT_MS);

    loop {
        let result = manager
            .get_output(task_id, true, 1_000)
            .expect("get_output");
        if result.status != ShellStatus::Running || Instant::now() >= deadline {
            return result;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn exec_shell_parallel_flags_are_input_aware() {
    let tool = ExecShellTool;
    let readonly = json!({"command": "git status -s"});
    assert!(tool.supports_parallel_for(&readonly));
    assert!(tool.is_read_only_for(&readonly));
    assert_eq!(
        tool.approval_requirement_for(&readonly),
        ApprovalRequirement::Auto
    );

    let bash_readonly = json!({"command": "bash -lc 'rg TODO crates/tui/src/tools'"});
    assert!(tool.supports_parallel_for(&bash_readonly));
    assert!(tool.is_read_only_for(&bash_readonly));
    assert_eq!(
        tool.approval_requirement_for(&bash_readonly),
        ApprovalRequirement::Auto
    );

    for input in [
        json!({"command": "git status -s", "background": true}),
        json!({"command": "git status -s", "stdin": ""}),
        json!({"command": "cargo build"}),
        json!({"command": "bash -lc 'rg TODO crates | head'"}),
    ] {
        assert!(!tool.supports_parallel_for(&input), "{input:?}");
        assert!(!tool.is_read_only_for(&input), "{input:?}");
        assert_eq!(
            tool.approval_requirement_for(&input),
            ApprovalRequirement::Required,
            "{input:?}"
        );
    }

    assert!(tool.starts_detached_for(&json!({
        "command": "cargo check --workspace",
        "background": true
    })));
    assert!(tool.starts_detached_for(&json!({
        "command": "cargo test -p codewhale-tui --bins",
        "tty": true
    })));
    assert!(!tool.starts_detached_for(&json!({
        "command": "cargo check --workspace"
    })));
    assert!(!tool.starts_detached_for(&json!({
        "command": "cargo check --workspace",
        "background": true,
        "interactive": true
    })));
}

#[tokio::test]
async fn read_only_shell_policy_blocks_non_readonly_commands() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path())
        .with_shell_policy(crate::worker_profile::ShellPolicy::ReadOnly);
    let tool = ExecShellTool;

    let result = tool
        .execute(json!({"command": "cargo build"}), &ctx)
        .await
        .expect("execute");
    assert!(!result.success);
    assert!(result.content.contains("read-only shell policy"));

    let result = tool
        .execute(
            json!({"command": "git status -s", "background": true}),
            &ctx,
        )
        .await
        .expect("execute");
    assert!(!result.success);
    assert!(result.content.contains("read-only shell policy"));
}

#[tokio::test]
async fn read_only_shell_policy_allows_readonly_inspection() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path())
        .with_shell_policy(crate::worker_profile::ShellPolicy::ReadOnly);

    let result = ExecShellTool
        .execute(json!({"command": "pwd"}), &ctx)
        .await
        .expect("execute");

    assert!(
        result.success,
        "unexpected shell failure: {}",
        result.content
    );
    assert_eq!(
        result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("status"))
            .and_then(Value::as_str),
        Some("Completed")
    );
}

#[test]
fn exec_shell_wait_schema_defaults_to_nonblocking_snapshot() {
    let schema = ShellWaitTool::new("exec_shell_wait").input_schema();
    assert_eq!(schema["properties"]["wait"]["default"], json!(false));
    assert!(
        ShellWaitTool::new("exec_shell_wait")
            .description()
            .contains("without blocking by default")
    );
}

#[tokio::test]
async fn exec_shell_wait_without_wait_arg_returns_snapshot() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let start_result = ExecShellTool
        .execute(
            json!({"command": sleep_command(2), "background": true}),
            &ctx,
        )
        .await
        .expect("start background");
    let task_id = start_result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("task_id"))
        .and_then(Value::as_str)
        .expect("task id")
        .to_string();

    let started = Instant::now();
    let wait_result = ShellWaitTool::new("exec_shell_wait")
        .execute(json!({"task_id": task_id, "timeout_ms": 5_000}), &ctx)
        .await
        .expect("wait snapshot");

    assert!(
        started.elapsed() < Duration::from_millis(1_000),
        "default wait path should return a snapshot instead of blocking"
    );
    assert_eq!(
        wait_result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("status"))
            .and_then(Value::as_str),
        Some("Running")
    );
}

#[tokio::test]
async fn background_start_advertises_task_status_completion() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let result = ExecShellTool
        .execute(
            json!({"command": sleep_command(1), "background": true}),
            &ctx,
        )
        .await
        .expect("start background");

    assert!(result.content.contains("completion is tracked"));
    let metadata = result.metadata.as_ref().expect("metadata");
    assert_eq!(
        metadata
            .get("auto_resume_on_completion")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        metadata.get("completion_surface").and_then(Value::as_str),
        Some("task_status")
    );
    assert_eq!(
        metadata.get("background_policy").and_then(Value::as_str),
        Some("nonblocking")
    );
}

#[tokio::test]
async fn background_shell_job_carries_subagent_owner() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path()).with_owner_agent("agent_owner", "verifier");
    let result = ExecShellTool
        .execute(
            json!({"command": sleep_command(2), "background": true}),
            &ctx,
        )
        .await
        .expect("start owned background shell");

    let metadata = result.metadata.as_ref().expect("metadata");
    assert_eq!(
        metadata.get("owner_agent_id").and_then(Value::as_str),
        Some("agent_owner")
    );
    assert_eq!(
        metadata.get("owner_agent_name").and_then(Value::as_str),
        Some("verifier")
    );
    let task_id = metadata
        .get("task_id")
        .and_then(Value::as_str)
        .expect("task id")
        .to_string();

    {
        let mut manager = ctx.shell_manager.lock().expect("shell manager");
        let snapshot = manager
            .list_jobs()
            .into_iter()
            .find(|job| job.id == task_id)
            .expect("owned shell job snapshot");
        assert_eq!(snapshot.owner_agent_id.as_deref(), Some("agent_owner"));
        assert_eq!(snapshot.owner_agent_name.as_deref(), Some("verifier"));
    }

    ShellCancelTool
        .execute(json!({"task_id": task_id}), &ctx)
        .await
        .expect("cancel owned background shell");
}

#[tokio::test]
async fn drain_finished_jobs_reports_once() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let result = ExecShellTool
        .execute(
            json!({"command": echo_command("drain-finished-once"), "background": true}),
            &ctx,
        )
        .await
        .expect("start background");
    let task_id = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("task_id"))
        .and_then(Value::as_str)
        .expect("task id")
        .to_string();

    let mut manager = ctx.shell_manager.lock().expect("shell manager");
    let completed = wait_for_completed_shell(&mut manager, &task_id);
    assert_ne!(completed.status, ShellStatus::Running);

    let first = manager.drain_finished_jobs();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].task_id, task_id);
    assert_eq!(first[0].status, ShellStatus::Completed);
    assert!(first[0].stdout_tail.contains("drain-finished-once"));

    let second = manager.drain_finished_jobs();
    assert!(second.is_empty(), "completion should be reported only once");
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
            "sh -c 'printf \"%s\\n%s\\n\" \"${DEEPSEEK_CHILD_ENV_SHELL_SECRET-unset}\" \"${DEEPSEEK_CHILD_ENV_EXPLICIT-unset}\"'",
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

    let final_result = wait_for_completed_shell(&mut manager, &task_id);

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
#[cfg(all(unix, not(target_env = "ohos")))]
fn background_tty_command_has_controlling_terminal() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result = manager
        .execute_with_options(
            "sh -c 'exec 3<>/dev/tty && printf tty-ok && exec 3>&-'",
            None,
            5000,
            true,
            None,
            true,
            Some(ExecutionSandboxPolicy::DangerFullAccess),
        )
        .expect("execute tty command");

    let task_id = result
        .task_id
        .expect("background tty execution should return task_id");

    let done = manager
        .get_output(&task_id, true, 10_000)
        .expect("get tty command output");

    assert_eq!(done.status, ShellStatus::Completed);
    assert_eq!(done.exit_code, Some(0));
    assert!(
        done.stdout.contains("tty-ok"),
        "tty output should confirm /dev/tty opened; got {done:?}"
    );
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
fn running_job_snapshot_marks_no_output_stale_after_threshold() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let started = manager
        .execute(&sleep_command(5), None, 5000, true)
        .expect("execute");
    let task_id = started.task_id.expect("task id");

    {
        let shell = manager.processes.get_mut(&task_id).expect("live shell");
        shell.last_output_at = Instant::now() - STALE_NO_OUTPUT_AFTER - Duration::from_millis(1);
    }

    let job = manager
        .list_jobs()
        .into_iter()
        .find(|job| job.id == task_id)
        .expect("running job");

    assert_eq!(job.status, ShellStatus::Running);
    assert!(job.stale, "silent running job should be marked stale");
    assert!(
        job.elapsed_since_output_ms
            .is_some_and(|elapsed| elapsed >= STALE_NO_OUTPUT_AFTER.as_millis() as u64),
        "elapsed no-output time should be exposed: {job:?}"
    );
}

#[test]
fn running_job_snapshot_keeps_recent_no_output_fresh() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let started = manager
        .execute(&sleep_command(5), None, 5000, true)
        .expect("execute");
    let task_id = started.task_id.expect("task id");

    let job = manager
        .list_jobs()
        .into_iter()
        .find(|job| job.id == task_id)
        .expect("running job");

    assert_eq!(job.status, ShellStatus::Running);
    assert!(!job.stale, "fresh running job should not start stale");
    assert!(job.elapsed_since_output_ms.is_some());
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
fn shell_delta_result_includes_cargo_failure_summary() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let result = ShellResult {
        task_id: None,
        status: ShellStatus::Failed,
        exit_code: Some(101),
        stdout: "running 1 test\ntest tests::fails ... FAILED\n\nfailures:\n\n---- tests::fails stdout ----\nthread 'tests::fails' panicked at src/lib.rs:7:9:\nboom\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; finished in 0.00s\n".to_string(),
        stderr: "error: test failed, to rerun pass `--lib`".to_string(),
        duration_ms: 12,
        stdout_len: 0,
        stderr_len: 0,
        stdout_omitted: 0,
        stderr_omitted: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        sandboxed: false,
        sandbox_type: None,
        sandbox_denied: false,
    };

    let tool_result = build_shell_delta_tool_result(
        ShellDeltaResult {
            command: "cargo test".to_string(),
            result,
            stdout_total_len: 0,
            stderr_total_len: 0,
        },
        &ctx,
    );

    let metadata = tool_result.metadata.expect("metadata");
    assert_eq!(
        metadata["cargo_failure_summary"]["kind"],
        json!("test_failure")
    );
    assert!(
        metadata["cargo_failure_summary"]["summary"]
            .as_str()
            .unwrap()
            .contains("Failing tests: tests::fails")
    );
    assert!(
        metadata["summary"]
            .as_str()
            .unwrap()
            .contains("error: test failed")
    );
}

#[test]
fn shell_delta_result_keeps_existing_summary_for_generic_cargo_failure() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let result = ShellResult {
        task_id: None,
        status: ShellStatus::Failed,
        exit_code: Some(1),
        stdout: "build failed".to_string(),
        stderr: "command failed without structured cargo diagnostics".to_string(),
        duration_ms: 12,
        stdout_len: 0,
        stderr_len: 0,
        stdout_omitted: 0,
        stderr_omitted: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        sandboxed: false,
        sandbox_type: None,
        sandbox_denied: false,
    };

    let tool_result = build_shell_delta_tool_result(
        ShellDeltaResult {
            command: "cargo test".to_string(),
            result,
            stdout_total_len: 0,
            stderr_total_len: 0,
        },
        &ctx,
    );

    let metadata = tool_result.metadata.expect("metadata");
    assert!(metadata.get("cargo_failure_summary").is_none());
    assert_eq!(
        metadata["summary"],
        json!("command failed without structured cargo diagnostics")
    );
}

#[test]
fn shell_delta_result_surfaces_python_build_dependency_hint() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let result = ShellResult {
        task_id: None,
        status: ShellStatus::Failed,
        exit_code: Some(1),
        stdout: String::new(),
        stderr: "running build_ext\nModuleNotFoundError: No module named 'setuptools'\n"
            .to_string(),
        duration_ms: 12,
        stdout_len: 0,
        stderr_len: 72,
        stdout_omitted: 0,
        stderr_omitted: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        sandboxed: false,
        sandbox_type: None,
        sandbox_denied: false,
    };

    let tool_result = build_shell_delta_tool_result(
        ShellDeltaResult {
            command: "python setup.py build_ext --inplace".to_string(),
            result,
            stdout_total_len: 0,
            stderr_total_len: 72,
        },
        &ctx,
    );

    assert!(!tool_result.success);
    assert!(
        tool_result
            .content
            .starts_with("Python build dependency missing")
    );
    let metadata = tool_result.metadata.expect("metadata");
    assert_eq!(
        metadata["python_build_dependency_hint"]["kind"],
        json!("missing_setuptools")
    );
    assert!(
        metadata["python_build_dependency_hint"]["hint"]
            .as_str()
            .unwrap()
            .contains("setuptools")
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

#[cfg(not(windows))]
#[tokio::test]
async fn test_exec_shell_combined_output_uses_single_stream() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let tool = ExecShellTool;
    let command = "printf 'out\\n'; printf 'err\\n' >&2";

    let result = tool
        .execute(json!({"command": command, "combined_output": true}), &ctx)
        .await
        .expect("execute");
    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("out"), "{}", result.content);
    assert!(result.content.contains("err"), "{}", result.content);

    let meta = result.metadata.expect("metadata");
    assert_eq!(
        meta.get("combined_output").and_then(Value::as_bool),
        Some(true)
    );
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

#[test]
fn test_exec_shell_schema_guides_gt_five_second_work_to_background() {
    let schema = ExecShellTool.input_schema();
    let description = schema["properties"]["background"]["description"]
        .as_str()
        .expect("background description");
    // The schema must steer >5s work to the background and point at the wait
    // tool for early output. The wording references `exec_shell_wait` (the
    // model-visible wait tool); the older `task_shell_start` phrasing was
    // dropped, but the >5s + wait-tool guidance is the load-bearing contract.
    assert!(description.contains(">5 seconds"), "{description}");
    assert!(description.contains("exec_shell_wait"), "{description}");
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
    // The detach message points the model at the wait tool for early output
    // (the cancel-tool reference was reworded to `exec_shell_wait`).
    assert!(result.content.contains("exec_shell_wait"));

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
                "timeout_ms": BACKGROUND_COMPLETION_WAIT_MS
            }),
            &ctx,
        )
        .await
        .expect("wait");

    assert!(result.success);
    let mut manager = shell_manager.lock().expect("shell manager lock");
    let result = wait_for_completed_shell(&mut manager, &task_id);
    assert_eq!(result.status, ShellStatus::Completed);
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
    assert!(result.content.contains("Canceled background command"));
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

fn make_failed_result(stderr: &str) -> ShellResult {
    ShellResult {
        task_id: None,
        status: ShellStatus::Failed,
        exit_code: Some(1),
        stdout: String::new(),
        stderr: stderr.to_string(),
        duration_ms: 0,
        stdout_len: 0,
        stderr_len: stderr.len(),
        stdout_omitted: 0,
        stderr_omitted: 0,
        stdout_truncated: false,
        sandboxed: false,
        sandbox_type: None,
        sandbox_denied: false,
        stderr_truncated: false,
    }
}

#[test]
fn test_macos_provenance_detected_by_activity_time_message() {
    let result = make_failed_result(
        "failed to update builder last activity time: open \
         /Users/user/.docker/buildx/activity/.tmp-abc: operation not permitted",
    );
    assert!(looks_like_macos_provenance_failure(&result));
}

#[test]
fn test_macos_provenance_detected_by_activity_path_and_eperm() {
    let result = make_failed_result(
        "error: open /home/user/.docker/buildx/activity/foo: operation not permitted",
    );
    assert!(looks_like_macos_provenance_failure(&result));
}

#[test]
fn test_macos_provenance_not_triggered_on_success() {
    let mut result = make_failed_result(
        "failed to update builder last activity time: open \
         /Users/user/.docker/buildx/activity/.tmp-abc: operation not permitted",
    );
    result.status = ShellStatus::Completed;
    result.exit_code = Some(0);
    assert!(!looks_like_macos_provenance_failure(&result));
}

#[test]
fn test_macos_provenance_not_triggered_on_unrelated_eperm() {
    let result = make_failed_result("open /some/other/path: operation not permitted");
    assert!(!looks_like_macos_provenance_failure(&result));
}

// Regression test for #828: shell spawns an orphaned background subprocess
// (simulating `nohup curl`) that keeps the pipe write-end open after the shell
// exits. collect_output() must not block indefinitely — it kills the whole
// process group first, allowing reader threads to get EOF and exit.
#[cfg(unix)]
#[test]
fn test_orphaned_subprocess_does_not_block_collect_output() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    // sh spawns `sleep 100 &` and exits; the sleep subprocess inherits the
    // pipe write-ends and would keep reader threads blocked without the fix.
    let result = manager
        .execute("sh -c 'sleep 100 &'", None, 5000, true)
        .expect("execute");
    let task_id = result.task_id.expect("task id");

    // Drive to completion with a tight timeout — must not hang.
    let done = manager
        .get_output(&task_id, true, 3000)
        .expect("get_output must complete, not hang");
    assert_eq!(done.status, ShellStatus::Completed);
}

#[cfg(unix)]
#[test]
fn foreground_shell_does_not_block_on_orphaned_subprocess_pipe() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let started = std::time::Instant::now();
    let result = manager
        .execute("sh -c 'sleep 100 &'", None, 5000, false)
        .expect("foreground execute must complete, not hang");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "foreground execute blocked on descendant pipe handles"
    );
    assert_eq!(result.status, ShellStatus::Completed);
}

// Windows equivalent of the orphaned pipe-handle regression. `cmd /c start /b`
// launches a descendant process that inherits stdout/stderr and outlives the
// shell. Job-object cleanup must terminate that descendant before reader-thread
// joins, otherwise get_output() blocks until ping exits.
#[cfg(windows)]
#[test]
fn background_collection_does_not_block_on_detached_descendant_pipe() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result = manager
        .execute(
            r#"cmd /c start "" /b ping 127.0.0.1 -n 4"#,
            None,
            5000,
            true,
        )
        .expect("execute");
    let task_id = result.task_id.expect("task id");

    let started = std::time::Instant::now();
    let done = manager
        .get_output(&task_id, true, 3000)
        .expect("get_output must complete, not hang");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(6),
        "get_output blocked on descendant pipe handles"
    );
    assert_eq!(done.status, ShellStatus::Completed);
}

#[cfg(windows)]
#[test]
fn windows_job_terminate_denied_falls_back_to_child_kill() {
    let mut child = Command::new("ping")
        .args(["127.0.0.1", "-n", "20"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ping");

    let job = WindowsJob::attach_to_child(&child).expect("attach job");
    let limited_job = duplicate_job_without_terminate_access(job);

    assert!(
        limited_job.terminate().is_err(),
        "limited job handle should not allow TerminateJobObject"
    );

    terminate_child_and_close_windows_job(Some(limited_job), &mut child)
        .expect("fallback child kill");

    let status = child
        .wait_timeout(std::time::Duration::from_secs(3))
        .expect("wait after fallback kill");
    assert!(
        status.is_some(),
        "fallback child kill should terminate child"
    );
}

#[cfg(windows)]
#[test]
fn windows_job_close_releases_foreground_reader_threads_when_terminate_denied() {
    let mut child = Command::new("ping")
        .args(["127.0.0.1", "-n", "8"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ping");

    let job = WindowsJob::attach_to_child(&child).expect("attach job");
    let limited_job = duplicate_job_without_terminate_access(job);
    assert!(
        limited_job.terminate().is_err(),
        "limited job handle should not allow TerminateJobObject"
    );

    let stdout_handle = child.stdout.take().expect("stdout pipe");
    let stderr_handle = child.stderr.take().expect("stderr pipe");
    let stdout_thread = std::thread::spawn(move || {
        let mut reader = stdout_handle;
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut reader = stderr_handle;
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    let started = std::time::Instant::now();
    terminate_and_close_windows_job(Some(limited_job));
    let _ = stdout_thread.join().unwrap_or_default();
    let _ = stderr_thread.join().unwrap_or_default();
    let status = child
        .wait_timeout(std::time::Duration::from_secs(3))
        .expect("wait after kill-on-close");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "reader joins waited for natural descendant exit instead of kill-on-close"
    );
    assert!(status.is_some(), "kill-on-close should terminate child");
}

#[cfg(windows)]
#[test]
fn windows_job_kill_on_close_releases_reader_threads_when_terminate_denied() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result = manager
        .execute(
            r#"cmd /c start "" /b ping 127.0.0.1 -n 8"#,
            None,
            5000,
            true,
        )
        .expect("execute");
    let task_id = result.task_id.expect("task id");

    {
        let shell = manager
            .processes
            .get_mut(&task_id)
            .expect("background shell");
        let job = shell.windows_job.take().expect("windows job attached");
        let limited_job = duplicate_job_without_terminate_access(job);
        assert!(
            limited_job.terminate().is_err(),
            "limited job handle should not allow TerminateJobObject"
        );
        shell.windows_job = Some(limited_job);
    }

    let started = std::time::Instant::now();
    let done = manager
        .get_output(&task_id, true, 3000)
        .expect("get_output must complete via kill-on-close fallback");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "get_output waited for natural descendant exit instead of kill-on-close"
    );
    assert_eq!(done.status, ShellStatus::Completed);
}

#[test]
fn test_list_jobs_cleans_up_completed_old_processes() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let bg = manager
        .execute(&echo_command("bg"), None, 5000, true)
        .expect("execute bg");
    let bg_id = bg.task_id.expect("bg task id");
    manager.get_output(&bg_id, true, 3000).expect("bg done");

    // Both the completed job and any tracking state should be present.
    assert!(!manager.processes.is_empty());

    // cleanup(ZERO) removes all completed processes immediately.
    manager.cleanup(Duration::ZERO);
    assert!(
        manager.processes.is_empty(),
        "completed processes should be evicted by cleanup"
    );
}

/// Regression for #1691: a `git commit -m "feat: complete sub-pages"` shell
/// command must reach the OS shell with its quoted message intact (one argv
/// slot), never split into `feat:` / `complete` / `sub-pages"`.
#[test]
fn issue_1691_quoted_commit_message_round_trips() {
    let cmd = r#"git commit -m "feat: complete sub-pages""#;
    let spec = CommandSpec::shell(
        cmd,
        std::path::PathBuf::from("/tmp"),
        Duration::from_secs(5),
    );

    let dispatcher = crate::shell_dispatcher::global_dispatcher();
    // The whole command (with quotes) is a single argv entry. The actual
    // shell binary can vary by platform, but the payload itself must stay
    // intact in one shell arg. We never split the command string ourselves.
    assert_eq!(spec.program, dispatcher.kind().binary());
    if dispatcher.kind().is_powershell() {
        assert_eq!(
            spec.args,
            [
                dispatcher.kind().command_flag().to_string(),
                "-Command".to_string(),
                format!("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {cmd}")
            ]
        );
    } else if matches!(dispatcher.kind(), crate::shell_dispatcher::ShellKind::Cmd) {
        assert_eq!(
            spec.args,
            ["/C".to_string(), format!("chcp 65001 >NUL & {cmd}")]
        );
    } else {
        assert_eq!(
            spec.args,
            [
                dispatcher.kind().command_flag().to_string(),
                cmd.to_string()
            ]
        );
    }
    assert_eq!(
        spec.args.len(),
        if dispatcher.kind().is_powershell() {
            3
        } else {
            2
        }
    );

    let mut built = Command::new(&spec.program);
    push_shell_args(&mut built, &spec.program, &spec.args);
    let got: Vec<String> = built
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(got, spec.args);
}
