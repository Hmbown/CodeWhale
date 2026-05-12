//! Advanced shell execution with background process support and sandboxing.
//!
//! Provides:
//! - Synchronous command execution with timeout
//! - Background process execution
//! - Process output retrieval
//! - Process termination
//! - Sandbox support (macOS Seatbelt)
//! - Streaming output (future)

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;
use wait_timeout::ChildExt;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use super::shell_output::{summarize_output, truncate_with_meta};
use crate::child_env;
use crate::sandbox::{
    CommandSpec,
    ExecEnv,
    SandboxManager,
    SandboxPolicy as ExecutionSandboxPolicy, // Rename to avoid conflict with spec::SandboxPolicy
    SandboxType,
};

/// Status of a shell process
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShellStatus {
    Running,
    Completed,
    Failed,
    Killed,
    TimedOut,
}

/// Result from a shell command execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellResult {
    pub task_id: Option<String>,
    pub status: ShellStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    /// Original stdout length in bytes.
    #[serde(default)]
    pub stdout_len: usize,
    /// Original stderr length in bytes.
    #[serde(default)]
    pub stderr_len: usize,
    /// Bytes omitted from stdout due to truncation.
    #[serde(default)]
    pub stdout_omitted: usize,
    /// Bytes omitted from stderr due to truncation.
    #[serde(default)]
    pub stderr_omitted: usize,
    /// Whether stdout was truncated.
    #[serde(default)]
    pub stdout_truncated: bool,
    /// Whether stderr was truncated.
    #[serde(default)]
    pub stderr_truncated: bool,
    /// Whether the command was executed in a sandbox.
    #[serde(default)]
    pub sandboxed: bool,
    /// Type of sandbox used (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_type: Option<String>,
    /// Whether the command was blocked by sandbox restrictions.
    #[serde(default)]
    pub sandbox_denied: bool,
}

/// Compact, UI-oriented view of a tracked background shell job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellJobSnapshot {
    pub id: String,
    pub job_id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub status: ShellStatus,
    pub exit_code: Option<i32>,
    pub elapsed_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub stdout_len: usize,
    pub stderr_len: usize,
    pub stdin_available: bool,
    pub stale: bool,
    pub linked_task_id: Option<String>,
}

/// Full output view used by `/jobs show <id>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellJobDetail {
    pub snapshot: ShellJobSnapshot,
    pub stdout: String,
    pub stderr: String,
}

pub struct ShellDeltaResult {
    pub command: String,
    pub result: ShellResult,
    pub stdout_total_len: usize,
    pub stderr_total_len: usize,
}

enum ShellChild {
    Process(Child),
    Pty(Box<dyn portable_pty::Child + Send>),
}

#[cfg(unix)]
fn kill_child_process_group(child: &mut Child) -> std::io::Result<()> {
    let pgid = child.id() as libc::pid_t;
    if pgid <= 0 {
        return child.kill();
    }

    let result = unsafe { libc::kill(-pgid, libc::SIGKILL) };
    if result == 0 {
        Ok(())
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            child.kill()
        }
    }
}

