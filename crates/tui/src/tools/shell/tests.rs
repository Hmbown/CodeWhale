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
fn lowercase_bash_schema_is_small_contract() {
    let schema = LowercaseBashTool.input_schema();
    assert_eq!(schema["required"], json!(["command"]));
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(
        schema["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["command", "justification", "sandbox_permissions", "timeout"]
            .into_iter()
            .map(str::to_string)
            .collect()
    );
    assert!(!BashTool::new("Bash").model_visible());
}

#[test]
fn lowercase_bash_description_matches_the_timeout_it_actually_applies() {
    use super::{
        CONTRACT_BASH_FOREGROUND_DEFAULT_TIMEOUT_MS, contract_bash_legacy_input,
        contract_bash_timeout_ms,
    };

    // `bash {command}` with no `timeout` translates to a legacy input carrying
    // no `timeout_ms`, and the contract delegate then bounds the foreground run
    // at the 120 s default and kills the process there.
    let translated =
        contract_bash_legacy_input(&json!({"command": "sleep 600"})).expect("translated input");
    assert!(
        translated.get("timeout_ms").is_none(),
        "omitting `timeout` must not synthesise one during translation: {translated}"
    );
    assert_eq!(
        contract_bash_timeout_ms(true, None, false, false),
        Some(CONTRACT_BASH_FOREGROUND_DEFAULT_TIMEOUT_MS)
    );

    // The tool description is the only place the model learns this. It used to
    // say "when omitted there is no default timeout", so a model running a
    // four-minute build had every reason not to pass a timeout, and got the
    // process killed at two minutes anyway.
    let description = LowercaseBashTool.description();
    assert!(
        !description.contains("no default timeout"),
        "description contradicts the applied default: {description}"
    );
    let default_seconds = CONTRACT_BASH_FOREGROUND_DEFAULT_TIMEOUT_MS / 1_000;
    assert!(
        description.contains(&format!("{default_seconds} seconds")),
        "description must name the default it applies: {description}"
    );
    let schema = LowercaseBashTool.input_schema();
    let timeout_doc = schema["properties"]["timeout"]["description"]
        .as_str()
        .expect("timeout description");
    assert!(
        !timeout_doc.contains("no default timeout"),
        "schema contradicts the applied default: {timeout_doc}"
    );
    assert!(
        timeout_doc.contains(&format!("{default_seconds} seconds")),
        "schema must name the default it applies: {timeout_doc}"
    );
}

#[test]
fn contract_bash_foreground_without_a_timeout_is_bounded_not_endless() {
    use super::{
        BASH_MAX_TIMEOUT_MS, CONTRACT_BASH_FOREGROUND_DEFAULT_TIMEOUT_MS, contract_bash_timeout_ms,
    };

    // The reported hang: `bash` in the foreground with no timeout. It used to
    // resolve to BASH_MAX_TIMEOUT_MS (~24.8 days), so an unauthenticated CLI
    // waiting on a prompt held the turn open indefinitely. It now takes the
    // default the tool's own schema advertises, which is what arms the
    // kill-and-rerun-in-background recovery.
    assert_eq!(
        contract_bash_timeout_ms(true, None, false, false),
        Some(CONTRACT_BASH_FOREGROUND_DEFAULT_TIMEOUT_MS)
    );
    const { assert!(CONTRACT_BASH_FOREGROUND_DEFAULT_TIMEOUT_MS < BASH_MAX_TIMEOUT_MS) };

    // An explicit request still wins, including one far above the default:
    // long foreground work stays possible when the model asks for it.
    assert_eq!(
        contract_bash_timeout_ms(true, Some(1_800_000), false, false),
        Some(1_800_000)
    );
    assert_eq!(
        contract_bash_timeout_ms(true, Some(5), false, false),
        Some(5)
    );

    // Background and interactive runs are meant to outlive the call, so they
    // keep "no timeout" and are never bounded by the foreground default.
    assert_eq!(contract_bash_timeout_ms(true, None, true, false), None);
    assert_eq!(contract_bash_timeout_ms(true, None, false, true), None);

    // The standalone Bash tools already resolve their own default upstream;
    // this helper must not second-guess the value they pass in.
    assert_eq!(
        contract_bash_timeout_ms(false, Some(120_000), false, false),
        Some(120_000)
    );
}

#[cfg(all(unix, not(target_env = "ohos")))]
#[test]
fn inherited_interactive_terminal_fails_closed_before_spawn() {
    let workspace = tempdir().expect("workspace");
    let mut manager = ShellManager::new(workspace.path().to_path_buf());
    let err = manager
        .execute_interactive_with_policy_env("codew", None, 10_000, None, HashMap::new())
        .expect_err("Unix inherited-terminal takeover must not spawn");
    let message = err.to_string();
    assert!(message.contains("foreground TTY ownership"), "{message}");
    assert!(message.contains("background: true, tty: true"), "{message}");
    assert!(message.contains("action: \"interact\""), "{message}");
    assert!(message.contains("task_id"), "{message}");
    assert!(message.contains("terminal/run"), "{message}");
    assert!(message.contains("terminal/send"), "{message}");
}

#[cfg(all(unix, target_env = "ohos"))]
#[test]
fn inherited_interactive_terminal_offers_only_ohos_recovery_paths() {
    let workspace = tempdir().expect("workspace");
    let mut manager = ShellManager::new(workspace.path().to_path_buf());
    let err = manager
        .execute_interactive_with_policy_env("codew", None, 10_000, None, HashMap::new())
        .expect_err("OHOS inherited-terminal takeover must not spawn");
    let message = err.to_string();
    assert!(message.contains("foreground TTY ownership"), "{message}");
    assert!(message.contains("new terminal"), "{message}");
    assert!(message.contains("omit `interactive: true`"), "{message}");
    assert!(!message.contains("background: true"), "{message}");
    assert!(!message.contains("terminal/run"), "{message}");
}

#[test]
fn contract_bash_nonzero_is_an_error_with_status_after_output() {
    let error = finish_contract_bash_result(
        ShellResult {
            task_id: None,
            status: ShellStatus::Failed,
            exit_code: Some(7),
            stdout: "before".to_string(),
            stderr: String::new(),
            duration_ms: 1,
            stdout_len: 6,
            stderr_len: 0,
            stdout_omitted: 0,
            stderr_omitted: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            sandboxed: false,
            sandbox_type: None,
            sandbox_denied: false,
        },
        None,
        &ToolContext::new("."),
    )
    .expect_err("nonzero must be a failed tool call");
    assert!(
        error
            .to_string()
            .ends_with("before\n\nCommand exited with code 7")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn lowercase_bash_returns_one_ordered_stream() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path());
    let result = LowercaseBashTool
        .execute(
            json!({"command": "printf out-1; printf err-2 >&2; printf out-3"}),
            &context,
        )
        .await
        .expect("bash");
    assert_eq!(result.content, "out-1err-2out-3");
}

#[cfg(unix)]
#[tokio::test]
async fn lowercase_bash_keeps_raw_command_under_readonly_policy() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path())
        .with_shell_policy(crate::worker_profile::ShellPolicy::ReadOnly);
    let result = LowercaseBashTool
        .execute(json!({"command": "pwd"}), &context)
        .await
        .expect("read-only bash");
    assert_eq!(
        result.content.trim(),
        workspace
            .path()
            .canonicalize()
            .expect("canonical workspace")
            .display()
            .to_string()
    );
}

#[tokio::test]
async fn lowercase_bash_readonly_refusal_names_work_mode() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path())
        .with_shell_policy(crate::worker_profile::ShellPolicy::ReadOnly);
    let result = LowercaseBashTool
        .execute(json!({"command": "touch blocked-by-plan"}), &context)
        .await
        .expect("policy refusal is a normal tool result");

    assert!(!result.success);
    assert!(result.content.contains("Work mode (`/mode work`)"));
    assert!(!result.content.contains("Act mode"));
    assert!(!workspace.path().join("blocked-by-plan").exists());
}

/// Regression for the wedge that took out the owner's own session under swap
/// exhaustion: the lowercase `bash` spill file could not be created (full temp
/// volume), every call — including `echo ok` — failed with the harness-internal
/// "Failed to create streaming shell output", and nothing recovered. Spill
/// failure must be soft: the command still runs, the tail is still returned,
/// the next call still works, and no job state leaks.
#[cfg(unix)]
#[tokio::test]
async fn lowercase_bash_survives_spill_file_failure_and_stays_usable() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path());
    let missing_spill_dir = workspace.path().join("no-such-temp-volume");
    assert!(!missing_spill_dir.exists());
    context
        .shell_manager
        .lock()
        .expect("shell manager")
        .set_output_spill_dir_for_test(Some(missing_spill_dir.clone()));

    // (ii) the command runs and (i) no harness-internal error leaks.
    let first = LowercaseBashTool
        .execute(json!({"command": echo_command("ok")}), &context)
        .await
        .expect("bash runs even when the spill file cannot be created");
    assert!(first.success, "{}", first.content);
    assert_eq!(first.content.trim(), "ok");
    assert!(
        !first
            .content
            .contains("Failed to create streaming shell output")
    );

    // The next call must work too — the whole point of the fix.
    let second = LowercaseBashTool
        .execute(json!({"command": echo_command("still-ok")}), &context)
        .await
        .expect("second bash call after a spill failure");
    assert!(second.success, "{}", second.content);
    assert_eq!(second.content.trim(), "still-ok");

    // Output past the bound is still delivered, and the notice explains why the
    // full-output path is missing instead of pointing at a file that was never
    // written.
    let long = LowercaseBashTool
        .execute(
            json!({"command": "i=0; while [ $i -lt 2100 ]; do echo line-$i; i=$((i+1)); done"}),
            &context,
        )
        .await
        .expect("long bash output without a spill file");
    assert!(long.success, "{}", long.content);
    assert!(long.content.contains("line-2099"));
    assert!(
        long.content.contains("Full output was not persisted:"),
        "{}",
        long.content
    );
    assert!(!long.content.contains("Full output: "));

    // (iii) no leaked session state: nothing is still running or unowned-pending.
    let mut manager = context.shell_manager.lock().expect("shell manager");
    let running = manager
        .list_jobs()
        .into_iter()
        .filter(|job| job.status == ShellStatus::Running)
        .count();
    assert_eq!(running, 0, "no shell job may be left running");
    assert!(
        !missing_spill_dir.exists(),
        "fail-soft must not create the dir"
    );
}

