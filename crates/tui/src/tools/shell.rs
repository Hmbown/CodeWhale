//! Advanced shell execution with background process support and sandboxing.
//!
//! Provides:
//! - Synchronous command execution with timeout
//! - Background process execution
//! - Process output retrieval
//! - Process termination
//! - Sandbox support (macOS Seatbelt and opt-in Linux bubblewrap)
//! - Streaming output (future)

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;
use wait_timeout::ChildExt;

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(not(target_env = "ohos"))]
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

mod output;

use super::shell_output::{summarize_output, truncate_with_meta};
use crate::child_env;
use crate::sandbox::{
    CommandSpec,
    ExecEnv,
    SandboxManager,
    SandboxPolicy as ExecutionSandboxPolicy, // Rename to avoid conflict with spec::SandboxPolicy
    SandboxType,
};
use crate::work_graph::{
    EvidenceKind, EvidenceRef, OperationIntent, OperationOwnerSnapshot, OwnerState,
    SharedWorkRuntime,
};
use crate::worker_profile::ShellPolicy;
use output::{tail_from_buffer, take_delta_from_buffer};

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
    /// Lossless process exit status. Windows exception/NTSTATUS values use
    /// the full unsigned 32-bit range, so an i32 would corrupt them.
    pub exit_code: Option<i64>,
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
    pub exit_code: Option<i64>,
    pub elapsed_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub stdout_len: usize,
    pub stderr_len: usize,
    pub stdin_available: bool,
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_since_output_ms: Option<u64>,
    pub linked_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_name: Option<String>,
}

/// Once-only completion event for a tracked background shell job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellCompletionEvent {
    pub task_id: String,
    pub command: String,
    pub status: ShellStatus,
    pub exit_code: Option<i64>,
    pub duration_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub linked_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_name: Option<String>,
}

/// Optional owner attribution for background shell work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellJobOwner {
    pub agent_id: String,
    pub agent_name: String,
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
    #[cfg(not(target_env = "ohos"))]
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
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
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

/// Attach `args` to a `std::process::Command`, honoring shell-quoting on
/// Windows.
///
/// Issue #1691: on Windows the shell command is invoked as
/// `cmd /C "chcp 65001 >NUL & <command>"`. Rust's `Command::arg` applies
/// MSVCRT (`CommandLineToArgvW`) escaping, turning the embedded `"` in a
/// quoted argument (e.g. `git commit -m "feat: complete sub-pages"`) into
/// `\"`. `cmd.exe` does NOT use MSVCRT parsing — it treats `\` literally and
/// `"` as a bare quote toggle — so the escaped payload is mis-tokenized and
/// `git` receives `feat:`, `complete`, `sub-pages"` as separate pathspecs
/// (the reported `pathspec 'sub-pages"' did not match` symptom). Passing the
/// `cmd /C` payload through `CommandExt::raw_arg` suppresses std's escaping so
/// the string reaches `cmd.exe` verbatim, exactly as a terminal would.
#[cfg(windows)]
fn push_shell_args(cmd: &mut Command, program: &str, args: &[String]) {
    use std::os::windows::process::CommandExt;
    // The `cmd /C <payload>` shape is the only place std's per-arg escaping
    // corrupts a quoted command. Pass `/C` and the payload raw so the quotes
    // survive; any other program keeps normal (correct) escaping. Match `cmd`
    // by file stem so a full path (`C:\Windows\System32\cmd.exe`) or `.exe`
    // suffix still triggers the raw-arg path.
    let is_cmd = std::path::Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("cmd"))
        .unwrap_or(false);
    if is_cmd && args.len() == 2 && args[0].eq_ignore_ascii_case("/C") {
        cmd.raw_arg(&args[0]);
        cmd.raw_arg(&args[1]);
    } else {
        cmd.args(args);
    }
}

#[cfg(not(windows))]
fn push_shell_args(cmd: &mut Command, _program: &str, args: &[String]) {
    // Unix delegates tokenization entirely to `sh -c <command>`; the command
    // string is passed as a single argv entry and never split by us.
    cmd.args(args);
}

#[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
fn install_parent_death_signal(_cmd: &mut Command) {
    // No kernel-level equivalent on macOS / Windows. The cooperative
    // cancellation + process_group SIGKILL path covers normal shutdown;
    // abnormal exit (panic without unwind, SIGKILL of the TUI) can still
    // leak children on those platforms — tracked as a follow-up.
}

#[cfg(windows)]
#[derive(Debug)]
struct WindowsJob {
    handle: HANDLE,
}

#[cfg(windows)]
// SAFETY: Windows job handles are process-wide kernel handles. Moving the
// wrapper between threads does not invalidate the handle, and access is
// externally synchronized by ShellManager's mutex.
unsafe impl Send for WindowsJob {}
#[cfg(windows)]
// SAFETY: The wrapper exposes only terminate/drop operations around a kernel
// handle; concurrent use is guarded by ShellManager.
unsafe impl Sync for WindowsJob {}

#[cfg(windows)]
impl WindowsJob {
    fn attach_to_child(child: &Child) -> std::io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()).map_err(windows_io_error)? };
        let job = Self { handle };

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(windows_io_error)?;

            let process_handle = HANDLE(child.as_raw_handle());
            AssignProcessToJobObject(job.handle, process_handle).map_err(windows_io_error)?;
        }

        Ok(job)
    }

    fn terminate(&self) -> std::io::Result<()> {
        unsafe { TerminateJobObject(self.handle, 1).map_err(windows_io_error) }
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn windows_io_error(error: windows::core::Error) -> std::io::Error {
    std::io::Error::other(error)
}

#[cfg(windows)]
fn terminate_windows_job(job: Option<&WindowsJob>, child: &mut Child) -> std::io::Result<()> {
    if let Some(job) = job {
        match job.terminate() {
            Ok(()) => return Ok(()),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "failed to terminate Windows job object; falling back to immediate child kill"
                );
            }
        }
    }
    child.kill()
}

#[cfg(windows)]
fn terminate_and_close_windows_job(windows_job: Option<WindowsJob>) {
    if let Some(job) = windows_job.as_ref()
        && let Err(err) = job.terminate()
    {
        tracing::warn!(
            ?err,
            "failed to terminate Windows shell job before closing job handle"
        );
    }
    drop(windows_job);
}