/// Configure parent-death signaling so shell-spawned children are reaped when
/// the TUI dies abnormally (#421). On Linux this installs
/// `PR_SET_PDEATHSIG(SIGTERM)` via `pre_exec` — the kernel then sends SIGTERM
/// to the child the moment the parent process exits, even on SIGKILL of the
/// TUI. The cancellation path already SIGKILLs the whole process group, so
/// this only fires when the parent dies without running its drop / cleanup
/// code (panic during shutdown, OOM, hardware crash, etc.).
///
/// On macOS / Windows there's no kernel equivalent. The existing graceful
/// path (`kill_child_process_group` from the cancellation token) still
/// handles normal shutdown; abnormal exit can leak children — tracked as a
/// follow-up watchdog item per the original issue's acceptance criteria.
#[cfg(target_os = "linux")]
fn install_parent_death_signal(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `pre_exec` runs in the child between fork and exec. The closure
    // only calls `libc::prctl` with stack-allocated constant arguments and
    // does not touch heap memory or the parent's locks. Both requirements
    // (async-signal-safe + no allocation in the post-fork window) are met.
    unsafe {
        cmd.pre_exec(|| {
            let result = libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
            if result == -1 {
                // Surface the errno but do not abort the spawn — the child
                // will simply lose the parent-death cleanup safety net.
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn install_parent_death_signal(_cmd: &mut Command) {
    // No kernel-level equivalent on macOS / Windows. The cooperative
    // cancellation + process_group SIGKILL path covers normal shutdown;
    // abnormal exit (panic without unwind, SIGKILL of the TUI) can still
    // leak children on those platforms — tracked as a follow-up.
}

#[derive(Clone, Copy, Debug)]
struct ShellExitStatus {
    code: Option<i32>,
    success: bool,
}

impl ShellExitStatus {
    fn from_std(status: std::process::ExitStatus) -> Self {
        Self {
            code: status.code(),
            success: status.success(),
        }
    }

    fn from_pty(status: portable_pty::ExitStatus) -> Self {
        let code = i32::try_from(status.exit_code()).unwrap_or(i32::MAX);
        Self {
            code: Some(code),
            success: status.success(),
        }
    }
}

impl ShellChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ShellExitStatus>> {
        match self {
            ShellChild::Process(child) => child
                .try_wait()
                .map(|status| status.map(ShellExitStatus::from_std)),
            ShellChild::Pty(child) => child
                .try_wait()
                .map(|status| status.map(ShellExitStatus::from_pty)),
        }
    }

    fn wait(&mut self) -> std::io::Result<ShellExitStatus> {
        match self {
            ShellChild::Process(child) => child.wait().map(ShellExitStatus::from_std),
            ShellChild::Pty(child) => child.wait().map(ShellExitStatus::from_pty),
        }
    }

    fn kill(&mut self) -> std::io::Result<()> {
        match self {
            #[cfg(unix)]
            ShellChild::Process(child) => kill_child_process_group(child),
            #[cfg(not(unix))]
            ShellChild::Process(child) => child.kill(),
            ShellChild::Pty(child) => child.kill(),
        }
    }
}

enum StdinWriter {
    Pipe(ChildStdin),
    Pty(Box<dyn Write + Send>),
}

impl StdinWriter {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            StdinWriter::Pipe(stdin) => stdin.write_all(data),
            StdinWriter::Pty(writer) => writer.write_all(data),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            StdinWriter::Pipe(stdin) => stdin.flush(),
            StdinWriter::Pty(writer) => writer.flush(),
        }
    }
}

fn spawn_reader_thread<R: Read + Send + 'static>(
    mut reader: R,
    buffer: Arc<Mutex<Vec<u8>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut guard) = buffer.lock() {
                        guard.extend_from_slice(&chunk[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

/// A background shell process being tracked
pub struct BackgroundShell {
    pub id: String,
    pub command: String,
    pub working_dir: PathBuf,
    pub status: ShellStatus,
    pub exit_code: Option<i32>,
    pub started_at: Instant,
    pub sandbox_type: SandboxType,
    pub linked_task_id: Option<String>,
    stdout_buffer: Arc<Mutex<Vec<u8>>>,
    stderr_buffer: Option<Arc<Mutex<Vec<u8>>>>,
    stdout_cursor: usize,
    stderr_cursor: usize,
    stdin: Option<StdinWriter>,
    child: Option<ShellChild>,
    stdout_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl BackgroundShell {
    /// Check if the process has completed and update status
    fn poll(&mut self) -> bool {
        if self.status != ShellStatus::Running {
            return true;
        }

        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_code = status.code;
                    self.status = if status.success {
                        ShellStatus::Completed
                    } else {
                        ShellStatus::Failed
                    };
                    self.collect_output();
                    true
                }
                Ok(None) => false, // Still running
                Err(_) => {
                    self.status = ShellStatus::Failed;
                    self.collect_output();
                    true
                }
            }
        } else {
            true
        }
    }

    /// Collect output from the background threads
    fn collect_output(&mut self) {
        // Kill the whole process group before joining reader threads.
        // When the shell spawned persistent background jobs (e.g. `nohup curl`),
        // those subprocesses keep the pipe write-ends open after the shell exits.
        // Without this kill, handle.join() blocks indefinitely, freezing the UI
        // event loop that calls list_jobs() → poll() → collect_output().
        #[cfg(unix)]
        if let Some(ShellChild::Process(ref mut proc)) = self.child {
            let _ = kill_child_process_group(proc);
        }
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
        self.stdin = None;
        self.child = None;
    }

    fn write_stdin(&mut self, input: &str, close: bool) -> Result<()> {
        if let Some(stdin) = self.stdin.as_mut() {
            if !input.is_empty() {
                stdin
                    .write_all(input.as_bytes())
                    .context("Failed to write to stdin")?;
                stdin.flush().ok();
            }
            if close {
                self.stdin = None;
            }
            return Ok(());
        }

        if input.is_empty() && close {
            return Ok(());
        }

        Err(anyhow!("stdin is not available for task {}", self.id))
    }

    fn full_output(&self) -> (String, String, usize, usize) {
        let stdout_bytes = self
            .stdout_buffer
            .lock()
            .map(|data| data.clone())
            .unwrap_or_default();
        let stderr_bytes = self
            .stderr_buffer
            .as_ref()
            .and_then(|buffer| buffer.lock().ok().map(|data| data.clone()))
            .unwrap_or_default();

        let stdout_len = stdout_bytes.len();
        let stderr_len = stderr_bytes.len();

        (
            String::from_utf8_lossy(&stdout_bytes).to_string(),
            String::from_utf8_lossy(&stderr_bytes).to_string(),
            stdout_len,
            stderr_len,
        )
    }

    fn take_delta(&mut self) -> (String, String, usize, usize, usize, usize) {
        let (stdout_delta, stdout_total) =
            take_delta_from_buffer(&self.stdout_buffer, &mut self.stdout_cursor);
        let (stderr_delta, stderr_total) = if let Some(buffer) = self.stderr_buffer.as_ref() {
            take_delta_from_buffer(buffer, &mut self.stderr_cursor)
        } else {
            (Vec::new(), 0)
        };

        let stdout_delta_len = stdout_delta.len();
        let stderr_delta_len = stderr_delta.len();

        (
            String::from_utf8_lossy(&stdout_delta).to_string(),
            String::from_utf8_lossy(&stderr_delta).to_string(),
            stdout_delta_len,
            stderr_delta_len,
            stdout_total,
            stderr_total,
        )
    }

    fn sandbox_denied(&self) -> bool {
        if matches!(self.status, ShellStatus::Running) {
            return false;
        }
        let (_, stderr_full, _, _) = self.full_output();
        SandboxManager::was_denied(
            self.sandbox_type,
            self.exit_code.unwrap_or(-1),
            &stderr_full,
        )
    }

    /// Kill the process
    #[allow(dead_code)]
    fn kill(&mut self) -> Result<()> {
        if let Some(ref mut child) = self.child {
            child.kill().context("Failed to kill process")?;
            let _ = child.wait();
        }
        self.status = ShellStatus::Killed;
        self.collect_output();
        Ok(())
    }

    /// Get a snapshot of the current state
    #[allow(dead_code)]
    pub fn snapshot(&self) -> ShellResult {
        let sandboxed = !matches!(self.sandbox_type, SandboxType::None);
        let (stdout_full, stderr_full, _, _) = self.full_output();
        let (stdout, stdout_meta) = truncate_with_meta(&stdout_full);
        let (stderr, stderr_meta) = truncate_with_meta(&stderr_full);
        ShellResult {
            task_id: Some(self.id.clone()),
            status: self.status.clone(),
            exit_code: self.exit_code,
            stdout,
            stderr,
            duration_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout_len: stdout_meta.original_len,
            stderr_len: stderr_meta.original_len,
            stdout_omitted: stdout_meta.omitted,
            stderr_omitted: stderr_meta.omitted,
            stdout_truncated: stdout_meta.truncated,
            stderr_truncated: stderr_meta.truncated,
            sandboxed,
            sandbox_type: if sandboxed {
                Some(self.sandbox_type.to_string())
            } else {
                None
            },
            sandbox_denied: self.sandbox_denied(),
        }
    }

    fn job_snapshot(&self) -> ShellJobSnapshot {
        let (stdout_full, stderr_full, stdout_len, stderr_len) = self.full_output();
        ShellJobSnapshot {
            id: self.id.clone(),
            job_id: self.id.clone(),
            command: self.command.clone(),
            cwd: self.working_dir.clone(),
            status: self.status.clone(),
            exit_code: self.exit_code,
            elapsed_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout_tail: tail_text(&stdout_full, 1200),
            stderr_tail: tail_text(&stderr_full, 1200),
            stdout_len,
            stderr_len,
            stdin_available: self.stdin.is_some() && self.status == ShellStatus::Running,
            stale: false,
            linked_task_id: self.linked_task_id.clone(),
        }
    }

    fn job_detail(&self) -> ShellJobDetail {
        let (stdout, stderr, _, _) = self.full_output();
        ShellJobDetail {
            snapshot: self.job_snapshot(),
            stdout,
            stderr,
        }
    }
}

impl Drop for BackgroundShell {
    fn drop(&mut self) {
        if self.status == ShellStatus::Running
            && let Some(ref mut child) = self.child
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Manages background shell processes with optional sandboxing.
pub struct ShellManager {
    processes: HashMap<String, BackgroundShell>,
    stale_jobs: HashMap<String, ShellJobSnapshot>,
    default_workspace: PathBuf,
    sandbox_manager: SandboxManager,
    sandbox_policy: ExecutionSandboxPolicy,
    foreground_background_requested: bool,
}

impl std::fmt::Debug for ShellManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellManager")
            .field("processes", &self.processes.len())
            .field("stale_jobs", &self.stale_jobs.len())
            .field("default_workspace", &self.default_workspace)
            .field("sandbox_policy", &self.sandbox_policy)
            .field(
                "foreground_background_requested",
                &self.foreground_background_requested,
            )
            .finish()
    }
}

impl ShellManager {
    /// Create a new `ShellManager` with default (no sandbox) policy.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            processes: HashMap::new(),
            stale_jobs: HashMap::new(),
            default_workspace: workspace,
            sandbox_manager: SandboxManager::new(),
            sandbox_policy: ExecutionSandboxPolicy::default(),
            foreground_background_requested: false,
        }
    }

    /// Create a new `ShellManager` with a specific sandbox policy.
    #[allow(dead_code)]
    pub fn with_sandbox(workspace: PathBuf, policy: ExecutionSandboxPolicy) -> Self {
        Self {
            processes: HashMap::new(),
            stale_jobs: HashMap::new(),
            default_workspace: workspace,
            sandbox_manager: SandboxManager::new(),
            sandbox_policy: policy,
            foreground_background_requested: false,
        }
    }

    /// Set the sandbox policy for future commands.
    #[allow(dead_code)]
    pub fn set_sandbox_policy(&mut self, policy: ExecutionSandboxPolicy) {
        self.sandbox_policy = policy;
    }

    /// Get the current sandbox policy.
    #[allow(dead_code)]
    pub fn sandbox_policy(&self) -> &ExecutionSandboxPolicy {
        &self.sandbox_policy
    }

    /// Request that the active foreground shell wait detach and leave its
    /// process running in the background job table.
    pub fn request_foreground_background(&mut self) {
        self.foreground_background_requested = true;
    }

    fn clear_foreground_background_request(&mut self) {
        self.foreground_background_requested = false;
    }

    fn take_foreground_background_request(&mut self) -> bool {
        let requested = self.foreground_background_requested;
        self.foreground_background_requested = false;
        requested
    }

    /// Check if sandboxing is available on this platform.
    #[allow(dead_code)]
    pub fn is_sandbox_available(&mut self) -> bool {
        self.sandbox_manager.is_available()
    }

    /// Execute a shell command with the configured sandbox policy.
    #[allow(dead_code)]
    pub fn execute(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
    ) -> Result<ShellResult> {
        self.execute_with_policy(command, working_dir, timeout_ms, background, None)
    }

    /// Execute a shell command with a specific sandbox policy (overrides default).
    #[allow(dead_code)]
    pub fn execute_with_policy(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
    ) -> Result<ShellResult> {
        self.execute_with_options(
            command,
            working_dir,
            timeout_ms,
            background,
            None,
            false,
            policy_override,
        )
    }

    /// Execute a shell command with stdin/TTY options.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_with_options(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        stdin_data: Option<&str>,
        tty: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
    ) -> Result<ShellResult> {
        self.execute_with_options_env(
            command,
            working_dir,
            timeout_ms,
            background,
            stdin_data,
            tty,
            policy_override,
            HashMap::new(),
        )
    }

    /// Same as `execute_with_options`, plus an extra env-var map that is
    /// merged into the spawned process environment. Used by the `shell_env`
    /// hook injection path (#456); other callers should use the simpler
    /// wrapper above.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_with_options_env(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        stdin_data: Option<&str>,
        tty: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
        extra_env: HashMap<String, String>,
    ) -> Result<ShellResult> {
        let work_dir = working_dir.map_or_else(|| self.default_workspace.clone(), PathBuf::from);

        // Clamp timeout to max 10 minutes (600000ms)
        let timeout_ms = timeout_ms.clamp(1000, 600_000);

        // Use override policy if provided, otherwise use the manager's policy
        let policy = policy_override.unwrap_or_else(|| self.sandbox_policy.clone());

        // Create command spec and prepare sandboxed environment
        let spec = CommandSpec::shell(command, work_dir.clone(), Duration::from_millis(timeout_ms))
            .with_policy(policy)
            .with_env(extra_env);
        let exec_env = self.sandbox_manager.prepare(&spec);

        if background {
            self.spawn_background_sandboxed(command, &work_dir, &exec_env, stdin_data, tty)
        } else {
            if tty {
                return Err(anyhow!(
                    "TTY mode requires background execution (set background: true)."
                ));
            }
            Self::execute_sync_sandboxed(command, &work_dir, timeout_ms, stdin_data, &exec_env)
        }
    }

    /// Execute a shell command interactively (stdin/stdout/stderr inherit from terminal).
    #[allow(dead_code)]
    pub fn execute_interactive(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
    ) -> Result<ShellResult> {
        self.execute_interactive_with_policy(command, working_dir, timeout_ms, None)
    }

    /// Execute a shell command interactively with a specific sandbox policy override.
    pub fn execute_interactive_with_policy(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        policy_override: Option<ExecutionSandboxPolicy>,
    ) -> Result<ShellResult> {
        self.execute_interactive_with_policy_env(
            command,
            working_dir,
            timeout_ms,
            policy_override,
            HashMap::new(),
        )
    }

    /// Interactive variant that accepts extra env vars (#456 shell_env hook).
    pub fn execute_interactive_with_policy_env(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        policy_override: Option<ExecutionSandboxPolicy>,
        extra_env: HashMap<String, String>,
    ) -> Result<ShellResult> {
        let work_dir = working_dir.map_or_else(|| self.default_workspace.clone(), PathBuf::from);

        let timeout_ms = timeout_ms.clamp(1000, 600_000);
        let policy = policy_override.unwrap_or_else(|| self.sandbox_policy.clone());

        let spec = CommandSpec::shell(command, work_dir.clone(), Duration::from_millis(timeout_ms))
            .with_policy(policy)
            .with_env(extra_env);
        let exec_env = self.sandbox_manager.prepare(&spec);

        Self::execute_interactive_sandboxed(command, &work_dir, timeout_ms, &exec_env)
    }

    /// Execute command synchronously with timeout (sandboxed).
    fn execute_sync_sandboxed(
        original_command: &str,
        working_dir: &std::path::Path,
        timeout_ms: u64,
        stdin_data: Option<&str>,
        exec_env: &ExecEnv,
    ) -> Result<ShellResult> {
        let started = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        let sandbox_type = exec_env.sandbox_type;
        let sandboxed = exec_env.is_sandboxed();

        // Build the command from ExecEnv
        let program = exec_env.program();
        let args = exec_env.args();

        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        install_parent_death_signal(&mut cmd);

        if stdin_data.is_some() {
            cmd.stdin(Stdio::piped());
        }

        child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to execute: {original_command}"))?;

        if let Some(input) = stdin_data
            && let Some(mut stdin) = child.stdin.take()
        {
            stdin
                .write_all(input.as_bytes())
                .context("Failed to write to stdin")?;
            stdin.flush().ok();
        }

        let stdout_handle = child.stdout.take().context("Failed to capture stdout")?;
        let stderr_handle = child.stderr.take().context("Failed to capture stderr")?;

        // Spawn threads to read output
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

        // Wait with timeout
        if let Some(status) = child.wait_timeout(timeout)? {
            let stdout = stdout_thread.join().unwrap_or_default();
            let stderr = stderr_thread.join().unwrap_or_default();
            let stdout_str = String::from_utf8_lossy(&stdout).to_string();
            let stderr_str = String::from_utf8_lossy(&stderr).to_string();
            let exit_code = status.code().unwrap_or(-1);

            // Check if sandbox denied the operation
            let sandbox_denied = SandboxManager::was_denied(sandbox_type, exit_code, &stderr_str);
            let (stdout, stdout_meta) = truncate_with_meta(&stdout_str);
            let (stderr, stderr_meta) = truncate_with_meta(&stderr_str);

            Ok(ShellResult {
                task_id: None,
                status: if status.success() {
                    ShellStatus::Completed
                } else {
                    ShellStatus::Failed
                },
                exit_code: status.code(),
                stdout,
                stderr,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                stdout_len: stdout_meta.original_len,
                stderr_len: stderr_meta.original_len,
                stdout_omitted: stdout_meta.omitted,
                stderr_omitted: stderr_meta.omitted,
                stdout_truncated: stdout_meta.truncated,
                stderr_truncated: stderr_meta.truncated,
                sandboxed,
                sandbox_type: if sandboxed {
                    Some(sandbox_type.to_string())
                } else {
                    None
                },
                sandbox_denied,
            })
        } else {
            // Timeout - kill the process
            #[cfg(unix)]
            let _ = kill_child_process_group(&mut child);
            #[cfg(not(unix))]
            let _ = child.kill();
            let status = child.wait().ok();
            let stdout = stdout_thread.join().unwrap_or_default();
            let stderr = stderr_thread.join().unwrap_or_default();
            let stdout_str = String::from_utf8_lossy(&stdout).to_string();
            let stderr_str = String::from_utf8_lossy(&stderr).to_string();
            let (stdout, stdout_meta) = truncate_with_meta(&stdout_str);
            let (stderr, stderr_meta) = truncate_with_meta(&stderr_str);

            Ok(ShellResult {
                task_id: None,
                status: ShellStatus::TimedOut,
                exit_code: status.and_then(|s| s.code()),
                stdout,
                stderr,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                stdout_len: stdout_meta.original_len,
                stderr_len: stderr_meta.original_len,
                stdout_omitted: stdout_meta.omitted,
                stderr_omitted: stderr_meta.omitted,
                stdout_truncated: stdout_meta.truncated,
                stderr_truncated: stderr_meta.truncated,
                sandboxed,
                sandbox_type: if sandboxed {
                    Some(sandbox_type.to_string())
                } else {
                    None
                },
                sandbox_denied: false,
            })
        }
    }

    /// Execute command interactively with timeout (sandboxed).
    fn execute_interactive_sandboxed(
        original_command: &str,
        working_dir: &std::path::Path,
        timeout_ms: u64,
        exec_env: &ExecEnv,
    ) -> Result<ShellResult> {
        let started = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        let sandbox_type = exec_env.sandbox_type;
        let sandboxed = exec_env.is_sandboxed();

        let program = exec_env.program();
        let args = exec_env.args();

        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(working_dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        install_parent_death_signal(&mut cmd);

        child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to execute: {original_command}"))?;

        if let Some(status) = child.wait_timeout(timeout)? {
            Ok(ShellResult {
                task_id: None,
                status: if status.success() {
                    ShellStatus::Completed
                } else {
                    ShellStatus::Failed
                },
                exit_code: status.code(),
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                stdout_len: 0,
                stderr_len: 0,
                stdout_omitted: 0,
                stderr_omitted: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                sandboxed,
                sandbox_type: if sandboxed {
                    Some(sandbox_type.to_string())
                } else {
                    None
                },
                sandbox_denied: false,
            })
        } else {
            #[cfg(unix)]
            let _ = kill_child_process_group(&mut child);
            #[cfg(not(unix))]
            let _ = child.kill();
            let status = child.wait().ok();

            Ok(ShellResult {
                task_id: None,
                status: ShellStatus::TimedOut,
                exit_code: status.and_then(|s| s.code()),
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                stdout_len: 0,
                stderr_len: 0,
                stdout_omitted: 0,
                stderr_omitted: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                sandboxed,
                sandbox_type: if sandboxed {
                    Some(sandbox_type.to_string())
                } else {
                    None
                },
                sandbox_denied: false,
            })
        }
    }

    /// Spawn a background process (sandboxed).
    fn spawn_background_sandboxed(
        &mut self,
        original_command: &str,
        working_dir: &std::path::Path,
        exec_env: &ExecEnv,
        stdin_data: Option<&str>,
        tty: bool,
    ) -> Result<ShellResult> {
        let task_id = format!("shell_{}", &Uuid::new_v4().to_string()[..8]);
        let started = Instant::now();
        let sandbox_type = exec_env.sandbox_type;
        let sandboxed = exec_env.is_sandboxed();

        // Build the command from ExecEnv
        let program = exec_env.program();
        let args = exec_env.args();

        let stdout_buffer = Arc::new(Mutex::new(Vec::new()));
        let stderr_buffer = if tty {
            None
        } else {
            Some(Arc::new(Mutex::new(Vec::new())))
        };

        let (child, stdin, stdout_thread, stderr_thread) = if tty {
            let pty_system = native_pty_system();
            let pair = pty_system
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .context("Failed to open PTY")?;

            let mut cmd = CommandBuilder::new(program);
            for arg in args {
                cmd.arg(arg);
            }
            cmd.cwd(working_dir);
            child_env::apply_to_pty_command(&mut cmd, child_env::string_map_env(&exec_env.env));

            let child = pair
                .slave
                .spawn_command(cmd)
                .with_context(|| format!("Failed to spawn PTY command: {original_command}"))?;
            drop(pair.slave);

            let reader = pair
                .master
                .try_clone_reader()
                .context("Failed to clone PTY reader")?;
            let stdout_thread = Some(spawn_reader_thread(reader, Arc::clone(&stdout_buffer)));
            let writer = pair
                .master
                .take_writer()
                .context("Failed to take PTY writer")?;

            (
                ShellChild::Pty(child),
                Some(StdinWriter::Pty(writer)),
                stdout_thread,
                None,
            )
        } else {
            let mut cmd = Command::new(program);
            cmd.args(args)
                .current_dir(working_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            {
                cmd.process_group(0);
            }

            child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));

            let mut child = cmd
                .spawn()
                .with_context(|| format!("Failed to spawn background: {original_command}"))?;

            let stdout_handle = child.stdout.take().context("Failed to capture stdout")?;
            let stderr_handle = child.stderr.take().context("Failed to capture stderr")?;
            let stdin_handle = child.stdin.take().map(StdinWriter::Pipe);

            let stdout_thread = Some(spawn_reader_thread(
                stdout_handle,
                Arc::clone(&stdout_buffer),
            ));
            let stderr_thread = stderr_buffer
                .as_ref()
                .map(|buffer| spawn_reader_thread(stderr_handle, Arc::clone(buffer)));

            (
                ShellChild::Process(child),
                stdin_handle,
                stdout_thread,
                stderr_thread,
            )
        };

        let mut bg_shell = BackgroundShell {
            id: task_id.clone(),
            command: original_command.to_string(),
            working_dir: working_dir.to_path_buf(),
            status: ShellStatus::Running,
            exit_code: None,
            started_at: started,
            sandbox_type,
            linked_task_id: None,
            stdout_buffer,
            stderr_buffer,
            stdout_cursor: 0,
            stderr_cursor: 0,
            stdin,
            child: Some(child),
            stdout_thread,
            stderr_thread,
        };

        if let Some(input) = stdin_data {
            bg_shell.write_stdin(input, false)?;
        }

        self.processes.insert(task_id.clone(), bg_shell);

        Ok(ShellResult {
            task_id: Some(task_id),
            status: ShellStatus::Running,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            stdout_len: 0,
            stderr_len: 0,
            stdout_omitted: 0,
            stderr_omitted: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            sandboxed,
            sandbox_type: if sandboxed {
                Some(sandbox_type.to_string())
            } else {
                None
            },
            sandbox_denied: false,
        })
    }

    /// Get output from a background process
    #[allow(dead_code)]
    pub fn get_output(
        &mut self,
        task_id: &str,
        block: bool,
        timeout_ms: u64,
    ) -> Result<ShellResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;

        if block && shell.status == ShellStatus::Running {
            let timeout = Duration::from_millis(timeout_ms.clamp(1000, 600_000));
            let deadline = Instant::now() + timeout;

            while shell.status == ShellStatus::Running && Instant::now() < deadline {
                if shell.poll() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }

            // If still running after timeout
            if shell.status == ShellStatus::Running {
                return Ok(shell.snapshot());
            }
        } else {
            shell.poll();
        }

        Ok(shell.snapshot())
    }

    /// Write data to stdin of a background process.
    pub fn write_stdin(&mut self, task_id: &str, input: &str, close: bool) -> Result<()> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;
        shell.write_stdin(input, close)?;
        Ok(())
    }

    /// Get incremental output from a background process, consuming any new output.
    fn get_output_delta(
        &mut self,
        task_id: &str,
        wait: bool,
        timeout_ms: u64,
    ) -> Result<ShellDeltaResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;

        if wait && shell.status == ShellStatus::Running {
            let timeout = Duration::from_millis(timeout_ms.clamp(1000, 600_000));
            let deadline = Instant::now() + timeout;

            while shell.status == ShellStatus::Running && Instant::now() < deadline {
                if shell.poll() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        } else {
            shell.poll();
        }

        let (
            stdout_delta,
            stderr_delta,
            stdout_delta_len,
            stderr_delta_len,
            stdout_total,
            stderr_total,
        ) = shell.take_delta();
        let (stdout, stdout_meta) = truncate_with_meta(&stdout_delta);
        let (stderr, stderr_meta) = truncate_with_meta(&stderr_delta);
        let sandboxed = !matches!(shell.sandbox_type, SandboxType::None);

        let command = shell.command.clone();
        let result = ShellResult {
            task_id: Some(shell.id.clone()),
            status: shell.status.clone(),
            exit_code: shell.exit_code,
            stdout,
            stderr,
            duration_ms: u64::try_from(shell.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout_len: stdout_meta.original_len.max(stdout_delta_len),
            stderr_len: stderr_meta.original_len.max(stderr_delta_len),
            stdout_omitted: stdout_meta.omitted,
            stderr_omitted: stderr_meta.omitted,
            stdout_truncated: stdout_meta.truncated,
            stderr_truncated: stderr_meta.truncated,
            sandboxed,
            sandbox_type: if sandboxed {
                Some(shell.sandbox_type.to_string())
            } else {
                None
            },
            sandbox_denied: shell.sandbox_denied(),
        };

        Ok(ShellDeltaResult {
            command,
            result,
            stdout_total_len: stdout_total,
            stderr_total_len: stderr_total,
        })
    }

    /// Kill a running background process
    #[allow(dead_code)]
    pub fn kill(&mut self, task_id: &str) -> Result<ShellResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;

        shell.kill()?;
        Ok(shell.snapshot())
    }

    /// Kill every currently running background shell process.
    pub fn kill_running(&mut self) -> Result<Vec<ShellResult>> {
        let ids = self
            .processes
            .iter()
            .filter(|(_, shell)| shell.status == ShellStatus::Running)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();

        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            results.push(self.kill(&id)?);
        }
        Ok(results)
    }

    /// Poll a background process and return incremental output.
    pub fn poll_delta(
        &mut self,
        task_id: &str,
        wait: bool,
        timeout_ms: u64,
    ) -> Result<ShellDeltaResult> {
        self.get_output_delta(task_id, wait, timeout_ms)
    }

    /// Attach durable task context to a live shell job.
    pub fn tag_linked_task(&mut self, task_id: &str, linked_task_id: Option<String>) -> Result<()> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("Task {task_id} not found"))?;
        shell.linked_task_id = linked_task_id;
        Ok(())
    }

    /// Inspect full output for a live or stale job.
    pub fn inspect_job(&mut self, task_id: &str) -> Result<ShellJobDetail> {
        if let Some(shell) = self.processes.get_mut(task_id) {
            shell.poll();
            return Ok(shell.job_detail());
        }
        if let Some(snapshot) = self.stale_jobs.get(task_id) {
            return Ok(ShellJobDetail {
                snapshot: snapshot.clone(),
                stdout: snapshot.stdout_tail.clone(),
                stderr: snapshot.stderr_tail.clone(),
            });
        }
        Err(anyhow!("Task {task_id} not found"))
    }

    /// List all live and known-stale background shell jobs for the TUI.
    pub fn list_jobs(&mut self) -> Vec<ShellJobSnapshot> {
        for shell in self.processes.values_mut() {
            shell.poll();
        }
        // Evict completed processes older than 1 hour to bound memory growth.
        self.cleanup(Duration::from_secs(3600));

        let mut jobs = self
            .processes
            .values()
            .map(BackgroundShell::job_snapshot)
            .collect::<Vec<_>>();
        jobs.extend(self.stale_jobs.values().cloned());
        jobs.sort_by(|a, b| {
            job_status_rank(&a.status, a.stale)
                .cmp(&job_status_rank(&b.status, b.stale))
                .then_with(|| a.id.cmp(&b.id))
        });
        jobs
    }

    /// Remember a restart-stale job so the UI can show it instead of hiding it.
    #[allow(dead_code)]
    pub fn remember_stale_job(
        &mut self,
        id: impl Into<String>,
        command: impl Into<String>,
        cwd: PathBuf,
        linked_task_id: Option<String>,
    ) {
        let id = id.into();
        self.stale_jobs.insert(
            id.clone(),
            ShellJobSnapshot {
                id: id.clone(),
                job_id: id,
                command: command.into(),
                cwd,
                status: ShellStatus::Killed,
                exit_code: None,
                elapsed_ms: 0,
                stdout_tail: String::new(),
                stderr_tail: "Process is no longer attached to this TUI session.".to_string(),
                stdout_len: 0,
                stderr_len: 0,
                stdin_available: false,
                stale: true,
                linked_task_id,
            },
        );
    }

    /// Clean up completed processes older than the given duration
    pub fn cleanup(&mut self, max_age: Duration) {
        let _now = Instant::now();
        self.processes.retain(|_, shell| {
            if shell.status == ShellStatus::Running {
                true
            } else {
                shell.started_at.elapsed() < max_age
            }
        });
    }
}