/// A spawn/stream failure caused by host exhaustion must reach the model as
/// an actionable message (cause chain + likely reason + retry), never as a
/// bare harness-internal context string.
#[test]
fn shell_execution_failure_names_resource_exhaustion_and_says_retry() {
    let error = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::StorageFull))
        .context("Failed to open PTY");
    let message = shell_execution_failed_message(&error);
    assert!(
        message.starts_with("Shell execution failed: Failed to open PTY"),
        "{message}"
    );
    assert!(
        message.contains("Likely host resource exhaustion"),
        "{message}"
    );
    assert!(message.contains("disk"), "{message}");
    assert!(message.contains("retry"), "{message}");
    assert!(message.contains("still usable"), "{message}");

    #[cfg(unix)]
    {
        let error = anyhow::Error::from(std::io::Error::from_raw_os_error(libc::EMFILE))
            .context("Failed to spawn PTY command: echo ok");
        let message = shell_execution_failed_message(&error);
        assert!(message.contains("file descriptors"), "{message}");
        assert!(message.contains("echo ok"), "{message}");
    }

    let plain = anyhow::anyhow!("working directory does not exist");
    let message = shell_execution_failed_message(&plain);
    assert_eq!(
        message,
        "Shell execution failed: working directory does not exist"
    );
}

#[tokio::test]
async fn lowercase_bash_timeout_uses_seconds_and_fails() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path());
    let error = LowercaseBashTool
        .execute(
            json!({"command": sleep_command(2), "timeout": 0.01}),
            &context,
        )
        .await
        .expect_err("timeout must fail");
    assert!(
        error
            .to_string()
            .contains("Command timed out after 0.01 seconds"),
        "{error}"
    );
}

fn execute_shell(
    manager: &mut ShellManager,
    command: &str,
    working_dir: Option<&str>,
    timeout_ms: u64,
    background: bool,
) -> Result<ShellResult> {
    manager.execute_with_options_env_for_session(
        command,
        working_dir,
        timeout_ms,
        background,
        None,
        false,
        None,
        HashMap::new(),
        "workspace",
    )
}