#[cfg(windows)]
fn terminate_child_and_close_windows_job(
    windows_job: Option<WindowsJob>,
    child: &mut Child,
) -> std::io::Result<()> {
    let result = terminate_windows_job(windows_job.as_ref(), child);
    drop(windows_job);
    result
}

#[cfg(windows)]
fn attach_windows_job(child: &Child, command: &str) -> Option<WindowsJob> {
    match WindowsJob::attach_to_child(child) {
        Ok(job) => Some(job),
        Err(error) => {
            tracing::warn!(
                ?error,
                command,
                "failed to attach Windows shell process to job object; descendant cleanup degraded"
            );
            None
        }
    }
}

#[cfg(windows)]
fn terminate_unregistered_process(child: &mut Child, job: Option<&WindowsJob>) {
    let _ = terminate_windows_job(job, child);
    let _ = child.wait();
}

#[cfg(not(windows))]
fn terminate_unregistered_process(child: &mut Child) {
    #[cfg(unix)]
    let _ = kill_child_process_group(child);
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
}

#[derive(Clone, Copy, Debug)]
struct ShellExitStatus {
    code: Option<i64>,
    success: bool,
}

impl ShellExitStatus {
    fn from_std(status: std::process::ExitStatus) -> Self {
        Self {
            code: status.code().map(std_exit_code_i64),
            success: status.success(),
        }
    }

    #[cfg(not(target_env = "ohos"))]
    fn from_pty(status: portable_pty::ExitStatus) -> Self {
        Self {
            code: Some(i64::from(status.exit_code())),
            success: status.success(),
        }
    }
}

#[cfg(windows)]
fn std_exit_code_i64(code: i32) -> i64 {
    // std exposes Windows DWORD process statuses through i32. Reinterpret
    // negative values as their original unsigned bit pattern so codes such
    // as 0xC0000005 survive JSON, persistence, and diagnostics unchanged.
    i64::from(code as u32)
}

#[cfg(not(windows))]
fn std_exit_code_i64(code: i32) -> i64 {
    i64::from(code)
}

impl ShellChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ShellExitStatus>> {
        match self {
            ShellChild::Process(child) => child
                .try_wait()
                .map(|status| status.map(ShellExitStatus::from_std)),
            #[cfg(not(target_env = "ohos"))]
            ShellChild::Pty(child) => child
                .try_wait()
                .map(|status| status.map(ShellExitStatus::from_pty)),
        }
    }

    fn wait(&mut self) -> std::io::Result<ShellExitStatus> {
        match self {
            ShellChild::Process(child) => child.wait().map(ShellExitStatus::from_std),
            #[cfg(not(target_env = "ohos"))]
            ShellChild::Pty(child) => child.wait().map(ShellExitStatus::from_pty),
        }
    }

    #[cfg(not(windows))]
    fn kill(&mut self) -> std::io::Result<()> {
        match self {
            #[cfg(unix)]
            ShellChild::Process(child) => kill_child_process_group(child),
            #[cfg(not(unix))]
            ShellChild::Process(child) => child.kill(),
            #[cfg(not(target_env = "ohos"))]
            ShellChild::Pty(child) => child.kill(),
        }
    }
}

enum StdinWriter {
    Pipe(ChildStdin),
    #[cfg(not(target_env = "ohos"))]
    Pty(Box<dyn Write + Send>),
}

impl StdinWriter {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            StdinWriter::Pipe(stdin) => stdin.write_all(data),
            #[cfg(not(target_env = "ohos"))]
            StdinWriter::Pty(writer) => writer.write_all(data),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            StdinWriter::Pipe(stdin) => stdin.flush(),
            #[cfg(not(target_env = "ohos"))]
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

const SYNC_READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const STALE_NO_OUTPUT_AFTER: Duration = Duration::from_secs(60);

fn spawn_sync_reader_thread<R: Read + Send + 'static>(
    mut reader: R,
) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        tx.send(buf).ok();
    });
    rx
}

fn recv_sync_reader_output(rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    rx.recv_timeout(SYNC_READER_DRAIN_TIMEOUT)
        .unwrap_or_default()
}

/// A background shell process being tracked
pub struct BackgroundShell {
    pub id: String,
    pub command: String,
    pub working_dir: PathBuf,
    pub status: ShellStatus,
    pub exit_code: Option<i64>,
    pub started_at: Instant,
    last_output_at: Instant,
    last_observed_output_len: usize,
    pub sandbox_type: SandboxType,
    pub linked_task_id: Option<String>,
    pub owner_agent: Option<ShellJobOwner>,
    stdout_buffer: Arc<Mutex<Vec<u8>>>,
    stderr_buffer: Option<Arc<Mutex<Vec<u8>>>>,
    stdout_cursor: usize,
    stderr_cursor: usize,
    completion_reported: bool,
    stdin: Option<StdinWriter>,
    child: Option<ShellChild>,
    #[cfg(windows)]
    windows_job: Option<WindowsJob>,
    stdout_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
    work_lifecycle: Option<ShellWorkLifecycle>,
    lifecycle_seq: u64,
    last_lifecycle_status: Option<ShellStatus>,
    last_lifecycle_bytes: usize,
}

#[derive(Clone)]
struct ShellWorkLifecycle {
    work: SharedWorkRuntime,
    session_id: String,
}

impl ShellWorkLifecycle {
    fn register(&self, id: &str, command: &str) -> Result<()> {
        self.work
            .register_operation(
                &self.session_id,
                OperationIntent::new(
                    format!("shell:{id}"),
                    format!("Shell · {command}"),
                    false,
                    "exec_shell",
                    id,
                ),
            )
            .map(|_| ())
            .map_err(anyhow::Error::msg)
    }

    fn observe(&self, id: &str, status: &ShellStatus, seq: u64, raw_bytes: usize) -> Result<()> {
        let owner_state = match status {
            ShellStatus::Running => OwnerState::Running,
            ShellStatus::Completed => OwnerState::Completed,
            ShellStatus::Failed | ShellStatus::TimedOut => OwnerState::Failed,
            ShellStatus::Killed => OwnerState::Cancelled,
        };
        let raw_bytes = u64::try_from(raw_bytes).unwrap_or(u64::MAX);
        let output = EvidenceRef::new(
            EvidenceKind::Receipt {
                owner: "shell".to_string(),
            },
            format!("shell:{id}:output"),
            Some(raw_bytes),
            false,
        )
        .map_err(|err| anyhow!(err.to_string()))?;
        self.work
            .reconcile_operation(
                &self.session_id,
                OperationOwnerSnapshot::new(
                    format!("shell:{id}"),
                    owner_state,
                    seq,
                    lifecycle_now_ms(),
                )
                .with_output(output),
            )
            .map(|_| ())
            .map_err(anyhow::Error::msg)
    }
}