fn take_delta_from_buffer(buffer: &Arc<Mutex<Vec<u8>>>, cursor: &mut usize) -> (Vec<u8>, usize) {
    let data = buffer.lock().map(|d| d.clone()).unwrap_or_default();
    let start = (*cursor).min(data.len());
    let delta = data[start..].to_vec();
    *cursor = data.len();
    (delta, data.len())
}

fn tail_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let tail = text
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

fn job_status_rank(status: &ShellStatus, stale: bool) -> u8 {
    if stale {
        return 4;
    }
    match status {
        ShellStatus::Running => 0,
        ShellStatus::Failed | ShellStatus::TimedOut => 1,
        ShellStatus::Killed => 2,
        ShellStatus::Completed => 3,
    }
}

/// Thread-safe wrapper for `ShellManager`
pub type SharedShellManager = Arc<Mutex<ShellManager>>;

/// Create a new shared shell manager with default sandbox policy.
pub fn new_shared_shell_manager(workspace: PathBuf) -> SharedShellManager {
    Arc::new(Mutex::new(ShellManager::new(workspace)))
}

// === ToolSpec Implementations ===

use crate::command_safety::{SafetyLevel, analyze_command, extract_primary_command};
use crate::execpolicy::{ExecPolicyDecision, load_default_policy};
use crate::features::Feature;
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_u64, required_str,
};
use async_trait::async_trait;
use serde_json::json;