#[test]
fn deleted_saved_workspace_reports_path_and_recovery_before_spawn() {
    let workspace = tempdir().expect("workspace");
    let stale = workspace.path().join("deleted-session-workspace");
    let mut manager = ShellManager::new(stale.clone());

    let error = execute_shell(&mut manager, "echo should-not-run", None, 1_000, false)
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

    let error = execute_shell(
        &mut manager,
        "echo should-not-run",
        missing.to_str(),
        1_000,
        false,
    )
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
    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn cheap descendant");
    std::fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
    std::thread::sleep(Duration::from_secs(30));
    let _ = child.wait();
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

    for input in [
        json!({"command": "fd -e rs ."}),
        json!({"command": "fd -H --type f src"}),
        json!({"command": "git grep TODO crates/tui/src/tools"}),
        json!({"action": "run", "command": "gh issue list --limit 10"}),
        json!({"action": "run", "command": "gh issue view 5287"}),
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
        json!({"command": "git status -s", "background": "false"}),
        json!({"command": "git status -s", "stdin": ""}),
        json!({"action": "wait", "command": "pwd", "task_id": "shell_1"}),
        json!({"action": "interact", "command": "pwd", "task_id": "shell_1"}),
        json!({"action": "cancel", "command": "pwd", "task_id": "shell_1"}),
        json!({"action": 3, "command": "pwd"}),
        json!({"command": "pwd", "unexpected": true}),
        json!({"command": "cargo build"}),
        json!({"command": "bash -lc 'git status'"}),
        json!({"command": "sh -c 'rg TODO crates'"}),
        json!({"command": "PAGER=./pwn.sh git log"}),
        json!({"command": "GH_PAGER=./pwn.sh gh issue view 5287"}),
        json!({"command": "rg ${9:---pre=./repo-script} needle ."}),
        json!({"command": "rg ${9:---hostname-bin=./repo-script} needle ."}),
        json!({"command": "fd ${9:---exec} ./repo-script"}),
        json!({"command": "rg $PATTERN ."}),
        json!({"command": "rg *.rs ."}),
        json!({"command": "bash -lc 'rg TODO crates | head'"}),
        json!({"command": "fd -x ./pwn.sh"}),
        json!({"command": "fd --exec ./pwn.sh"}),
        json!({"command": "fd -uHtx ./pwn.sh"}),
        json!({"command": "rg --pre /tmp/evil.sh needle ."}),
        json!({"command": "rg --hostname-bin ./repo-script --hyperlink-format=file://{host}{path} needle ."}),
        json!({"command": "rg --search-zip needle ."}),
        json!({"command": "rg -z needle ."}),
        json!({"command": "git grep -O needle"}),
        json!({"command": "git grep -nO needle"}),
        json!({"command": "git grep --textconv needle"}),
        json!({"command": "git diff --ext-diff HEAD"}),
        json!({"command": "git diff --textconv HEAD"}),
        json!({"command": "git log --show-signature -1"}),
        json!({"command": "git show --format=%GS HEAD"}),
        json!({"command": "gh issue close 5287"}),
        json!({"command": "gh issue view 5287 > issue.txt"}),
        json!({"command": "gh pr checks 42 --watch"}),
        json!({"command": "gh issue view 5287 -R git.example.com/o/r"}),
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
async fn readonly_shell_refuses_raw_string_external_backend() {
    struct Backend(std::sync::atomic::AtomicBool);
    #[async_trait::async_trait]
    impl crate::sandbox::backend::SandboxBackend for Backend {
        async fn exec(
            &self,
            _cmd: &str,
            _env: &std::collections::HashMap<String, String>,
        ) -> anyhow::Result<crate::sandbox::backend::SandboxOutput> {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::sandbox::backend::SandboxOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
    }

    let tmp = tempdir().expect("tempdir");
    let backend = std::sync::Arc::new(Backend(std::sync::atomic::AtomicBool::new(false)));
    let mut context = ToolContext::new(tmp.path().to_path_buf())
        .with_shell_policy(crate::worker_profile::ShellPolicy::ReadOnly);
    context.sandbox_backend = Some(backend.clone());
    let error = BashTool::read_only("Bash")
        .execute(json!({"action": "run", "command": "pwd"}), &context)
        .await
        .expect_err("raw-string backend must not receive a classifier-approved argv")
        .to_string();
    assert!(error.contains("raw command string"), "{error}");
    assert!(!backend.0.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn lowercase_bash_refuses_non_streaming_external_backend() {
    struct Backend(std::sync::atomic::AtomicBool);
    #[async_trait::async_trait]
    impl crate::sandbox::backend::SandboxBackend for Backend {
        async fn exec(
            &self,
            _cmd: &str,
            _env: &std::collections::HashMap<String, String>,
        ) -> anyhow::Result<crate::sandbox::backend::SandboxOutput> {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            unreachable!("lowercase bash must fail before external dispatch")
        }
    }

    let workspace = tempdir().expect("workspace");
    let backend = std::sync::Arc::new(Backend(std::sync::atomic::AtomicBool::new(false)));
    let mut context = ToolContext::new(workspace.path());
    context.sandbox_backend = Some(backend.clone());
    let error = LowercaseBashTool
        .execute(json!({"command": "pwd", "timeout": 1}), &context)
        .await
        .expect_err("non-streaming backend must be rejected");
    assert!(error.to_string().contains("combined streaming output"));
    assert!(!backend.0.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn readonly_argv_is_shell_free_and_disables_git_helpers() {
    let (program, args) = hardened_readonly_argv("git show HEAD").expect("argv");
    assert_eq!(program, "git");
    assert_eq!(
        &args[..4],
        [
            "show",
            "--no-ext-diff",
            "--no-textconv",
            "--no-show-signature"
        ]
    );
    assert_eq!(args.last().map(String::as_str), Some("HEAD"));

    let (program, args) = hardened_readonly_argv("rg $PATTERN .").expect("literal argv");
    assert_eq!(program, "rg");
    assert_eq!(args, ["$PATTERN", "."]);
}

#[cfg(any(unix, windows))]
#[test]
fn readonly_program_resolution_ignores_workspace_shadow_executables() {
    let workspace = tempdir().expect("workspace");
    let trusted = tempdir().expect("trusted bin");
    let path = std::env::join_paths([workspace.path(), trusted.path()]).expect("test PATH");

    for program in ["git", "gh", "rg"] {
        let file = if cfg!(windows) {
            format!("{program}.exe")
        } else {
            program.to_string()
        };
        for directory in [workspace.path(), trusted.path()] {
            let executable = directory.join(&file);
            std::fs::write(&executable, b"fixture").expect("fixture executable");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mut permissions = executable.metadata().unwrap().permissions();
                permissions.set_mode(0o755);
                std::fs::set_permissions(&executable, permissions).unwrap();
            }
        }
        let resolved =
            resolve_readonly_program_from_path(program, workspace.path(), &path).expect("resolved");
        assert_eq!(resolved, trusted.path().join(file).canonicalize().unwrap());
        assert!(resolved.is_absolute() && !resolved.starts_with(workspace.path()));
    }
}

#[test]
fn readonly_child_env_removes_git_and_github_redirects() {
    let mut command = std::process::Command::new("unused");
    let redirects = [
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_EXEC_PATH",
        "GIT_OBJECT_DIRECTORY",
        "GIT_SSH_COMMAND",
        "GH_CONFIG_DIR",
        "GH_OTHER_PATH",
    ];
    for key in redirects {
        command.env(key, "outside");
    }
    command.env(READONLY_ENV_MARKER, "1");
    let env = HashMap::from([(READONLY_ENV_MARKER.to_string(), "1".to_string())]);
    remove_readonly_redirect_env(&mut command, &env);
    for key in redirects {
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == std::ffi::OsStr::new(key) && value.is_none()),
            "{key} must be removed from the child environment"
        );
    }
    assert!(
        command
            .get_envs()
            .any(|(name, value)| name == READONLY_ENV_MARKER && value.is_none())
    );
}

#[test]
fn readonly_operands_are_workspace_bounded_and_symlink_aware() {
    let workspace = tempdir().expect("workspace");
    let outside = tempdir().expect("outside");
    std::fs::write(workspace.path().join("inside.txt"), "inside").expect("inside file");
    std::fs::write(outside.path().join("secret.txt"), "secret").expect("outside file");

    enforce_readonly_workspace_operands("cat inside.txt", workspace.path(), workspace.path())
        .expect("in-workspace operand");
    let inside_absolute = workspace
        .path()
        .join("inside.txt")
        .canonicalize()
        .expect("canonical inside file");
    enforce_readonly_workspace_operands(
        &format!("cat {}", inside_absolute.display()),
        workspace.path(),
        workspace.path(),
    )
    .expect("absolute in-workspace operand");

    let outside_absolute = outside
        .path()
        .join("secret.txt")
        .canonicalize()
        .expect("canonical outside file");
    let error = enforce_readonly_workspace_operands(
        &format!("cat {}", outside_absolute.display()),
        workspace.path(),
        workspace.path(),
    )
    .expect_err("absolute outside operand must fail")
    .to_string();
    assert!(error.contains("operand.outside_workspace"), "{error}");

    for command in [
        "cat ../secret.txt",
        "cat ~/.ssh/id_rsa",
        "cat /rooted-current-drive.txt",
        "cat C:secret",
        r"cat C:\secret",
        r"cat \\server\share\secret",
    ] {
        let error =
            enforce_readonly_workspace_operands(command, workspace.path(), workspace.path())
                .expect_err("out-of-workspace operand must fail")
                .to_string();
        assert!(error.contains("inside the workspace"), "{command}: {error}");
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            workspace.path().join("secret-link"),
        )
        .expect("outside symlink");
        let error = enforce_readonly_workspace_operands(
            "cat secret-link",
            workspace.path(),
            workspace.path(),
        )
        .expect_err("symlink escape must fail")
        .to_string();
        assert!(error.contains("resolves outside"), "{error}");

        let subdir = workspace.path().join("subdir");
        std::fs::create_dir(&subdir).expect("subdir");
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            subdir.join("secret-link"),
        )
        .expect("cwd-relative outside symlink");
        enforce_readonly_workspace_operands("cat secret-link", workspace.path(), &subdir)
            .expect_err("operands must resolve relative to the effective cwd");
    }
}

#[test]
fn windows_verbatim_and_drive_operands_survive_posix_split() {
    // `shell_words` splits with POSIX backslash-escaping, which silently eats
    // the separators of Windows absolute paths (`C:\Users\...` becomes
    // `C:Users...`) and mangles the `\\?\` verbatim prefix before operand
    // classification can see it. The protection doubles those backslashes so
    // the splitter round-trips the real path (the `\\?\` cases that the
    // verbatim strip alone could never reach).
    for (raw, expected) in [
        (r"\\?\C:\Users\foo\inside.txt", r"C:\Users\foo\inside.txt"),
        (r"C:\Users\foo\inside.txt", r"C:\Users\foo\inside.txt"),
        (r"\\server\share\secret", r"\\server\share\secret"),
        (r"\\.\device\path", r"\\.\device\path"),
    ] {
        let protected = normalize_windows_command_paths(&format!("cat {raw}"));
        let argv = shell_words::split(&protected).expect("split must succeed");
        assert_eq!(
            argv,
            vec!["cat".to_string(), expected.to_string()],
            "{raw} must survive the POSIX split"
        );
    }
}

#[test]
fn windows_path_protection_leaves_other_words_untouched() {
    // POSIX escapes, drive-relative spellings, and plain relative operands
    // are not Windows absolute paths and must round-trip unchanged.
    assert_eq!(
        normalize_windows_command_paths("echo a\\ b && cat inside.txt"),
        "echo a\\ b && cat inside.txt"
    );
    assert_eq!(
        normalize_windows_command_paths("cat C:secret"),
        "cat C:secret"
    );
    assert_eq!(
        normalize_windows_command_paths("cat inside.txt"),
        "cat inside.txt"
    );
}

#[test]
fn readonly_github_shell_calls_obey_the_host_network_policy_before_spawn() {
    let tmp = tempdir().expect("tempdir");
    let context = |default| {
        ToolContext::new(tmp.path()).with_network_policy(
            crate::network_policy::NetworkPolicyDecider::new(
                crate::network_policy::NetworkPolicy {
                    default,
                    ..crate::network_policy::NetworkPolicy::default()
                },
                None,
            ),
        )
    };

    let allow = context(crate::network_policy::DecisionToml::Allow);
    enforce_readonly_github_network_policy("gh issue view 5287", &allow)
        .expect("allowed github.com policy");

    let deny = context(crate::network_policy::DecisionToml::Deny);
    let denied = enforce_readonly_github_network_policy("gh issue list", &deny)
        .expect_err("deny must stop before spawning gh")
        .to_string();
    assert!(denied.contains("blocked by the active network policy"));
    enforce_readonly_github_network_policy("git status", &deny)
        .expect("local reads do not consult the network policy");

    let prompt = context(crate::network_policy::DecisionToml::Prompt);
    let prompted = enforce_readonly_github_network_policy("gh issue view 5287", &prompt)
        .expect_err("headless Scout cannot prompt interactively")
        .to_string();
    assert!(prompted.contains("requires network approval"));
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

    for command in [
        "git --config-env=core.fsmonitor=SHELL status",
        "git -cdiff.foo.textconv=./repo-script diff HEAD",
        "rg -f/etc/passwd needle .",
    ] {
        let result = tool
            .execute(json!({"command": command}), &ctx)
            .await
            .expect("classifier refusal");
        assert!(!result.success, "{command}: {}", result.content);
        assert!(
            result.content.contains("read-only shell policy"),
            "{command}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn read_only_shell_resolves_operands_from_the_effective_cwd() {
    let workspace = tempdir().expect("workspace");
    let outside = tempdir().expect("outside");
    let subdir = workspace.path().join("subdir");
    std::fs::create_dir(&subdir).expect("subdir");
    std::fs::write(outside.path().join("secret"), "secret").expect("outside secret");
    std::os::unix::fs::symlink(outside.path().join("secret"), subdir.join("secret-link"))
        .expect("symlink");
    let ctx = ToolContext::new(workspace.path())
        .with_shell_policy(crate::worker_profile::ShellPolicy::ReadOnly);
    let error = BashTool::new("Bash")
        .execute(
            json!({"action": "run", "command": "cat secret-link", "cwd": "subdir"}),
            &ctx,
        )
        .await
        .expect_err("cwd-relative symlink escape must fail before spawn")
        .to_string();
    assert!(error.contains("resolves outside"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn read_only_shell_skips_shell_env_hooks() {
    let tmp = tempdir().expect("tempdir");
    let marker = tmp.path().join("hook-ran");
    let hook = crate::hooks::Hook::new(
        crate::hooks::HookEvent::ShellEnv,
        &format!("printf hit > '{}'", marker.display()),
    );
    let executor = crate::hooks::HookExecutor::new(
        crate::hooks::HooksConfig {
            enabled: true,
            hooks: vec![hook],
            ..crate::hooks::HooksConfig::default()
        },
        tmp.path().to_path_buf(),
    );
    let mut context = ToolContext::new(tmp.path())
        .with_shell_policy(crate::worker_profile::ShellPolicy::ReadOnly);
    context.runtime.hook_executor = Some(std::sync::Arc::new(executor));

    let result = BashTool::read_only("Bash")
        .execute(json!({"command": "pwd"}), &context)
        .await
        .expect("read-only inspection");
    assert!(result.success, "{}", result.content);
    assert!(!marker.exists(), "shell_env hook must not run for ReadOnly");
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
fn exec_shell_wait_schema_defaults_to_blocking() {
    let schema = BashTool::alias("exec_shell_wait", "wait").input_schema();
    assert!(
        schema["properties"]["wait"]["description"]
            .as_str()
            .is_some_and(|description| description.contains("default: true"))
    );
    assert!(
        BashTool::alias("exec_shell_wait", "wait")
            .description()
            .contains("wait")
    );
}

#[tokio::test]
async fn exec_shell_wait_without_wait_arg_blocks_until_completion() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let start_result = BashTool::new("Bash")
        .execute(
            json!({"command": sleep_command(1), "background": true}),
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

    let wait_result = BashTool::new("Bash")
        .execute(
            json!({"action": "wait", "task_id": task_id, "timeout_ms": 5_000}),
            &ctx,
        )
        .await
        .expect("wait for completion");

    assert_eq!(
        wait_result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("status"))
            .and_then(Value::as_str),
        Some("Completed")
    );
}

#[tokio::test]
async fn exec_shell_wait_false_returns_nonblocking_snapshot() {
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
    let wait_result = BashTool::new("Bash")
        .execute(
            json!({"action": "wait", "task_id": task_id, "timeout_ms": 5_000, "wait": false}),
            &ctx,
        )
        .await
        .expect("poll snapshot");

    assert!(
        started.elapsed() < Duration::from_millis(1_000),
        "wait=false should return a snapshot without blocking"
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
async fn exec_shell_wait_without_wait_arg_returns_running_at_timeout() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let start_result = BashTool::new("Bash")
        .execute(
            json!({"command": sleep_command(5), "background": true}),
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
    let result = BashTool::new("Bash")
        .execute(
            json!({"action": "wait", "task_id": task_id, "timeout_ms": 1_000}),
            &ctx,
        )
        .await
        .expect("bounded wait");
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(
        result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("status"))
            .and_then(Value::as_str),
        Some("Running")
    );

    BashTool::new("Bash")
        .execute(json!({"action": "cancel", "task_id": task_id}), &ctx)
        .await
        .expect("cancel background");
}

#[tokio::test]
async fn exec_shell_wait_many_until_any_returns_when_the_first_task_settles() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let short = BashTool::new("Bash")
        .execute(
            json!({"command": sleep_command(1), "background": true}),
            &ctx,
        )
        .await
        .expect("start short");
    let long = BashTool::new("Bash")
        .execute(
            json!({"command": sleep_command(30), "background": true}),
            &ctx,
        )
        .await
        .expect("start long");
    let short_id = short
        .metadata
        .as_ref()
        .and_then(|m| m.get("task_id"))
        .and_then(Value::as_str)
        .expect("short id")
        .to_string();
    let long_id = long
        .metadata
        .as_ref()
        .and_then(|m| m.get("task_id"))
        .and_then(Value::as_str)
        .expect("long id")
        .to_string();

    let started = Instant::now();
    let result = BashTool::new("Bash")
        .execute(
            json!({
                "action": "wait",
                "task_ids": [short_id, long_id],
                "until": "any",
                "timeout_ms": 8_000
            }),
            &ctx,
        )
        .await
        .expect("multi wait");

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "until=any must not wait for the long task"
    );
    let statuses = result
        .metadata
        .as_ref()
        .expect("metadata")
        .get("statuses")
        .expect("statuses");
    assert_eq!(statuses[short_id.as_str()], "Completed");
    assert_eq!(statuses[long_id.as_str()], "Running");
    assert_eq!(
        result.metadata.as_ref().and_then(|m| m.get("until")),
        Some(&json!("any"))
    );
    assert_eq!(
        result.metadata.as_ref().and_then(|m| m.get("timed_out")),
        Some(&json!(false))
    );
}

#[tokio::test]
async fn exec_shell_wait_many_all_reports_timeout_and_still_running() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let first = BashTool::new("Bash")
        .execute(
            json!({"command": sleep_command(30), "background": true}),
            &ctx,
        )
        .await
        .expect("start first");
    let second = BashTool::new("Bash")
        .execute(
            json!({"command": sleep_command(30), "background": true}),
            &ctx,
        )
        .await
        .expect("start second");
    let first_id = first
        .metadata
        .as_ref()
        .and_then(|m| m.get("task_id"))
        .and_then(Value::as_str)
        .expect("first id")
        .to_string();
    let second_id = second
        .metadata
        .as_ref()
        .and_then(|m| m.get("task_id"))
        .and_then(Value::as_str)
        .expect("second id")
        .to_string();

    let result = BashTool::new("Bash")
        .execute(
            json!({ "action": "wait", "task_ids": [first_id, second_id], "timeout_ms": 1_500 }),
            &ctx,
        )
        .await
        .expect("multi wait times out");
    assert_eq!(
        result.metadata.as_ref().and_then(|m| m.get("timed_out")),
        Some(&json!(true))
    );
    assert!(result.content.contains("still running"));
    let statuses = result
        .metadata
        .as_ref()
        .expect("metadata")
        .get("statuses")
        .expect("statuses");
    assert_eq!(statuses[first_id.as_str()], "Running");
    assert_eq!(statuses[second_id.as_str()], "Running");
}

#[tokio::test]
async fn exec_shell_wait_many_rejects_an_unknown_task_id() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let err = BashTool::new("Bash")
        .execute(
            json!({ "action": "wait", "task_ids": ["nope-1", "nope-2"], "wait": false }),
            &ctx,
        )
        .await
        .expect_err("unknown ids must fail the same way as the single-task path");
    assert!(err.to_string().contains("nope-1"), "{err}");
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
    assert!(result.content.contains("session exits") && result.content.contains("persist=true"));
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
    assert!(
        result
            .content
            .contains("not injected into the parent model"),
        "owned background work must describe its real completion route: {}",
        result.content
    );
    assert!(result.content.contains("Bash action=\"wait\""));
    assert_eq!(
        metadata
            .get("auto_resume_on_completion")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        metadata.get("completion_surface").and_then(Value::as_str),
        Some("task_status_and_explicit_wait")
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
    assert!(manager.may_have_undelivered_completion());
    assert!(
        manager.may_have_undelivered_completion(),
        "read-only detection must not consume the pending completion"
    );
    let completed = wait_for_completed_shell(&mut manager, &task_id);
    assert_ne!(completed.status, ShellStatus::Running);
    assert!(manager.may_have_undelivered_completion());

    let first = manager
        .drain_finished_jobs_with_evidence()
        .into_iter()
        .map(|completion| completion.event)
        .collect::<Vec<_>>();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].task_id, task_id);
    assert_eq!(first[0].status, ShellStatus::Completed);
    assert!(first[0].stdout_tail.contains("drain-finished-once"));

    let second = manager.drain_finished_jobs_with_evidence();
    assert!(second.is_empty(), "completion should be reported only once");
    assert!(!manager.may_have_undelivered_completion());
}

#[tokio::test]
async fn background_job_is_hidden_from_replacement_session_and_resumes_once_for_owner() {
    let tmp = tempdir().expect("tempdir");
    let ctx_a = ToolContext::new(tmp.path()).with_state_namespace("session-a");
    let ctx_b = ctx_a.clone().with_state_namespace("session-b");
    let result = BashTool::new("Bash")
        .execute(
            json!({"command": echo_command("owned-by-a"), "background": true}),
            &ctx_a,
        )
        .await
        .expect("start A background job");
    let task_id = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("task_id"))
        .and_then(Value::as_str)
        .expect("task id")
        .to_string();

    let mut manager = ctx_b.shell_manager.lock().expect("shell manager");
    assert!(manager.list_jobs_for_session("session-b").is_empty());
    assert!(
        manager
            .inspect_job_for_session("session-b", &task_id)
            .is_err()
    );
    assert!(
        manager
            .write_stdin_for_session("session-b", &task_id, "foreign", false)
            .is_err()
    );
    assert!(manager.kill_for_session("session-b", &task_id).is_err());

    let completed = wait_for_completed_shell(&mut manager, &task_id);
    assert_ne!(completed.status, ShellStatus::Running);
    let owned = manager
        .list_jobs_for_session("session-a")
        .into_iter()
        .find(|job| job.id == task_id)
        .expect("A job remains visible to A");
    assert_eq!(owned.owner_session_id, "session-a");
    assert!(
        manager
            .drain_finished_jobs_with_evidence_for_session("session-b")
            .is_empty(),
        "B must not claim A's completion"
    );
    assert!(manager.has_finished_unreported_jobs_for_session("session-a"));
    let first = manager.drain_finished_jobs_with_evidence_for_session("session-a");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].event.owner_session_id, "session-a");
    assert!(first[0].event.stdout_tail.contains("owned-by-a"));
    assert!(
        manager
            .drain_finished_jobs_with_evidence_for_session("session-a")
            .is_empty(),
        "A completion is delivered exactly once"
    );
}

