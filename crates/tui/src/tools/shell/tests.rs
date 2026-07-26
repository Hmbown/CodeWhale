use super::*;

use crate::tools::spec::ToolContext;
use serde_json::{Value, json};
use tempfile::tempdir;

#[cfg(windows)]
use windows::Win32::Foundation::{DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::System::Threading::GetCurrentProcess;

// `env_lock` serializes tests that mutate the process environment.
#[cfg(any(unix, windows))]
use std::sync::{Mutex, OnceLock};

#[cfg(any(unix, windows))]
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

const BACKGROUND_COMPLETION_WAIT_MS: u64 = 30_000;

#[test]
fn deleted_saved_workspace_reports_path_and_recovery_before_spawn() {
    let workspace = tempdir().expect("workspace");
    let stale = workspace.path().join("deleted-session-workspace");
    let mut manager = ShellManager::new(stale.clone());

    let error = manager
        .execute("echo should-not-run", None, 1_000, false)
        .expect_err("missing saved workspace must fail before shell spawn");
    let message = error.to_string();
    assert!(message.contains("saved session workspace is unavailable"));
    assert!(message.contains(&stale.display().to_string()));
    assert!(message.contains("working_dir") || message.contains("cwd"));
    assert!(message.contains("resume/fork"));
}

#[test]
fn explicit_missing_working_dir_is_not_misreported_as_session_corruption() {
    let workspace = tempdir().expect("workspace");
    let missing = workspace.path().join("explicit-missing");
    let mut manager = ShellManager::new(workspace.path().to_path_buf());

    let error = manager
        .execute("echo should-not-run", missing.to_str(), 1_000, false)
        .expect_err("missing explicit cwd must fail before shell spawn");
    let message = error.to_string();
    assert!(message.contains("requested working directory is unavailable"));
    assert!(message.contains(&missing.display().to_string()));
    assert!(!message.contains("saved session workspace"));
}

#[cfg(not(target_env = "ohos"))]
#[test]
fn pty_exit_status_preserves_high_windows_code_losslessly() {
    let raw = 0xC000_0005;
    let status = ShellExitStatus::from_pty(portable_pty::ExitStatus::with_exit_code(raw));

    assert!(!status.success);
    assert_eq!(status.code, Some(i64::from(raw)));
    assert_eq!(
        exit_code_label(status.code),
        "exit code 3221225477 (0xC0000005)"
    );
    assert_eq!(exit_code_hex(status.code).as_deref(), Some("0xC0000005"));
}

#[cfg(not(target_env = "ohos"))]
#[test]
fn ordinary_pty_exit_status_keeps_concise_label() {
    let status = ShellExitStatus::from_pty(portable_pty::ExitStatus::with_exit_code(127));

    assert_eq!(status.code, Some(127));
    assert_eq!(exit_code_label(status.code), "exit code 127");
    assert_eq!(exit_code_hex(status.code), None);
}

#[cfg(windows)]
#[test]
fn std_windows_exit_status_reinterprets_signed_dword() {
    assert_eq!(std_exit_code_i64(0xC000_0005_u32 as i32), 0xC000_0005);
}

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

#[cfg(unix)]
const SHELL_DESCENDANT_HELPER_ENV: &str = "CODEWHALE_SHELL_DESCENDANT_HELPER";
#[cfg(unix)]
const SHELL_DESCENDANT_PID_FILE_ENV: &str = "CODEWHALE_SHELL_DESCENDANT_PID_FILE";

#[cfg(unix)]
#[test]
fn shell_descendant_helper_process() {
    if std::env::var(SHELL_DESCENDANT_HELPER_ENV).ok().as_deref() != Some("1") {
        return;
    }
    let pid_file =
        PathBuf::from(std::env::var(SHELL_DESCENDANT_PID_FILE_ENV).expect("descendant pid file"));
    let child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn cheap descendant");
    std::fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
    std::thread::sleep(Duration::from_secs(30));
}

#[cfg(unix)]
fn wait_for_shell_pid_file(path: &Path) -> libc::pid_t {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(raw) = std::fs::read_to_string(path)
            && let Ok(pid) = raw.trim().parse()
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "descendant pid file never appeared"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn wait_for_shell_pid_exit(pid: libc::pid_t) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
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
fn shell_owner_registers_before_spawn_and_silent_work_stays_live() {
    let work = crate::work_graph::new_shared_work_runtime(
        crate::tools::todo::new_shared_todo_list(),
        crate::tools::plan::new_shared_plan_state(),
    );
    let lifecycle = ShellWorkLifecycle {
        work: work.clone(),
        session_id: "shell-session".to_string(),
    };

    {
        let _guard = ShellSpawnIntentGuard::new(
            Some(lifecycle.clone()),
            "shell_spawn_failure",
            "missing-program",
        )
        .expect("register spawn intent");
    }
    lifecycle
        .register("shell_silent", "sleep 30")
        .expect("register silent shell");
    lifecycle
        .observe("shell_silent", &ShellStatus::Running, 1, 0)
        .expect("live owner observation");
    lifecycle
        .observe("shell_silent", &ShellStatus::Running, 2, 512)
        .expect("growing output observation");

    let graph = work
        .capture(Some("shell-session"))
        .expect("capture")
        .expect("graph")
        .graph;
    let operation = |external: &str| {
        graph.nodes.iter().find(|node| {
            node.binding
                .as_ref()
                .is_some_and(|binding| binding.external == external)
        })
    };
    assert_eq!(
        operation("shell:shell_spawn_failure").map(|node| node.state),
        Some(crate::work_graph::NodeState::Failed),
        "dropping an armed spawn guard must terminalize pre-spawn failure"
    );
    let silent = operation("shell:shell_silent").expect("silent shell operation");
    assert_eq!(silent.state, crate::work_graph::NodeState::Active);
    let observation = silent
        .binding
        .as_ref()
        .and_then(|binding| binding.last_observation.as_ref())
        .expect("last shell observation");
    assert_eq!(observation.seq, 2);
    assert_eq!(
        observation
            .output
            .as_ref()
            .and_then(crate::work_graph::EvidenceRef::raw_bytes),
        Some(512)
    );
}

#[test]
fn exec_shell_parallel_flags_are_input_aware() {
    let tool = BashTool::new("Bash");
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
        json!({"command": "fd -e rs ."}),
        json!({"command": "fd -H --type f src"}),
        json!({"command": "git grep TODO crates/tui/src/tools"}),
        json!({"command": "bash -lc 'fd -e toml .'"}),
        json!({"command": "bash -lc 'git grep TODO crates/tui/src/tools'"}),
    ] {
        assert!(tool.supports_parallel_for(&input), "{input:?}");
        assert!(tool.is_read_only_for(&input), "{input:?}");
        assert_eq!(
            tool.approval_requirement_for(&input),
            ApprovalRequirement::Auto,
            "{input:?}"
        );
    }

    for input in [
        json!({"command": "git status -s", "background": true}),
        json!({"command": "git status -s", "stdin": ""}),
        json!({"command": "cargo build"}),
        json!({"command": "bash -lc 'rg TODO crates | head'"}),
        json!({"command": "fd -x ./pwn.sh"}),
        json!({"command": "fd --exec ./pwn.sh"}),
        json!({"command": "fd -uHtx ./pwn.sh"}),
        json!({"command": "rg --pre /tmp/evil.sh needle ."}),
        json!({"command": "git grep -O needle"}),
        json!({"command": "git grep -nO needle"}),
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

#[test]
fn exec_shell_interact_requires_approval() {
    let tool = BashTool::alias("exec_shell_interact", "interact");
    assert_eq!(tool.approval_requirement(), ApprovalRequirement::Required);
    assert!(
        tool.capabilities()
            .contains(&ToolCapability::RequiresApproval)
    );
}

#[tokio::test]
async fn read_only_shell_policy_blocks_non_readonly_commands() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path())
        .with_shell_policy(crate::worker_profile::ShellPolicy::ReadOnly);
    let tool = BashTool::new("Bash");

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

    let result = BashTool::new("Bash")
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

#[tokio::test]
async fn exec_shell_multiline_block_explains_allow_shell_boundary() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());

    let result = BashTool::new("Bash")
        .execute(
            json!({"command": "python3 -c \"print(1)\nprint(2)\""}),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(!result.success);
    assert!(result.content.contains("Command contains multiple lines"));
    assert!(
        result
            .content
            .contains("allow_shell=true exposes shell tools"),
        "{}",
        result.content
    );
    assert!(
        result
            .content
            .contains("Write multiline scripts to a file first"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("task_shell_start"),
        "{}",
        result.content
    );
}

#[test]
fn exec_shell_wait_schema_defaults_to_nonblocking_snapshot() {
    let schema = BashTool::alias("exec_shell_wait", "wait").input_schema();
    assert!(schema["properties"]["wait"].is_object());
    assert!(
        BashTool::alias("exec_shell_wait", "wait")
            .description()
            .contains("wait")
    );
}

#[tokio::test]
async fn exec_shell_wait_without_wait_arg_returns_snapshot() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let start_result = BashTool::new("Bash")
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
    let wait_result = BashTool::alias("exec_shell_wait", "wait")
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
    let result = BashTool::new("Bash")
        .execute(
            json!({"command": sleep_command(1), "background": true}),
            &ctx,
        )
        .await
        .expect("start background");

    assert!(result.content.contains("completion is delivered"));
    let metadata = result.metadata.as_ref().expect("metadata");
    assert_eq!(
        metadata
            .get("auto_resume_on_completion")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        metadata.get("completion_surface").and_then(Value::as_str),
        Some("runtime_event_and_task_status")
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
    let result = BashTool::new("Bash")
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
        let owners = manager.running_owner_agent_ids();
        assert_eq!(owners, vec!["agent_owner".to_string()]);
    }

    BashTool::alias("exec_shell_cancel", "cancel")
        .execute(json!({"task_id": task_id}), &ctx)
        .await
        .expect("cancel owned background shell");
}

#[tokio::test]
async fn drain_finished_jobs_reports_once() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let result = BashTool::new("Bash")
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
#[cfg(windows)]
fn shell_execution_preserves_custom_windows_sdk_root_env() {
    let _guard = env_lock().lock().expect("env lock");
    let previous_sdk = std::env::var_os("BIMRV_SDK_ROOT");
    let previous_secret = std::env::var_os("MY_SECRET_ROOT");
    unsafe {
        std::env::set_var("BIMRV_SDK_ROOT", r"F:\Lib\BimRv27.5");
        std::env::set_var("MY_SECRET_ROOT", r"F:\Secrets");
    }

    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());
    let command = if crate::shell_dispatcher::global_dispatcher()
        .kind()
        .is_powershell()
    {
        r#"[Console]::WriteLine($env:BIMRV_SDK_ROOT); if ($null -eq $env:MY_SECRET_ROOT) { [Console]::WriteLine("secret-unset") } else { [Console]::WriteLine("secret-set") }"#
            .to_string()
    } else {
        r#"echo %BIMRV_SDK_ROOT% & if defined MY_SECRET_ROOT (echo secret-set) else (echo secret-unset)"#
            .to_string()
    };

    let result = manager
        .execute(&command, None, 5000, false)
        .expect("execute");

    unsafe {
        match previous_sdk {
            Some(value) => std::env::set_var("BIMRV_SDK_ROOT", value),
            None => std::env::remove_var("BIMRV_SDK_ROOT"),
        }
        match previous_secret {
            Some(value) => std::env::set_var("MY_SECRET_ROOT", value),
            None => std::env::remove_var("MY_SECRET_ROOT"),
        }
    }

    assert_eq!(result.status, ShellStatus::Completed);
    assert!(
        result.stdout.contains(r"F:\Lib\BimRv27.5"),
        "custom SDK root should reach exec_shell stdout: {:?}",
        result
    );
    assert!(
        result.stdout.contains("secret-unset"),
        "secret-like env should stay scrubbed: {:?}",
        result
    );
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
fn shell_delta_result_exposes_lossless_high_exit_code_and_hex() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let mut result = failed_network_shell_result("", "");
    result.exit_code = Some(0xC000_0005);

    let tool_result = build_shell_delta_tool_result(
        ShellDeltaResult {
            command: "echo probe".to_string(),
            result,
            stdout_total_len: 0,
            stderr_total_len: 0,
        },
        &ctx,
    );

    assert!(
        tool_result
            .content
            .contains("exit code 3221225477 (0xC0000005)"),
        "{}",
        tool_result.content
    );
    let metadata = tool_result.metadata.expect("metadata");
    assert_eq!(metadata["exit_code"], json!(3221225477_i64));
    assert_eq!(metadata["exit_code_hex"], json!("0xC0000005"));
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
    let tool = BashTool::new("Bash");

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
    let tool = BashTool::new("Bash");
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
    let tool = BashTool::new("Bash");

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
    let schema = BashTool::new("Bash").input_schema();
    let description = schema["properties"]["background"]["description"]
        .as_str()
        .expect("background description");
    assert!(description.contains(">5 seconds"), "{description}");
}