const FOREGROUND_TIMEOUT_RECOVERY_HINT: &str = "Foreground exec_shell is for bounded commands. \
The timed-out process was killed; rerun long work with task_shell_start or exec_shell with \
background: true, then poll with task_shell_wait or exec_shell_wait.";

const MACOS_PROVENANCE_HINT: &str = "Docker buildx failed to update its activity file due to a macOS \
com.apple.provenance restriction. Files created by Docker Desktop's signed process carry a \
kernel-enforced provenance tag that blocks writes from child processes (including the TUI \
shell sandbox). Workarounds: (1) run the Docker build from a regular terminal outside the \
TUI, or (2) disable BuildKit with DOCKER_BUILDKIT=0 (only works if your Dockerfiles do not \
use RUN --mount directives).";

pub(crate) fn looks_like_macos_provenance_failure(result: &ShellResult) -> bool {
    if matches!(result.status, ShellStatus::Completed) && result.exit_code == Some(0) {
        return false;
    }
    let combined = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
    combined.contains("com.apple.provenance")
        || combined.contains("update builder last activity")
        || (combined.contains("buildx/activity") && combined.contains("operation not permitted"))
}

fn macos_provenance_hint(result: &ShellResult) -> Option<&'static str> {
    if looks_like_macos_provenance_failure(result) {
        Some(MACOS_PROVENANCE_HINT)
    } else {
        None
    }
}