#[test]
fn completion_evidence_preserves_arbitrary_stream_bytes() {
    use base64::Engine as _;

    let stdout = vec![b'o', 0, 0xff, b'k'];
    let stderr = vec![0xfe, b'e', b'r', b'r'];
    let evidence = ShellCompletionEvidence {
        event: ShellCompletionEvent {
            task_id: "shell_binary".to_string(),
            command: "binary-output".to_string(),
            status: ShellStatus::Completed,
            exit_code: Some(0),
            duration_ms: 17,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            stdout_len: stdout.len(),
            stderr_len: stderr.len(),
            evidence_ref: None,
            linked_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            owner_session_id: "session-test".to_string(),
        },
        stdout: stdout.clone(),
        stderr: stderr.clone(),
        stdout_omitted: 0,
        stderr_omitted: 0,
    };

    let payload: serde_json::Value =
        serde_json::from_slice(&evidence.artifact_bytes()).expect("evidence JSON");
    assert_eq!(payload["stdout"]["encoding"], "base64");
    assert_eq!(payload["stderr"]["encoding"], "base64");
    let decoded_stdout = base64::engine::general_purpose::STANDARD
        .decode(payload["stdout"]["content"].as_str().expect("stdout data"))
        .expect("decode stdout");
    let decoded_stderr = base64::engine::general_purpose::STANDARD
        .decode(payload["stderr"]["content"].as_str().expect("stderr data"))
        .expect("decode stderr");
    assert_eq!(decoded_stdout, stdout);
    assert_eq!(decoded_stderr, stderr);
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

    let result = execute_shell(&mut manager, &command, None, 5000, false).expect("execute");

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

    let result =
        execute_shell(&mut manager, &echo_command("hello"), None, 5000, false).expect("execute");

    assert_eq!(result.status, ShellStatus::Completed);
    assert!(result.stdout.contains("hello"));
    assert!(result.task_id.is_none());
}

#[test]
fn test_background_execution() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result = execute_shell(
        &mut manager,
        &sleep_then_echo_command(1, "done"),
        None,
        5000,
        true,
    )
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

    let result =
        execute_shell(&mut manager, &sleep_command(10), None, 1000, false).expect("execute");

    assert_eq!(result.status, ShellStatus::TimedOut);
}