struct ShellSpawnIntentGuard {
    lifecycle: Option<ShellWorkLifecycle>,
    id: String,
    armed: bool,
}

struct ShellSpawnContext {
    owner_agent: Option<ShellJobOwner>,
    work_lifecycle: Option<ShellWorkLifecycle>,
}

impl ShellSpawnIntentGuard {
    fn new(lifecycle: Option<ShellWorkLifecycle>, id: &str, command: &str) -> Result<Self> {
        if let Some(lifecycle) = lifecycle.as_ref() {
            lifecycle.register(id, command)?;
        }
        Ok(Self {
            lifecycle,
            id: id.to_string(),
            armed: true,
        })
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ShellSpawnIntentGuard {
    fn drop(&mut self) {
        if self.armed
            && let Some(lifecycle) = self.lifecycle.as_ref()
            && let Err(err) = lifecycle.observe(&self.id, &ShellStatus::Failed, 1, 0)
        {
            tracing::warn!(shell_id = %self.id, error = %err, "failed to record shell spawn failure");
        }
    }
}

impl BackgroundShell {
    /// Check if the process has completed and update status
    fn poll(&mut self) -> bool {
        self.refresh_output_activity();
        if self.status != ShellStatus::Running {
            self.publish_lifecycle_best_effort();
            return true;
        }

        let completed = if let Some(ref mut child) = self.child {
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
        };
        self.publish_lifecycle_best_effort();
        completed
    }

    fn publish_lifecycle(&mut self) -> Result<()> {
        let bytes = self.observed_output_len();
        if self.last_lifecycle_status.as_ref() == Some(&self.status)
            && self.last_lifecycle_bytes == bytes
        {
            return Ok(());
        }
        let next_seq = self.lifecycle_seq.saturating_add(1);
        if let Some(lifecycle) = self.work_lifecycle.as_ref() {
            lifecycle.observe(&self.id, &self.status, next_seq, bytes)?;
        }
        self.lifecycle_seq = next_seq;
        self.last_lifecycle_status = Some(self.status.clone());
        self.last_lifecycle_bytes = bytes;
        Ok(())
    }

    fn publish_lifecycle_best_effort(&mut self) {
        if let Err(err) = self.publish_lifecycle() {
            tracing::warn!(shell_id = %self.id, error = %err, "failed to reconcile shell lifecycle");
        }
    }

    fn refresh_output_activity(&mut self) {
        let observed_len = self.observed_output_len();
        if observed_len != self.last_observed_output_len {
            self.last_observed_output_len = observed_len;
            self.last_output_at = Instant::now();
        }
    }

    fn observed_output_len(&self) -> usize {
        let stdout_len = self
            .stdout_buffer
            .lock()
            .map(|data| data.len())
            .unwrap_or(0);
        let stderr_len = self
            .stderr_buffer
            .as_ref()
            .and_then(|buffer| buffer.lock().ok().map(|data| data.len()))
            .unwrap_or(0);
        stdout_len.saturating_add(stderr_len)
    }

    /// Collect output from the background threads
    fn collect_output(&mut self) {
        // Kill the whole process group before joining reader threads.
        // When the shell spawned persistent background jobs (e.g. `nohup curl`),
        // those subprocesses keep the pipe write-ends open after the shell exits.
        // Without this kill, handle.join() blocks indefinitely, freezing the UI
        // event loop that calls list_jobs() → poll() → collect_output().
        #[cfg(unix)]
        if let Some(child) = self.child.as_mut() {
            match child {
                ShellChild::Process(proc) => {
                    let _ = kill_child_process_group(proc);
                }
                #[cfg(not(target_env = "ohos"))]
                ShellChild::Pty(_) => {}
            }
        }
        #[cfg(windows)]
        terminate_and_close_windows_job(self.windows_job.take());
        if let Some(handle) = self.stdout_thread.take() {
            finish_background_reader(handle, &self.status);
        }
        if let Some(handle) = self.stderr_thread.take() {
            finish_background_reader(handle, &self.status);
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

        if stdout_delta_len > 0 || stderr_delta_len > 0 {
            self.last_output_at = Instant::now();
            self.last_observed_output_len = stdout_total.saturating_add(stderr_total);
        }

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
            self.exit_code
                .and_then(|code| i32::try_from(code).ok())
                .unwrap_or(-1),
            &stderr_full,
        )
    }

    /// Kill the process
    fn kill(&mut self) -> Result<()> {
        if let Some(ref mut child) = self.child {
            match child {
                ShellChild::Process(proc) => {
                    #[cfg(windows)]
                    {
                        terminate_windows_job(self.windows_job.as_ref(), proc)
                            .context("Failed to kill process tree")?;
                        let _ = proc.wait();
                    }
                    #[cfg(not(windows))]
                    {
                        proc.kill().context("Failed to kill process")?;
                        let _ = proc.wait();
                    }
                }
                #[cfg(not(target_env = "ohos"))]
                ShellChild::Pty(child) => {
                    child.kill().context("Failed to kill process")?;
                    let _ = child.wait();
                }
            }
        }
        self.status = ShellStatus::Killed;
        self.collect_output();
        self.publish_lifecycle_best_effort();
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
        // Use tail_from_buffer instead of full_output so we never clone the
        // entire accumulated stdout/stderr for display purposes.  full_output
        // is O(total_bytes_written), which caused the ShellManager mutex to be
        // held for an arbitrarily long time during list_jobs() calls from the
        // TUI event loop — freezing input handling on long automation runs.
        let (stdout_len, stdout_tail) = tail_from_buffer(&self.stdout_buffer, 1200);
        let (stderr_len, stderr_tail) = self
            .stderr_buffer
            .as_ref()
            .map(|buf| tail_from_buffer(buf, 1200))
            .unwrap_or((0, String::new()));
        let elapsed_since_output_ms = (self.status == ShellStatus::Running)
            .then(|| u64::try_from(self.last_output_at.elapsed().as_millis()).unwrap_or(u64::MAX));
        let stale = elapsed_since_output_ms.is_some_and(|elapsed| {
            elapsed >= u64::try_from(STALE_NO_OUTPUT_AFTER.as_millis()).unwrap_or(u64::MAX)
        });
        ShellJobSnapshot {
            id: self.id.clone(),
            job_id: self.id.clone(),
            command: self.command.clone(),
            cwd: self.working_dir.clone(),
            status: self.status.clone(),
            exit_code: self.exit_code,
            elapsed_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout_tail,
            stderr_tail,
            stdout_len,
            stderr_len,
            stdin_available: self.stdin.is_some() && self.status == ShellStatus::Running,
            stale,
            elapsed_since_output_ms,
            linked_task_id: self.linked_task_id.clone(),
            owner_agent_id: self
                .owner_agent
                .as_ref()
                .map(|owner| owner.agent_id.clone()),
            owner_agent_name: self
                .owner_agent
                .as_ref()
                .map(|owner| owner.agent_name.clone()),
        }
    }

    fn completion_event(&self) -> ShellCompletionEvent {
        let snapshot = self.job_snapshot();
        ShellCompletionEvent {
            task_id: snapshot.id,
            command: snapshot.command,
            status: snapshot.status,
            exit_code: snapshot.exit_code,
            duration_ms: snapshot.elapsed_ms,
            stdout_tail: snapshot.stdout_tail,
            stderr_tail: snapshot.stderr_tail,
            linked_task_id: snapshot.linked_task_id,
            owner_agent_id: snapshot.owner_agent_id,
            owner_agent_name: snapshot.owner_agent_name,
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

fn finish_background_reader(handle: std::thread::JoinHandle<()>, status: &ShellStatus) {
    // A killed Windows process can leave a pipe reader blocked even after its
    // Job Object has been closed. Cancellation must return promptly instead of
    // waiting for that reader to observe EOF. Other terminal states still join
    // so their final output is collected before the shell is discarded.
    #[cfg(windows)]
    if *status == ShellStatus::Killed {
        drop(handle);
        return;
    }

    #[cfg(not(windows))]
    let _ = status;

    let _ = handle.join();
}

impl Drop for BackgroundShell {
    fn drop(&mut self) {
        if self.status == ShellStatus::Running
            && let Some(ref mut child) = self.child
        {
            #[cfg(windows)]
            match child {
                ShellChild::Process(proc) => {
                    let _ = terminate_windows_job(self.windows_job.as_ref(), proc);
                }
                #[cfg(not(target_env = "ohos"))]
                ShellChild::Pty(child) => {
                    let _ = child.kill();
                }
            }
            #[cfg(not(windows))]
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

    /// Enable or disable bubblewrap passthrough (#2184).
    ///
    /// When enabled and `/usr/bin/bwrap` is executable on Linux, exec_shell
    /// commands are routed through bubblewrap for filesystem isolation.
    pub fn set_prefer_bwrap(&mut self, prefer: bool) {
        self.sandbox_manager.set_prefer_bwrap(prefer);
    }

    /// Return the OS sandbox wrapper this shell manager is configured and able
    /// to apply to commands.
    pub fn configured_sandbox_type(&self) -> Option<SandboxType> {
        self.sandbox_manager.configured_sandbox()
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

    #[allow(dead_code)]
    pub fn default_workspace(&self) -> &Path {
        &self.default_workspace
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
        self.execute_with_options_env_for_owner(
            command,
            working_dir,
            timeout_ms,
            background,
            stdin_data,
            tty,
            policy_override,
            extra_env,
            None,
        )
    }

    /// Same as `execute_with_options_env`, with optional background-job owner
    /// attribution for sub-agent launched jobs.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_with_options_env_for_owner(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        stdin_data: Option<&str>,
        tty: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
        extra_env: HashMap<String, String>,
        owner_agent: Option<ShellJobOwner>,
    ) -> Result<ShellResult> {
        self.execute_with_options_env_for_owner_and_work(
            command,
            working_dir,
            timeout_ms,
            background,
            stdin_data,
            tty,
            policy_override,
            extra_env,
            owner_agent,
            None,
        )
    }

    /// Owner-aware execution with an optional Work Graph lifecycle sink.
    #[allow(clippy::too_many_arguments)]
    fn execute_with_options_env_for_owner_and_work(
        &mut self,
        command: &str,
        working_dir: Option<&str>,
        timeout_ms: u64,
        background: bool,
        stdin_data: Option<&str>,
        tty: bool,
        policy_override: Option<ExecutionSandboxPolicy>,
        extra_env: HashMap<String, String>,
        owner_agent: Option<ShellJobOwner>,
        work_lifecycle: Option<ShellWorkLifecycle>,
    ) -> Result<ShellResult> {
        // Log execution via ShellDispatcher when SHELL_DISPATCHER_LOG is set.
        crate::shell_dispatcher::ShellDispatcher::log_exec(command);

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
            self.spawn_background_sandboxed(
                command,
                &work_dir,
                &exec_env,
                stdin_data,
                tty,
                ShellSpawnContext {
                    owner_agent,
                    work_lifecycle,
                },
            )
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
        crate::shell_dispatcher::ShellDispatcher::log_exec(command);

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
        crate::utils::suppress_console_window(&mut cmd);
        push_shell_args(&mut cmd, program, args);
        cmd.current_dir(working_dir)
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

        // Disable raw mode before spawn; restore only if raw mode was active
        // on entry (issue #1690).
        let raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_mode_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        struct SyncRawModeGuard {
            restore: bool,
        }
        impl Drop for SyncRawModeGuard {
            fn drop(&mut self) {
                if self.restore {
                    let _ = crossterm::terminal::enable_raw_mode();
                }
            }
        }
        let _guard = SyncRawModeGuard {
            restore: raw_mode_was_enabled,
        };

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to execute: {original_command}"))?;
        #[cfg(windows)]
        let windows_job = attach_windows_job(&child, original_command);

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

        // Spawn threads to read output. Use bounded receives below so a killed
        // or detached descendant that keeps pipe handles open cannot wedge the
        // foreground shell path while the global tool lock is held (#2571).
        let stdout_rx = spawn_sync_reader_thread(stdout_handle);
        let stderr_rx = spawn_sync_reader_thread(stderr_handle);

        // Wait with timeout
        if let Some(status) = child.wait_timeout(timeout)? {
            let status = ShellExitStatus::from_std(status);
            #[cfg(unix)]
            let _ = kill_child_process_group(&mut child);
            #[cfg(windows)]
            terminate_and_close_windows_job(windows_job);
            let stdout = recv_sync_reader_output(&stdout_rx);
            let stderr = recv_sync_reader_output(&stderr_rx);
            let stdout_str = String::from_utf8_lossy(&stdout).to_string();
            let stderr_str = String::from_utf8_lossy(&stderr).to_string();
            let exit_code = status
                .code
                .and_then(|code| i32::try_from(code).ok())
                .unwrap_or(-1);

            // Check if sandbox denied the operation
            let sandbox_denied = SandboxManager::was_denied(sandbox_type, exit_code, &stderr_str);
            let (stdout, stdout_meta) = truncate_with_meta(&stdout_str);
            let (stderr, stderr_meta) = truncate_with_meta(&stderr_str);

            Ok(ShellResult {
                task_id: None,
                status: if status.success {
                    ShellStatus::Completed
                } else {
                    ShellStatus::Failed
                },
                exit_code: status.code,
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
            #[cfg(windows)]
            let _ = terminate_child_and_close_windows_job(windows_job, &mut child);
            #[cfg(all(not(unix), not(windows)))]
            let _ = child.kill();
            let status = child.wait().ok();
            let stdout = recv_sync_reader_output(&stdout_rx);
            let stderr = recv_sync_reader_output(&stderr_rx);
            let stdout_str = String::from_utf8_lossy(&stdout).to_string();
            let stderr_str = String::from_utf8_lossy(&stderr).to_string();
            let (stdout, stdout_meta) = truncate_with_meta(&stdout_str);
            let (stderr, stderr_meta) = truncate_with_meta(&stderr_str);

            Ok(ShellResult {
                task_id: None,
                status: ShellStatus::TimedOut,
                exit_code: status
                    .map(ShellExitStatus::from_std)
                    .and_then(|status| status.code),
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
        crate::utils::suppress_console_window(&mut cmd);
        push_shell_args(&mut cmd, program, args);
        cmd.current_dir(working_dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }
        install_parent_death_signal(&mut cmd);

        // Disable raw mode before spawn; restore only if raw mode was active
        // on entry (issue #1690).
        let raw_mode_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_mode_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        struct InteractiveRawModeGuard {
            restore: bool,
        }
        impl Drop for InteractiveRawModeGuard {
            fn drop(&mut self) {
                if self.restore {
                    let _ = crossterm::terminal::enable_raw_mode();
                }
            }
        }
        let _guard = InteractiveRawModeGuard {
            restore: raw_mode_was_enabled,
        };

        child_env::apply_to_command(&mut cmd, child_env::string_map_env(&exec_env.env));

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to execute: {original_command}"))?;
        #[cfg(windows)]
        let windows_job = attach_windows_job(&child, original_command);

        if let Some(status) = child.wait_timeout(timeout)? {
            let status = ShellExitStatus::from_std(status);
            #[cfg(windows)]
            terminate_and_close_windows_job(windows_job);
            Ok(ShellResult {
                task_id: None,
                status: if status.success {
                    ShellStatus::Completed
                } else {
                    ShellStatus::Failed
                },
                exit_code: status.code,
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
            #[cfg(windows)]
            let _ = terminate_child_and_close_windows_job(windows_job, &mut child);
            #[cfg(all(not(unix), not(windows)))]
            let _ = child.kill();
            let status = child.wait().ok();

            Ok(ShellResult {
                task_id: None,
                status: ShellStatus::TimedOut,
                exit_code: status
                    .map(ShellExitStatus::from_std)
                    .and_then(|status| status.code),
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
        spawn_context: ShellSpawnContext,
    ) -> Result<ShellResult> {
        let ShellSpawnContext {
            owner_agent,
            work_lifecycle,
        } = spawn_context;
        let task_id = format!("shell_{}", &Uuid::new_v4().to_string()[..8]);
        let mut spawn_guard =
            ShellSpawnIntentGuard::new(work_lifecycle.clone(), &task_id, original_command)?;
        let started = Instant::now();
        let sandbox_type = exec_env.sandbox_type;
        let sandboxed = exec_env.is_sandboxed();

        // Build the command from ExecEnv
        let program = exec_env.program();
        let args = exec_env.args();

        #[cfg(target_env = "ohos")]
        if tty {
            return Err(anyhow!(
                "TTY shell mode is not supported on HarmonyOS/OpenHarmony yet."
            ));
        }

        let stdout_buffer = Arc::new(Mutex::new(Vec::new()));
        let stderr_buffer = if tty {
            None
        } else {
            Some(Arc::new(Mutex::new(Vec::new())))
        };

        #[cfg(windows)]
        let mut windows_job = None;

        let (child, stdin, stdout_thread, stderr_thread) = if tty {
            #[cfg(target_env = "ohos")]
            unreachable!("OHOS TTY mode returns before PTY setup");

            #[cfg(not(target_env = "ohos"))]
            {
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

                let mut child = pair
                    .slave
                    .spawn_command(cmd)
                    .with_context(|| format!("Failed to spawn PTY command: {original_command}"))?;
                drop(pair.slave);

                let reader = match pair.master.try_clone_reader() {
                    Ok(reader) => reader,
                    Err(err) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(err).context("Failed to clone PTY reader");
                    }
                };
                let writer = match pair.master.take_writer() {
                    Ok(writer) => writer,
                    Err(err) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(err).context("Failed to take PTY writer");
                    }
                };
                let stdout_thread = Some(spawn_reader_thread(reader, Arc::clone(&stdout_buffer)));

                (
                    ShellChild::Pty(child),
                    Some(StdinWriter::Pty(writer)),
                    stdout_thread,
                    None,
                )
            }
        } else {
            let mut cmd = Command::new(program);
            crate::utils::suppress_console_window(&mut cmd);
            push_shell_args(&mut cmd, program, args);
            cmd.current_dir(working_dir)
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
            #[cfg(windows)]
            {
                windows_job = attach_windows_job(&child, original_command);
            }

            let stdout_handle = match child.stdout.take() {
                Some(stdout) => stdout,
                None => {
                    #[cfg(windows)]
                    terminate_unregistered_process(&mut child, windows_job.as_ref());
                    #[cfg(not(windows))]
                    terminate_unregistered_process(&mut child);
                    return Err(anyhow!("Failed to capture stdout"));
                }
            };
            let stderr_handle = match child.stderr.take() {
                Some(stderr) => stderr,
                None => {
                    #[cfg(windows)]
                    terminate_unregistered_process(&mut child, windows_job.as_ref());
                    #[cfg(not(windows))]
                    terminate_unregistered_process(&mut child);
                    return Err(anyhow!("Failed to capture stderr"));
                }
            };
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
            last_output_at: started,
            last_observed_output_len: 0,
            sandbox_type,
            linked_task_id: None,
            owner_agent,
            stdout_buffer,
            stderr_buffer,
            stdout_cursor: 0,
            stderr_cursor: 0,
            completion_reported: false,
            stdin,
            child: Some(child),
            #[cfg(windows)]
            windows_job,
            stdout_thread,
            stderr_thread,
            work_lifecycle,
            lifecycle_seq: 0,
            last_lifecycle_status: None,
            last_lifecycle_bytes: 0,
        };

        if let Some(input) = stdin_data
            && let Err(err) = bg_shell.write_stdin(input, false)
        {
            let _ = bg_shell.kill();
            return Err(err);
        }

        if let Err(err) = bg_shell.publish_lifecycle() {
            let _ = bg_shell.kill();
            return Err(err);
        }

        self.processes.insert(task_id.clone(), bg_shell);
        spawn_guard.disarm();

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

    /// Drain finished background shell jobs that have not yet been reported to
    /// runtime status.
    pub fn drain_finished_jobs(&mut self) -> Vec<ShellCompletionEvent> {
        let mut events = Vec::new();
        for shell in self.processes.values_mut() {
            shell.poll();
            if shell.status != ShellStatus::Running && !shell.completion_reported {
                shell.completion_reported = true;
                events.push(shell.completion_event());
            }
        }
        events.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        events
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
                elapsed_since_output_ms: None,
                linked_task_id,
                owner_agent_id: None,
                owner_agent_name: None,
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

use crate::command_safety::{
    SafetyLevel, analyze_command, extract_primary_command, is_parallel_readonly_command,
};
use crate::execpolicy::{ExecPolicyDecision, load_default_policy};
use crate::features::Feature;
use crate::tools::cargo_failure_summary::summarize_cargo_failure;
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

/// Human-readable exit status for a shell result: the numeric code when the
/// process returned one, or "terminated by signal" when it did not (rather
/// than leaking `Some(127)` / `None` Debug output to the user).
fn exit_code_label(code: Option<i64>) -> String {
    match (code, exit_code_hex(code)) {
        (Some(code), Some(hex)) => format!("exit code {code} ({hex})"),
        (Some(code), None) => format!("exit code {code}"),
        (None, _) => "terminated by signal".to_string(),
    }
}

fn exit_code_hex(code: Option<i64>) -> Option<String> {
    code.filter(|code| *code > i64::from(i32::MAX) && *code <= i64::from(u32::MAX))
        .map(|code| format!("0x{code:08X}"))
}
const PYTHON_BUILD_DEPENDENCY_HINT: &str = "Python build dependency missing: setuptools is not \
available in the active environment. Install the declared build requirements first, for example \
`python -m pip install -U pip setuptools wheel build`, then rerun the build command.";

fn attach_cargo_failure_summary(
    metadata: &mut serde_json::Value,
    command: &str,
    result: &ShellResult,
) {
    if let Some(summary) = summarize_cargo_failure(
        command,
        &result.stdout,
        &result.stderr,
        result.exit_code.and_then(|code| i32::try_from(code).ok()),
    ) {
        metadata["cargo_failure_summary"] = summary.to_metadata_value();
    }
}

fn attach_python_build_dependency_hint(
    metadata: &mut serde_json::Value,
    hint: Option<&'static str>,
) {
    if let Some(hint) = hint {
        metadata["python_build_dependency_hint"] = json!({
            "kind": "missing_setuptools",
            "hint": hint,
            "recommended_first_step": "python -m pip install -U pip setuptools wheel build",
        });
    }
}

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

fn python_build_dependency_hint(command: &str, result: &ShellResult) -> Option<&'static str> {
    if matches!(result.status, ShellStatus::Completed) && result.exit_code == Some(0) {
        return None;
    }

    let command = command.to_ascii_lowercase();
    let combined = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
    let mentions_missing_setuptools = [
        "no module named 'setuptools'",
        "no module named \"setuptools\"",
        "setuptools is not available",
        "cannot import 'setuptools",
        "cannot import \"setuptools",
        "missing dependencies",
    ]
    .iter()
    .any(|needle| combined.contains(needle))
        && combined.contains("setuptools");
    if !mentions_missing_setuptools {
        return None;
    }

    let pythonish_command = [
        "python",
        "pip",
        "pytest",
        "tox",
        "nox",
        "cython",
        "setup.py",
        "build_ext",
    ]
    .iter()
    .any(|needle| command.contains(needle));
    let pythonish_output = [
        "setup.py",
        "pyproject.toml",
        "build_meta",
        "build_ext",
        "pep 517",
        "cython",
    ]
    .iter()
    .any(|needle| combined.contains(needle));

    if pythonish_command || pythonish_output {
        Some(PYTHON_BUILD_DEPENDENCY_HINT)
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

fn shell_job_owner_from_context(context: &ToolContext) -> Option<ShellJobOwner> {
    let agent_id = context
        .owner_agent_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let agent_name = context
        .owner_agent_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(agent_id);
    Some(ShellJobOwner {
        agent_id: agent_id.to_string(),
        agent_name: agent_name.to_string(),
    })
}

fn shell_work_lifecycle_from_context(context: &ToolContext) -> Option<ShellWorkLifecycle> {
    context
        .runtime
        .work
        .as_ref()
        .map(|work| ShellWorkLifecycle {
            work: work.clone(),
            session_id: context.state_namespace.clone(),
        })
}

fn lifecycle_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn attach_shell_owner_metadata(metadata: &mut serde_json::Value, context: &ToolContext) {
    let Some(owner) = shell_job_owner_from_context(context) else {
        return;
    };
    metadata["owner_agent_id"] = json!(owner.agent_id);
    metadata["owner_agent_name"] = json!(owner.agent_name);
}

fn exec_shell_input_is_parallel_readonly(input: &serde_json::Value) -> bool {
    let Some(command) = input.get("command").and_then(serde_json::Value::as_str) else {
        return false;
    };
    if ["background", "interactive", "tty", "combined_output"]
        .iter()
        .any(|key| input.get(*key).and_then(serde_json::Value::as_bool) == Some(true))
    {
        return false;
    }
    if ["stdin", "input", "data"]
        .iter()
        .any(|key| input.get(*key).is_some())
    {
        return false;
    }

    is_parallel_readonly_command(command)
}

fn exec_shell_input_starts_detached(input: &serde_json::Value) -> bool {
    input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .is_some()
        && input
            .get("interactive")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        && (input.get("background").and_then(serde_json::Value::as_bool) == Some(true)
            || input.get("tty").and_then(serde_json::Value::as_bool) == Some(true))
}

async fn execute_foreground_via_background(
    context: &ToolContext,
    command: &str,
    working_dir: Option<String>,
    timeout_ms: u64,
    stdin_data: Option<&str>,
    tty: bool,
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
        manager.execute_with_options_env_for_owner_and_work(
            command,
            working_dir.as_deref(),
            timeout_ms,
            true,
            stdin_data,
            tty,
            policy_override,
            extra_env,
            shell_job_owner_from_context(context),
            shell_work_lifecycle_from_context(context),
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

/// Unified shell tool (#4625).
///
/// The model sees one tool: `Bash` with an `action` parameter
/// (run | wait | interact | cancel). Legacy names (`exec_shell`,
/// `exec_shell_wait`, etc.) stay registered as hidden compat aliases
/// that force the action so saved transcripts replay correctly.
pub struct BashTool {
    name: &'static str,
    forced_action: Option<&'static str>,
}

impl BashTool {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
        }
    }

    pub const fn alias(name: &'static str, action: &'static str) -> Self {
        Self {
            name,
            forced_action: Some(action),
        }
    }
}

#[async_trait]
impl ToolSpec for BashTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn model_visible(&self) -> bool {
        self.name == "Bash"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command in the workspace. Action \"run\" (default) executes a command; \"wait\" polls a background task; \"interact\" sends stdin to a background task; \"cancel\" kills a background task. Foreground mode is for bounded commands; use background=true for work expected to take >5 seconds."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["run", "wait", "interact", "cancel"],
                    "description": "Action to perform (default: run)"
                },
                "command": {
                    "type": "string",
                    "description": "The shell command to execute (action=run)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 120000, max: 600000)"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background and return task_id (default: false). Prefer this for commands expected to take >5 seconds."
                },
                "interactive": {
                    "type": "boolean",
                    "description": "Run interactively with terminal IO (default: false)"
                },
                "stdin": {
                    "type": "string",
                    "description": "Stdin data to send (action=run: before waiting; action=interact: to the background task)"
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory for the command"
                },
                "tty": {
                    "type": "boolean",
                    "description": "Allocate a pseudo-terminal for interactive programs (implies background)"
                },
                "combined_output": {
                    "type": "boolean",
                    "description": "Capture stdout and stderr as one chronological PTY stream (default false)"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID for action=wait/interact/cancel"
                },
                "wait": {
                    "type": "boolean",
                    "description": "Block until task completes (action=wait, default: false)"
                },
                "close_stdin": {
                    "type": "boolean",
                    "description": "Close stdin after sending (action=interact)"
                },
                "all": {
                    "type": "boolean",
                    "description": "Cancel all running background tasks (action=cancel)"
                }
            }
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

    fn approval_requirement_for(&self, input: &serde_json::Value) -> ApprovalRequirement {
        if exec_shell_input_is_parallel_readonly(input) {
            ApprovalRequirement::Auto
        } else {
            self.approval_requirement()
        }
    }

    fn is_read_only_for(&self, input: &serde_json::Value) -> bool {
        exec_shell_input_is_parallel_readonly(input)
    }

    fn supports_parallel_for(&self, input: &serde_json::Value) -> bool {
        exec_shell_input_is_parallel_readonly(input)
    }

    fn starts_detached_for(&self, input: &serde_json::Value) -> bool {
        exec_shell_input_starts_detached(input)
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let action = self.forced_action.unwrap_or_else(|| {
            input
                .get("action")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("run")
        });
        match action {
            "wait" => return self.execute_wait(&input, context).await,
            "interact" => return self.execute_interact(&input, context).await,
            "cancel" => return self.execute_cancel(&input, context).await,
            _ => {}
        }
        let command = required_str(&input, "command")?;
        match context.shell_policy {
            ShellPolicy::None => {
                return Ok(ToolResult::error(
                    "Shell tools are disabled by the active permission profile.",
                ));
            }
            ShellPolicy::ReadOnly if !exec_shell_input_is_parallel_readonly(&input) => {
                return Ok(ToolResult::error(
                    "Shell command blocked by read-only shell policy. Use a non-mutating, non-background inspection command, or switch to Act mode (`/mode act`) for write-capable shell work.",
                ));
            }
            ShellPolicy::ReadOnly | ShellPolicy::Full => {}
        }
        let timeout_ms = optional_u64(&input, "timeout_ms", 120_000).min(600_000);
        let background = optional_bool(&input, "background", false);
        let interactive = optional_bool(&input, "interactive", false);
        let combined_output = optional_bool(&input, "combined_output", false);
        let tty = optional_bool(&input, "tty", false) || (combined_output && background);
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
        if interactive && (tty || combined_output) {
            return Ok(ToolResult::error(
                "Interactive mode cannot be combined with TTY or combined_output sessions.",
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
                            "BLOCKED: This command was blocked for safety reasons.\n\nReasons: {reasons}{suggestions}\n\nNote: allow_shell=true exposes shell tools, but it does not disable built-in shell safety validation."
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
            // Default to the tool context's workspace (which reflects the
            // child agent's worktree when `worktree: true` was used), not the
            // shared ShellManager's parent-workspace default_workspace.
            None => Some(context.workspace.display().to_string()),
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
                        exit_code: Some(i64::from(output.exit_code)),
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
            let python_dependency_hint = python_build_dependency_hint(command, &result);
            let mut output = if result.stdout.is_empty() && result.stderr.is_empty() {
                "(no output)".to_string()
            } else if result.stderr.is_empty() {
                result.stdout.clone()
            } else {
                format!("{}\n\nSTDERR:\n{}", result.stdout, result.stderr)
            };
            if let Some(hint) = python_dependency_hint {
                output = format!("{hint}\n\n{output}");
            }

            let mut metadata = json!({
                "exit_code": result.exit_code,
                "exit_code_hex": exit_code_hex(result.exit_code),
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
            attach_shell_owner_metadata(&mut metadata, context);
            attach_cargo_failure_summary(&mut metadata, command, &result);
            attach_python_build_dependency_hint(&mut metadata, python_dependency_hint);

            return Ok(ToolResult {
                content: output,
                success: result.status == ShellStatus::Completed,
                metadata: Some(metadata),
            });
        }

        let mut lifecycle_warning = None;
        let result = if interactive {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            let work_lifecycle = shell_work_lifecycle_from_context(context);
            let task_id = format!("shell_{}", &Uuid::new_v4().to_string()[..8]);
            let mut spawn_guard =
                ShellSpawnIntentGuard::new(work_lifecycle.clone(), &task_id, command)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            let result = manager.execute_interactive_with_policy_env(
                command,
                working_dir.as_deref(),
                timeout_ms,
                policy_override,
                extra_env,
            );
            match result {
                Ok(result) => {
                    // The process result is authoritative once execution has
                    // completed. Disarm before observing it so a graph-write
                    // failure cannot relabel a successful command as Failed.
                    spawn_guard.disarm();
                    if let Some(lifecycle) = work_lifecycle.as_ref() {
                        let raw_bytes = result.stdout_len.saturating_add(result.stderr_len);
                        if let Err(err) = lifecycle.observe(&task_id, &result.status, 1, raw_bytes)
                        {
                            tracing::warn!(shell_id = %task_id, error = %err, "interactive shell completed but Work lifecycle reconciliation failed");
                            lifecycle_warning = Some(err.to_string());
                        }
                    }
                    Ok(result)
                }
                Err(err) => Err(err),
            }
        } else if background {
            let mut manager = context
                .shell_manager
                .lock()
                .map_err(|_| ToolError::execution_failed("shell manager lock poisoned"))?;
            manager.execute_with_options_env_for_owner_and_work(
                command,
                working_dir.as_deref(),
                timeout_ms,
                true,
                stdin_data.as_deref(),
                tty,
                policy_override,
                extra_env,
                shell_job_owner_from_context(context),
                shell_work_lifecycle_from_context(context),
            )
        } else {
            execute_foreground_via_background(
                context,
                command,
                working_dir,
                timeout_ms,
                stdin_data.as_deref(),
                combined_output,
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
                let python_dependency_hint = python_build_dependency_hint(command, &result);
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
                            "Foreground shell wait moved to /jobs: {task_id_str}\n\nReturns immediately; completion is tracked in task/status state. Keep working; call exec_shell_wait only if you need early output, final output, or wait=true at a true dependency."
                        )
                    } else {
                        format!(
                            "Background task started: {task_id_str}\n\nReturns immediately; completion is tracked in task/status state. Keep working; call exec_shell_wait only if you need early output, final output, or wait=true at a true dependency."
                        )
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
                        "Command failed ({})\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
                        exit_code_label(result.exit_code),
                        result.stdout,
                        result.stderr
                    )
                };
                if let Some(hint) = network_restricted_hint.as_deref() {
                    output = format!("{hint}\n\n{output}");
                }
                if let Some(hint) = provenance_hint {
                    output = format!("{hint}\n\n{output}");
                }
                if let Some(hint) = python_dependency_hint {
                    output = format!("{hint}\n\n{output}");
                }

                let mut metadata = json!({
                    "exit_code": result.exit_code,
                    "exit_code_hex": exit_code_hex(result.exit_code),
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
                    "lifecycle_warning": lifecycle_warning,
                    "summary": summary,
                    "stdout_summary": stdout_summary,
                    "stderr_summary": stderr_summary,
                    "safety_level": format!("{:?}", safety.level),
                    "interactive": interactive,
                    "combined_output": combined_output,
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
                if background || backgrounded_foreground {
                    metadata["auto_resume_on_completion"] = json!(false);
                    metadata["completion_surface"] = json!("task_status");
                    metadata["background_policy"] = json!("nonblocking");
                }
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
                attach_shell_owner_metadata(&mut metadata, context);
                attach_cargo_failure_summary(&mut metadata, command, &result);
                attach_python_build_dependency_hint(&mut metadata, python_dependency_hint);

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

/// Maximum deliberate dependency-barrier wait accepted by `exec_shell_wait`.
pub(crate) const EXEC_SHELL_WAIT_MAX_TIMEOUT_MS: u64 = 600_000;

impl BashTool {
    async fn execute_wait(
        &self,
        input: &serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task_id = required_task_id(input)?;
        let wait = optional_bool(input, "wait", false);
        let timeout_ms = optional_u64(input, "timeout_ms", 30_000);

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

    async fn execute_interact(
        &self,
        input: &serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task_id = required_task_id(input)?;
        let close_stdin = optional_bool(input, "close_stdin", false);
        let timeout_ms = optional_u64(input, "timeout_ms", 1_000);
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

    async fn execute_cancel(
        &self,
        input: &serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let cancel_all = optional_bool(input, "all", false);
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
                    content: "No running background commands.".to_string(),
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
                    "Canceled {} background command{}: {}",
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

        let task_id = required_task_id(input)?;
        let result = manager
            .kill(task_id)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        let task_id = result
            .task_id
            .clone()
            .unwrap_or_else(|| task_id.to_string());
        Ok(ToolResult {
            content: format!("Canceled background command: {task_id}"),
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
    let python_dependency_hint = python_build_dependency_hint(&delta.command, &result);
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
            ShellStatus::Failed => {
                format!("Command failed ({})", exit_code_label(result.exit_code))
            }
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
    if let Some(hint) = python_dependency_hint {
        output = format!("{hint}\n\n{output}");
    }

    let mut metadata = json!({
        "exit_code": result.exit_code,
        "exit_code_hex": exit_code_hex(result.exit_code),
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
        "command": delta.command,
        "stream_delta": true,
    });
    attach_shell_owner_metadata(&mut metadata, context);
    attach_cargo_failure_summary(&mut metadata, &delta.command, &result);
    attach_python_build_dependency_hint(&mut metadata, python_dependency_hint);

    let mut tool_result = ToolResult {
        content: output,
        success: matches!(result.status, ShellStatus::Completed | ShellStatus::Running),
        metadata: Some(metadata),
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
    let timeout_ms = timeout_ms.clamp(1000, EXEC_SHELL_WAIT_MAX_TIMEOUT_MS);
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