fn command_likely_needs_network(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    let Some(primary) = extract_primary_command(&normalized) else {
        return false;
    };
    let primary = primary.rsplit(['/', '\\']).next().unwrap_or(primary);

    match primary {
        "curl" | "wget" | "fetch" | "nc" | "netcat" | "ncat" | "ssh" | "scp" | "sftp" | "rsync"
        | "ftp" | "ping" | "traceroute" | "nslookup" | "dig" | "host" | "nmap" | "gh" | "hub" => {
            true
        }
        "git" => [
            " fetch",
            " pull",
            " clone",
            " ls-remote",
            " submodule",
            " push",
        ]
        .iter()
        .any(|needle| normalized.contains(needle)),
        "cargo" => [" install", " fetch", " update", " publish", " search"]
            .iter()
            .any(|needle| normalized.contains(needle)),
        "npm" | "pnpm" | "yarn" => [" install", " i", " add", " update", " publish"]
            .iter()
            .any(|needle| normalized.contains(needle)),
        "pip" | "pip3" | "uv" | "poetry" => [" install", " add", " sync", " update"]
            .iter()
            .any(|needle| normalized.contains(needle)),
        "brew" | "apt" | "apt-get" | "yum" | "dnf" | "pacman" => true,
        "go" => [" get", " install", " mod download"]
            .iter()
            .any(|needle| normalized.contains(needle)),
        _ => false,
    }
}

fn looks_like_network_blocked_failure(result: &ShellResult) -> bool {
    if matches!(result.status, ShellStatus::Completed | ShellStatus::Running)
        || result.exit_code == Some(0)
    {
        return false;
    }

    if result.stdout.trim() == "000" {
        return true;
    }
    if result.sandboxed && result.stdout.is_empty() && result.stderr.is_empty() {
        return true;
    }

    let output = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
    [
        "operation not permitted",
        "network is unreachable",
        "could not resolve host",
        "couldn't resolve host",
        "failed to resolve",
        "temporary failure in name resolution",
        "name or service not known",
        "nodename nor servname provided",
        "no address associated",
        "failed to connect",
        "couldn't connect",
        "connection timed out",
        "connection reset",
    ]
    .iter()
    .any(|pattern| output.contains(pattern))
}

fn shell_network_restricted_hint<'a>(
    context: &'a ToolContext,
    command: &str,
    result: &ShellResult,
) -> Option<&'a str> {
    let hint = context.shell_network_denied_hint.as_deref()?;
    let policy_blocks_network = context
        .elevated_sandbox_policy
        .as_ref()
        .is_some_and(|policy| !policy.has_network_access());
    if !policy_blocks_network || !command_likely_needs_network(command) {
        return None;
    }
    if result.sandbox_denied || looks_like_network_blocked_failure(result) {
        Some(hint)
    } else {
        None
    }
}

async fn execute_foreground_via_background(
    context: &ToolContext,
    command: &str,
    timeout_ms: u64,
    stdin_data: Option<&str>,
    policy_override: Option<ExecutionSandboxPolicy>,
    extra_env: HashMap<String, String>,
) -> Result<ShellResult> {
    let timeout_ms = timeout_ms.clamp(1000, 600_000);
    let spawned = {
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?;
        manager.clear_foreground_background_request();
        manager.execute_with_options_env(
            command,
            None,
            timeout_ms,
            true,
            stdin_data,
            false,
            policy_override,
            extra_env,
        )?
    };
    let task_id = spawned
        .task_id
        .ok_or_else(|| anyhow!("foreground shell did not return a process id"))?;

    if stdin_data.is_some() {
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?;
        manager.write_stdin(&task_id, "", true)?;
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if context
            .cancel_token
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| anyhow!("shell manager lock poisoned"))?;
            return manager.kill(&task_id);
        }

        let snapshot = {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| anyhow!("shell manager lock poisoned"))?;
            if manager.take_foreground_background_request() {
                return manager.get_output(&task_id, false, 0);
            }
            manager.get_output(&task_id, false, 0)?
        };

        if snapshot.status != ShellStatus::Running {
            return Ok(snapshot);
        }

        if Instant::now() >= deadline {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| anyhow!("shell manager lock poisoned"))?;
            let mut result = manager.kill(&task_id)?;
            result.status = ShellStatus::TimedOut;
            return Ok(result);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Tool for executing shell commands.
pub struct ExecShellTool;