#[test]
fn test_kill() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());

    let result =
        execute_shell(&mut manager, &sleep_command(60), None, 5000, true).expect("execute");

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
        .execute_with_options_env(
            &echo_stdin_command(),
            None,
            5000,
            true,
            None,
            false,
            None,
            HashMap::new(),
        )
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
        .execute_with_options_env(
            "sh -c 'exec 3<>/dev/tty && printf tty-ok && exec 3>&-'",
            None,
            5000,
            true,
            None,
            true,
            Some(ExecutionSandboxPolicy::DangerFullAccess),
            HashMap::new(),
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

    let started = execute_shell(
        &mut manager,
        &sleep_then_echo_command(1, "done"),
        None,
        5000,
        true,
    )
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

    let started =
        execute_shell(&mut manager, &sleep_command(5), None, 5000, true).expect("execute");
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

    let started =
        execute_shell(&mut manager, &sleep_command(5), None, 5000, true).expect("execute");
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

    let started =
        execute_shell(&mut manager, &sleep_command(60), None, 5000, true).expect("execute");
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
fn sandbox_denied_hint_names_the_effective_posture() {
    // DGF-02: an approved write blocked by a read-only sandbox must come
    // back naming the sandbox as the blocker, never as a bare failure.
    let tmp = tempdir().expect("tempdir");
    let ctx =
        ToolContext::new(tmp.path()).with_elevated_sandbox_policy(ExecutionSandboxPolicy::ReadOnly);
    let mut result =
        failed_network_shell_result("", "sh: cannot create out.txt: Operation not permitted");
    result.sandbox_denied = true;

    let hint = shell_sandbox_denied_hint(&ctx, &result).expect("sandbox-denied hint");

    assert!(hint.contains("read-only"), "{hint}");
    assert!(hint.contains("Ask-only escalation"), "{hint}");
    assert!(
        hint.contains("retry this exact command once with sandbox_permissions"),
        "{hint}"
    );
    assert!(hint.contains("justification"), "{hint}");
}

#[test]
fn contract_bash_denial_surfaces_the_escalation_shape() {
    let tmp = tempdir().expect("tempdir");
    let ctx =
        ToolContext::new(tmp.path()).with_elevated_sandbox_policy(ExecutionSandboxPolicy::ReadOnly);
    let mut result = failed_network_shell_result("", "Operation not permitted");
    result.sandbox_denied = true;

    let error = finish_contract_bash_result(result, None, &ctx)
        .expect_err("sandbox denial is a failed call");

    assert!(
        error
            .to_string()
            .contains("retry this exact command once with sandbox_permissions"),
        "{error}"
    );
}

#[test]
fn sandbox_denied_hint_absent_without_denial_or_policy() {
    let tmp = tempdir().expect("tempdir");
    let ctx =
        ToolContext::new(tmp.path()).with_elevated_sandbox_policy(ExecutionSandboxPolicy::ReadOnly);
    let undenied = failed_network_shell_result("", "No such file or directory");
    assert!(shell_sandbox_denied_hint(&ctx, &undenied).is_none());

    let mut denied = failed_network_shell_result("", "");
    denied.sandbox_denied = true;
    let no_policy_ctx = ToolContext::new(tmp.path());
    assert!(shell_sandbox_denied_hint(&no_policy_ctx, &denied).is_none());
}

#[test]
fn shell_delta_result_surfaces_sandbox_denied_hint() {
    let tmp = tempdir().expect("tempdir");
    let ctx =
        ToolContext::new(tmp.path()).with_elevated_sandbox_policy(ExecutionSandboxPolicy::ReadOnly);
    let mut result = failed_network_shell_result("", "Operation not permitted");
    result.sandbox_denied = true;

    let tool_result = build_shell_delta_tool_result(
        ShellDeltaResult {
            command: "touch out.txt".to_string(),
            result,
            stdout_total_len: 0,
            stderr_total_len: 0,
        },
        &ctx,
    );

    assert!(
        tool_result
            .content
            .contains("The execution sandbox blocked this command"),
        "{}",
        tool_result.content
    );
    let metadata = tool_result.metadata.expect("metadata");
    assert!(metadata.get("sandbox_denied_hint").is_some());
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
fn shell_delta_result_surfaces_elapsed_time_in_content() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let mut result = failed_network_shell_result("", "");
    result.status = ShellStatus::Running;
    result.duration_ms = 42_500;
    result.task_id = Some("shell-7".to_string());

    let tool_result = build_shell_delta_tool_result(
        ShellDeltaResult {
            command: "cargo test --workspace".to_string(),
            result,
            stdout_total_len: 0,
            stderr_total_len: 0,
        },
        &ctx,
    );

    assert!(
        tool_result
            .content
            .starts_with("Task shell-7 still running after 42.5 s."),
        "{}",
        tool_result.content
    );
}

#[test]
fn shell_delta_timing_line_omits_task_id_when_unknown() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    // failed_network_shell_result: ShellStatus::Failed, duration_ms: 25, task_id: None.
    let result = failed_network_shell_result("", "");

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
        tool_result.content.starts_with("Task failed after 25 ms."),
        "{}",
        tool_result.content
    );
}

#[test]
fn shell_delta_timing_line_phrases_cover_terminal_statuses() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    for (status, phrase) in [
        (ShellStatus::Completed, "completed"),
        (ShellStatus::Killed, "killed"),
        (ShellStatus::TimedOut, "timed out"),
    ] {
        let mut result = failed_network_shell_result("", "");
        result.status = status;
        result.duration_ms = 5_000;
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
                .starts_with(&format!("Task {phrase} after 5 s.")),
            "{}",
            tool_result.content
        );
    }
}

#[test]
fn shell_delta_timing_line_handles_zero_duration() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let mut result = failed_network_shell_result("", "");
    result.status = ShellStatus::Completed;
    result.duration_ms = 0;
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
            .starts_with("Task completed after 0 ms."),
        "{}",
        tool_result.content
    );
}