#[tokio::test]
async fn test_exec_shell_foreground_cancel_kills_process() {
    let tmp = tempdir().expect("tempdir");
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext::new(tmp.path()).with_cancel_token(cancel_token.clone());
    let command = sleep_command(30);

    let task = tokio::spawn(async move {
        BashTool::new("Bash")
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
        BashTool::new("Bash")
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
    assert!(
        result
            .content
            .contains("Foreground shell wait moved to /jobs")
    );
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
        BashTool::alias("exec_shell_wait", "wait")
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

    let result = BashTool::alias("exec_shell_wait", "wait")
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

#[cfg(unix)]
#[tokio::test]
async fn exec_shell_cancel_kills_descendant_process_group() {
    let tmp = tempdir().expect("tempdir");
    let pid_file = tmp.path().join("descendant.pid");
    let test_binary = std::env::current_exe().expect("current test binary");
    let command = format!(
        "{} --exact {} --nocapture",
        shell_words::quote(&test_binary.display().to_string()),
        shell_words::quote("tools::shell::tests::shell_descendant_helper_process"),
    );
    let ctx = ToolContext::new(tmp.path());
    let mut env = std::collections::HashMap::new();
    env.insert(SHELL_DESCENDANT_HELPER_ENV.to_string(), "1".to_string());
    env.insert(
        SHELL_DESCENDANT_PID_FILE_ENV.to_string(),
        pid_file.display().to_string(),
    );
    let started = ctx
        .shell_manager
        .lock()
        .expect("shell manager")
        .execute_with_options_env(&command, None, 60_000, true, None, false, None, env)
        .expect("start descendant tree");
    let task_id = started.task_id.expect("task id");
    let descendant = wait_for_shell_pid_file(&pid_file);

    let result = BashTool::alias("exec_shell_cancel", "cancel")
        .execute(json!({"task_id": task_id}), &ctx)
        .await
        .expect("cancel process group");
    assert!(result.success);
    assert!(
        wait_for_shell_pid_exit(descendant),
        "descendant {descendant} survived shell process-group cancellation"
    );
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

    let result = BashTool::alias("exec_shell_cancel", "cancel")
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

    let result = BashTool::alias("exec_shell_cancel", "cancel")
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

#[cfg(windows)]
#[test]
fn killed_shell_does_not_wait_for_blocked_reader_threads() {
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let stdout_thread = std::thread::spawn(move || {
        let _ = release_rx.recv();
    });
    let now = std::time::Instant::now();
    let mut shell = BackgroundShell {
        id: "killed-reader".to_string(),
        command: "test".to_string(),
        working_dir: std::path::PathBuf::from("."),
        status: ShellStatus::Killed,
        exit_code: None,
        started_at: now,
        last_output_at: now,
        last_observed_output_len: 0,
        sandbox_type: SandboxType::None,
        linked_task_id: None,
        owner_agent: None,
        stdout_buffer: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        stderr_buffer: None,
        heavy_permit: None,
        stdout_cursor: 0,
        stderr_cursor: 0,
        completion_reported: false,
        stdin: None,
        child: None,
        windows_job: None,
        stdout_thread: Some(stdout_thread),
        stderr_thread: None,
        work_lifecycle: None,
        lifecycle_seq: 0,
        last_lifecycle_status: None,
        last_lifecycle_bytes: 0,
    };

    let started = std::time::Instant::now();
    shell.collect_output();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "killed shell must not synchronously join a blocked reader"
    );
    release_tx.send(()).expect("release detached reader");
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
    // shell binary can vary by platform — and the dispatcher may wrap the
    // payload (encoding prefix, exit-code capture) — but the payload itself
    // must stay intact in ONE shell arg. We never split the command string
    // ourselves. This single-line ASCII command never takes the PowerShell
    // temp `-File` path, so the payload stays on the argv.
    assert_eq!(spec.program, dispatcher.kind().binary());
    let carriers = spec
        .args
        .iter()
        .filter(|arg| arg.contains(r#""feat: complete sub-pages""#))
        .count();
    assert_eq!(carriers, 1, "args: {:?}", spec.args);
    assert!(
        !spec
            .args
            .iter()
            .any(|arg| arg == "feat:" || arg == "complete" || arg == "sub-pages\""),
        "args: {:?}",
        spec.args
    );
    assert_eq!(spec.display_command(), cmd);

    let mut built = Command::new(&spec.program);
    push_shell_args(&mut built, &spec.program, &spec.args);
    let got: Vec<String> = built
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(got, spec.args);
}

/// When no `cwd` is provided, the shell should run in `context.workspace`,
/// not in the ShellManager's default_workspace. This ensures sub-agents in
/// worktrees run commands in the worktree directory rather than the parent.
///
/// Without the `context.workspace` default (stashed): runs in sm_dir → FAILS
/// With the `context.workspace` default (unstashed): runs in ctx_dir → PASSES
#[tokio::test]
async fn default_cwd_uses_context_workspace_not_shell_manager_default() {
    let ctx_dir = tempdir().expect("ctx tempdir");
    let sm_dir = tempdir().expect("sm tempdir");

    // Create distinct dirs — write a marker in each so we can tell them apart.
    std::fs::write(ctx_dir.path().join("I_AM_CTX_DIR"), "").unwrap();
    std::fs::write(sm_dir.path().join("I_AM_SM_DIR"), "").unwrap();

    // ToolContext whose workspace is ctx_dir...
    let ctx = ToolContext::new(ctx_dir.path())
        // ...but whose ShellManager's default_workspace is sm_dir.
        .with_shell_manager(new_shared_shell_manager(sm_dir.path().to_path_buf()));

    // Assert directory identity through marker files instead of comparing the
    // shell's printed path. PowerShell and `canonicalize` can spell the same
    // Windows path differently (for example, with a verbatim-path prefix).
    let command = if cfg!(windows) {
        "if (Test-Path -LiteralPath 'I_AM_CTX_DIR') { Write-Output 'context-workspace' } elseif (Test-Path -LiteralPath 'I_AM_SM_DIR') { Write-Output 'manager-workspace' } else { Write-Output 'missing-workspace' }"
    } else {
        "if [ -f I_AM_CTX_DIR ]; then printf 'context-workspace'; elif [ -f I_AM_SM_DIR ]; then printf 'manager-workspace'; else printf 'missing-workspace'; fi"
    };
    let result = BashTool::new("Bash")
        .execute(json!({"command": command}), &ctx)
        .await
        .expect("shell execute");
    assert!(result.success, "command failed: {:?}", result.content);

    assert!(
        result
            .content
            .lines()
            .any(|line| line.trim() == "context-workspace"),
        "expected context.workspace marker, but shell reported: {:?}",
        result.content
    );
}