#[async_trait]
impl ToolSpec for ExecShellTool {
    fn name(&self) -> &'static str {
        "exec_shell"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command in the workspace directory. Foreground mode is for bounded commands; use background=true or task_shell_start for long-running work, then poll/wait."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 120000, max: 600000)"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background and return task_id (default: false). Prefer true for commands that may run for minutes; poll with exec_shell_wait or task_shell_wait."
                },
                "interactive": {
                    "type": "boolean",
                    "description": "Run interactively with terminal IO (default: false)"
                },
                "stdin": {
                    "type": "string",
                    "description": "Optional stdin data to send before waiting (non-interactive only)"
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory for the command"
                },
                "tty": {
                    "type": "boolean",
                    "description": "Allocate a pseudo-terminal for interactive programs (implies background)"
                }
            },
            "required": ["command"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::Sandboxable,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let command = required_str(&input, "command")?;
        let timeout_ms = optional_u64(&input, "timeout_ms", 120_000).min(600_000);
        let background = optional_bool(&input, "background", false);
        let interactive = optional_bool(&input, "interactive", false);
        let tty = optional_bool(&input, "tty", false);
        let stdin_data = input
            .get("stdin")
            .or_else(|| input.get("input"))
            .or_else(|| input.get("data"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        if interactive && background {
            return Ok(ToolResult::error(
                "Interactive commands cannot run in background mode.",
            ));
        }
        if interactive && tty {
            return Ok(ToolResult::error(
                "Interactive mode cannot be combined with TTY sessions.",
            ));
        }
        if interactive && stdin_data.is_some() {
            return Ok(ToolResult::error(
                "Interactive mode cannot be combined with stdin data.",
            ));
        }

        let background = background || tty;

        let mut execpolicy_decision: Option<ExecPolicyDecision> = None;
        if context.features.enabled(Feature::ExecPolicy)
            && let Some(policy) = load_default_policy()
                .map_err(|e| ToolError::execution_failed(format!("execpolicy load failed: {e}")))?
        {
            let decision = policy.evaluate(command);
            execpolicy_decision = Some(decision.clone());
            if let ExecPolicyDecision::Deny(reason) = decision {
                return Ok(ToolResult {
                    content: format!("BLOCKED: {reason}"),
                    success: false,
                    metadata: Some(json!({
                        "execpolicy": {
                            "decision": "deny",
                            "reason": reason,
                        }
                    })),
                });
            }
        }

        // Safety analysis (always run for metadata, but only block when not in YOLO mode)
        let safety = analyze_command(command);
        if !context.auto_approve {
            match safety.level {
                SafetyLevel::Dangerous => {
                    let reasons = safety.reasons.join("; ");
                    let suggestions = if safety.suggestions.is_empty() {
                        String::new()
                    } else {
                        format!("\nSuggestions: {}", safety.suggestions.join("; "))
                    };
                    return Ok(ToolResult {
                        content: format!(
                            "BLOCKED: This command was blocked for safety reasons.\n\nReasons: {reasons}{suggestions}"
                        ),
                        success: false,
                        metadata: Some(json!({
                            "safety_level": "dangerous",
                            "blocked": true,
                            "reasons": safety.reasons,
                            "suggestions": safety.suggestions,
                        })),
                    });
                }
                SafetyLevel::RequiresApproval | SafetyLevel::Safe | SafetyLevel::WorkspaceSafe => {
                    // Proceed normally
                }
            }
        }

        let policy_override = context.elevated_sandbox_policy.clone();
        let working_dir = match input
            .get("cwd")
            .or_else(|| input.get("working_dir"))
            .and_then(serde_json::Value::as_str)
        {
            Some(dir) => {
                // Validate cwd against workspace boundary (same as file tools)
                let resolved = context.resolve_path(dir)?;
                Some(resolved.to_string_lossy().to_string())
            }
            None => None,
        };

        // #456 — collect env from any configured `shell_env` hooks. Runs
        // synchronously, captures stdout, parses `KEY=VAL` lines, audit-logs
        // the keys (never the values). Empty / no-op when no hook is
        // configured.
        let extra_env = if let Some(hook_executor) = &context.runtime.hook_executor {
            let hook_ctx = crate::hooks::HookContext::new()
                .with_tool_name("exec_shell")
                .with_tool_args(&input);
            hook_executor.collect_shell_env(&hook_ctx)
        } else {
            std::collections::HashMap::new()
        };

        // Route through external sandbox backend when configured.
        if let Some(backend) = &context.sandbox_backend {
            if interactive {
                return Ok(ToolResult::error(
                    "Interactive mode is not supported with external sandbox backends.",
                ));
            }
            if background {
                return Ok(ToolResult::error(
                    "Background mode is not supported with external sandbox backends.",
                ));
            }
            if tty {
                return Ok(ToolResult::error(
                    "TTY mode is not supported with external sandbox backends.",
                ));
            }

            let started = std::time::Instant::now();
            let backend_result = backend.exec(command, &extra_env).await;

            let result = match backend_result {
                Ok(output) => {
                    let (stdout, stdout_meta) = truncate_with_meta(&output.stdout);
                    let (stderr, stderr_meta) = truncate_with_meta(&output.stderr);
                    ShellResult {
                        task_id: None,
                        status: if output.exit_code == 0 {
                            ShellStatus::Completed
                        } else {
                            ShellStatus::Failed
                        },
                        exit_code: Some(output.exit_code),
                        stdout,
                        stderr,
                        duration_ms: u64::try_from(started.elapsed().as_millis())
                            .unwrap_or(u64::MAX),
                        stdout_len: stdout_meta.original_len,
                        stderr_len: stderr_meta.original_len,
                        stdout_omitted: stdout_meta.omitted,
                        stderr_omitted: stderr_meta.omitted,
                        stdout_truncated: stdout_meta.truncated,
                        stderr_truncated: stderr_meta.truncated,
                        sandboxed: true,
                        sandbox_type: Some("opensandbox".to_string()),
                        sandbox_denied: false,
                    }
                }
                Err(e) => {
                    return Ok(ToolResult::error(format!("Sandbox backend error: {e}")));
                }
            };

            // Build result (reuse the existing output rendering below).
            let stdout_summary = summarize_output(&result.stdout);
            let stderr_summary = summarize_output(&result.stderr);
            let summary = if !stderr_summary.is_empty() {
                stderr_summary.clone()
            } else {
                stdout_summary.clone()
            };
            let output = if result.stdout.is_empty() && result.stderr.is_empty() {
                "(no output)".to_string()
            } else if result.stderr.is_empty() {
                result.stdout.clone()
            } else {
                format!("{}\n\nSTDERR:\n{}", result.stdout, result.stderr)
            };

            let metadata = json!({
                "exit_code": result.exit_code,
                "status": format!("{:?}", result.status),
                "duration_ms": result.duration_ms,
                "sandboxed": true,
                "sandbox_type": "opensandbox",
                "sandbox_denied": false,
                "task_id": result.task_id,
                "stdout_len": result.stdout_len,
                "stderr_len": result.stderr_len,
                "stdout_truncated": result.stdout_truncated,
                "stderr_truncated": result.stderr_truncated,
                "stdout_omitted": result.stdout_omitted,
                "stderr_omitted": result.stderr_omitted,
                "summary": summary,
                "stdout_summary": stdout_summary,
                "stderr_summary": stderr_summary,
                "safety_level": format!("{:?}", safety.level),
                "interactive": false,
                "canceled": false,
                "sandbox_backend": "opensandbox",
            });

            return Ok(ToolResult {
                content: output,
                success: result.status == ShellStatus::Completed,
                metadata: Some(metadata),
            });
        }

        let result = if interactive {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            manager.execute_interactive_with_policy_env(
                command,
                working_dir.as_deref(),
                timeout_ms,
                policy_override,
                extra_env,
            )
        } else if background {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            manager.execute_with_options_env(
                command,
                working_dir.as_deref(),
                timeout_ms,
                true,
                stdin_data.as_deref(),
                tty,
                policy_override,
                extra_env,
            )
        } else {
            execute_foreground_via_background(
                context,
                command,
                timeout_ms,
                stdin_data.as_deref(),
                policy_override,
                extra_env,
            )
            .await
        };

        match result {
            Ok(result) => {
                let backgrounded_foreground =
                    !background && !interactive && result.status == ShellStatus::Running;
                if (background || backgrounded_foreground)
                    && let (Some(shell_id), Some(task_id)) = (
                        result.task_id.as_deref(),
                        context.runtime.active_task_id.clone(),
                    )
                    && let Ok(mut manager) = context.shell_manager.lock()
                {
                    let _ = manager.tag_linked_task(shell_id, Some(task_id));
                }

                let was_cancelled = context
                    .cancel_token
                    .as_ref()
                    .is_some_and(|token| token.is_cancelled());
                let task_id_str = result.task_id.clone().unwrap_or_default();
                let stdout_summary = summarize_output(&result.stdout);
                let stderr_summary = summarize_output(&result.stderr);
                let summary = if !stderr_summary.is_empty() {
                    stderr_summary.clone()
                } else {
                    stdout_summary.clone()
                };
                let network_restricted_hint =
                    shell_network_restricted_hint(context, command, &result).map(str::to_string);
                let provenance_hint = macos_provenance_hint(&result);
                let mut output = if interactive {
                    format!(
                        "Interactive command completed (exit code: {:?})",
                        result.exit_code
                    )
                } else if result.status == ShellStatus::Completed {
                    if result.stdout.is_empty() && result.stderr.is_empty() {
                        "(no output)".to_string()
                    } else if result.stderr.is_empty() {
                        result.stdout.clone()
                    } else {
                        format!("{}\n\nSTDERR:\n{}", result.stdout, result.stderr)
                    }
                } else if result.status == ShellStatus::Running {
                    if backgrounded_foreground {
                        format!(
                            "Command moved to background: {task_id_str}\n\nPoll with exec_shell_wait or cancel with exec_shell_cancel."
                        )
                    } else {
                        format!("Background task started: {task_id_str}")
                    }
                } else if result.status == ShellStatus::Killed && was_cancelled {
                    format!(
                        "Command canceled; process killed.\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
                        result.stdout, result.stderr
                    )
                } else if result.status == ShellStatus::TimedOut {
                    format!(
                        "Command timed out after {timeout_ms}ms; process killed.\n\n{FOREGROUND_TIMEOUT_RECOVERY_HINT}\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
                        result.stdout, result.stderr
                    )
                } else {
                    format!(
                        "Command failed (exit code: {:?})\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
                        result.exit_code, result.stdout, result.stderr
                    )
                };
                if let Some(hint) = network_restricted_hint.as_deref() {
                    output = format!("{hint}\n\n{output}");
                }
                if let Some(hint) = provenance_hint {
                    output = format!("{hint}\n\n{output}");
                }

                let mut metadata = json!({
                    "exit_code": result.exit_code,
                    "status": format!("{:?}", result.status),
                    "duration_ms": result.duration_ms,
                    "sandboxed": result.sandboxed,
                    "sandbox_type": result.sandbox_type,
                    "sandbox_denied": result.sandbox_denied,
                    "task_id": result.task_id,
                    "stdout_len": result.stdout_len,
                    "stderr_len": result.stderr_len,
                    "stdout_truncated": result.stdout_truncated,
                    "stderr_truncated": result.stderr_truncated,
                    "stdout_omitted": result.stdout_omitted,
                    "stderr_omitted": result.stderr_omitted,
                    "summary": summary,
                    "stdout_summary": stdout_summary,
                    "stderr_summary": stderr_summary,
                    "safety_level": format!("{:?}", safety.level),
                    "interactive": interactive,
                    "canceled": was_cancelled,
                    "execpolicy": execpolicy_decision.as_ref().map(|decision| match decision {
                        ExecPolicyDecision::Allow => json!({
                            "decision": "allow",
                        }),
                        ExecPolicyDecision::Deny(reason) => json!({
                            "decision": "deny",
                            "reason": reason,
                        }),
                        ExecPolicyDecision::AskUser(reason) => json!({
                            "decision": "ask_user",
                            "reason": reason,
                        }),
                    }),
                });
                metadata["backgrounded"] = json!(background || backgrounded_foreground);
                if result.status == ShellStatus::TimedOut && !background && !interactive {
                    metadata["foreground_timeout_recovery"] = json!({
                        "process_killed": true,
                        "hint": FOREGROUND_TIMEOUT_RECOVERY_HINT,
                        "recommended_tools": [
                            "task_shell_start",
                            "task_shell_wait",
                            "exec_shell",
                            "exec_shell_wait"
                        ],
                        "exec_shell_background": true,
                        "poll_with": ["task_shell_wait", "exec_shell_wait"]
                    });
                }
                if let Some(hint) = network_restricted_hint {
                    metadata["sandbox_network_restricted"] = json!(true);
                    metadata["sandbox_network_denied_hint"] = json!(hint);
                }
                if provenance_hint.is_some() {
                    metadata["macos_provenance_restricted"] = json!(true);
                }

                Ok(ToolResult {
                    content: output,
                    success: result.status == ShellStatus::Completed
                        || result.status == ShellStatus::Running,
                    metadata: Some(metadata),
                })
            }
            Err(e) => Ok(ToolResult::error(format!("Shell execution failed: {e}"))),
        }
    }
}

pub struct ShellWaitTool {
    name: &'static str,
}

impl ShellWaitTool {
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

pub struct ShellInteractTool {
    name: &'static str,
}

impl ShellInteractTool {
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

fn required_task_id(input: &serde_json::Value) -> Result<&str, ToolError> {
    input
        .get("task_id")
        .or_else(|| input.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ToolError::missing_field("task_id"))
}

fn build_shell_delta_tool_result(delta: ShellDeltaResult, context: &ToolContext) -> ToolResult {
    let result = delta.result;
    let network_restricted_hint =
        shell_network_restricted_hint(context, &delta.command, &result).map(str::to_string);
    let provenance_hint = macos_provenance_hint(&result);
    let stdout_summary = summarize_output(&result.stdout);
    let stderr_summary = summarize_output(&result.stderr);
    let summary = if !stderr_summary.is_empty() {
        stderr_summary.clone()
    } else {
        stdout_summary.clone()
    };

    let mut output = if result.stdout.is_empty() && result.stderr.is_empty() {
        match result.status {
            ShellStatus::Running => "Background task running (no new output).".to_string(),
            ShellStatus::Completed => "(no new output)".to_string(),
            ShellStatus::Failed => format!("Command failed (exit code: {:?})", result.exit_code),
            ShellStatus::TimedOut => "Command timed out (no new output).".to_string(),
            ShellStatus::Killed => "Command killed (no new output).".to_string(),
        }
    } else if result.stderr.is_empty() {
        result.stdout.clone()
    } else {
        format!("{}\n\nSTDERR:\n{}", result.stdout, result.stderr)
    };
    if let Some(hint) = network_restricted_hint.as_deref() {
        output = format!("{hint}\n\n{output}");
    }
    if let Some(hint) = provenance_hint {
        output = format!("{hint}\n\n{output}");
    }

    let mut tool_result = ToolResult {
        content: output,
        success: matches!(result.status, ShellStatus::Completed | ShellStatus::Running),
        metadata: Some(json!({
            "exit_code": result.exit_code,
            "status": format!("{:?}", result.status),
            "duration_ms": result.duration_ms,
            "sandboxed": result.sandboxed,
            "sandbox_type": result.sandbox_type,
            "sandbox_denied": result.sandbox_denied,
            "task_id": result.task_id,
            "stdout_len": result.stdout_len,
            "stderr_len": result.stderr_len,
            "stdout_truncated": result.stdout_truncated,
            "stderr_truncated": result.stderr_truncated,
            "stdout_omitted": result.stdout_omitted,
            "stderr_omitted": result.stderr_omitted,
            "stdout_total_len": delta.stdout_total_len,
            "stderr_total_len": delta.stderr_total_len,
            "summary": summary,
            "stdout_summary": stdout_summary,
            "stderr_summary": stderr_summary,
            "stream_delta": true,
        })),
    };
    if let Some(hint) = network_restricted_hint
        && let Some(metadata) = tool_result.metadata.as_mut()
        && let Some(object) = metadata.as_object_mut()
    {
        object.insert("sandbox_network_restricted".to_string(), json!(true));
        object.insert("sandbox_network_denied_hint".to_string(), json!(hint));
    }
    if provenance_hint.is_some()
        && let Some(metadata) = tool_result.metadata.as_mut()
        && let Some(object) = metadata.as_object_mut()
    {
        object.insert("macos_provenance_restricted".to_string(), json!(true));
    }
    tool_result
}

async fn wait_for_shell_delta_cancellable(
    context: &ToolContext,
    task_id: &str,
    timeout_ms: u64,
) -> Result<(ShellDeltaResult, bool), ToolError> {
    let timeout_ms = timeout_ms.clamp(1000, 600_000);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut stdout_accum = String::new();
    let mut stderr_accum = String::new();

    let (command, result, stdout_total_len, stderr_total_len) = loop {
        if context
            .cancel_token
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            let delta = manager
                .get_output_delta(task_id, false, 0)
                .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            append_shell_delta_output(&mut stdout_accum, &mut stderr_accum, &delta.result);
            return Ok((
                shell_delta_with_accumulated_output(
                    delta.command,
                    delta.result,
                    &stdout_accum,
                    &stderr_accum,
                    delta.stdout_total_len,
                    delta.stderr_total_len,
                ),
                true,
            ));
        }

        let delta = {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            manager
                .get_output_delta(task_id, false, 0)
                .map_err(|err| ToolError::execution_failed(err.to_string()))?
        };

        let stdout_total_len = delta.stdout_total_len;
        let stderr_total_len = delta.stderr_total_len;
        let command = delta.command.clone();
        append_shell_delta_output(&mut stdout_accum, &mut stderr_accum, &delta.result);

        let status = delta.result.status.clone();
        if status != ShellStatus::Running || Instant::now() >= deadline {
            break (command, delta.result, stdout_total_len, stderr_total_len);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    Ok((
        shell_delta_with_accumulated_output(
            command,
            result,
            &stdout_accum,
            &stderr_accum,
            stdout_total_len,
            stderr_total_len,
        ),
        false,
    ))
}

fn append_shell_delta_output(
    stdout_accum: &mut String,
    stderr_accum: &mut String,
    result: &ShellResult,
) {
    if !result.stdout.is_empty() {
        stdout_accum.push_str(&result.stdout);
    }
    if !result.stderr.is_empty() {
        stderr_accum.push_str(&result.stderr);
    }
}

fn shell_delta_with_accumulated_output(
    command: String,
    mut result: ShellResult,
    stdout_accum: &str,
    stderr_accum: &str,
    stdout_total_len: usize,
    stderr_total_len: usize,
) -> ShellDeltaResult {
    let (stdout, stdout_meta) = truncate_with_meta(stdout_accum);
    let (stderr, stderr_meta) = truncate_with_meta(stderr_accum);
    result.stdout = stdout;
    result.stderr = stderr;
    result.stdout_len = stdout_meta.original_len;
    result.stderr_len = stderr_meta.original_len;
    result.stdout_omitted = stdout_meta.omitted;
    result.stderr_omitted = stderr_meta.omitted;
    result.stdout_truncated = stdout_meta.truncated;
    result.stderr_truncated = stderr_meta.truncated;

    ShellDeltaResult {
        command,
        result,
        stdout_total_len,
        stderr_total_len,
    }
}

pub struct ShellCancelTool;

#[async_trait]
impl ToolSpec for ShellCancelTool {
    fn name(&self) -> &'static str {
        "exec_shell_cancel"
    }

    fn description(&self) -> &'static str {
        "Cancel a running background shell task by task_id, or cancel all running background shell tasks with all=true."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID returned by exec_shell or task_shell_start"
                },
                "id": {
                    "type": "string",
                    "description": "Alias for task_id"
                },
                "all": {
                    "type": "boolean",
                    "description": "Cancel all currently running background shell tasks"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let cancel_all = optional_bool(&input, "all", false);
        let mut manager = context
            .shell_manager
            .lock()
            .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;

        if cancel_all {
            let results = manager
                .kill_running()
                .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            if results.is_empty() {
                return Ok(ToolResult {
                    content: "No running background shell jobs.".to_string(),
                    success: true,
                    metadata: Some(json!({
                        "status": "Noop",
                        "canceled": 0,
                        "task_ids": [],
                    })),
                });
            }

            let task_ids = results
                .iter()
                .filter_map(|result| result.task_id.clone())
                .collect::<Vec<_>>();
            return Ok(ToolResult {
                content: format!(
                    "Canceled {} background shell job{}: {}",
                    task_ids.len(),
                    if task_ids.len() == 1 { "" } else { "s" },
                    task_ids.join(", ")
                ),
                success: true,
                metadata: Some(json!({
                    "status": "Killed",
                    "canceled": task_ids.len(),
                    "task_ids": task_ids,
                })),
            });
        }

        let task_id = required_task_id(&input)?;
        let result = manager
            .kill(task_id)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        let task_id = result
            .task_id
            .clone()
            .unwrap_or_else(|| task_id.to_string());
        Ok(ToolResult {
            content: format!("Canceled background shell job: {task_id}"),
            success: true,
            metadata: Some(json!({
                "status": format!("{:?}", result.status),
                "task_id": task_id,
                "exit_code": result.exit_code,
                "duration_ms": result.duration_ms,
            })),
        })
    }
}

#[async_trait]
impl ToolSpec for ShellWaitTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "Wait for a background shell task and return incremental output. Turn cancellation stops waiting but leaves the background task running."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID returned by exec_shell"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 5000)"
                },
                "wait": {
                    "type": "boolean",
                    "description": "Wait for completion before returning (default: true)"
                }
            },
            "required": ["task_id"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task_id = required_task_id(&input)?;
        let wait = optional_bool(&input, "wait", true);
        let timeout_ms = optional_u64(&input, "timeout_ms", 5_000);

        let (delta, wait_canceled) = if wait {
            wait_for_shell_delta_cancellable(context, task_id, timeout_ms).await?
        } else {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            let delta = manager
                .get_output_delta(task_id, false, timeout_ms)
                .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            (delta, false)
        };

        let status = delta.result.status.clone();
        let mut result = build_shell_delta_tool_result(delta, context);
        if wait_canceled {
            if matches!(status, ShellStatus::Running) {
                result.content = format!(
                    "Wait canceled; background shell task {task_id} is still running.\n\n{}",
                    result.content
                );
            }
            if let Some(metadata) = result.metadata.as_mut()
                && let Some(object) = metadata.as_object_mut()
            {
                object.insert("wait_canceled".to_string(), json!(true));
            }
        }

        Ok(result)
    }
}