#[test]
fn shell_delta_timing_line_sits_below_network_hint() {
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
    let content = tool_result.content;
    let hint_pos = content.find("Shell command blocked").expect("hint present");
    let timing_pos = content
        .find("failed after 25 ms")
        .expect("timing line present");
    assert!(
        hint_pos < timing_pos,
        "hint must precede timing line: {content}"
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
    // The rerun instruction has to be spelled in the canonical action form:
    // `exec_shell` / `task_shell_start` are not both dispatchable, and the
    // model can only reach the shell through `Bash`.
    assert!(
        result
            .content
            .contains("Bash action=\"run\" background=true")
    );
    assert!(result.content.contains("Bash action=\"wait\""));
    assert!(!result.content.contains("exec_shell"));
    assert!(result.content.contains("process killed"));
    let meta = result.metadata.expect("metadata");
    assert_eq!(meta.get("status").and_then(Value::as_str), Some("TimedOut"));
    let recovery = meta
        .get("foreground_timeout_recovery")
        .expect("timeout recovery metadata");
    assert_eq!(
        recovery
            .get("rerun_as")
            .and_then(|rerun| rerun.get("background"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        recovery
            .get("rerun_as")
            .and_then(|rerun| rerun.get("tool"))
            .and_then(Value::as_str),
        Some("Bash")
    );
    let hint = recovery
        .get("hint")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(hint.contains("Bash action=\"wait\""), "{hint}");
    assert!(!hint.contains("exec_shell"), "{hint}");
    // The structured tool list is read by the model too; it must not hand
    // over names the registry does not resolve.
    let recommended = recovery.to_string();
    assert!(!recommended.contains("exec_shell"), "{recommended}");
}

#[test]
fn background_schema_distinguishes_temporary_jobs_from_persistent_services() {
    let schema = BashTool::new("Bash").input_schema();
    let d = schema["properties"]["background"]["description"]
        .as_str()
        .expect("background description");
    assert!(d.contains("killed") && d.contains("background:true") && d.contains("persist:true"));
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
    // The detach message points the model at the wait action for early
    // output, and hands over the task_id it needs to make that call.
    assert!(
        result.content.contains("Bash action=\"wait\""),
        "{}",
        result.content
    );
    assert!(result.content.contains("task_id="), "{}", result.content);
    assert!(!result.content.contains("exec_shell"), "{}", result.content);

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
async fn lowercase_bash_foreground_detach_is_a_successful_running_receipt() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let shell_manager = ctx.shell_manager.clone();
    let command = sleep_command(30);
    let task_ctx = ctx.clone();

    let task = tokio::spawn(async move {
        LowercaseBashTool
            .execute(json!({"command": command}), &task_ctx)
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

    assert!(result.success, "{}", result.content);
    assert!(
        result.content.contains("moved to /jobs"),
        "{}",
        result.content
    );
    assert!(!result.content.contains("code -1"), "{}", result.content);
    let metadata = result.metadata.expect("metadata");
    assert_eq!(metadata["status"], "Running");
    assert_eq!(metadata["backgrounded"], true);
    let task_id = metadata["task_id"].as_str().expect("task id");

    let mut manager = shell_manager.lock().expect("shell manager lock");
    let job = manager.inspect_job(task_id).expect("inspect job");
    assert_eq!(job.snapshot.status, ShellStatus::Running);
    manager.kill(task_id).expect("kill test job");
}

#[tokio::test]
async fn test_exec_shell_wait_cancel_leaves_background_process_running() {
    let tmp = tempdir().expect("tempdir");
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext::new(tmp.path()).with_cancel_token(cancel_token.clone());
    let shell_manager = ctx.shell_manager.clone();
    let started = execute_shell(
        &mut shell_manager.lock().expect("shell manager lock"),
        &sleep_command(30),
        None,
        600_000,
        true,
    )
    .expect("execute");
    let task_id = started.task_id.expect("task id");
    let wait_task_id = task_id.clone();
    let task_ctx = ctx.clone();

    let task = tokio::spawn(async move {
        BashTool::new("Bash")
            .execute(
                json!({
                    "action": "wait",
                    "task_id": wait_task_id,
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
    let started = execute_shell(
        &mut shell_manager.lock().expect("shell manager lock"),
        &echo_command("done"),
        None,
        600_000,
        true,
    )
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
        .execute_with_options_env_for_session(
            &command,
            None,
            60_000,
            true,
            None,
            false,
            None,
            env,
            &ctx.state_namespace,
        )
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
    let started = execute_shell(
        &mut shell_manager.lock().expect("shell manager lock"),
        &sleep_command(30),
        None,
        600_000,
        true,
    )
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
    let first = execute_shell(
        &mut shell_manager.lock().expect("shell manager lock"),
        &sleep_command(30),
        None,
        600_000,
        true,
    )
    .expect("execute first")
    .task_id
    .expect("first task id");
    let second = execute_shell(
        &mut shell_manager.lock().expect("shell manager lock"),
        &sleep_command(30),
        None,
        600_000,
        true,
    )
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
    let result =
        execute_shell(&mut manager, "sh -c 'sleep 100 &'", None, 5000, true).expect("execute");
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
    let result = execute_shell(&mut manager, "sh -c 'sleep 100 &'", None, 5000, false)
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

    let result = execute_shell(
        &mut manager,
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

    let result = execute_shell(
        &mut manager,
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
        owner_session_id: "windows-test-session".to_string(),
        command: "test".to_string(),
        working_dir: std::path::PathBuf::from("."),
        status: ShellStatus::Killed,
        exit_code: None,
        started_at: now,
        finished_at: Some(now),
        last_output_at: now,
        last_observed_output_len: 0,
        sandbox_type: SandboxType::None,
        ownership: ShellOwnership::Managed,
        linked_task_id: None,
        owner_agent: None,
        stdout_buffer: super::new_shared_raw_output(),
        stderr_buffer: None,
        heavy_permit: None,
        stdout_cursor: 0,
        stderr_cursor: 0,
        completion_reported: false,
        bounded_output: None,
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

    let bg =
        execute_shell(&mut manager, &echo_command("bg"), None, 5000, true).expect("execute bg");
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

// ── Kill-path overshoot regression tests (FINISH-0.9.4 #52 multiplier 2) ─────
//
// The foreground Bash kill path must return at ~timeout + a small bounded
// grace, even when the command ignores SIGTERM or a descendant escapes the
// process group while holding the output pipe open. Before the fix, an
// escaped descendant wedged the blocking reader-thread join inside kill()
// until the descendant exited on its own (observed: ~180s past a 120s
// timeout in the wild).

#[cfg(unix)]
const SHELL_SIGTERM_HELPER_ENV: &str = "CODEWHALE_SHELL_SIGTERM_HELPER";
#[cfg(unix)]
const SHELL_ESCAPE_HELPER_ENV: &str = "CODEWHALE_SHELL_ESCAPE_HELPER";
#[cfg(unix)]
const SHELL_ESCAPED_GRANDCHILD_ENV: &str = "CODEWHALE_SHELL_ESCAPED_GRANDCHILD";

/// Helper role: ignore SIGTERM and idle. Runs as the shell's direct child
/// (same process group), so only the SIGKILL escalation can stop it.
#[cfg(unix)]
#[test]
fn shell_sigterm_ignoring_helper_process() {
    if std::env::var(SHELL_SIGTERM_HELPER_ENV).ok().as_deref() != Some("1") {
        return;
    }
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    let pid_file = PathBuf::from(
        std::env::var(SHELL_DESCENDANT_PID_FILE_ENV).expect("sigterm helper pid file"),
    );
    std::fs::write(pid_file, std::process::id().to_string()).expect("write sigterm helper pid");
    loop {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Helper role: spawn a grandchild in its OWN process group (escaping the
/// shell's group) that inherits the output pipe, then exit immediately. The
/// wrapper shell keeps running (`sleep` after `&`), so the job stays Running
/// while the escaped grandchild holds the reader thread's pipe open.
#[cfg(unix)]
// The grandchild deliberately outlives this helper and is never wait()ed on —
// escaping reaping is exactly what the regression exercises; the test reaps
// it directly via SIGKILL at the end.
#[allow(clippy::zombie_processes)]
#[test]
fn shell_group_escape_helper_process() {
    if std::env::var(SHELL_ESCAPE_HELPER_ENV).ok().as_deref() != Some("1") {
        return;
    }
    let test_binary = std::env::current_exe().expect("current test binary");
    let pid_file = std::env::var(SHELL_DESCENDANT_PID_FILE_ENV).expect("escape pid file");
    let mut cmd = Command::new(test_binary);
    cmd.arg("--exact")
        .arg("tools::shell::tests::shell_escaped_grandchild_helper_process")
        .arg("--nocapture")
        .env(SHELL_ESCAPED_GRANDCHILD_ENV, "1")
        .env(SHELL_DESCENDANT_PID_FILE_ENV, pid_file);
    // A distinct process group is enough to escape `kill(-wrapper_pgid)`;
    // stdout/stderr are inherited, so the grandchild keeps the pipe open.
    #[cfg(unix)]
    cmd.process_group(0);
    let _child = cmd.spawn().expect("spawn escaped grandchild");
}

/// Helper role: the escaped grandchild — ignores SIGTERM, reports its pid,
/// then idles (holding the inherited output pipe open the whole time).
#[cfg(unix)]
#[test]
fn shell_escaped_grandchild_helper_process() {
    if std::env::var(SHELL_ESCAPED_GRANDCHILD_ENV).ok().as_deref() != Some("1") {
        return;
    }
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    let pid_file =
        PathBuf::from(std::env::var(SHELL_DESCENDANT_PID_FILE_ENV).expect("grandchild pid file"));
    std::fs::write(pid_file, std::process::id().to_string()).expect("write grandchild pid");
    std::thread::sleep(Duration::from_secs(30));
}

/// Required regression: a foreground command that ignores SIGTERM must be
/// dead and the tool must have returned within timeout + a small grace
/// (2s timeout, assert wall < 10s).
#[cfg(unix)]
#[tokio::test]
async fn foreground_timeout_kills_sigterm_ignoring_command_within_grace() {
    let tmp = tempdir().expect("tempdir");
    let pid_file = tmp.path().join("sigterm-helper.pid");
    let test_binary = std::env::current_exe().expect("current test binary");
    let command = format!(
        "{SHELL_SIGTERM_HELPER_ENV}=1 {SHELL_DESCENDANT_PID_FILE_ENV}={} exec {} --exact {} --nocapture",
        shell_words::quote(&pid_file.display().to_string()),
        shell_words::quote(&test_binary.display().to_string()),
        shell_words::quote("tools::shell::tests::shell_sigterm_ignoring_helper_process"),
    );
    let ctx = ToolContext::new(tmp.path());

    let started = Instant::now();
    let result = BashTool::new("Bash")
        .execute(json!({"command": command, "timeout_ms": 2_000}), &ctx)
        .await
        .expect("execute");
    let wall = started.elapsed();

    assert!(!result.success);
    let meta = result.metadata.expect("metadata");
    assert_eq!(meta.get("status").and_then(Value::as_str), Some("TimedOut"));
    assert!(
        wall < Duration::from_secs(10),
        "kill path overshot the 2s timeout: wall {wall:?}"
    );
    let helper_pid = wait_for_shell_pid_file(&pid_file);
    assert!(
        wait_for_shell_pid_exit(helper_pid),
        "SIGTERM-ignoring helper {helper_pid} survived the timeout kill"
    );
}

/// Regression for the ~180s kill-path overshoot: a descendant that escaped
/// the process group keeps the output pipe open after the group is killed.
/// kill() must still return within a bounded grace instead of blocking on
/// the reader-thread join until the descendant exits on its own.
#[cfg(unix)]
#[tokio::test]
async fn kill_returns_promptly_when_escaped_descendant_holds_pipe_open() {
    let tmp = tempdir().expect("tempdir");
    let pid_file = tmp.path().join("escaped-grandchild.pid");
    let test_binary = std::env::current_exe().expect("current test binary");
    let command = format!(
        "{SHELL_ESCAPE_HELPER_ENV}=1 {SHELL_DESCENDANT_PID_FILE_ENV}={} {} --exact {} --nocapture & sleep 60",
        shell_words::quote(&pid_file.display().to_string()),
        shell_words::quote(&test_binary.display().to_string()),
        shell_words::quote("tools::shell::tests::shell_group_escape_helper_process"),
    );
    let mut manager = ShellManager::new(tmp.path().to_path_buf());
    let started_bg =
        execute_shell(&mut manager, &command, None, 600_000, true).expect("start wrapper");
    let task_id = started_bg.task_id.expect("task id");
    let grandchild = wait_for_shell_pid_file(&pid_file);

    let started = Instant::now();
    let killed = manager.kill(&task_id).expect("kill");
    let wall = started.elapsed();

    assert_eq!(killed.status, ShellStatus::Killed);
    assert!(
        wall < Duration::from_secs(10),
        "kill blocked {wall:?} on a reader wedged by an escaped descendant"
    );

    // Cleanup: the escaped grandchild is out of reach of the group kill by
    // construction; reap it directly so the test does not leak a sleeper.
    unsafe {
        libc::kill(grandchild, libc::SIGKILL);
    }
    assert!(wait_for_shell_pid_exit(grandchild));
}

/// `Bash` was the only action wrapper whose catch-all fell through to its most
/// dangerous branch: an unrecognised action ran the command instead.
#[tokio::test]
async fn unknown_bash_action_is_refused_instead_of_running_the_command() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path().to_path_buf());
    let marker = workspace.path().join("should-not-exist");

    let error = BashTool::new("Bash")
        .execute(
            json!({
                "action": "kill",
                "command": format!("touch {}", marker.display()),
            }),
            &context,
        )
        .await
        .expect_err("unknown action must be refused");

    let message = error.to_string();
    assert!(message.contains("Unknown Bash action"), "{message}");
    assert!(message.contains("kill"), "{message}");
    assert!(
        message.contains("run, wait, interact, cancel"),
        "must name the actions that dispatch: {message}"
    );
    assert!(!marker.exists(), "the command must not have run");
}

/// The same hole one type down. `and_then(as_str).unwrap_or("run")` read a
/// non-string `action` as absent and fell through to the branch that executes
/// arbitrary code, so `Bash{action: 3, command: "…"}` ran the command. `File`,
/// `Git`, `Web`, and `Run` all refuse a non-string action; the tool that runs
/// shell commands must not be the lenient one.
#[tokio::test]
async fn non_string_bash_action_is_refused_instead_of_running_the_command() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path().to_path_buf());

    for action in [json!(3), json!(true), json!(["run"]), json!({"run": true})] {
        let marker = workspace.path().join(format!("marker-{action}"));
        let error = BashTool::new("Bash")
            .execute(
                json!({
                    "action": action,
                    "command": format!("touch {}", marker.display()),
                }),
                &context,
            )
            .await
            .expect_err("a non-string action must be refused");

        let message = error.to_string();
        assert!(
            message.contains("'action'"),
            "must name the parameter: {message}"
        );
        assert!(
            message.contains("must be a string"),
            "must name the expected type: {message}"
        );
        assert!(!marker.exists(), "the command must not have run: {action}");
    }
}

/// The same hole for the data fields (2026-08-04 review). A non-string
/// `stdin` was silently dropped — the command ran with NO stdin and reported
/// success, the silent-drop failure this lane exists to close. A non-string
/// `cwd` silently ran in the workspace default. And a numeric `task_id` was
/// reported as "missing", steering the model's retry the wrong way.
#[tokio::test]
async fn wrongly_typed_stdin_cwd_and_task_id_are_refused_not_dropped() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path().to_path_buf());

    let marker = workspace.path().join("stdin-marker");
    let error = BashTool::new("Bash")
        .execute(
            json!({
                "command": format!("touch {}", marker.display()),
                "stdin": 12345,
            }),
            &context,
        )
        .await
        .expect_err("non-string stdin must be refused, never silently dropped");
    let message = error.to_string();
    assert!(message.contains("'stdin'"), "names the field: {message}");
    assert!(
        message.contains("must be a string"),
        "names the expected type: {message}"
    );
    assert!(!marker.exists(), "the command must not have run");

    let error = BashTool::new("Bash")
        .execute(json!({ "command": "pwd", "cwd": 123 }), &context)
        .await
        .expect_err("non-string cwd must be refused, never defaulted");
    assert!(error.to_string().contains("'cwd'"), "{error}");

    let error = BashTool::new("Bash")
        .execute(json!({ "action": "wait", "task_id": 42 }), &context)
        .await
        .expect_err("non-string task_id is a type error");
    let message = error.to_string();
    assert!(
        message.contains("'task_id'") && message.contains("must be a string"),
        "a supplied-but-mistyped task_id must not read as missing: {message}"
    );
}

/// `null` is the wire spelling of absence, and `action` documents a `run`
/// default — so the strictness above must not swallow the default.
#[tokio::test]
async fn absent_or_null_bash_action_still_defaults_to_run() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path().to_path_buf());

    for input in [
        json!({"command": "echo defaulted"}),
        json!({"action": null, "command": "echo defaulted"}),
    ] {
        let result = BashTool::new("Bash")
            .execute(input.clone(), &context)
            .await
            .unwrap_or_else(|err| panic!("{input} must still run: {err}"));
        assert!(result.success, "{input}: {}", result.content);
        assert!(result.content.contains("defaulted"), "{}", result.content);
    }
}

/// Negative case for the strictness above: every legitimate action still
/// dispatches to its own handler rather than the action refusal. `wait`,
/// `interact`, and `cancel` are checked by the error they raise *after*
/// dispatch (a missing/unknown task), which only their own handlers produce.
#[tokio::test]
async fn every_valid_bash_action_still_dispatches() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path().to_path_buf());
    let tool = BashTool::new("Bash");

    let ran = tool
        .execute(
            json!({"action": "run", "command": "echo dispatched"}),
            &context,
        )
        .await
        .expect("action=run must dispatch");
    assert!(ran.success, "{}", ran.content);

    for input in [
        json!({"action": "wait", "task_id": "no-such-task"}),
        json!({"action": "interact", "task_id": "no-such-task", "stdin": "y\n"}),
        json!({"action": "cancel", "task_id": "no-such-task"}),
    ] {
        let outcome = tool.execute(input.clone(), &context).await;
        let message = match outcome {
            Ok(result) => result.content,
            Err(err) => err.to_string(),
        };
        assert!(
            !message.contains("Unknown Bash action") && !message.contains("must be a string"),
            "{input} must reach its own handler, got: {message}"
        );
    }

    // `cancel` with `all` needs no task at all and must stay a success.
    let cancelled = tool
        .execute(json!({"action": "cancel", "all": true}), &context)
        .await
        .expect("action=cancel all=true must dispatch");
    assert!(cancelled.success, "{}", cancelled.content);
}

/// The stdin aliases were real but undocumented: a model that wrote `input`
/// or `data` got them honoured with nothing in the schema saying so, and a
/// maintainer reading the schema would have removed them as dead. Advertise
/// them, and hold every spelling to the same behavior.
#[tokio::test]
async fn every_advertised_stdin_spelling_reaches_the_command() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path().to_path_buf());
    let schema = BashTool::new("Bash").input_schema();

    // `cat` is Unix-only; the dispatcher runs PowerShell or `cmd` on Windows,
    // where it is either absent or an alias for `Get-Content`, which reads a
    // file and not stdin. Ask for this platform's echo-stdin spelling — the
    // same helper `test_write_stdin_streams_output` uses.
    let echo_stdin = echo_stdin_command();
    for spelling in ["stdin", "input", "data"] {
        assert!(
            schema["properties"][spelling].is_object(),
            "`{spelling}` is honoured at runtime and must be advertised"
        );
        let result = BashTool::new("Bash")
            .execute(
                json!({"command": echo_stdin, spelling: "PIPED_THROUGH_ALIAS\n"}),
                &context,
            )
            .await
            .unwrap_or_else(|err| panic!("`{spelling}` must deliver stdin: {err}"));
        assert!(
            result.content.contains("PIPED_THROUGH_ALIAS"),
            "`{spelling}` did not reach the command: {}",
            result.content
        );
    }

    // `id` is the same undocumented shape one parameter over.
    assert!(
        schema["properties"]["id"].is_object(),
        "`id` is accepted for `task_id` at runtime and must be advertised"
    );
    assert!(
        schema["properties"]["task_id"]["description"]
            .as_str()
            .is_some_and(|text| text.contains("`id`")),
        "task_id must name its alias"
    );
}

/// The schema declared no `required` key at all, so `Bash{}` — no command, no
/// task — was schema-valid for the tool that runs shell commands. What is
/// required is per-action, so it is spelled as root `anyOf` required groups,
/// the same shape `finance` and `apply_patch` already use.
#[test]
fn bash_schema_declares_what_each_action_requires() {
    let schema = BashTool::new("Bash").input_schema();
    let groups: Vec<Vec<String>> = schema["anyOf"]
        .as_array()
        .expect("root anyOf required groups")
        .iter()
        .map(|group| {
            group["required"]
                .as_array()
                .expect("required group")
                .iter()
                .map(|name| name.as_str().expect("required name").to_string())
                .collect()
        })
        .collect();

    for expected in [["command"], ["task_id"], ["id"], ["all"]] {
        assert!(
            groups.iter().any(|group| group.as_slice() == expected),
            "missing required group {expected:?} in {groups:?}"
        );
    }
    // A required name the same schema does not advertise would be
    // unsatisfiable: the model could not learn what to send.
    for group in &groups {
        for name in group {
            assert!(
                schema["properties"][name].is_object(),
                "`{name}` is required but not advertised"
            );
        }
    }
}

/// A root `anyOf` is not portable to every provider, so prove the fallback
/// the sanitizer promises: Responses/xAI drop root composition, and the
/// constraint has to survive as a description note rather than vanishing.
#[test]
fn bash_required_groups_survive_a_provider_that_drops_root_composition() {
    let mut schema = BashTool::new("Bash").input_schema();
    let note = crate::tools::schema_sanitize::sanitize_for_responses(&mut schema)
        .expect("dropped required groups must be restated for the model");

    assert!(note.contains("At least one"), "{note}");
    for name in ["`command`", "`task_id`", "`id`", "`all`"] {
        assert!(note.contains(name), "note must name {name}: {note}");
    }
    assert_eq!(schema["type"], "object");
    assert!(schema.get("anyOf").is_none(), "root anyOf must be removed");
    assert!(schema["properties"]["command"].is_object());
}

/// Every hint in this file has to name a tool the model can actually call.
/// `exec_shell` / `exec_shell_wait` were retired in v0.9.3.
#[test]
fn shell_recovery_hints_name_only_dispatchable_tools() {
    assert!(!FOREGROUND_TIMEOUT_RECOVERY_HINT.contains("exec_shell"));
    assert!(FOREGROUND_TIMEOUT_RECOVERY_HINT.contains("Bash"));
    assert!(FOREGROUND_TIMEOUT_RECOVERY_HINT.contains("action=\"wait\""));
}

/// One documented default hid three real ones: `wait` uses 30s and
/// `interact` 1s, so a model omitting `timeout_ms` on `wait` got a quarter of
/// the timeout the schema promised.
#[test]
fn timeout_ms_description_covers_every_action_default() {
    let schema = BashTool::new("Bash").input_schema();
    let description = schema["properties"]["timeout_ms"]["description"]
        .as_str()
        .expect("timeout_ms description");

    for expected in ["120000", "600000", "30000", "1000"] {
        assert!(
            description.contains(expected),
            "missing {expected}: {description}"
        );
    }
}

#[cfg(unix)]
fn authorized_persistent_service_context(workspace: &Path) -> ToolContext {
    let mut context = ToolContext::new(workspace.to_path_buf())
        .with_elevated_sandbox_policy(ExecutionSandboxPolicy::DangerFullAccess);
    context.persist_services_enabled = true;
    context.tool_authority = None;
    context.shell_policy = ShellPolicy::Full;
    context.auto_approve = true;
    context
}

#[cfg(unix)]
fn persistent_service_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(unix)]
#[tokio::test]
async fn persistent_service_requires_explicit_headless_exec_authority() {
    let workspace = tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path().to_path_buf());

    let error = BashTool::new("Bash")
        .execute(
            json!({
                "action": "run",
                "command": "sleep 30",
                "background": true,
                "persist": true,
            }),
            &context,
        )
        .await
        .expect_err("ordinary tool contexts must reject ownership transfer");

    assert!(error.to_string().contains("real headless `codewhale exec`"));
}

#[cfg(unix)]
#[tokio::test]
async fn committed_persistent_service_survives_manager_drop_and_reports_identity() {
    let _guard = persistent_service_test_lock().lock().await;
    let workspace = tempdir().expect("workspace");
    let marker = workspace.path().join("persistent-service-finished");
    let context = authorized_persistent_service_context(workspace.path());

    let result = BashTool::new("Bash")
        .execute(
            json!({
                "action": "run",
                "command": format!("sleep 1; printf released > '{}'", marker.display()),
                "background": true,
                "persist": true,
            }),
            &context,
        )
        .await
        .expect("stage persistent service");
    assert!(result.success, "{result:?}");
    assert_eq!(
        result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata["ownership"].as_str()),
        Some("managed_pending_exec_success")
    );
    let task_id = result.metadata.as_ref().unwrap()["task_id"]
        .as_str()
        .expect("task id")
        .to_string();

    let receipts = context
        .shell_manager
        .lock()
        .expect("shell manager")
        .commit_persistent_services()
        .expect("commit persistent service");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].task_id, task_id);
    assert_eq!(receipts[0].process_group_id, receipts[0].pid);
    assert_eq!(receipts[0].ownership, "external");

    drop(context);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        marker.exists(),
        "released service must survive Codewhale manager teardown"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn signal_cleanup_kills_staged_persistent_service_group() {
    let _guard = persistent_service_test_lock().lock().await;
    let workspace = tempdir().expect("workspace");
    let context = authorized_persistent_service_context(workspace.path());

    let result = BashTool::new("Bash")
        .execute(
            json!({
                "action": "run",
                "command": "sleep 30",
                "background": true,
                "persist": true,
            }),
            &context,
        )
        .await
        .expect("stage persistent service");
    let task_id = result.metadata.as_ref().unwrap()["task_id"]
        .as_str()
        .expect("task id")
        .to_string();
    let pid = context
        .shell_manager
        .lock()
        .expect("shell manager")
        .processes[&task_id]
        .child
        .as_ref()
        .and_then(ShellChild::process_id)
        .expect("persistent process id");

    abort_pending_persistent_process_groups_for_exit();
    let pid = i32::try_from(pid).expect("pid fits pid_t");
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        let mut status = 0;
        // SAFETY: `pid` is the direct child owned by this test's manager; the
        // nonblocking wait only reaps that exact process.
        let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if waited == pid {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "signal cleanup must kill the staged service process group"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(libc::WIFSIGNALED(status));
    assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
}

// === #5472: in-memory retention must be bounded ===

/// ~1.1 MB on stdout, fast: 30,000 lines of 37 bytes.
#[cfg(unix)]
fn chatty_command() -> String {
    "yes 0123456789abcdefghijklmnopqrstuvwxyz | head -n 30000".to_string()
}

/// The finding-1 regression: before this bound, a single uppercase `Bash` call
/// left its entire stdout resident in `ShellManager.processes` until the 1 h
/// `cleanup`, and only if the user happened to open the jobs panel.
#[cfg(unix)]
#[tokio::test]
async fn foreground_bash_releases_its_output_once_the_result_is_returned() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let result = BashTool::new("Bash")
        .execute(json!({"command": chatty_command()}), &ctx)
        .await
        .expect("run chatty foreground command");

    // The result itself is unaffected: still the same 30 KB truncation.
    assert!(
        result
            .content
            .contains("0123456789abcdefghijklmnopqrstuvwxyz")
    );

    let manager = ctx.shell_manager.lock().expect("shell manager");
    let retained = manager.retained_output_bytes_total();
    assert!(
        retained <= RAW_STREAM_SETTLED_TAIL_BYTES * 2,
        "a finished foreground call must not keep its full stdout resident: \
         {retained} bytes still held (bound {})",
        RAW_STREAM_SETTLED_TAIL_BYTES * 2
    );
}