#[async_trait]
impl ToolSpec for ShellInteractTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "Send input to a background shell task and return incremental output."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID returned by exec_shell"
                },
                "input": {
                    "type": "string",
                    "description": "Input to send to the task's stdin"
                },
                "stdin": {
                    "type": "string",
                    "description": "Alias for input"
                },
                "data": {
                    "type": "string",
                    "description": "Alias for input"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Wait for output after sending input (default: 1000)"
                },
                "close_stdin": {
                    "type": "boolean",
                    "description": "Close stdin after sending input"
                }
            },
            "required": ["task_id"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ExecutesCode]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task_id = required_task_id(&input)?;
        let close_stdin = optional_bool(&input, "close_stdin", false);
        let timeout_ms = optional_u64(&input, "timeout_ms", 1_000);
        let interaction_input = input
            .get("input")
            .or_else(|| input.get("stdin"))
            .or_else(|| input.get("data"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            if !interaction_input.is_empty() || close_stdin {
                manager
                    .write_stdin(task_id, interaction_input, close_stdin)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            }
        }

        let mut elapsed = 0u64;
        loop {
            if context
                .cancel_token
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                let mut manager = context
                    .shell_manager
                    .lock()
                    .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
                let delta = manager
                    .get_output_delta(task_id, false, 0)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?;
                let mut result = build_shell_delta_tool_result(delta, context);
                if let Some(metadata) = result.metadata.as_mut()
                    && let Some(object) = metadata.as_object_mut()
                {
                    object.insert("wait_canceled".to_string(), json!(true));
                }
                return Ok(result);
            }

            let delta = {
                let mut manager = context
                    .shell_manager
                    .lock()
                    .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
                manager
                    .get_output_delta(task_id, false, 0)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?
            };

            if !delta.result.stdout.is_empty()
                || !delta.result.stderr.is_empty()
                || delta.result.status != ShellStatus::Running
                || elapsed >= timeout_ms
            {
                return Ok(build_shell_delta_tool_result(delta, context));
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
            elapsed = elapsed.saturating_add(50);
        }
    }
}

/// Tool for appending notes to a notes file.
pub struct NoteTool;

#[async_trait]
impl ToolSpec for NoteTool {
    fn name(&self) -> &'static str {
        "note"
    }

    fn description(&self) -> &'static str {
        "Append a note to the agent notes file for persistent context across sessions."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The note content to append"
                }
            },
            "required": ["content"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto // Notes are low-risk
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let note_content = required_str(&input, "content")?;

        // Ensure parent directory exists
        if let Some(parent) = context.notes_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::execution_failed(format!("Failed to create notes directory: {e}"))
            })?;
        }

        // Append to notes file
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&context.notes_path)
            .map_err(|e| ToolError::execution_failed(format!("Failed to open notes file: {e}")))?;

        writeln!(file, "\n---\n{note_content}")
            .map_err(|e| ToolError::execution_failed(format!("Failed to write note: {e}")))?;

        Ok(ToolResult::success(format!(
            "Note appended to {}",
            context.notes_path.display()
        )))
    }
}

#[cfg(test)]
mod tests;