/// Releasing memory must not rewrite history: the job panel keeps reporting how
/// much the command actually printed.
#[cfg(unix)]
#[tokio::test]
async fn released_output_still_reports_the_real_stream_length() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    BashTool::new("Bash")
        .execute(json!({"command": chatty_command()}), &ctx)
        .await
        .expect("run chatty foreground command");

    let mut manager = ctx.shell_manager.lock().expect("shell manager");
    let jobs = manager.list_jobs();
    let job = jobs.first().expect("the finished job is still listed");
    assert!(
        job.stdout_len >= 1_000_000,
        "stdout_len must stay honest after release, got {}",
        job.stdout_len
    );
    assert!(
        !job.stdout_tail.is_empty(),
        "a diagnostic tail must survive the release"
    );
}

/// A background job's bytes become a durable session artifact at drain time;
/// keeping a second copy in the manager afterwards is the pure-waste term.
#[cfg(unix)]
#[tokio::test]
async fn draining_completion_evidence_releases_the_retained_copy() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path());
    let started = BashTool::new("Bash")
        .execute(
            json!({"command": chatty_command(), "background": true}),
            &ctx,
        )
        .await
        .expect("start background");
    let task_id = started
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("task_id"))
        .and_then(Value::as_str)
        .expect("task id")
        .to_string();

    let mut manager = ctx.shell_manager.lock().expect("shell manager");
    let completed = wait_for_completed_shell(&mut manager, &task_id);
    assert_ne!(completed.status, ShellStatus::Running);

    let evidence = manager.drain_finished_jobs_with_evidence();
    assert_eq!(evidence.len(), 1);
    assert!(
        evidence[0].event.stdout_len >= 1_000_000,
        "the event still reports the full length"
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&evidence[0].artifact_bytes()).expect("evidence JSON");
    assert!(
        payload["stdout"]["content"]
            .as_str()
            .expect("stdout content")
            .len()
            >= 1_000_000,
        "the artifact carries the exact bytes; only the manager's copy is dropped"
    );

    let retained = manager.retained_output_bytes_total();
    assert!(
        retained <= RAW_STREAM_SETTLED_TAIL_BYTES * 2,
        "{retained} bytes still held after the evidence was published"
    );
}

/// Age was the only bound, and it only ran from `list_jobs()`. Hundreds of
/// finished records inside one hour were all retained.
#[test]
fn cleanup_bounds_finished_records_by_count() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());
    let seeded_count = MAX_FINISHED_SHELL_RECORDS + 40;
    for index in 0..seeded_count {
        // Give every fixture a deterministic ordering while keeping all of
        // them far younger than the age ceiling. Lower ids are older.
        manager.seed_finished_record_for_test(
            format!("record-{index}"),
            Duration::from_millis((seeded_count - index) as u64),
        );
    }
    manager.cleanup(FINISHED_SHELL_MAX_AGE);
    assert_eq!(manager.tracked_job_count(), MAX_FINISHED_SHELL_RECORDS);
    assert!(
        manager.inspect_job("record-39").is_err(),
        "the oldest overflow record must be evicted"
    );
    assert!(
        manager.inspect_job("record-40").is_ok(),
        "the first record inside the cap must survive"
    );
    assert!(
        manager
            .inspect_job(&format!("record-{}", seeded_count - 1))
            .is_ok(),
        "the newest record must survive"
    );
}

/// #5478 nit: `/jobs` reported `2m 07s` for a 12-second command, because
/// elapsed was `started_at.elapsed()` even after the job finished. A completed
/// job reports the duration it finished with.
#[test]
fn a_finished_job_reports_its_duration_not_a_growing_elapsed() {
    let tmp = tempdir().expect("tempdir");
    let mut manager = ShellManager::new(tmp.path().to_path_buf());
    let result = manager
        .execute_with_options_env(
            &echo_command("frozen-elapsed"),
            None,
            10_000,
            true,
            None,
            false,
            None,
            std::collections::HashMap::new(),
        )
        .expect("spawn");
    let task_id = result.task_id.expect("task id");
    let completed = wait_for_completed_shell(&mut manager, &task_id);
    assert_ne!(completed.status, ShellStatus::Running);

    let first = manager
        .list_jobs()
        .into_iter()
        .find(|job| job.id == task_id)
        .expect("job listed")
        .elapsed_ms;

    std::thread::sleep(Duration::from_millis(400));

    let second = manager
        .list_jobs()
        .into_iter()
        .find(|job| job.id == task_id)
        .expect("job still listed")
        .elapsed_ms;

    assert_eq!(
        first, second,
        "a finished job's elapsed must stop moving; it read {first}ms then {second}ms"
    );
    assert!(
        second < 10_000,
        "the frozen value must be the real duration, not the timeout: {second}ms"
    );
}
